//! Anchored Git change preview. Rendering is stateless: the caller owns the
//! selected source line, scroll offset, and mouse capture rectangle.
use super::{DiffLineKind, DiffSide, Hunk};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SURFACE: Color = Color::Rgb(23, 27, 35);
const HEADER: Color = Color::Rgb(31, 37, 47);
const BORDER: Color = Color::Rgb(72, 82, 97);
const MUTED: Color = Color::Rgb(138, 151, 169);
const FG: Color = Color::Rgb(210, 221, 234);
const GREEN: Color = Color::Rgb(110, 215, 155);
const RED: Color = Color::Rgb(240, 137, 143);
const BLUE: Color = Color::Rgb(120, 176, 245);
const MAX_WIDTH: u16 = 96;
const MAX_ROWS: usize = 12;

fn clipped(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut count = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if count + w >= width {
            break;
        }
        result.push(if c.is_control() { ' ' } else { c });
        count += w;
    }
    result.push('…');
    result
}

fn rectangle(area: Rect, anchor: usize, scroll: usize, hunk: &Hunk) -> Option<Rect> {
    if area.width < 20 || area.height < 5 || hunk.lines.is_empty() {
        return None;
    }
    let visible = hunk.lines.len().min(MAX_ROWS);
    let height = (visible as u16 + 4).min(area.height);
    let width = area.width.saturating_sub(5).min(MAX_WIDTH);
    if width < 14 {
        return None;
    }
    let relative = anchor.saturating_sub(scroll) as u16;
    let anchor_y = area.y.saturating_add(relative);
    let space_below = area.bottom().saturating_sub(anchor_y.saturating_add(1));
    let space_above = anchor_y.saturating_sub(area.y);
    let y = if space_below >= height {
        anchor_y + 1
    } else if space_above >= height {
        anchor_y - height
    } else {
        area.bottom().saturating_sub(height)
    };
    Some(Rect::new(area.x + 5, y, width, height))
}

fn paint_row(frame: &mut Frame<'_>, rect: Rect, glyph: &str, style: Style) {
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            frame.buffer_mut()[(x, y)]
                .set_symbol(glyph)
                .set_style(style);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    anchor: usize,
    source_scroll: usize,
    hunk: &Hunk,
    side: DiffSide,
    hunk_number: usize,
    hunk_count: usize,
    scroll_offset: usize,
    selected_source: Option<usize>,
) -> Option<(Rect, usize)> {
    let popup = rectangle(area, anchor, source_scroll, hunk)?;
    let visible_rows = popup.height.saturating_sub(4) as usize;
    let max_offset = hunk.lines.len().saturating_sub(visible_rows);
    let offset = scroll_offset.min(max_offset);
    let accent = if side == DiffSide::Staged {
        BLUE
    } else {
        GREEN
    };
    let side_label = if side == DiffSide::Staged {
        "STAGED"
    } else {
        "WORKTREE"
    };
    let add = hunk
        .lines
        .iter()
        .filter(|(kind, _)| *kind == DiffLineKind::Add)
        .count();
    let del = hunk
        .lines
        .iter()
        .filter(|(kind, _)| *kind == DiffLineKind::Delete)
        .count();

    frame.render_widget(Clear, popup);
    paint_row(frame, popup, " ", Style::default().fg(FG).bg(SURFACE));
    let left = popup.x;
    let right = popup.right() - 1;
    let top = popup.y;
    let bottom = popup.bottom() - 1;
    let outline = Style::default().fg(BORDER).bg(SURFACE);
    for y in top..=bottom {
        frame.buffer_mut()[(left, y)]
            .set_symbol("│")
            .set_style(outline);
        frame.buffer_mut()[(right, y)]
            .set_symbol("│")
            .set_style(outline);
    }
    for x in left..=right {
        frame.buffer_mut()[(x, top)]
            .set_symbol("─")
            .set_style(outline);
        frame.buffer_mut()[(x, bottom)]
            .set_symbol("─")
            .set_style(outline);
    }
    for (x, y, glyph) in [
        (left, top, "╭"),
        (right, top, "╮"),
        (left, bottom, "╰"),
        (right, bottom, "╯"),
    ] {
        frame.buffer_mut()[(x, y)]
            .set_symbol(glyph)
            .set_style(outline);
    }
    // A quiet surface and a narrow left accent separate Peek from the source.
    for y in top + 1..bottom {
        frame.buffer_mut()[(left, y)]
            .set_symbol("▏")
            .set_style(Style::default().fg(accent).bg(SURFACE));
    }
    for x in left + 1..right {
        frame.buffer_mut()[(x, top + 1)]
            .set_symbol(" ")
            .set_style(Style::default().bg(HEADER));
        frame.buffer_mut()[(x, top + 2)]
            .set_symbol("─")
            .set_style(Style::default().fg(BORDER).bg(SURFACE));
    }
    let title = Line::from(vec![
        Span::styled(" ◆ ", Style::default().fg(accent).bg(SURFACE)),
        Span::styled(
            "Changes",
            Style::default()
                .fg(FG)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {side_label}"),
            Style::default().fg(accent).bg(SURFACE),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(left + 1, top, popup.width.saturating_sub(17), 1),
    );
    if popup.width >= 43 {
        frame.render_widget(
            Paragraph::new(format!("Hunk {hunk_number}/{hunk_count}"))
                .style(Style::default().fg(MUTED).bg(SURFACE)),
            Rect::new(right.saturating_sub(18), top, 15, 1),
        );
    }
    frame.render_widget(
        Paragraph::new("×").style(Style::default().fg(FG).bg(SURFACE)),
        Rect::new(right - 1, top, 1, 1),
    );
    let focus = selected_source
        .map(|n| format!("L{n}  ·  "))
        .unwrap_or_default();
    let summary = clipped(
        &format!(" {focus}{}", hunk.header),
        popup.width.saturating_sub(5) as usize,
    );
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(MUTED).bg(HEADER)),
        Rect::new(left + 2, top + 1, popup.width.saturating_sub(4), 1),
    );
    // Deletion rows all anchor to the same surviving source line. Select
    // just one diff row to avoid an entire deleted block looking focused.
    let focused_row = selected_source.and_then(|source| {
        hunk.row_lines
            .iter()
            .enumerate()
            .find(|(i, line)| **line == Some(source) && hunk.lines[*i].0 == DiffLineKind::Add)
            .or_else(|| {
                hunk.row_lines.iter().enumerate().find(|(i, line)| {
                    **line == Some(source) && hunk.lines[*i].0 != DiffLineKind::Context
                })
            })
            .map(|(i, _)| i)
    });
    for (row, (kind, content)) in hunk
        .lines
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_rows)
    {
        let y = top + 3 + (row - offset) as u16;
        let source = hunk.row_lines.get(row).copied().flatten();
        let focused = Some(row) == focused_row;
        let (fg, bg, symbol) = match kind {
            DiffLineKind::Add => (
                Color::Rgb(188, 231, 205),
                if focused {
                    Color::Rgb(37, 80, 60)
                } else {
                    Color::Rgb(27, 51, 42)
                },
                "+",
            ),
            DiffLineKind::Delete => (
                Color::Rgb(245, 190, 194),
                if focused {
                    Color::Rgb(88, 45, 52)
                } else {
                    Color::Rgb(55, 36, 44)
                },
                "−",
            ),
            _ => (FG, SURFACE, " "),
        };
        let content_area = Rect::new(left + 1, y, popup.width - 2, 1);
        paint_row(frame, content_area, " ", Style::default().bg(bg).fg(fg));
        if focused {
            frame.buffer_mut()[(left, y)]
                .set_symbol("┃")
                .set_style(Style::default().fg(accent).bg(bg));
        }
        let number = if *kind == DiffLineKind::Delete {
            "  ·".to_owned()
        } else {
            source.map_or_else(|| "  ·".to_owned(), |n| n.to_string())
        };
        let number = format!("{number:>5}");
        frame.render_widget(
            Paragraph::new(number)
                .style(Style::default().fg(if focused { FG } else { MUTED }).bg(bg)),
            Rect::new(left + 2, y, 5.min(popup.width.saturating_sub(4)), 1),
        );
        frame.render_widget(
            Paragraph::new(symbol).style(
                Style::default()
                    .fg(match kind {
                        DiffLineKind::Add => GREEN,
                        DiffLineKind::Delete => RED,
                        _ => MUTED,
                    })
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(left + 8, y, 1, 1),
        );
        let shown = if matches!(kind, DiffLineKind::Add | DiffLineKind::Delete) {
            content.get(1..).unwrap_or(content)
        } else {
            content.as_str()
        };
        let text_width = popup.width.saturating_sub(13);
        let shown = clipped(shown, text_width as usize);
        frame.render_widget(
            Paragraph::new(shown).style(Style::default().fg(fg).bg(bg)),
            Rect::new(left + 10, y, text_width, 1),
        );
    }
    // The right border becomes a compact position indicator for long hunks.
    if hunk.lines.len() > visible_rows && visible_rows > 0 {
        let travel = visible_rows.saturating_sub(1);
        let row =
            offset.saturating_mul(travel) / hunk.lines.len().saturating_sub(visible_rows).max(1);
        frame.buffer_mut()[(right, top + 3 + row as u16)]
            .set_symbol("┃")
            .set_style(Style::default().fg(accent).bg(SURFACE));
    }
    let footer = format!(
        " +{add}  −{del}  ·  {}–{} / {} ",
        offset + 1,
        (offset + visible_rows).min(hunk.lines.len()),
        hunk.lines.len()
    );
    let footer = clipped(&footer, popup.width.saturating_sub(5) as usize);
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(accent).bg(SURFACE)),
        Rect::new(left + 2, bottom, popup.width.saturating_sub(4), 1),
    );
    if popup.width >= 70 {
        frame.render_widget(
            Paragraph::new("wheel · Esc").style(Style::default().fg(MUTED).bg(SURFACE)),
            Rect::new(right - 13, bottom, 11, 1),
        );
    }
    Some((popup, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_and_wide_text_is_clipped_without_splitting_chinese() {
        assert_eq!(clipped("中文abc", 4), "中…");
        assert_eq!(clipped("abcd", 3), "ab…");
        assert_eq!(clipped("x", 2), "x");
    }

    #[test]
    fn selected_line_is_unique_and_popup_shows_patch_metadata() {
        let hunk = Hunk {
            header: "@@ -30,4 +30,6 @@ fn run()".into(),
            lines: vec![
                (DiffLineKind::Context, " old".into()),
                (DiffLineKind::Delete, "-removed one".into()),
                (DiffLineKind::Delete, "-removed two".into()),
                (DiffLineKind::Add, "+new 中文".into()),
                (DiffLineKind::Context, " next".into()),
            ],
            row_lines: vec![Some(30), Some(31), Some(31), Some(31), Some(32)],
            marks: vec![],
        };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 23)).unwrap();
        let mut position = None;
        term.draw(|frame| {
            position = draw(
                frame,
                Rect::new(5, 2, 100, 20),
                31,
                30,
                &hunk,
                DiffSide::Unstaged,
                3,
                7,
                0,
                Some(31),
            );
        })
        .unwrap();
        let (popup, _) = position.unwrap();
        assert!(popup.width <= MAX_WIDTH);
        let display: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(display.contains("Changes") && display.contains("WORKTREE"));
        assert!(display.contains("Hunk 3/7") && display.contains("@@ -30,4"));
        assert!(
            display.contains("new 中 文") && display.contains("+1") && display.contains("−2"),
            "rendered peek: {display}"
        );
        // The two deleted lines share source anchor 31, but only the added
        // line should receive the selected-row background emphasis.
        let first_deleted_y = popup.y + 4;
        let second_deleted_y = popup.y + 5;
        let added_y = popup.y + 6;
        let x = popup.x + 12;
        let cells = term.backend().buffer();
        assert_eq!(
            cells[(x, first_deleted_y)].bg,
            cells[(x, second_deleted_y)].bg
        );
        assert_ne!(cells[(x, first_deleted_y)].bg, cells[(x, added_y)].bg);
        assert_eq!(cells[(popup.x, added_y)].symbol(), "┃");
        assert_eq!(cells[(popup.right() - 2, popup.y)].symbol(), "×");
    }

    #[test]
    fn popup_stays_in_pane_and_avoids_anchor_when_room_exists() {
        let hunk = Hunk {
            header: "@@".into(),
            lines: vec![(DiffLineKind::Add, "+x".into()); 32],
            row_lines: vec![Some(1); 32],
            marks: vec![],
        };
        let pane = Rect::new(10, 3, 120, 35);
        let near_top = rectangle(pane, 4, 3, &hunk).unwrap();
        assert_eq!(near_top.y, 5);
        let near_bottom = rectangle(pane, 30, 3, &hunk).unwrap();
        assert!(near_bottom.bottom() <= pane.y + 27);
        assert_eq!(near_top.width, MAX_WIDTH);
        assert!(rectangle(Rect::new(0, 0, 20, 4), 1, 0, &hunk).is_none());
    }
}

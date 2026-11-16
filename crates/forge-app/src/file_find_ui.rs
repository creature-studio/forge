//! File-local find overlay and source-coordinate match highlighting.
use super::*;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

pub(super) fn highlight_file_find_matches(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &App,
    theme: &forge_theme::Theme,
) {
    let find = &model.preview_state.find;
    if !find.open
        || find.matches.is_empty()
        || !matches!(model.preview_state.preview_mode, PreviewMode::File)
        || model.markdown_available() && !model.preview_state.markdown_code
    {
        return;
    }
    let cache = &model.preview_state.text_rows;
    let scroll = model.preview_state.preview_scroll;
    let bottom = scroll.saturating_add(area.height as usize);
    let mut index = find.matches.partition_point(|m| {
        cache
            .source_row_range(m.line)
            .is_some_and(|r| r.end <= scroll)
    });
    while index < find.matches.len() {
        let first = &find.matches[index];
        let Some(range) = cache.source_row_range(first.line) else {
            break;
        };
        if range.start >= bottom {
            break;
        }
        let stop = find.matches[index..].partition_point(|m| m.line == first.line) + index;
        if range.end > scroll {
            let mut byte_at = 0usize;
            let mut content_started = false;
            let mut current_hit = index;
            for visual_row in range {
                let Some(row) = cache.rows().get(visual_row) else {
                    continue;
                };
                let mut x = 0usize;
                for grapheme in row.styled_graphemes(Style::default()) {
                    let w = UnicodeWidthStr::width(grapheme.symbol);
                    if !content_started {
                        if grapheme.symbol == "│" {
                            content_started = true;
                        }
                        x += w;
                        continue;
                    }
                    let next = byte_at + grapheme.symbol.len();
                    if visual_row >= scroll && visual_row < bottom && w > 0 {
                        while current_hit < stop && find.matches[current_hit].end <= byte_at {
                            current_hit += 1;
                        }
                        if current_hit < stop
                            && find.matches[current_hit].start < next
                            && find.matches[current_hit].end > byte_at
                        {
                            let absolute_index = current_hit;
                            for dx in 0..w {
                                if x + dx < area.width as usize {
                                    let px = area.x + (x + dx) as u16;
                                    let py = area.y + (visual_row - scroll) as u16;
                                    if let Some(cell) = frame.buffer_mut().cell_mut((px, py)) {
                                        let active = find.active == Some(absolute_index);
                                        cell.set_bg(forge_ui::to_color(if active {
                                            theme.warning
                                        } else {
                                            theme.selection_bg
                                        }));
                                        if active {
                                            cell.set_fg(forge_ui::to_color(theme.panel_bg));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    byte_at = next;
                    x += w;
                }
            }
        }
        index = stop;
    }
}

pub(super) fn draw_file_find_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &App,
    p: Palette,
) -> Rect {
    let find = &model.preview_state.find;
    if !find.open || area.width < 14 || area.height < 3 {
        return Rect::default();
    }
    let width = area.width.min(68);
    let height = area.height.min(4);
    let rect = Rect::new(
        area.right().saturating_sub(width),
        area.y + u16::from(area.height > height),
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let border = if find.input_active {
        p.accent
    } else {
        p.border
    };
    frame.render_widget(
        Block::default()
            .title(" Find in file ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .style(Style::default().fg(p.fg).bg(p.bg)),
        rect,
    );
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    let status = if let Some(error) = &find.error {
        error.clone()
    } else if find.pending.is_some() {
        "Searching…".to_owned()
    } else if find.query.is_empty() {
        String::new()
    } else {
        format!(
            "{}/{}{}",
            find.active.map_or(0, |i| i + 1),
            find.matches.len(),
            if find.capped { "+" } else { "" }
        )
    };
    let status_w = UnicodeWidthStr::width(status.as_str());
    let input_w = inner.width as usize;
    let available = input_w.saturating_sub(status_w + 2);
    let cursor = find.cursor.min(find.query.len());
    let prefix = &find.query[..cursor];
    let suffix = &find.query[cursor..];
    let visible = if find.query.is_empty() {
        Line::from(Span::styled(" Search text", Style::default().fg(p.fg_dim)))
    } else {
        let mut start = 0;
        while UnicodeWidthStr::width(&find.query[start..]) > available.saturating_sub(2)
            && start < cursor
        {
            start += find.query[start..].chars().next().unwrap().len_utf8();
        }
        Line::from(vec![
            Span::styled(format!(" {}", &prefix[start..]), Style::default().fg(p.fg)),
            Span::styled(
                if find.input_active { "▏" } else { "" },
                Style::default().fg(p.accent),
            ),
            Span::styled(suffix.to_owned(), Style::default().fg(p.fg)),
        ])
    };
    frame.render_widget(
        Paragraph::new(visible),
        Rect::new(
            inner.x,
            inner.y,
            inner.width.saturating_sub(status_w as u16 + 1),
            1,
        ),
    );
    if status_w > 0 && status_w < input_w {
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(if find.error.is_some() {
                p.warning
            } else {
                p.fg_dim
            })),
            Rect::new(
                inner.right().saturating_sub(status_w as u16),
                inner.y,
                status_w as u16,
                1,
            ),
        );
    }
    if inner.height > 1 {
        let toggle = |active: bool| {
            Style::default()
                .fg(if active { p.on_accent } else { p.fg_dim })
                .bg(if active { p.accent } else { p.bg })
        };
        let controls = Line::from(vec![
            Span::raw(" "),
            Span::styled(" Aa ", toggle(find.match_case)),
            Span::raw(" "),
            Span::styled(" W ", toggle(find.whole_word)),
            Span::raw(" "),
            Span::styled(" .* ", toggle(find.regex)),
        ]);
        frame.render_widget(
            Paragraph::new(controls),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        let actions = "↑Prev  ↓Next  ×";
        let actions_w = UnicodeWidthStr::width(actions) as u16;
        if inner.width >= 34 {
            frame.render_widget(
                Paragraph::new(actions).style(Style::default().fg(p.accent)),
                Rect::new(
                    inner.right().saturating_sub(actions_w),
                    inner.y + 1,
                    actions_w,
                    1,
                ),
            );
        }
    }
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ui::{PreviewView, TextLang};
    use ratatui::{backend::TestBackend, Terminal};

    #[tokio::test]
    async fn find_highlights_match_after_cjk_wrap_without_coloring_gutter() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        let source = "前缀中文".to_string() + &"a".repeat(12) + "needle尾部";
        let start = source.find("needle").unwrap();
        app.preview_state.find.open = true;
        app.preview_state.find.path = dir.path().join("code.log");
        app.preview_state.find.matches = vec![super::file_find::FileMatch {
            line: 1,
            start,
            end: start + 6,
        }];
        app.preview_state.find.active = Some(0);
        let theme = forge_theme::Theme::modern_ide();
        app.preview_state
            .text_rows
            .ensure(&theme, &[source], false, 100, TextLang::Plain, 22);
        assert!(app.preview_state.text_rows.line_count() > 1);
        assert!(
            app.preview_state.text_rows.source_byte_to_row(1, start)
                > app.preview_state.text_rows.source_to_row(1),
            "visual find navigation must land on a continuation row"
        );
        let area = Rect::new(0, 0, 22, 5);
        let mut terminal = Terminal::new(TestBackend::new(22, 5)).unwrap();
        terminal
            .draw(|frame| {
                let mut scroll = 0;
                let mut cache = forge_ui::SplitRowsCache::default();
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::TextRows {
                        rows: app.preview_state.text_rows.rows(),
                    },
                    &mut scroll,
                    &mut cache,
                );
                highlight_file_find_matches(frame, area, &app, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let active_bg = forge_ui::to_color(theme.warning);
        let highlighted = buffer
            .content()
            .iter()
            .filter(|cell| cell.bg == active_bg)
            .map(|cell| cell.symbol().to_owned())
            .collect::<String>();
        assert_eq!(highlighted, "needle");
        assert_ne!(
            buffer.cell((4, 0)).unwrap().bg,
            active_bg,
            "gutter unaffected"
        );
    }
}

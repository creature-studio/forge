//! Code markers, blame badges and popup rendering.
use super::*;

enum MarkRow {
    File(PathBuf),
    Git(PathBuf, DiffSide),
    None,
}

/// Pick the richest inline attribution that fits without hiding source code.
/// Full metadata stays available in the reserved bottom row and F6 popup.
pub(super) fn compact_blame_label(
    full: &str,
    author: &str,
    age: &str,
    available: usize,
) -> Option<String> {
    use unicode_width::UnicodeWidthStr;
    let variants = [
        full.to_owned(),
        format!("{author} · {age}"),
        author.to_owned(),
    ];
    variants.into_iter().find(|text| text.width() <= available)
}

pub(super) fn draw_inline_blame(
    frame: &mut Frame<'_>,
    inner: Rect,
    fallback: Rect,
    model: &App,
    p: Palette,
) {
    use unicode_width::UnicodeWidthStr;
    let Some((blame, line)) = model.current_blame() else {
        return;
    };
    let label = if blame.uncommitted() {
        "Uncommitted changes".to_string()
    } else {
        format!(
            "{} · {} · {} {}",
            blame.author,
            super::blame::relative_time(blame.time),
            &blame.oid[..blame.oid.len().min(8)],
            blame.summary
        )
    };
    // Place the attribution after the current source row when space allows.
    // Full-width source stays intact; its attribution uses the reserved strip.
    let row = model
        .blame
        .cursor
        .as_ref()
        .filter(|(_, _, scroll)| *scroll == model.preview_state.preview_scroll)
        .map_or(model.preview_state.preview_scroll, |(_, row, _)| *row);
    let y = inner
        .y
        .saturating_add(row.saturating_sub(model.preview_state.preview_scroll) as u16);
    let mut end = inner.x;
    if y < inner.bottom() {
        for x in inner.x..inner.right() {
            if let Some(cell) = frame.buffer_mut().cell((x, y)) {
                if cell.symbol() != " " {
                    end = end.max(x.saturating_add(cell.symbol().width() as u16));
                }
            }
        }
    }
    let available = inner.right().saturating_sub(end.saturating_add(3));
    let short_age = if blame.uncommitted() {
        "modified".to_owned()
    } else {
        super::blame::relative_time(blame.time)
    };
    let short_author = if blame.uncommitted() {
        "Uncommitted"
    } else {
        blame.author.as_str()
    };
    let inline = if y < inner.bottom() {
        compact_blame_label(&label, short_author, &short_age, available as usize)
    } else {
        None
    };
    if let Some(text) = inline {
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(p.fg_dim)),
            Rect::new(end.saturating_add(3), y, available, 1),
        );
    }
    // The bottom strip remains the discoverable complete attribution,
    // regardless of whether a compact inline label could be shown.
    let full = format!("L{line}  {label}");
    frame.render_widget(
        Paragraph::new(full).style(Style::default().fg(p.fg_dim)),
        fallback,
    );
}

pub(crate) fn draw_blame_popup(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let Some(popup) = &mut model.blame.popup else {
        return;
    };
    frame.render_widget(Clear, area);
    let committed = !popup.oid.bytes().all(|b| b == b'0');
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", popup.title))
        .title_bottom(if committed {
            " Esc Close · d Diff · h History · c Copy SHA · F6 Details "
        } else {
            " Esc Close · Uncommitted changes "
        })
        .style(Style::default().fg(p.fg).bg(p.bg));
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if !popup.message.is_empty() && inner.height > 1 {
        frame.render_widget(
            Paragraph::new(popup.message.clone()).style(Style::default().fg(p.fg_dim)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
        inner.height -= 1;
    }
    let rows = popup
        .text
        .lines()
        .flat_map(|line| forge_ui::wrap_text_line(&Line::from(line.to_owned()), inner.width.max(1)))
        .collect::<Vec<_>>();
    popup.max_scroll = rows
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    popup.scroll = popup.scroll.min(popup.max_scroll);
    let visible = rows
        .into_iter()
        .skip(popup.scroll as usize)
        .take(inner.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), inner);
}

pub(super) fn draw_marks(frame: &mut Frame<'_>, model: &App, p: Palette) {
    let area = model.session.layout.list_inner;
    if area.width == 0 {
        return;
    }
    let rows: Vec<MarkRow> = if model.in_git_sidebar() {
        let root = model.repo_root();
        let (u, s) = model.scm_slices();
        let rows = model.scm_rows();
        let indices = model
            .scm
            .sticky_headers
            .iter()
            .copied()
            .chain(model.scm.scroll_scm..rows.len());
        indices
            .take(area.height as usize)
            .map(|index| match &rows[index] {
                ScmRow::Folder { path, column, .. } => root
                    .as_ref()
                    .map(|root| MarkRow::Git(root.join(path), column.side()))
                    .unwrap_or(MarkRow::None),
                row @ ScmRow::Change { .. } => scm_entry(row, u, s)
                    .and_then(|entry| {
                        root.as_ref()
                            .map(|root| MarkRow::Git(root.join(&entry.path), entry.side))
                    })
                    .unwrap_or(MarkRow::None),
                _ => MarkRow::None,
            })
            .collect()
    } else {
        let indices = model
            .files
            .sticky_headers
            .iter()
            .copied()
            .chain(model.files.scroll..model.files.visible.len());
        indices
            .take(area.height as usize)
            .map(|index| MarkRow::File(model.files.visible[index].entry.path.clone()))
            .collect()
    };
    for (offset, row) in rows.iter().take(area.height as usize).enumerate() {
        let marked = match row {
            MarkRow::File(path) => model.tools.marks.contains_file(path),
            MarkRow::Git(path, side) => model.tools.marks.contains_git(path, *side),
            MarkRow::None => false,
        };
        if marked {
            frame.render_widget(
                Paragraph::new("●").style(Style::default().fg(p.warning)),
                Rect::new(area.x, area.y + offset as u16, 1, 1),
            );
        }
    }
}

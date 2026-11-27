//! Tool panel, search results and settings rendering.
use super::*;

pub(super) fn draw_tools(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    if model
        .tools
        .panel
        .as_ref()
        .is_some_and(|panel| matches!(panel.kind, tools::PanelKind::Settings))
    {
        draw_settings(frame, area, model, p);
        return;
    }
    let escape_label = model.tools_escape_label();
    let loading = model.tools.pending_read.is_some();
    let context = model.tools.content_context.as_ref();
    let context_path = model.tools.context_path.as_deref();
    let Some(panel) = &mut model.tools.panel else {
        return;
    };
    if let Some(mode) = dialog::FormMode::from_panel(&panel.kind) {
        dialog::draw_form(frame, area, panel, p, mode);
        return;
    }
    if matches!(panel.kind, tools::PanelKind::Text) {
        dialog::draw_result(frame, area, panel, p);
        return;
    }
    panel.area = area;
    panel.form_cancel = Rect::default();
    panel.form_submit = Rect::default();
    let border_help = if matches!(panel.kind, tools::PanelKind::Content) {
        format!(" Esc {escape_label} · ↑/↓ Results · Ctrl+PgUp/PgDn Context ")
    } else if panel.rows.is_empty() {
        format!(" Esc {escape_label} ")
    } else {
        format!(" ↑/↓ Select · Enter Open · Esc {escape_label} ")
    };
    let inner = dialog::draw_shell(frame, area, &panel.title, &border_help, p);
    let input_height = if panel.is_input() { 2 } else { 0 };
    let list_height = if panel.rows.is_empty() {
        0
    } else if matches!(
        panel.kind,
        tools::PanelKind::Commands
            | tools::PanelKind::Files
            | tools::PanelKind::Workspaces
            | tools::PanelKind::Git(_)
    ) {
        // Compact dialogs devote space to the available choices rather than
        // a large empty context/details region.
        inner
            .height
            .saturating_sub(input_height + 3)
            .min(panel.rows.len().min(u16::MAX as usize) as u16)
            .max(1)
    } else {
        (inner.height.saturating_sub(input_height + 3) / 2).max(1)
    };
    let rows = Layout::vertical([
        Constraint::Length(input_height),
        Constraint::Length(1),
        Constraint::Length(list_height),
        Constraint::Min(0),
    ])
    .split(inner);
    if panel.is_input() {
        let value = panel.input.value();
        let cursor = panel.input.visual_cursor();
        let start = cursor.saturating_sub(rows[0].width.saturating_sub(4) as usize);
        let mut skipped = 0;
        let text: String = value
            .chars()
            .skip_while(|ch| {
                let width = unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0);
                if skipped + width <= start {
                    skipped += width;
                    true
                } else {
                    false
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(format!("> {text}")), rows[0]);
        if rows[0].width > 2 {
            frame.set_cursor_position((
                rows[0].x + 2 + (cursor.saturating_sub(skipped) as u16).min(rows[0].width - 3),
                rows[0].y,
            ));
        }
    }
    let message = if loading
        && !matches!(
            panel.kind,
            tools::PanelKind::Files | tools::PanelKind::Content
        ) {
        format!("Loading… {}", panel.message)
    } else {
        panel.message.clone()
    };
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(p.warning)),
        rows[1],
    );
    if matches!(panel.kind, tools::PanelKind::Content) {
        let body = Rect::new(
            inner.x,
            rows[2].y,
            inner.width,
            inner.bottom().saturating_sub(rows[2].y),
        );
        draw_content_search(
            frame,
            body,
            panel,
            model.files.root(),
            context_path,
            context,
            loading,
            p,
        );
        return;
    }
    panel.list_area = rows[2];
    panel.list_offset = panel.list_offset.min(
        panel
            .rows
            .len()
            .saturating_sub(panel.list_area.height as usize),
    );
    let offset = panel.list_offset;
    for (i, row) in panel
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows[2].height as usize)
    {
        let label: String = row
            .label
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let style = if i == panel.selected {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        frame.render_widget(
            Paragraph::new(format!(
                "{} {label}",
                if i == panel.selected { "›" } else { " " }
            ))
            .style(style),
            Rect::new(rows[2].x, rows[2].y + (i - offset) as u16, rows[2].width, 1),
        );
    }
    let detail_lines = wrap_detail(&panel.detail, rows[3].width.max(1) as usize);
    panel.scroll_limit = detail_lines.len().saturating_sub(rows[3].height as usize);
    let details = Paragraph::new(detail_lines);
    panel.scroll = panel.scroll.min(panel.scroll_limit);
    frame.render_widget(
        details
            .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0))
            .style(Style::default().fg(p.fg_dim)),
        rows[3],
    );
}

/// Crop by display cells around the first match, preserving UTF-8 byte offsets.
pub(super) fn content_line(
    text: &str,
    width: usize,
    matcher: Option<&forge_fs::workspace::ContentMatcher>,
    p: Palette,
) -> Line<'static> {
    let ranges = matcher.map(|m| m.ranges(text)).unwrap_or_default();
    let hit = ranges.first().map_or(0, |r| r.start);
    let before = unicode_width::UnicodeWidthStr::width(&text[..hit]);
    let skip = before.saturating_sub(width / 3);
    let mut cells = 0;
    let mut used = 0;
    let mut spans = Vec::new();
    if skip > 0 && width > 0 {
        spans.push(Span::raw("…"));
        used += 1;
    }
    for (byte, ch) in text.char_indices() {
        let ch = if ch.is_control() { ' ' } else { ch };
        let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        cells += size;
        if cells <= skip {
            continue;
        }
        if used + size > width {
            break;
        }
        used += size;
        let style = if ranges.iter().any(|r| r.contains(&byte)) {
            Style::default()
                .fg(p.bg)
                .bg(p.warning)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_content_search(
    frame: &mut Frame<'_>,
    area: Rect,
    panel: &mut tools::Panel,
    root: &Path,
    context_path: Option<&Path>,
    context: Option<&std::result::Result<forge_fs::workspace::ContentContext, String>>,
    updating: bool,
    p: Palette,
) {
    let panes = if area.width >= 95 {
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area)
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area)
    };
    let matcher =
        if updating || panel.message.starts_with("Failed:") || panel.input.value().is_empty() {
            None
        } else {
            forge_fs::workspace::ContentMatcher::new(panel.input.value(), panel.regex).ok()
        };
    let list_block = Block::default().borders(Borders::ALL).title(format!(
        " {} results{} ",
        panel.rows.len(),
        if updating { " · Updating" } else { "" }
    ));
    let list = list_block.inner(panes[0]);
    frame.render_widget(list_block, panes[0]);
    panel.list_area = list;
    panel.list_offset = panel
        .list_offset
        .min(panel.rows.len().saturating_sub(list.height as usize));
    let offset = panel.list_offset;
    for (index, row) in panel
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
    {
        let boundary = index == 0
            || match (&panel.rows[index - 1].value, &row.value) {
                (tools::RowValue::File(a, _), tools::RowValue::File(b, _)) => a != b,
                _ => true,
            };
        let style = if index == panel.selected {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        let mut line = content_line(
            &row.label,
            list.width.saturating_sub(2) as usize,
            matcher.as_ref(),
            p,
        );
        line.spans.insert(
            0,
            Span::raw(if index == panel.selected {
                "› "
            } else if boundary {
                "◆ "
            } else {
                "│ "
            }),
        );
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(list.x, list.y + (index - offset) as u16, list.width, 1),
        );
    }
    let selected = panel.rows.get(panel.selected).and_then(|r| match &r.value {
        tools::RowValue::File(path, line) => Some((path, *line, &r.label)),
        _ => None,
    });
    let title = selected
        .map(|(path, line, _)| {
            let count = panel
                .rows
                .iter()
                .filter(|r| matches!(&r.value, tools::RowValue::File(p, _) if p == path))
                .count();
            let column = context
                .filter(|_| context_path == Some(path.as_path()))
                .and_then(|c| c.as_ref().ok())
                .and_then(|c| c.lines.get(line.saturating_sub(1)))
                .and_then(|text| {
                    matcher.as_ref().and_then(|m| {
                        m.ranges(text)
                            .first()
                            .map(|r| text[..r.start].chars().count() + 1)
                    })
                })
                .map(|column| format!(":{column}"))
                .unwrap_or_default();
            format!(
                " {}:{line}{column} · {count} matches ",
                path.strip_prefix(root).unwrap_or(path).display()
            )
        })
        .unwrap_or_else(|| " Context ".into());
    let block = Block::default().borders(Borders::ALL).title(title);
    let preview = block.inner(panes[1]);
    frame.render_widget(block, panes[1]);
    let Some((path, selected_line, fallback)) = selected else {
        frame.render_widget(Paragraph::new("Type to search · Alt-R regex\nSelect a result to preview surrounding code\nEnter opens the exact line"), preview);
        return;
    };
    let context = if context_path == Some(path.as_path()) {
        context
    } else {
        None
    };
    match context {
        Some(Ok(context)) => {
            let height = preview.height as usize;
            let start = selected_line
                .saturating_sub(1)
                .saturating_sub((height / 2).min(5));
            panel.scroll_limit = context.lines.len().saturating_sub(start + height);
            panel.scroll = panel.scroll.min(panel.scroll_limit);
            for (index, text) in context
                .lines
                .iter()
                .enumerate()
                .skip(start + panel.scroll)
                .take(height)
            {
                let active = index + 1 == selected_line;
                let prefix = format!("{}{:>5} │ ", if active { "›" } else { " " }, index + 1);
                let mut line = content_line(
                    text,
                    preview.width.saturating_sub(9) as usize,
                    matcher.as_ref(),
                    p,
                );
                line.spans.insert(
                    0,
                    Span::styled(
                        prefix,
                        Style::default().fg(if active { p.accent } else { p.fg_dim }),
                    ),
                );
                let style = if active {
                    Style::default().bg(p.selection).fg(p.on_accent)
                } else {
                    Style::default().fg(p.fg)
                };
                frame.render_widget(
                    Paragraph::new(line).style(style),
                    Rect::new(
                        preview.x,
                        preview.y + (index - start - panel.scroll) as u16,
                        preview.width,
                        1,
                    ),
                );
            }
        }
        other => {
            let message = match other {
                Some(Err(error)) => error.as_str(),
                _ => "Loading context…",
            };
            frame.render_widget(
                Paragraph::new(format!("{message}\n{fallback}"))
                    .style(Style::default().fg(p.fg_dim)),
                preview,
            );
        }
    }
}

pub(super) fn draw_settings(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let escape_label = model.tools_escape_label();
    let Some(panel) = model.tools.panel.as_mut() else {
        return;
    };
    let border_help = format!(" Up/Down Select · Enter/Space Toggle · Esc {escape_label} ");
    let inner = dialog::draw_shell(frame, area, "Settings", &border_help, p);
    panel.area = area;
    panel.list_area = Rect::default();
    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let header = " APPEARANCE & NAVIGATION ";
    frame.render_widget(
        Paragraph::new(header).style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let rows_y = inner.y + 2;
    let visible = (inner.bottom().saturating_sub(rows_y)).min(9);
    panel.list_area = Rect::new(inner.x, rows_y, inner.width, visible);

    for i in 0..visible as usize {
        let chosen = i == panel.selected;
        let style = if chosen {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg).bg(p.bg)
        };
        let label = match i {
            0 => "Tree Sticky Scroll",
            1 => "Compact Folders",
            2 => "Show hidden files",
            3 => "Color theme",
            4 => "Code auto wrap",
            5 => "Focus Overview",
            6 => "Git Gutter: staged",
            7 => "Git Gutter: unstaged",
            _ => "Keyboard shortcuts",
        };
        let value = match i {
            0 => {
                if model.config.tree_sticky_scroll {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            1 => {
                if model.config.compact_folders {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            2 => {
                if model.config.show_hidden_files {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            3 => match model.config.theme.as_deref().unwrap_or("current") {
                "vscode-dark" => "[ VS Code Dark ]",
                "vscode-black" => "[ VS Code Black ]",
                "github-dark" => "[ GitHub Dark ]",
                "nord" => "[ Nord ]",
                _ => "[ Current ]",
            },
            4 => {
                if model.config.wrap_code {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            5 => {
                if model.config.show_overview_ruler {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            6 => {
                if model.config.git.show_staged_gutter {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            7 => {
                if model.config.git.show_unstaged_gutter {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            _ => "[ Open F4 ]",
        };
        let width = inner.width as usize;
        let content = format!(
            " {} {:width$} {}",
            if chosen { "›" } else { " " },
            label,
            value,
            width = width.saturating_sub(value.len() + 5).min(45)
        );
        frame.render_widget(
            Paragraph::new(content).style(style),
            Rect::new(inner.x, rows_y + i as u16, inner.width, 1),
        );
    }
    if inner.height >= 10 {
        let detail_y = rows_y + 10;
        let desc = match panel.selected {
            0 => "Pin expanded parent directories above the scrolled tree.\nApplies to Files, Git Changes/Staged and Commit History.",
            1 => "Join consecutive single-child folder paths into one row.\nApplies to Files, Git Changes/Staged and Commit History.",
            2 => "Show dotfiles in the Files tree.\n.git remains hidden; tracked changes in Git are unaffected.",
            3 => "Enter/Space cycles Current, VS Code Dark, VS Code Black, GitHub Dark and Nord.\nTheme changes apply immediately and persist across restarts.",
            4 => "Soft-wrap long source lines (default On). When Off, horizontal scrolling is enabled.\nAlt+Left/Right pans by eight columns; Alt+Home/End jumps to edges.",
            5 => "Show HEAD → Index staged changes in a secondary blue source gutter lane.\nCan overlap vivid unstaged markers; click the blue lane to peek.",
            6 => "Show Index → Working Tree changes in source line numbers.\nClick a changed number to inspect its unstaged hunk.",
            _ => "Open the searchable F4 keyboard shortcuts view.\nExisting custom bindings remain unchanged.",
        };
        let detail = format!(" DESCRIPTION\n\n{desc}\n\nChanges save to forge/config.toml and survive a restart.\n{}",
            if panel.message.is_empty() { "" } else { &panel.message });
        frame.render_widget(
            Paragraph::new(detail).style(Style::default().fg(p.fg_dim)),
            Rect::new(
                inner.x,
                detail_y,
                inner.width,
                inner.bottom().saturating_sub(detail_y),
            ),
        );
    }
}

pub(super) fn wrap_detail(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cells = 0;
        let mut part = String::new();
        for ch in line.chars() {
            let ch = if ch == '\t' { ' ' } else { ch };
            let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cells + size > width && !part.is_empty() {
                out.push(Line::from(std::mem::take(&mut part)));
                cells = 0;
            }
            part.push(ch);
            cells += size;
        }
        out.push(Line::from(part));
    }
    out
}

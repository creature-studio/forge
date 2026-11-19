//! Workspace headers, labels, tabs and contextual footer.
use super::*;

pub(super) fn draw_workspace_header(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    // The footer owns the branch and upstream divergence. Use the header
    // for complementary workspace counts instead of duplicating or cropping
    // a raw "## branch...origin" line.
    let branch = if area.width >= 70 {
        if let Some(status) = &model.scm.git_status {
            Line::from(vec![
                Span::styled(
                    format!("{} changed", status.unstaged.len()),
                    Style::default().fg(p.warning),
                ),
                Span::styled(
                    format!("  {} staged", status.staged.len()),
                    Style::default().fg(p.success),
                ),
            ])
        } else {
            Line::default()
        }
    } else {
        Line::default()
    };
    let right_width = branch
        .width()
        .min(area.width.saturating_sub(4) as usize / 2);
    let path_width = (area.width as usize)
        .saturating_sub(14 + right_width + if right_width > 0 { 2 } else { 0 });
    draw_balanced_line(
        frame,
        rows[0],
        Line::from(vec![
            Span::styled(
                "forge",
                Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  /  ", Style::default().fg(p.fg_dim)),
            Span::styled(
                fit_label(
                    &model.files.tree_root.display().to_string(),
                    path_width,
                    true,
                ),
                Style::default().fg(p.fg_dim),
            ),
        ]),
        branch,
        p,
    );

}

pub(super) fn draw_pane_heading(
    frame: &mut Frame<'_>,
    area: Rect,
    p: Palette,
    title: &str,
    detail: &str,
    focused: bool,
) -> Rect {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let title_style = Style::default().fg(if focused { p.fg } else { p.fg_dim });
    let detail_width = (area.width as usize).saturating_sub(title.len() + 7);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if focused { " › " } else { "   " },
                Style::default().fg(p.accent),
            ),
            Span::styled(
                title.to_owned(),
                if focused {
                    title_style.add_modifier(Modifier::BOLD)
                } else {
                    title_style
                },
            ),
            Span::styled(
                format!("  {}", fit_label(detail, detail_width, true)),
                Style::default().fg(p.fg_dim),
            ),
        ])),
        rows[0],
    );
    let body = rows[1];
    // A single blank row gives headings space on a normal-sized terminal.
    let gap = u16::from(area.height >= 6);
    Rect::new(
        body.x,
        body.y + gap,
        body.width,
        body.height.saturating_sub(gap),
    )
}

pub(super) fn draw_preview_heading(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) -> Rect {
    model.session.layout.markdown_preview_tab = Rect::default();
    model.session.layout.markdown_code_tab = Rect::default();
    model.session.layout.open_file_tab = Rect::default();
    let open_file_chip = if area.width >= 34
        && matches!(model.preview_state.preview_mode, PreviewMode::Diff)
        && model.preview_state.file_diff.as_ref().is_some_and(|diff| {
            diff.repo_root.join(&diff.path).is_file() && model.diff_matches_selection(diff)
        }) {
        14
    } else {
        0
    };
    let tabs_width = if model.markdown_available() && area.width >= 24 {
        area.width.saturating_sub(12).min(24)
    } else {
        0
    };
    let (mut detail, _) = preview_content(model);
    if model.inline_diff_hint() {
        detail = format!("F7 Peek · {detail}");
    }
    let body = draw_pane_heading(
        frame,
        Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(tabs_width + open_file_chip),
            area.height,
        ),
        p,
        if model.session.activity == ActivityTab::Git
            && matches!(model.selected_scm_row(), Some(ScmRow::Commit { .. }))
        {
            "Commit"
        } else if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
            "Diff"
        } else {
            "Preview"
        },
        &detail,
        model.session.focus == PaneFocus::Preview,
    );
    if open_file_chip > 0 {
        let rect = Rect::new(area.right() - open_file_chip, area.y, open_file_chip, 1);
        let hovered = model.activity_hits.is_hovered(rect);
        frame.render_widget(
            Paragraph::new(" [Open file] ").style(
                Style::default()
                    .fg(if hovered { p.on_accent } else { p.accent })
                    .bg(if hovered { p.accent } else { p.bg })
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            rect,
        );
        model.session.layout.open_file_tab = rect;
    }
    if tabs_width > 0 {
        draw_markdown_tabs(
            frame,
            Rect::new(
                area.right() - tabs_width,
                area.y,
                tabs_width,
                area.height.min(1),
            ),
            model,
            p,
        );
    }
    Rect::new(body.x, body.y, area.width, body.height)
}

pub(super) fn draw_markdown_tabs(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let compact = area.width < 19;
    let preview_width = area.width.min(if compact { 8 } else { 11 });
    let preview = Rect::new(area.x, area.y, preview_width, area.height.min(1));
    let code = Rect::new(
        preview.right(),
        area.y,
        area.width.saturating_sub(preview_width).min(8),
        area.height.min(1),
    );
    for (rect, label, active) in [
        (
            preview,
            if compact { "Preview" } else { " Preview " },
            !model.preview_state.markdown_code,
        ),
        (
            code,
            if compact { "Code" } else { " Code " },
            model.preview_state.markdown_code,
        ),
    ] {
        let style = if active {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.fg_dim).bg(p.bg)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
    }
    if area.width >= 24 {
        frame.render_widget(
            Paragraph::new("F3").style(Style::default().fg(p.fg_dim)),
            Rect::new(code.right() + 1, area.y, 2, area.height.min(1)),
        );
    }
    model.session.layout.markdown_preview_tab = preview;
    model.session.layout.markdown_code_tab = code;
}

/// A compact branch/tracking chip. Counts come only from the Git status
/// response for the *current* branch. No upstream = no invented ↓0 ↑0.
pub(super) fn git_status_chip(model: &App, p: Palette, max_width: usize) -> Option<(Line<'static>, usize)> {
    use super::git_sync_badge::GitSyncBadge;
    use unicode_width::UnicodeWidthStr;
    let badge = GitSyncBadge::parse(&model.scm.branch, &model.tools.sync)?;
    if model.scm.git_status.is_none() || max_width < 10 {
        return None;
    }

    let prefix = " ⎇ ";
    let suffix = " ↻ ";
    let counter = badge
        .upstream
        .map(|(behind, ahead)| format!(" ↓{behind} ↑{ahead}"));
    let missing = badge
        .upstream_missing
        .then_some(" upstream gone".to_owned());
    let candidate = counter.or(missing).unwrap_or_default();
    let available = max_width.saturating_sub(prefix.width() + suffix.width() + 1);
    let extra = if candidate.width() + 3 <= available {
        candidate
    } else {
        String::new()
    };
    let branch_width = available.saturating_sub(extra.width());
    if branch_width == 0 {
        return None;
    }
    let branch = fit_label(&badge.branch, branch_width, false);
    let mut spans = vec![
        Span::styled(prefix.to_string(), Style::default().fg(p.accent)),
        Span::styled(
            branch,
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if !extra.is_empty() {
        if let Some((behind, ahead)) = badge.upstream {
            spans.push(Span::styled(
                format!(" ↓{behind}"),
                Style::default().fg(p.fg_dim),
            ));
            spans.push(Span::styled(
                format!(" ↑{ahead}"),
                Style::default().fg(p.fg_dim),
            ));
        } else {
            spans.push(Span::styled(extra, Style::default().fg(p.warning)));
        }
    }
    spans.push(Span::styled(
        suffix.to_string(),
        Style::default().fg(p.accent),
    ));
    let line = Line::from(spans);
    let width = line.width();
    if width > max_width {
        return None;
    }
    Some((line, width))
}

/// The bottom row is a context-sensitive key guide, not a static key legend.
#[cfg(test)]
pub(super) fn file_find_has_focus(model: &App) -> bool {
    matches!(
        model.key_owner(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        input_router::KeyOwner::Find
    )
}

/// Keep the navigation and exit keys visible even in a narrow terminal.
/// The remaining space is filled with the file-find mode toggles. This
/// intentionally leaves out the usual global F4 / ? footer hints while
/// the find input owns keyboard focus.
pub(super) fn file_find_footer_shortcuts(model: &App, width: usize) -> Vec<interaction::Shortcut> {
    let navigation = [
        interaction::Shortcut { key: "Enter", label: "Next" },
        interaction::Shortcut { key: "Shift+Enter", label: "Prev" },
    ];
    let toggles = [
        interaction::Shortcut { key: "Alt+C", label: "Case" },
        interaction::Shortcut { key: "Alt+W", label: "Word" },
        interaction::Shortcut { key: "Alt+R", label: "Regex" },
    ];
    let close = interaction::Shortcut { key: "Esc", label: "Close" };
    let cost = |s: &interaction::Shortcut| model.keys.label(s.key).len() + s.label.len() + 5;
    let mut remaining = width.saturating_sub(1); // leading space in footer renderer
    let mut result = Vec::new();
    let close_cost = cost(&close);
    for hint in navigation.into_iter().chain(toggles) {
        let size = cost(&hint);
        if size + close_cost <= remaining {
            remaining -= size;
            result.push(hint);
        }
    }
    if close_cost <= remaining {
        result.push(close);
    }
    result
}

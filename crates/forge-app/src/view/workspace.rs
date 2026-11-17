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
    let tabs = if area.height > 1 { rows[1] } else { rows[0] };
    let widths = if area.width >= 32 {
        (14, 12)
    } else {
        (area.width.min(9), area.width.saturating_sub(9).min(7))
    };
    let files = Rect::new(tabs.x, tabs.y, widths.0, tabs.height.min(1));
    let git = Rect::new(files.right(), tabs.y, widths.1, tabs.height.min(1));
    for (rect, label, active) in [
        (
            files,
            if area.width >= 32 {
                " F1  Files "
            } else {
                " Files "
            },
            !model.in_git_sidebar(),
        ),
        (
            git,
            if area.width >= 32 {
                " F2  Git "
            } else {
                " Git "
            },
            model.in_git_sidebar(),
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
    // Keep Settings visible in the persistent tab strip rather than taking
    // space away from contextual footer hints (stage/unstage, history, etc.).
    if tabs.width >= 44 {
        frame.render_widget(
            Paragraph::new(" F9 Settings ").style(
                Style::default().fg(p.accent).bg(p.bg).add_modifier(Modifier::BOLD),
            ),
            Rect::new(git.right() + 1, tabs.y, 14, tabs.height.min(1)),
        );
    }
    model.session.layout.files_tab = files;
    model.session.layout.git_tab = git;
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
        frame.render_widget(
            Paragraph::new(" [Open file] ").style(
                Style::default()
                    .fg(p.accent)
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

pub(super) fn draw_workspace_footer(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) -> Rect {
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    if rows[0].height > 0 {
        let rule_height = u16::from(rows[0].height >= 2);
        if rule_height > 0 {
            frame.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(p.border)),
                rows[0],
            );
        }
        let status_area = Rect::new(area.x, rows[0].y + rule_height, area.width, 1);
        let context = model.context_description();
        let error_status =
            |s: &str| s.contains("error") || s.contains("failed:") || s == "empty commit message";
        let status = if error_status(&model.session.status) {
            &model.session.status
        } else {
            model.session.write_status.as_deref().unwrap_or_else(|| {
                if model.session.status.is_empty() {
                    &context
                } else {
                    &model.session.status
                }
            })
        };
        let is_error = error_status(status);
        let position = if model.session.focus == PaneFocus::Preview {
            format!("Preview  line {}", model.preview_state.preview_scroll + 1)
        } else if model.in_git_sidebar() {
            format!(
                "Git  {} / {}",
                model.scm.scm_selected + 1,
                model.scm_rows().len()
            )
        } else {
            String::new()
        };
        draw_balanced_line(
            frame,
            status_area,
            Line::from(Span::styled(
                status.to_owned(),
                Style::default().fg(if is_error { p.error } else { p.fg_dim }),
            )),
            Line::from(Span::styled(position, Style::default().fg(p.fg_dim))),
            p,
        );
    }
    if area.width < 10 {
        frame.render_widget(
            Paragraph::new(if model.session.commit_open {
                "Esc Cancel"
            } else if model.session.help_open {
                "? Close"
            } else {
                "? Help"
            }),
            rows[1],
        );
        return Rect::default();
    }
    // A persistent, clickable Git status chip lives in the bottom-left
    // status line, VS Code style. It only opens the Sync panel: never Push.
    let chip = if !model.session.commit_open
        && !model.session.help_open
        && model.session.context_menu.is_none()
        && !file_find_has_focus(model)
    {
        git_status_chip(model, p, (area.width as usize / 2).min(46))
    } else {
        None
    };
    let mut hitbox = Rect::default();
    let mut keys_area = rows[1];
    if let Some((content, width)) = chip {
        hitbox = Rect::new(keys_area.x, keys_area.y, width as u16, 1);
        frame.render_widget(Paragraph::new(content), hitbox);
        keys_area.x += width as u16;
        keys_area.width = keys_area.width.saturating_sub(width as u16);
    }
    let keys = footer_shortcuts(model, keys_area.width as usize);
    let mut spans = vec![Span::raw(" ")];
    for interaction::Shortcut { key, label } in keys {
        spans.push(Span::styled(
            format!(" {} ", model.keys.label(key)),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{label}  "),
            Style::default().fg(p.fg_dim),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), keys_area);
    hitbox
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
pub(super) fn file_find_has_focus(model: &App) -> bool {
    model.preview_state.find.open
        && model.preview_state.find.input_active
        && model.tools.panel.is_none()
        && !model.editing.active()
        && !model.session.help_open
        && !model.session.commit_open
        && model.session.context_menu.is_none()
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

pub(super) fn footer_shortcuts(model: &App, width: usize) -> Vec<interaction::Shortcut> {
    if file_find_has_focus(model) {
        return file_find_footer_shortcuts(model, width);
    }
    let help_label = if model.session.help_open {
        "Close"
    } else {
        "Help"
    };
    let reserved = if model.session.commit_open {
        1
    } else {
        help_label.len() + 7
    };
    let mut remaining = width.saturating_sub(reserved);
    let mut keys = Vec::new();
    let candidates = if model.session.commit_open {
        vec![
            interaction::Shortcut {
                key: "Enter",
                label: "Commit staged",
            },
            interaction::Shortcut {
                key: "Esc",
                label: "Cancel",
            },
        ]
    } else if model.session.help_open {
        vec![
            interaction::Shortcut {
                key: "Esc",
                label: "Close help",
            },
            interaction::Shortcut {
                key: "j/k",
                label: "Scroll help",
            },
        ]
    } else if model.session.context_menu.is_some() {
        vec![
            interaction::Shortcut {
                key: "Enter",
                label: "Run action",
            },
            interaction::Shortcut {
                key: "j/k",
                label: "Select action",
            },
            interaction::Shortcut {
                key: "Esc",
                label: "Close menu",
            },
        ]
    } else {
        let mut shortcuts = model.context_shortcuts();
        if (60..80).contains(&width) {
            shortcuts.insert(
                0,
                interaction::Shortcut {
                    key: "Tab",
                    label: if model.session.focus == PaneFocus::Preview {
                        "Files"
                    } else {
                        "Preview"
                    },
                },
            );
        }
        shortcuts
    };
    for shortcut in candidates {
        let size = model.keys.label(shortcut.key).len() + shortcut.label.len() + 5;
        if size <= remaining {
            remaining -= size;
            keys.push(shortcut);
        }
    }
    if !model.session.commit_open {
        if remaining >= 18 {
            keys.push(interaction::Shortcut {
                key: "F4",
                label: "Shortcuts",
            });
        }
        keys.push(interaction::Shortcut {
            key: "?",
            label: if width >= 10 { help_label } else { "" },
        });
    }
    keys
}

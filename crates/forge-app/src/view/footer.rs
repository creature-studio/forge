//! Context-aware footer model and renderer.
//! Focus resolution is separate from drawing; providers supply explanation
//! and actions while the view handles sizing, styles and Git sync hit testing.
use super::*;
use super::workspace::{git_status_chip, file_find_footer_shortcuts};

/// One resolved focus per frame, with modal ownership taking priority over
/// the underlying pane. Avoid relying on stale workspace status messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterContext {
    Commit, Help, Shortcuts, ContextMenu, Blame, Editor, Tools, Find,
    Terminal, FilesList, FilesPreview, GitList, GitPreview,
}

impl FooterContext {
    fn is_regular_pane(self) -> bool {
        matches!(self, Self::FilesList | Self::FilesPreview | Self::GitList | Self::GitPreview)
    }

    fn resolve(model: &App) -> Self {
        // Match the same modal ownership used by actual keyboard dispatch.
        // A passive Find bar is not a keyboard owner until its input is active.
        match model.key_owner(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)) {
            input_router::KeyOwner::Commit => return Self::Commit,
            input_router::KeyOwner::Shortcuts => return Self::Shortcuts,
            input_router::KeyOwner::Help => return Self::Help,
            input_router::KeyOwner::Blame => return Self::Blame,
            input_router::KeyOwner::Editor => return Self::Editor,
            input_router::KeyOwner::Tools => return Self::Tools,
            input_router::KeyOwner::Menu => return Self::ContextMenu,
            input_router::KeyOwner::Find => return Self::Find,
            input_router::KeyOwner::Terminal => return Self::Terminal,
            input_router::KeyOwner::Workspace => {}
        }
        match (model.in_git_sidebar(), !model.session.sidebar_visible || model.session.focus == PaneFocus::Preview) {
            (false,false) => Self::FilesList,
            (false,true) => Self::FilesPreview,
            (true,false) => Self::GitList,
            (true,true) => Self::GitPreview,
        }
    }
}

pub(super) struct FooterModel {
    pub context: FooterContext,
    pub description: String,
    pub position: String,
    pub is_error: bool,
}

impl FooterModel {
    fn resolve(model: &App) -> Self {
        let context = FooterContext::resolve(model);
        let description = match context {
            FooterContext::Commit => if model.session.commit_cancel_pending {
                "Unsaved commit draft · D discards · Esc keeps editing".into()
            } else if model.session.commit_confirming {
                "Commit review · Enter confirms · Esc returns to editing".into()
            } else {
                "Commit composer · edit subject/body · Ctrl+S review".into()
            },
            FooterContext::Help => "Help · Tab switch page · j/k scroll · Esc close".into(),
            FooterContext::Shortcuts => if model.keys.recording() {
                "Shortcut capture · press new key · Esc cancels capture".into()
            } else {
                "Shortcuts · Tab group · Enter rebind · Esc close".into()
            },
            FooterContext::ContextMenu => "Context menu · Enter select · Esc close".into(),
            FooterContext::Blame => "Commit details · j/k scroll · Esc close".into(),
            FooterContext::Editor => if model.editing.exit_confirmation() {
                "Unsaved changes · D discards · Esc keeps editing".into()
            } else if model.editing.has_selection() {
                "Editor selection · Esc clears selection".into()
            } else {
                "Editor · Ctrl+S save · Esc exit".into()
            },
            FooterContext::Tools => model.tools.panel.as_ref().map(|panel| panel.title.clone())
                .unwrap_or_else(|| "Tools".into()),
            FooterContext::Find => "Find in file · Enter next · Shift+Enter previous · Esc close".into(),
            FooterContext::Terminal => {
                let count = model.terminal_tabs.len()+1;
                let current = model.terminal.as_ref().map(|t|t.id);
                let mut ids = model.terminal_tabs.iter().map(|t|t.id).collect::<Vec<_>>();
                if let Some(id) = current { ids.push(id); }
                ids.sort_unstable();
                let position = ids.iter().position(|id|Some(*id)==current).unwrap_or(0)+1;
                format!("Terminal {position}/{count} · Esc belongs to Shell · F8 returns to Forge")
            }
            FooterContext::FilesPreview => {
                if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
                    if let Some(diff) = &model.preview_state.file_diff {
                        format!("Diff · {} · hunk {}/{}", diff.path,
                            model.preview_state.hunk_index.unwrap_or(0) + 1, diff.hunks().len())
                    } else { "Diff · loading".into() }
                } else {
                    format!("File preview · line {}", model.preview_state.preview_scroll+1)
                }
            }
            FooterContext::GitPreview => model.context_description(),
            FooterContext::FilesList | FooterContext::GitList => model.context_description(),
        };
        let error_status = |s: &str| {
            let s = s.to_ascii_lowercase();
            s.contains("error") || s.contains("failed:") || s == "empty commit message"
        };
        // A result popup already contains the operation's complete message.
        // Keep its title in the outer status area instead of repeating the
        // same success/error text a second time behind the modal.
        let result_dialog_open = context == FooterContext::Tools
            && model.tools.panel.as_ref().is_some_and(|p|
                matches!(p.kind, tools::PanelKind::Text));
        let notice = if result_dialog_open {
            None
        } else {
            model.session.write_status.as_deref()
                .or_else(|| error_status(&model.session.status)
                    .then_some(model.session.status.as_str()))
        };
        let is_error = notice.is_some_and(error_status);
        let description = notice.map(str::to_owned).unwrap_or(description);
        let position = match context {
            FooterContext::FilesPreview | FooterContext::GitPreview =>
                format!("Preview  line {}",model.preview_state.preview_scroll+1),
            FooterContext::GitList => format!("Git  {} / {}", model.scm.scm_selected+1,model.scm_rows().len()),
            _ => String::new(),
        };
        Self { context, description, position, is_error }
    }
}

pub(super) fn draw_workspace_footer(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) -> Rect {
    let presentation = FooterModel::resolve(model);
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
        draw_balanced_line(
            frame,
            status_area,
            Line::from(Span::styled(
                presentation.description.clone(),
                Style::default().fg(if presentation.is_error { p.error } else { p.fg_dim }),
            )),
            Line::from(Span::styled(presentation.position, Style::default().fg(p.fg_dim))),
            p,
        );
    }
    if area.width < 10 {
        frame.render_widget(
            Paragraph::new(if presentation.context == FooterContext::Commit {
                "Esc Cancel"
            } else if presentation.context == FooterContext::Help {
                "Esc Close"
            } else {
                "F5 Help"
            }),
            rows[1],
        );
        return Rect::default();
    }
    // A persistent, clickable Git status chip lives in the bottom-left
    // status line, VS Code style. It only opens the Sync panel: never Push.
    let chip = if presentation.context.is_regular_pane() {
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
    let keys = footer_shortcuts(model,keys_area.width as usize);
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


fn shortcut(key: &'static str, label: &'static str) -> interaction::Shortcut {
    interaction::Shortcut { key, label }
}

/// The footer describes operations on the currently focused pane, not the
/// global command catalog. Mutating Git actions still use stage_target's
/// existing eligibility checks through context_shortcuts().
fn focused_pane_actions(model: &App, context: FooterContext) -> Vec<interaction::Shortcut> {
    let mut actions = Vec::new();
    match context {
        FooterContext::FilesList => {
            if let Some(entry) = model.selected_entry() {
                actions.push(shortcut("Enter", if entry.kind == EntryKind::Directory {
                    "Toggle folder"
                } else { "Open file" }));
            }
            // Panel switch is essential in single-pane terminals; optional
            // multi-select and navigation hints can yield screen space.
            // Only advertise Left when there is an actual in-tree parent or
            // an expanded folder to collapse. At workspace root it is a no-op.
            if let Some(row) = model.files.visible.get(model.files.selected) {
                if row.expanded && matches!(row.entry.kind, EntryKind::Directory) {
                    actions.push(shortcut("Left", "Collapse"));
                } else if row.depth > 0 {
                    actions.push(shortcut("Left", "Parent"));
                }
            }
            actions.push(shortcut("Tab", "Preview"));
            if model.files.visible.len() > 1 { actions.push(shortcut("j/k", "Navigate")); }
            if model.selected_entry().is_some() { actions.push(shortcut("v", "Select")); }
            if let Some(label) = model.workspace_escape().label() {
                actions.push(shortcut("Esc", label));
            }
        }
        FooterContext::GitList => {
            if model.scm.pending_write.is_none() && model.tools.pending_task.is_none()
                && !model.editing.saving() {
                actions.extend(model.context_shortcuts().into_iter()
                    .filter(|candidate| matches!(candidate.key, "s" | "u")));
            }
            let row = model.selected_scm_row();
            let enter = match row.as_ref() {
                Some(ScmRow::Folder { expanded: true, .. }) => Some("Collapse folder"),
                Some(ScmRow::Folder { .. }) => Some("Expand folder"),
                Some(ScmRow::Change { .. }) => Some("Show diff"),
                Some(ScmRow::HistorySection { expanded: true, .. }) => Some("Hide history"),
                Some(ScmRow::HistorySection { .. }) => Some("Show history"),
                Some(ScmRow::Commit { index }) => Some(if model.history.commits.get(*index)
                    .is_some_and(|commit| model.history.expanded_commits.contains(&commit.id)) {
                    "Collapse commit"
                } else { "Expand commit" }),
                Some(ScmRow::CommitFolder { expanded: true, .. }) => Some("Collapse folder"),
                Some(ScmRow::CommitFolder { .. }) => Some("Expand folder"),
                Some(ScmRow::CommitFile { .. }) => Some("Show commit diff"),
                _ => None,
            };
            if let Some(label) = enter { actions.push(shortcut("Enter", label)); }
            actions.push(shortcut("Tab", "Preview"));
            if model.scm_rows().len() > 1 { actions.push(shortcut("j/k", "Navigate")); }
            if matches!(row, Some(ScmRow::Change { .. })) { actions.push(shortcut("v", "Select")); }
            if let Some(label) = model.workspace_escape().label() {
                actions.push(shortcut("Esc", label));
            }
        }
        FooterContext::FilesPreview | FooterContext::GitPreview => {
            let link_count = crate::view::commit_detail::file_link_count(model);
            if link_count > 0 {
                if model.preview_state.commit_link_focus.is_some() {
                    actions.push(shortcut("Enter", "Open link"));
                    actions.push(shortcut("Tab", "Next link"));
                    actions.push(shortcut("Shift+Tab", "Previous"));
                } else {
                    actions.push(shortcut("Tab", "Focus links"));
                }
            }
            let diff_ready = matches!(model.preview_state.preview_mode, PreviewMode::Diff)
                && !model.preview_state.preview_loading
                && model.preview_state.file_diff.is_some();
            if context == FooterContext::GitPreview && diff_ready {
                actions.extend(model.context_shortcuts().into_iter()
                    .filter(|candidate| matches!(candidate.key, "s" | "u")));
            }
            if diff_ready && model.preview_state.hunk_index.is_some()
                && model.preview_state.file_diff.as_ref().is_some_and(|d| d.hunks().len() > 1) {
                actions.push(shortcut("n/Alt+n", "Next/prev hunk"));
            }
            if model.session.sidebar_visible { actions.push(shortcut("Tab", "Focus list")); }
            if diff_ready || (model.preview_state.preview.is_some()
                && !model.preview_state.preview_loading) {
                actions.push(shortcut("j/k", "Scroll"));
            }
            if context == FooterContext::FilesPreview && model.markdown_available() {
                actions.push(shortcut("F3", if model.preview_state.markdown_code {
                    "Preview"
                } else { "Code" }));
            }
            if let Some(label) = model.workspace_escape().label() {
                actions.push(shortcut("Esc", label));
            }
            // Secondary preview actions must never crowd out the actionable
            // hunk navigation, explicit Back and sidebar restore hints.
            if link_count == 0 {
                if model.preview_state.selection.active() {
                    actions.push(shortcut("Ctrl+C", "Copy selection"));
                }
                if model.preview_state.preview.is_some() || model.preview_state.file_diff.is_some() {
                    actions.push(shortcut("Shift+Arrows", "Select text"));
                }
                if model.config.show_overview_ruler
                    && (model.overview_git_data().is_some_and(|(_, marks)| !marks.is_empty())
                        || model.preview_state.find.open
                            && !model.preview_state.find.matches.is_empty())
                {
                    actions.push(shortcut("Alt+↑/↓", "Next mark"));
                }
            }
        }
        _ => {}
    }
    if !model.session.sidebar_visible {
        let index = actions.iter().position(|candidate| candidate.key == "j/k")
            .map(|index| index + 1).unwrap_or(0);
        actions.insert(index, shortcut("Ctrl+B", "Show Sidebar"));
    }
    actions
}

fn focused_tools_actions(model: &App) -> Vec<interaction::Shortcut> {
    let Some(panel) = model.tools.panel.as_ref() else { return vec![] };
    let mut actions = Vec::new();
    match &panel.kind {
        tools::PanelKind::Prompt(_) | tools::PanelKind::Confirm(_) => {
            // This form already puts clickable Enter/Esc hints in its native
            // lower border. Do not repeat those actions in the global footer.
            // On a very narrow terminal the form cannot fit these hitboxes,
            // so keep the global hints as a keyboard-accessible fallback.
            if panel.form_submit.width > 0 {
                return actions;
            }
            let mode = dialog::FormMode::from_panel(&panel.kind).expect("form kind");
            actions.push(shortcut("Enter", mode.action_name(&panel.kind)));
        },
        tools::PanelKind::Content => {
            if panel.rows.len() > 1 { actions.push(shortcut("↑/↓", "Select result")); }
            if !panel.rows.is_empty() { actions.push(shortcut("Enter", "Open result")); }
            actions.push(shortcut("Ctrl+R", "Regex"));
        }
        tools::PanelKind::Files | tools::PanelKind::Commands | tools::PanelKind::Workspaces => {
            if panel.rows.len() > 1 { actions.push(shortcut("↑/↓", "Select result")); }
            if !panel.rows.is_empty() { actions.push(shortcut("Enter", "Open")); }
        }
        tools::PanelKind::Settings => {
            if panel.rows.len() > 1 { actions.push(shortcut("j/k", "Navigate")); }
            if !panel.rows.is_empty() { actions.push(shortcut("Enter", "Change setting")); }
        }
        tools::PanelKind::Text if panel.area.width >= 16 => {
            // The rounded result border already advertises Esc close.
            return actions;
        }
        _ => {
            if panel.rows.len() > 1 { actions.push(shortcut("j/k", "Navigate")); }
            if !panel.rows.is_empty() { actions.push(shortcut("Enter", "Select")); }
        }
    }
    let escape_label = model.tools_escape_label();
    actions.push(shortcut("Esc", escape_label));
    actions
}

pub(super) fn footer_shortcuts(model: &App, width: usize) -> Vec<interaction::Shortcut> {
    footer_shortcuts_for(model,FooterContext::resolve(model),width)
}

fn footer_shortcuts_for(model: &App, context: FooterContext, width: usize) -> Vec<interaction::Shortcut> {
    if context == FooterContext::Terminal {
        // Show context-sensitive terminal shortcuts in the normal footer,
        // rather than an unrelated Files/Git command bar.
        let tab_count = model.terminal_tabs.len() + usize::from(model.terminal.is_some());
        let mut candidates = vec![shortcut("Ctrl+Shift+Tab", "Focus Forge")];
        if tab_count > 1 {
            candidates.push(shortcut("Alt+1-8", "Tab"));
        }
        candidates.extend([
            shortcut("F4", "Help"),
            shortcut("F8", "Hide"),
        ]);
        if tab_count < 8 { candidates.push(shortcut("Shift+F8", "New")); }
        if tab_count > 1 { candidates.push(shortcut("F12", "Next")); }
        candidates.extend([
            shortcut("Ctrl+Shift+W", "Close"),
            shortcut("F11", "Max"),
        ]);
        let mut remaining = width.saturating_sub(2);
        let mut result = Vec::new();
        for candidate in candidates {
            let size = candidate.key.len() + candidate.label.len() + 5;
            if size <= remaining {
                result.push(candidate);
                remaining -= size;
            }
        }
        return result;
    }
    if context == FooterContext::Find {
        return file_find_footer_shortcuts(model, width);
    }
    let help_label = if model.session.help_open {
        "Close"
    } else {
        "Help"
    };
    // Only the leading space is mandatory. Help is a low-priority optional
    // hint, whereas Esc must not be crowded out by less important actions.
    let mut remaining = width.saturating_sub(1);
    let mut keys = Vec::new();
    let candidates = if context == FooterContext::Commit {
        if model.session.commit_cancel_pending {
            vec![shortcut("D", "Discard"), shortcut("Esc", "Keep editing")]
        } else if model.session.commit_confirming {
            vec![shortcut("Enter", "Confirm commit"), shortcut("Esc", "Edit")]
        } else {
            vec![shortcut("Ctrl+S", "Review"), shortcut("Tab", "Field"),
                shortcut("Esc", "Cancel")]
        }
    } else if context == FooterContext::Help {
        vec![
            shortcut("Esc", "Close help"),
            shortcut("Tab", "Switch page"),
            shortcut("j/k", "Scroll help"),
        ]
    } else if context == FooterContext::Shortcuts {
        if model.keys.recording() {
            vec![shortcut("Esc", "Cancel recording")]
        } else {
            vec![
                interaction::Shortcut { key: "Tab", label: "Group" },
                interaction::Shortcut { key: "Enter", label: "Rebind" },
                interaction::Shortcut { key: "Esc", label: "Close" },
            ]
        }
    } else if context == FooterContext::Editor {
        if model.editing.exit_confirmation() {
            vec![shortcut("D", "Discard"), shortcut("Esc", "Keep editing")]
        } else if model.editing.saving() {
            vec![]
        } else if model.editing.has_selection() {
            vec![shortcut("Ctrl+S", "Save"), shortcut("Esc", "Clear selection")]
        } else {
            vec![shortcut("Ctrl+S", "Save"), shortcut("Esc", "Exit")]
        }

    } else if context == FooterContext::Tools {
        focused_tools_actions(model)
    } else if context == FooterContext::Blame {
        vec![shortcut("j/k", "Scroll"), shortcut("Esc", "Close")]
    } else if context == FooterContext::ContextMenu {
        let mut actions = vec![];
        if let Some(menu) = model.session.context_menu.as_ref() {
            if menu.items.iter().filter(|item| item.enabled).count() > 1 {
                actions.push(shortcut("j/k", "Select"));
            }
            if menu.current_action().is_some() { actions.push(shortcut("Enter", "Run action")); }
        }
        actions.push(shortcut("Esc", "Close"));
        actions
    } else {
        focused_pane_actions(model, context)
    };
    // A collapsed sidebar already has Ctrl+B as its restore action. Do not
    // reserve a second back hint at the expense of Diff navigation or Ctrl+B.
    let reserve_escape = !context.is_regular_pane() || model.session.sidebar_visible
        || model.inline_peek_open() || model.preview_state.selection.active();
    let escape_cost = if reserve_escape {
        candidates.iter().find(|s| s.key == "Esc")
            .map(|s| model.keys.label(s.key).len() + s.label.len() + 5)
            .unwrap_or(0)
    } else {
        0
    };
    let mut escape_added = false;
    for shortcut in candidates {
        let size = model.keys.label(shortcut.key).len() + shortcut.label.len() + 5;
        // Prioritize an actionable Stage/Unstage target over a secondary
        // Back hint when a terminal is too narrow to display both.
        let primary_git_action = matches!(shortcut.key, "s" | "u")
            && !model.inline_peek_open() && !model.preview_state.selection.active();
        let reserve = if !escape_added && shortcut.key != "Esc" && !primary_git_action {
            escape_cost
        } else {
            0
        };
        if size.saturating_add(reserve) <= remaining {
            remaining -= size;
            if shortcut.key == "Esc" { escape_added = true; }
            keys.push(shortcut);
        }
    }
    if context.is_regular_pane() {
        let help = shortcut("F5", help_label);
        if model.keys.label(help.key).len() + help.label.len() + 5 <= remaining {
            keys.push(help);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[tokio::test]
    async fn left_hint_only_appears_when_tree_navigation_can_do_something() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.files.visible = vec![TreeRow {
            entry: Entry {
                id: forge_model::PathId(1),
                name: "top.txt".into(),
                path: dir.path().join("top.txt"),
                kind: EntryKind::File,
            },
            depth: 0,
            expanded: false,
        }];
        app.files.selected = 0;
        assert!(!focused_pane_actions(&app, FooterContext::FilesList).iter()
            .any(|hint| hint.key == "Left"), "root-level file has no parent in tree");

        app.files.visible[0].entry.kind = EntryKind::Directory;
        app.files.visible[0].expanded = true;
        assert!(focused_pane_actions(&app, FooterContext::FilesList).iter()
            .any(|hint| hint.key == "Left" && hint.label == "Collapse"));

        app.files.visible[0].expanded = false;
        app.files.visible[0].depth = 1;
        assert!(focused_pane_actions(&app, FooterContext::FilesList).iter()
            .any(|hint| hint.key == "Left" && hint.label == "Parent"));
    }

    #[tokio::test]
    async fn esc_footer_distinguishes_root_close_child_back_and_form_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.open_tools(tools::PanelKind::Settings);
        assert!(footer_shortcuts(&app, 110).iter().any(|s| s.key=="Esc" && s.label=="Close"));
        app.prompt(tools::PromptAction::OpenWorkspace,
            "Open workspace", String::new(), "Path");
        assert!(footer_shortcuts(&app, 110).iter().any(|s| s.key=="Esc" && s.label=="Cancel"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.tools.panel.as_ref().map(|p| &p.kind),
            Some(tools::PanelKind::Settings)));
        assert!(footer_shortcuts(&app, 110).iter().any(|s| s.key=="Esc" && s.label=="Close"));
    }

    #[tokio::test]
    async fn passive_file_find_shows_close_hint_and_esc_only_dismisses_find() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = false;
        assert_eq!(FooterContext::resolve(&app), FooterContext::FilesList);
        assert_eq!(app.workspace_escape(), input_router::WorkspaceEscape::CloseFind);
        assert!(footer_shortcuts(&app, 110).iter().any(|s|s.key=="Esc" && s.label=="Close find"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preview_state.find.open);
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(app.session.focus, PaneFocus::List);
    }

    #[tokio::test]
    async fn pane_footer_does_not_list_global_commands_or_hidden_controls() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        let global = [":", "Ctrl-P", "Ctrl-F", "q", "r", "F4", "G"];
        let files = footer_shortcuts_for(&app, FooterContext::FilesList, 120);
        assert!(files.iter().any(|s| s.key == "Tab"));
        assert!(files.iter().all(|s| !global.contains(&s.key)), "Files list is not a command catalog");
        app.session.focus = PaneFocus::Preview;
        app.session.sidebar_visible = false;
        let preview = footer_shortcuts_for(&app, FooterContext::FilesPreview, 120);
        assert!(preview.iter().any(|s| s.key == "Ctrl+B"));
        assert!(preview.iter().all(|s| s.key != "Esc"),
            "plain preview has nothing transient for Esc to dismiss");
        assert!(!preview.iter().any(|s| s.key == "Tab"), "no hidden-list focus hint");
        assert!(!preview.iter().any(|s| s.key == "n/Alt+n"), "not a Diff");
        assert!(preview.iter().all(|s| !global.contains(&s.key)));
        app.session.activity = ActivityTab::Git;
        app.session.sidebar_visible = true;
        app.session.focus = PaneFocus::List;
        let git = footer_shortcuts_for(&app, FooterContext::GitList, 120);
        assert!(git.iter().all(|s| !global.contains(&s.key)));
        assert!(git.iter().all(|s| !matches!(s.key, "s" | "u")), "no eligible stage target");
    }

    #[tokio::test]
    async fn tools_footer_only_shows_commands_the_panel_can_execute() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.open_tools(tools::PanelKind::Settings);
        assert_eq!(FooterContext::resolve(&app), FooterContext::Tools);
        let keys = footer_shortcuts(&app, 100);
        assert!(keys.iter().any(|s| s.key == "Esc"));
        assert!(keys.iter().all(|s| !matches!(s.key, "Ctrl-P" | "Ctrl-F" | ":" | "q")));
        app.tools.panel.as_mut().unwrap().rows.clear();
        let empty = footer_shortcuts(&app, 100);
        assert_eq!(empty.iter().map(|s| s.key).collect::<Vec<_>>(), vec!["Esc"]);
        app.open_tools(tools::PanelKind::Content);
        app.tools.panel.as_mut().unwrap().rows.clear();
        let searching = footer_shortcuts(&app, 100);
        assert!(searching.iter().any(|s| s.key == "Ctrl+R"));
        assert!(!searching.iter().any(|s| s.key == "Enter"), "no result to open");
    }

    #[tokio::test]
    async fn focused_terminal_explains_why_escape_does_not_leave_shell() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16,root.path().to_path_buf());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let description = FooterModel::resolve(&app);
        assert_eq!(description.context, FooterContext::Terminal);
        assert!(description.description.contains("Esc belongs to Shell"));
        let footer = footer_shortcuts(&app, 110);
        assert!(footer.iter().any(|s| s.key=="Ctrl+Shift+Tab" && s.label=="Focus Forge"));
        assert!(footer.iter().any(|s| s.key=="F8" && s.label=="Hide"));
        assert!(footer.iter().all(|s| s.key != "Esc"));
    }

    #[tokio::test]
    async fn focus_switch_drives_description_and_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16,dir.path().to_owned());
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        let files = FooterModel::resolve(&app);
        assert_eq!(files.context,FooterContext::FilesList);
        assert!(files.context.is_regular_pane());

        app.session.focus = PaneFocus::Preview;
        let preview = FooterModel::resolve(&app);
        assert_eq!(preview.context,FooterContext::FilesPreview);
        assert!(preview.description.contains("File preview"));
        assert!(preview.position.contains("Preview"));

        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        let git = FooterModel::resolve(&app);
        assert_eq!(git.context,FooterContext::GitList);
        assert!(git.position.contains("Git"));

        app.session.focus = PaneFocus::Preview;
        assert_eq!(FooterModel::resolve(&app).context,FooterContext::GitPreview);

        app.handle_key(KeyEvent::new(KeyCode::F(8),KeyModifiers::NONE));
        let terminal = FooterModel::resolve(&app);
        assert_eq!(terminal.context,FooterContext::Terminal);
        assert!(terminal.description.contains("Terminal"));
        assert!(!terminal.context.is_regular_pane());
        let keys = footer_shortcuts_for(&app,terminal.context,110);
        assert!(keys.iter().any(|s| s.key == "Shift+F8"));
        assert!(keys.iter().all(|s| s.key != "Alt+1-8" && s.key != "F12"),
            "single Terminal tab has no tab-switch operation");
        assert!(!keys.iter().any(|s|s.label=="Stage"));
    }

    #[tokio::test]
    async fn focused_diff_advertises_hunk_navigation_before_generic_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.session.sidebar_visible = false;
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.preview_state.preview_loading = false;
        app.preview_state.hunk_index = Some(0);
        app.preview_state.file_diff = Some(FileDiff {
            full_new_lines: None,
            repo_root: dir.path().to_owned(),
            path: "src/example.rs".into(),
            side: DiffSide::Unstaged,
            lines: vec![
                forge_model::DiffLine { kind: forge_model::DiffLineKind::Header,
                    text: "@@ -1 +1 @@".into() },
                forge_model::DiffLine { kind: forge_model::DiffLineKind::Add,
                    text: "+first".into() },
                forge_model::DiffLine { kind: forge_model::DiffLineKind::Header,
                    text: "@@ -10 +10 @@".into() },
                forge_model::DiffLine { kind: forge_model::DiffLineKind::Add,
                    text: "+second".into() },
            ],
        });
        assert_eq!(FooterContext::resolve(&app), FooterContext::GitPreview);
        let keys = footer_shortcuts_for(&app, FooterContext::GitPreview, 78);
        let find = |key: &str| keys.iter().position(|s| s.key == key);
        assert!(find("n/Alt+n").is_some(), "hunk navigation must survive narrow footer");
        assert!(find("j/k").is_some(), "scroll navigation must remain visible");
        assert!(find("Ctrl+k").is_none_or(|index| find("n/Alt+n").unwrap() < index),
            "Diff actions precede generic commands when both fit");
        assert!(find("Ctrl+B").is_some(), "collapsed Sidebar has a visible restore action");
        assert!(!keys.iter().any(|s| s.key == "Tab" && s.label == "Focus list"));
        let mut terminal = ratatui::Terminal::new(
            ratatui::backend::TestBackend::new(104, 3)).unwrap();
        terminal.draw(|frame| {
            draw_workspace_footer(frame, Rect::new(0, 0, 104, 3), &app,
                super::super::workspace_palette());
        }).unwrap();
        let footer = (0..104).map(|x| terminal.backend().buffer()[(x, 2)].symbol())
            .collect::<String>();
        assert!(footer.contains("n/Alt+n") && footer.contains("Next/prev hunk"),
            "visible Diff footer must advertise n/Alt+n: {footer}");
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.preview_state.hunk_index, Some(1));
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT));
        assert_eq!(app.preview_state.hunk_index, Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.session.sidebar_visible, "Esc must not alter sidebar visibility");
        assert_eq!(app.session.focus, PaneFocus::Preview);
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.session.sidebar_visible, "Ctrl+B explicitly restores the sidebar");
        assert_eq!(app.session.focus, PaneFocus::List);

        // In the list context there is no false promise of hunk navigation.
        app.session.focus = PaneFocus::List;
        app.session.sidebar_visible = true;
        let list_keys = footer_shortcuts_for(&app, FooterContext::GitList, 78);
        assert!(!list_keys.iter().any(|s| s.key == "n/Alt+n"));
        // Files can also open a Diff with `d`; Alt+n works there too.
        app.session.activity = ActivityTab::Files;
        app.session.focus = PaneFocus::Preview;
        let files_preview = footer_shortcuts_for(&app, FooterContext::FilesPreview, 78);
        assert!(files_preview.iter().any(|s| s.key == "n/Alt+n"));
    }

    #[tokio::test]
    async fn mouse_clicks_transfer_keyboard_focus_between_terminal_and_workspace() {
        use crossterm::event::{MouseButton,MouseEvent,MouseEventKind};
        let dir=tempfile::tempdir().unwrap();
        let mut app=create_workspace_at(ColorMode::Ansi16,dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8),KeyModifiers::NONE));
        app.session.layout.list=Rect::new(0,2,26,20);
        app.session.layout.list_inner=Rect::new(1,3,24,18);
        app.session.layout.preview=Rect::new(27,2,70,20);
        app.session.layout.preview_inner=Rect::new(29,3,66,18);
        app.session.layout.files_tab=Rect::new(1,1,12,1);
        app.session.layout.git_tab=Rect::new(13,1,12,1);
        let term=app.terminal.as_mut().unwrap();
        term.area=Rect::new(0,23,96,8);
        app.terminal_tab_bar=Rect::new(0,22,96,1);
        let click = |app: &mut App, x, y| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x, row: y, modifiers: KeyModifiers::NONE,
            });
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: x, row: y, modifiers: KeyModifiers::NONE,
            });
        };
        assert_eq!(FooterContext::resolve(&app),FooterContext::Terminal);
        click(&mut app, 4, 5); // Files sidebar
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        assert!(!app.terminal.as_ref().unwrap().focused);
        assert!(app.terminal.as_ref().unwrap().visible);
        click(&mut app, 35, 6); // Preview pane
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesPreview);
        click(&mut app, 5, 25); // Terminal body
        assert_eq!(FooterContext::resolve(&app),FooterContext::Terminal);
        click(&mut app, 5, 1); // active Files collapses Sidebar
        assert!(!app.terminal.as_ref().unwrap().focused);
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesPreview);
        click(&mut app, 5, 1); // second click restores Files Sidebar
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        click(&mut app, 17, 1); // Switch to Git via tab click
        assert_eq!(FooterContext::resolve(&app),FooterContext::GitList);

        // No foreground PTY may swallow clicks while an overlay is active.
        app.session.help_open=true;
        app.session.help_area=Rect::new(0,0,100,35);
        app.terminal.as_mut().unwrap().focused=false;
        click(&mut app, 5, 25);
        assert_eq!(FooterContext::resolve(&app),FooterContext::Help);
        assert!(!app.terminal.as_ref().unwrap().focused);
    }

    #[tokio::test]
    async fn shortcut_capture_footer_esc_label_matches_action() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        app.keys.open = true;
        app.keys.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.keys.recording());
        assert_eq!(FooterContext::resolve(&app), FooterContext::Shortcuts);
        let hints = footer_shortcuts(&app, 100);
        assert!(hints.iter().any(|h| h.key == "Esc" && h.label == "Cancel recording"));
        assert!(!hints.iter().any(|h| h.label == "Close"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.keys.recording());
        assert!(app.keys.open);
        assert!(footer_shortcuts(&app, 100).iter().any(|h| h.key == "Esc" && h.label == "Close"));
    }

    #[tokio::test]
    async fn commit_discard_footer_is_distinct_from_commit_review() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        app.session.commit_open = true;
        app.session.commit_cancel_pending = true;
        assert_eq!(FooterContext::resolve(&app), FooterContext::Commit);
        let hints = footer_shortcuts(&app, 100);
        assert!(hints.iter().any(|h| h.key == "D" && h.label == "Discard"));
        assert!(hints.iter().any(|h| h.key == "Esc" && h.label == "Keep editing"));
        assert!(!hints.iter().any(|h| h.label == "Confirm commit"));
    }

    #[tokio::test]
    async fn narrow_footer_prioritizes_return_over_secondary_actions() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        let list = footer_shortcuts_for(&app, FooterContext::GitList, 26);
        assert!(list.iter().all(|hint| hint.key != "Esc"),
            "Esc must not imply switching Files/Git at the workspace root");

        app.session.activity = ActivityTab::Files;
        app.session.focus = PaneFocus::Preview;
        let preview = footer_shortcuts_for(&app, FooterContext::FilesPreview, 40);
        assert!(preview.iter().any(|hint| hint.key == "Tab" && hint.label == "Focus list"),
            "Tab should own pane navigation");
        assert!(preview.iter().all(|hint| hint.key != "Esc"),
            "Plain preview must not advertise a false Esc back operation");

        app.session.help_open = true;
        let help = footer_shortcuts(&app, 100);
        assert!(help.iter().any(|hint| hint.key == "Tab" && hint.label == "Switch page"),
            "Help footer must describe its real Tab behavior");
        assert!(!help.iter().any(|hint| hint.key == "Ctrl-P"));
    }

    #[tokio::test]
    async fn git_preview_returns_focus_to_list_if_write_removes_selected_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.preview_state.preview_mode = PreviewMode::Diff;
        let mut initial = StatusSnapshot::new(dir.path().to_owned());
        initial.push_change("moved.rs", forge_model::GitStatus::Modified, DiffSide::Unstaged);
        app.scm.git_status = Some(initial);
        app.scm.scm_selected = app.scm_rows().iter()
            .position(|row| matches!(row, ScmRow::Change { .. }))
            .expect("fixture has one change");
        app.scm.pending_transfer = None; // Hunk writes do not use whole-file transfer.
        let mut after = StatusSnapshot::new(dir.path().to_owned());
        after.push_change("moved.rs", forge_model::GitStatus::Modified, DiffSide::Staged);
        app.apply_status_result(dir.path().to_owned(), Ok(Ok(Some(after))));
        if app.selected_scm_entry().is_none() {
            assert_eq!(app.session.focus, PaneFocus::List,
                "a heading must not keep invisible Preview focus");
        }
    }

    #[tokio::test]
    async fn result_popup_does_not_echo_its_message_in_workspace_footer() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        let message = "Renamed to /home/fxh/tools/forge/new.rs";
        app.session.write_status = Some(message.into());
        app.tools.panel = Some(tools::Panel::new(tools::PanelKind::Text, "Completed"));
        app.tools.panel.as_mut().unwrap().message = message.into();
        let footer = FooterModel::resolve(&app);
        assert_eq!(footer.context, FooterContext::Tools);
        assert_eq!(footer.description, "Completed");
        assert!(!footer.is_error);
        app.tools.panel = None;
        assert_eq!(FooterModel::resolve(&app).description, message);
    }

    #[tokio::test]
    async fn commit_and_editor_footer_reflect_their_current_substate() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.commit_open = true;
        let editor = footer_shortcuts(&app, 100);
        assert!(editor.iter().any(|s| s.key == "Ctrl+S" && s.label == "Review"));
        assert!(editor.iter().all(|s| s.key != "Enter"),
            "Enter edits the draft; it cannot commit without confirmation");
        app.session.commit_confirming = true;
        let confirming = footer_shortcuts(&app, 100);
        assert!(confirming.iter().any(|s| s.key == "Enter" && s.label == "Confirm commit"));
        assert!(confirming.iter().any(|s| s.key == "Esc" && s.label == "Edit"));
        assert!(!confirming.iter().any(|s| s.key == "Ctrl+S"));
    }

    #[tokio::test]
    async fn modal_focus_takes_priority_over_terminal_and_error_notifications() {
        let dir=tempfile::tempdir().unwrap();
        let mut app=create_workspace_at(ColorMode::Ansi16,dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8),KeyModifiers::NONE));
        app.session.help_open = true;
        assert_eq!(FooterContext::resolve(&app),FooterContext::Help);
        let keys=footer_shortcuts_for(&app,FooterContext::Help,110);
        assert!(keys.iter().any(|s|s.label=="Close help"));
        app.keys.open = true;
        assert_eq!(FooterContext::resolve(&app),FooterContext::Shortcuts);
        let keys=footer_shortcuts_for(&app,FooterContext::Shortcuts,110);
        assert!(keys.iter().any(|s|s.label=="Rebind"));
        app.keys.open = false;
        app.session.help_open = false;
        app.session.status="operation failed: write denied".into();
        let present=FooterModel::resolve(&app);
        assert!(present.is_error);
        assert!(present.description.contains("failed"));
    }
}

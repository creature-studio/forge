//! Context-aware footer model and renderer.
//! Focus resolution is separate from drawing; providers supply explanation
//! and actions while the view handles sizing, styles and Git sync hit testing.
use super::*;
use super::workspace::{git_status_chip, file_find_has_focus, file_find_footer_shortcuts};

/// One resolved focus per frame, with modal ownership taking priority over
/// the underlying pane. Avoid relying on stale workspace status messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterContext {
    Commit, Help, Shortcuts, ContextMenu, Editor, Tools, Find,
    Terminal, FilesList, FilesPreview, GitList, GitPreview,
}

impl FooterContext {
    fn is_regular_pane(self) -> bool {
        matches!(self, Self::FilesList | Self::FilesPreview | Self::GitList | Self::GitPreview)
    }

    fn resolve(model: &App) -> Self {
        if model.session.commit_open { return Self::Commit; }
        if model.keys.open { return Self::Shortcuts; }
        if model.session.help_open { return Self::Help; }
        if model.session.context_menu.is_some() { return Self::ContextMenu; }
        if model.editing.active() { return Self::Editor; }
        if model.tools.panel.is_some() { return Self::Tools; }
        if file_find_has_focus(model) { return Self::Find; }
        if model.terminal.as_ref().is_some_and(|t| t.visible && t.focused) { return Self::Terminal; }
        match (model.in_git_sidebar(),model.session.focus == PaneFocus::Preview) {
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
            FooterContext::Commit => "Commit editor · Enter commit · Esc cancel".into(),
            FooterContext::Help => "Help · Tab switch page · j/k scroll · Esc close".into(),
            FooterContext::Shortcuts => "Shortcuts · Tab group · Enter rebind · Esc close".into(),
            FooterContext::ContextMenu => "Context menu · Enter select · Esc close".into(),
            FooterContext::Editor => "Editor · edit content · Esc back".into(),
            FooterContext::Tools => "Tools panel · follow focused controls".into(),
            FooterContext::Find => "Find in file · Enter next · Shift+Enter previous · Esc close".into(),
            FooterContext::Terminal => {
                let count = model.terminal_tabs.len()+1;
                let current = model.terminal.as_ref().map(|t|t.id);
                let mut ids = model.terminal_tabs.iter().map(|t|t.id).collect::<Vec<_>>();
                if let Some(id) = current { ids.push(id); }
                ids.sort_unstable();
                let position = ids.iter().position(|id|Some(*id)==current).unwrap_or(0)+1;
                format!("Terminal {position}/{count} · Alt+1–8 jump · F4 help")
            }
            FooterContext::FilesPreview => {
                // The list selection is not the keyboard owner when Preview
                // is focused, so describe the preview rather than file actions.
                format!("File preview · line {} · j/k scroll · Tab file actions",
                    model.preview_state.preview_scroll+1)
            }
            FooterContext::GitPreview => model.context_description(),
            FooterContext::FilesList | FooterContext::GitList => model.context_description(),
        };
        let error_status = |s: &str| {
            let s = s.to_ascii_lowercase();
            s.contains("error") || s.contains("failed:") || s == "empty commit message"
        };
        let notice = model.session.write_status.as_deref()
            .or_else(|| error_status(&model.session.status).then_some(model.session.status.as_str()));
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


pub(super) fn footer_shortcuts(model: &App, width: usize) -> Vec<interaction::Shortcut> {
    footer_shortcuts_for(model,FooterContext::resolve(model),width)
}

fn footer_shortcuts_for(model: &App, context: FooterContext, width: usize) -> Vec<interaction::Shortcut> {
    if context == FooterContext::Terminal {
        // Show context-sensitive terminal shortcuts in the normal footer,
        // rather than an unrelated Files/Git command bar.
        let candidates = [
            interaction::Shortcut { key: "Alt+1-8", label: "Tab" },
            interaction::Shortcut { key: "F12", label: "Next" },
            interaction::Shortcut { key: "Shift+F8", label: "New" },
            interaction::Shortcut { key: "F4", label: "Help" },
            interaction::Shortcut { key: "Ctrl+Shift+W", label: "Close" },
            interaction::Shortcut { key: "F11", label: "Max" },
            interaction::Shortcut { key: "F8", label: "Hide" },
        ];
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
    let reserved = if model.session.commit_open {
        1
    } else {
        help_label.len() + 7
    };
    let mut remaining = width.saturating_sub(reserved);
    let mut keys = Vec::new();
    let candidates = if context == FooterContext::Commit {
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
    } else if context == FooterContext::Help {
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
    } else if context == FooterContext::Shortcuts {
        vec![
            interaction::Shortcut { key: "Tab", label: "Group" },
            interaction::Shortcut { key: "Enter", label: "Rebind" },
            interaction::Shortcut { key: "Esc", label: "Close" },
        ]
    } else if context == FooterContext::Editor {
        vec![
            interaction::Shortcut { key: "Ctrl+S", label: "Save" },
            interaction::Shortcut { key: "Esc", label: "Back" },
        ]
    } else if context == FooterContext::Tools {
        vec![
            interaction::Shortcut { key: "Tab", label: "Next field" },
            interaction::Shortcut { key: "Esc", label: "Back" },
        ]
    } else if context == FooterContext::ContextMenu {
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
                        "List"
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
    if context.is_regular_pane() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
        assert!(keys.iter().any(|s|s.key=="Alt+1-8"));
        assert!(!keys.iter().any(|s|s.label=="Stage"));
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
        let click=|x,y| MouseEvent {kind:MouseEventKind::Down(MouseButton::Left),
            column:x,row:y,modifiers:KeyModifiers::NONE};
        assert_eq!(FooterContext::resolve(&app),FooterContext::Terminal);
        app.handle_mouse(click(4,5)); // Files sidebar
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        assert!(!app.terminal.as_ref().unwrap().focused);
        assert!(app.terminal.as_ref().unwrap().visible);
        app.handle_mouse(click(35,6)); // Preview pane
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesPreview);
        app.handle_mouse(click(5,25)); // Terminal body
        assert_eq!(FooterContext::resolve(&app),FooterContext::Terminal);
        app.handle_mouse(click(5,1)); // Files tab
        assert!(!app.terminal.as_ref().unwrap().focused);
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        app.session.focus=PaneFocus::Preview;
        app.handle_mouse(click(5,1)); // Even the already-selected Files tab focuses the list
        assert_eq!(FooterContext::resolve(&app),FooterContext::FilesList);
        app.handle_mouse(click(17,1)); // Switch to Git via tab click
        assert_eq!(FooterContext::resolve(&app),FooterContext::GitList);

        // No foreground PTY may swallow clicks while an overlay is active.
        app.session.help_open=true;
        app.session.help_area=Rect::new(0,0,100,35);
        app.terminal.as_mut().unwrap().focused=false;
        app.handle_mouse(click(5,25));
        assert_eq!(FooterContext::resolve(&app),FooterContext::Help);
        assert!(!app.terminal.as_ref().unwrap().focused);
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

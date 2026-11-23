//! Keyboard ownership and workspace Esc policy.
//! Dialogs and editors own local cancellation. Only the workspace escape
//! decision is shared with the context-sensitive footer.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyOwner {
    Commit,
    Shortcuts,
    Help,
    Blame,
    Editor,
    Tools,
    Menu,
    Find,
    Terminal,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceEscape {
    CloseFind,
    ClosePeek,
    ClearPreviewSelection,
    ClearCommitLinkFocus,
    ClearMarks,
    CloseFileDiff,
    None,
}

impl WorkspaceEscape {
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::CloseFind => Some("Close find"),
            Self::ClosePeek => Some("Close peek"),
            Self::ClearPreviewSelection | Self::ClearMarks => Some("Clear selection"),
            Self::ClearCommitLinkFocus => Some("Unfocus link"),
            Self::CloseFileDiff => Some("Close diff"),
            Self::None => None,
        }
    }
}

/// Prevent undefined modifier chords from becoming destructive plain-letter
/// commands (Ctrl+Q => q quit, Ctrl+X => x discard, etc.).
pub(super) fn workspace_modifier_allowed(key: KeyEvent) -> bool {
    let mods = key.modifiers;
    if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return true;
    }
    match key.code {
        KeyCode::Char('b' | 'B' | 'p' | 'k' | 'c' | 'C' | 'r' | 'R')
            if mods == KeyModifiers::CONTROL =>
        {
            true
        }
        KeyCode::Char('f' | 'F')
            if mods == KeyModifiers::CONTROL
                || mods == KeyModifiers::CONTROL | KeyModifiers::SHIFT =>
        {
            true
        }
        KeyCode::Char('o') if mods == KeyModifiers::ALT => true,
        KeyCode::Up | KeyCode::Down if mods == KeyModifiers::ALT => true,
        KeyCode::Left | KeyCode::Right
            if mods == KeyModifiers::ALT || mods == KeyModifiers::CONTROL | KeyModifiers::ALT =>
        {
            true
        }
        KeyCode::Home | KeyCode::End if mods == KeyModifiers::ALT => true,
        _ => false,
    }
}

impl App {
    pub(super) fn key_owner(&self, key: KeyEvent) -> KeyOwner {
        if self.session.commit_open {
            return KeyOwner::Commit;
        }
        if self.keys.open {
            return KeyOwner::Shortcuts;
        }
        if self.session.help_open {
            return KeyOwner::Help;
        }
        if self.blame.popup.is_some() {
            return KeyOwner::Blame;
        }
        if self.editing.active() {
            return KeyOwner::Editor;
        }
        if self.tools.panel.is_some() {
            return KeyOwner::Tools;
        }
        if self.session.context_menu.is_some() {
            return KeyOwner::Menu;
        }
        // The active find input owns keys even over a stale focused PTY.
        if self.preview_state.find.open
            && self.preview_state.find.input_active
            && !matches!(key.code, KeyCode::F(1) | KeyCode::F(2))
        {
            return KeyOwner::Find;
        }
        // A *passive* find bar must not steal Esc from a focused terminal;
        // Esc belongs to the child application running inside the PTY.
        if self
            .terminal
            .as_ref()
            .is_some_and(|t| t.visible && t.focused)
        {
            return KeyOwner::Terminal;
        }
        if self.preview_state.find.open && matches!(key.code, KeyCode::Esc | KeyCode::F(3)) {
            return KeyOwner::Find;
        }
        KeyOwner::Workspace
    }

    fn has_focused_marks(&self) -> bool {
        self.session.focus != PaneFocus::Preview
            && if self.in_git_sidebar() {
                !self.tools.marks.git.is_empty()
            } else {
                !self.tools.marks.files.is_empty()
            }
    }

    pub(super) fn workspace_escape(&self) -> WorkspaceEscape {
        if self.preview_state.find.open && !self.preview_state.find.input_active {
            // A passive find bar does not own navigation keys, but Esc still
            // dismisses it. This is the same rule advertised in the footer.
            WorkspaceEscape::CloseFind
        } else if self.session.focus == PaneFocus::Preview && self.inline_peek_open() {
            WorkspaceEscape::ClosePeek
        } else if self.session.focus == PaneFocus::Preview
            && self.preview_state.commit_link_focus.is_some()
        {
            WorkspaceEscape::ClearCommitLinkFocus
        } else if self.session.focus == PaneFocus::Preview && self.preview_state.selection.active()
        {
            WorkspaceEscape::ClearPreviewSelection
        } else if self.has_focused_marks() {
            WorkspaceEscape::ClearMarks
        } else if self.session.focus == PaneFocus::Preview
            && !self.in_git_sidebar()
            && matches!(self.preview_state.preview_mode, PreviewMode::Diff)
        {
            WorkspaceEscape::CloseFileDiff
        } else {
            // A focused file/Diff preview is not a modal. Esc never navigates
            // between panes or Activities: Tab or a deliberate click owns
            // focus changes.
            WorkspaceEscape::None
        }
    }

    pub(super) fn handle_workspace_escape(&mut self) {
        match self.workspace_escape() {
            WorkspaceEscape::CloseFind => self.close_file_find(),
            WorkspaceEscape::ClosePeek => {
                self.close_inline_peek();
            }
            WorkspaceEscape::ClearCommitLinkFocus => {
                self.preview_state.commit_link_focus = None;
            }
            WorkspaceEscape::ClearPreviewSelection => {
                self.preview_state.selection.clear();
                self.session.status = "Preview selection cleared".into();
            }
            WorkspaceEscape::ClearMarks => {
                if self.in_git_sidebar() {
                    self.tools.marks.git.clear();
                } else {
                    self.tools.marks.files.clear();
                }
                self.session.status = "Selection cleared".into();
            }
            WorkspaceEscape::CloseFileDiff => {
                // Closing a preview submode must not *also* change focus and
                // expand the sidebar. One Esc undoes only one interaction.
                self.services.jobs.cancel_key("git:diff");
                self.preview_state.pending_diff = None;
                self.preview_state.preview_mode = PreviewMode::File;
                self.clear_diff_view();
                self.request_preview_for_selection();
            }
            WorkspaceEscape::None => {}
        }
    }
}

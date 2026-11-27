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

/// Existing gesture owners are the source of truth. No second capture
/// flag is maintained in App or in the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerCapture {
    Splitter,
    Terminal,
    Scrollbar,
    PreviewSelection,
    Activity,
}

impl PointerCapture {
    pub(super) fn resolve(
        splitter: bool,
        terminal: bool,
        scrollbar: bool,
        preview: bool,
        activity: bool,
    ) -> Option<Self> {
        [
            (splitter, Self::Splitter),
            (terminal, Self::Terminal),
            (scrollbar, Self::Scrollbar),
            (preview, Self::PreviewSelection),
            (activity, Self::Activity),
        ]
        .into_iter()
        .find_map(|(active, owner)| active.then_some(owner))
    }
}

/// Immutable routing inputs. Resolving a key never mutates a domain.
#[derive(Default)]
struct InputContext {
    modal: Option<KeyOwner>,
    find_open: bool,
    find_active: bool,
    terminal_focused: bool,
}

impl InputContext {
    fn typing_owner(&self) -> KeyOwner {
        if let Some(modal) = self.modal {
            return modal;
        }
        if self.find_open && self.find_active {
            return KeyOwner::Find;
        }
        if self.terminal_focused {
            return KeyOwner::Terminal;
        }
        KeyOwner::Workspace
    }

    fn owner(&self, key: KeyEvent) -> KeyOwner {
        let owner = self.typing_owner();
        if self.modal.is_some() {
            return owner;
        }
        if owner == KeyOwner::Find && matches!(key.code, KeyCode::F(1) | KeyCode::F(2)) {
            return if self.terminal_focused {
                KeyOwner::Terminal
            } else {
                KeyOwner::Workspace
            };
        }
        if owner == KeyOwner::Workspace
            && self.find_open
            && matches!(key.code, KeyCode::Esc | KeyCode::F(3))
        {
            return KeyOwner::Find;
        }
        owner
    }
}

impl App {
    pub(super) fn modal_owner(&self) -> Option<KeyOwner> {
        [
            (self.session.commit_open, KeyOwner::Commit),
            (self.keys.open, KeyOwner::Shortcuts),
            (self.session.help_open, KeyOwner::Help),
            (self.blame.popup.is_some(), KeyOwner::Blame),
            (self.editing.active(), KeyOwner::Editor),
            (self.tools.panel.is_some(), KeyOwner::Tools),
            (self.session.context_menu.is_some(), KeyOwner::Menu),
        ]
        .into_iter()
        .find_map(|(active, owner)| active.then_some(owner))
    }

    pub(super) fn modal_open(&self) -> bool {
        self.modal_owner().is_some()
    }

    fn input_context(&self) -> InputContext {
        InputContext {
            modal: self.modal_owner(),
            find_open: self.preview_state.find.open,
            find_active: self.preview_state.find.input_active,
            terminal_focused: self
                .terminals
                .active
                .as_ref()
                .is_some_and(|t| t.is_visible() && t.is_focused()),
        }
    }

    pub(super) fn key_owner(&self, key: KeyEvent) -> KeyOwner {
        self.input_context().owner(key)
    }

    /// Text/paste/cursor use the same owner without inventing a key event.
    pub(super) fn typing_owner(&self) -> KeyOwner {
        self.input_context().typing_owner()
    }

    /// Exclusive base input handoff. Modal ownership is an independent
    /// temporary overlay and does not destroy the saved workspace focus.
    fn transfer_input(&mut self, owner: KeyOwner) {
        match owner {
            KeyOwner::Find if self.preview_state.find.open => {
                self.preview_state.find.input_active = true;
                if let Some(term) = &mut self.terminals.active {
                    term.set_focused(false);
                }
                self.session.focus = PaneFocus::Preview;
            }
            KeyOwner::Terminal => {
                if let Some(term) = &mut self.terminals.active {
                    term.set_focused(true);
                    if term.is_focused() {
                        self.preview_state.find.input_active = false;
                    }
                }
            }
            KeyOwner::Workspace => {
                self.preview_state.find.input_active = false;
                if let Some(term) = &mut self.terminals.active {
                    term.set_focused(false);
                }
            }
            _ => {}
        }
    }

    pub(super) fn focus_find_input(&mut self) {
        self.transfer_input(KeyOwner::Find);
    }

    pub(super) fn focus_terminal_input(&mut self) {
        self.transfer_input(KeyOwner::Terminal);
    }

    pub(super) fn focus_workspace_input(&mut self) {
        self.transfer_input(KeyOwner::Workspace);
    }

    pub(super) fn release_terminal_input(&mut self) {
        if let Some(term) = &mut self.terminals.active {
            term.set_focused(false);
        }
    }

    pub(super) fn blur_find_input(&mut self) {
        self.preview_state.find.input_active = false;
    }

    /// Session activation is owned by TerminalSessions; reconcile its
    /// successful activation through the same exclusive handoff.
    pub(super) fn reconcile_terminal_focus(&mut self) {
        if self
            .terminals
            .active
            .as_ref()
            .is_some_and(|t| t.is_visible() && t.is_focused())
        {
            self.focus_terminal_input();
        }
    }

    pub(super) fn handle_paste(&mut self, text: &str) -> bool {
        match self.typing_owner() {
            KeyOwner::Shortcuts => self.keys.paste(text),
            KeyOwner::Editor => self.handle_editor_paste(text),
            KeyOwner::Tools => {
                if self.tools.panel.as_ref().is_some_and(|p| p.is_input()) {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.handle_tools_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                    }
                }
            }
            KeyOwner::Find => self.handle_file_find_paste(text),
            KeyOwner::Terminal => {
                if let Some(term) = &mut self.terminals.active {
                    if let Err(error) = term.paste(text) {
                        term.set_focused(false);
                        self.session.status = format!("Terminal paste failed: {error}");
                    }
                }
            }
            _ => return false,
        }
        true
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
        } else if self.session.focus == PaneFocus::Preview
            && self.preview_state.presentation.selection.active()
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
                self.preview_state.presentation.selection.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_owner_matches_ordinary_keys_for_every_focus_combination() {
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        for modal in [
            None,
            Some(KeyOwner::Editor),
            Some(KeyOwner::Menu),
            Some(KeyOwner::Commit),
        ] {
            for find_open in [false, true] {
                for find_active in [false, true] {
                    for terminal_focused in [false, true] {
                        let ctx = InputContext {
                            modal,
                            find_open,
                            find_active,
                            terminal_focused,
                        };
                        let expected = modal.unwrap_or(if find_open && find_active {
                            KeyOwner::Find
                        } else if terminal_focused {
                            KeyOwner::Terminal
                        } else {
                            KeyOwner::Workspace
                        });
                        assert_eq!(ctx.typing_owner(), expected);
                        assert_eq!(ctx.owner(key), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn input_owner_is_independent_of_unfocused_domain_state() {
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let mut context = InputContext {
            find_open: true,
            terminal_focused: true,
            ..Default::default()
        };
        assert_eq!(context.owner(esc), KeyOwner::Terminal);
        context.find_active = true;
        assert_eq!(context.owner(esc), KeyOwner::Find);
        context.modal = Some(KeyOwner::Editor);
        assert_eq!(context.owner(esc), KeyOwner::Editor);
        context.terminal_focused = false;
        assert_eq!(context.owner(esc), KeyOwner::Editor);
        context.modal = None;
        assert_eq!(context.owner(esc), KeyOwner::Find);
        context.find_open = false;
        assert_eq!(context.owner(esc), KeyOwner::Workspace);
    }
}

//! Keyboard, mouse, scrollbar and directory-navigation interactions.
//! Business state is still owned by App; moving these methods changes no behavior.
use super::*;

impl App {
    fn open_terminal_tab(&mut self) {
        if self.terminal_tabs.len() >= 7
            && self.terminal.as_ref().is_some_and(|t| !t.ended.load(std::sync::atomic::Ordering::Acquire))
        {
            self.session.status = "Terminal limit: 8 tabs (Ctrl+Shift+W closes one)".into();
            return;
        }
        let next = embedded_terminal::EmbeddedTerminal::start(
            &self.files.tree_root, 80, 12, self.terminal_notify.clone(),
        );
        match next {
            Ok(mut next) => {
                next.maximized = self.terminal.as_ref().is_some_and(|t| t.maximized);
                if let Some(previous) = self.terminal.take() {
                    // A finished session is not a usable tab. Restart replaces
                    // it rather than keeping a dead shell in tab history.
                    if !previous.ended.load(std::sync::atomic::Ordering::Acquire) {
                        self.terminal_tabs.push(previous);
                    }
                }
                self.session.status = "New terminal tab".into();
                self.terminal = Some(next);
            }
            Err(error) => self.session.status = format!("Terminal: {error}"),
        }
    }

    fn activate_terminal_number(&mut self, position: usize) {
        // The displayed numbers are ordered by creation id, not by rotation
        // of the inactive sessions during F12 switching.
        let mut ids = self.terminal_tabs.iter().map(|t| t.id).collect::<Vec<_>>();
        if let Some(term) = &self.terminal { ids.push(term.id); }
        ids.sort_unstable();
        if let Some(id) = ids.get(position).copied() {
            self.activate_terminal_tab(id);
        }
    }

    fn switch_terminal_tab(&mut self, backwards: bool) {
        if self.terminal_tabs.is_empty() { return; }
        let Some(active) = self.terminal.take() else { return; };
        let new = if backwards {
            self.terminal_tabs.pop().expect("nonempty terminal tab set")
        } else {
            self.terminal_tabs.remove(0)
        };
        self.terminal_tabs.push(active);
        let mut new = new;
        new.visible = true;
        new.focused = !new.ended.load(std::sync::atomic::Ordering::Acquire);
        self.terminal = Some(new);
    }

    fn activate_terminal_tab(&mut self, id: u64) {
        if self.terminal.as_ref().is_some_and(|term| term.id == id) {
            if let Some(term) = &mut self.terminal {
                term.focused = !term.ended.load(std::sync::atomic::Ordering::Acquire);
            }
            return;
        }
        let Some(index) = self.terminal_tabs.iter().position(|term| term.id == id) else { return; };
        let Some(active) = self.terminal.take() else { return; };
        let mut selected = self.terminal_tabs.remove(index);
        selected.visible = true;
        selected.focused = !selected.ended.load(std::sync::atomic::Ordering::Acquire);
        selected.maximized = active.maximized;
        self.terminal_tabs.push(active);
        self.terminal = Some(selected);
    }

    fn close_terminal_tab_by_id(&mut self, id: u64) {
        if self.terminal.as_ref().is_some_and(|term| term.id == id) {
            self.close_terminal_tab();
        } else if let Some(index) = self.terminal_tabs.iter().position(|term| term.id == id) {
            self.terminal_tabs.remove(index);
            self.session.status = "Terminal tab closed".into();
        }
    }

    fn open_terminal_help(&mut self) {
        self.terminal_help_return = self.terminal.as_ref().is_some_and(|term| term.visible && term.focused);
        if let Some(term) = &mut self.terminal { term.focused = false; }
        self.session.help_open = true;
        self.session.help_reference = false;
        self.session.help_scroll = 0;
    }

    fn handle_terminal_tab_action(&mut self, action: TerminalTabAction) {
        match action {
            TerminalTabAction::Activate(id) => self.activate_terminal_tab(id),
            TerminalTabAction::Close(id) => self.close_terminal_tab_by_id(id),
            TerminalTabAction::New => self.open_terminal_tab(),
            TerminalTabAction::Previous => self.switch_terminal_tab(true),
            TerminalTabAction::Next => self.switch_terminal_tab(false),
            TerminalTabAction::Maximize => {
                if let Some(term) = &mut self.terminal { term.maximized = !term.maximized; }
            }
            TerminalTabAction::Help => self.open_terminal_help(),
            TerminalTabAction::Hide => {
                if let Some(term) = &mut self.terminal {
                    term.visible = false;
                    term.focused = false;
                }
            }
        }
    }

    fn copy_terminal_text(&mut self, text: &str) {
        if text.is_empty() {
            self.session.status = "No terminal selection".into();
            return;
        }
        if text.len() > 256 * 1024 {
            self.session.status = "Terminal selection exceeds 256 KiB".into();
            return;
        }
        #[cfg(not(test))]
        {
            let native = copy_path_clipboard(text).is_ok();
            let osc52 = if native { false } else {
                crossterm::execute!(
                    std::io::stdout(),
                    crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)
                ).is_ok()
            };
            self.session.status = if native {
                format!("Copied {} terminal characters",text.chars().count())
            } else if osc52 {
                format!("Terminal copied via OSC 52 ({} chars)",text.chars().count())
            } else {
                "Terminal clipboard unavailable".into()
            };
        }
        #[cfg(test)]
        { self.session.status = format!("Terminal selected {} characters",text.chars().count()); }
    }

    fn close_terminal_tab(&mut self) {
        if self.terminal.is_none() { return; }
        self.terminal.take(); // Drop terminates the local PTY
        if let Some(mut previous) = self.terminal_tabs.pop() {
            previous.visible = true;
            previous.focused = !previous.ended.load(std::sync::atomic::Ordering::Acquire);
            self.terminal = Some(previous);
        }
        self.session.status = "Terminal tab closed".into();
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        // Modal keyboard ownership must precede all PTY and workspace hotkeys.
        // In particular a previously open find input must not eat Esc or Tab
        // while the Help overlay is visible.
        if self.keys.open {
            self.keys.handle(key);
            return;
        }
        if self.session.help_open {
            self.handle_help_key(key);
            return;
        }
        // Alt+1..8 selects the *visible tab number* from anywhere in the
        // workspace while Terminal is shown. Do not steal modal/editor keys.
        if self.terminal.as_ref().is_some_and(|term| term.visible)
            && !self.keys.open && !self.session.help_open
            && self.tools.panel.is_none() && !self.editing.active()
            && key.modifiers == KeyModifiers::ALT
        {
            if let KeyCode::Char(d @ '1'..='8') = key.code {
                self.activate_terminal_number(d as usize - '1' as usize);
                return;
            }
        }
        // F4 in Terminal opens the in-app help with a dedicated Terminal guide.
        if key.code == KeyCode::F(4)
            && self.terminal.as_ref().is_some_and(|term| term.visible && term.focused)
        {
            self.open_terminal_help();
            return;
        }
        // Function-key alternatives work even in host terminals that swallow
        // Ctrl+Shift combinations or report them as plain Ctrl.
        if key.code == KeyCode::F(8) && key.modifiers.contains(KeyModifiers::SHIFT) {
            self.open_terminal_tab(); return;
        }
        if key.code == KeyCode::F(12) && self.terminal.as_ref().is_some_and(|t| t.visible) {
            self.switch_terminal_tab(key.modifiers.contains(KeyModifiers::SHIFT));
            return;
        }
        // Host shortcuts are intercepted before the child PTY receives keys.
        let both = key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        if both {
            match key.code {
                KeyCode::Char('N' | 'n') => { self.open_terminal_tab(); return; }
                KeyCode::Char('W' | 'w') if self.terminal.is_some() => { self.close_terminal_tab(); return; }
                KeyCode::Left if self.terminal.is_some() => { self.switch_terminal_tab(true); return; }
                KeyCode::Right if self.terminal.is_some() => { self.switch_terminal_tab(false); return; }
                KeyCode::Char('C' | 'c') if self.terminal.is_some() => {
                    let selected = self.terminal.as_ref().and_then(|t| t.selected_text());
                    if let Some(selected) = selected.filter(|s| !s.is_empty()) {
                        self.copy_terminal_text(&selected);
                    } else {
                        self.session.status = "Select terminal text with the mouse first".into();
                    }
                    return;
                }
                KeyCode::Char('V' | 'v') if self.terminal.as_ref().is_some_and(|t| t.visible) => {
                    let pasted = arboard::Clipboard::new().and_then(|mut cb| cb.get_text());
                    match pasted {
                        Ok(text) => {
                            if let Some(term) = &mut self.terminal {
                                if let Err(error) = term.paste(&text) {
                                    self.session.status = format!("Paste failed: {error}");
                                }
                            }
                        }
                        Err(e) => self.session.status = format!("Clipboard unavailable: {e}"),
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Some(term) = &mut self.terminal {
            if term.visible && term.focused && !term.on_alternate_screen()
                && key.modifiers.contains(KeyModifiers::SHIFT) {
                match key.code {
                    KeyCode::PageUp => { term.scroll(-10); return; },
                    KeyCode::PageDown => { term.scroll(10); return; },
                    KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        term.scroll(isize::MIN); return;
                    }
                    KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        term.scroll_to_bottom(); return;
                    }
                    _ => {}
                }
            }
        }
        // F11 toggles terminal maximize while preserving the PTY and its
        // shell session. Ctrl+Shift+Enter is supported when distinguishable.
        if key.code == KeyCode::F(11)
            || (key.code == KeyCode::Enter
                && key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT))
        {
            if let Some(term) = &mut self.terminal {
                if term.visible {
                    term.maximized = !term.maximized;
                    term.focused = !term.ended.load(std::sync::atomic::Ordering::Acquire);
                    return;
                }
            }
        }
        // F8 is dependable across terminal emulators; Ctrl+` is a convenience
        // where the host reports it separately from Ctrl+Space.
        if key.code == KeyCode::F(8)
            || (key.code == KeyCode::Char('`') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            let restart = self.terminal.as_ref().is_none_or(|term| {
                !term.visible && term.ended.load(std::sync::atomic::Ordering::Acquire)
            });
            if restart {
                let was_opened = self.terminal.is_some();
                self.open_terminal_tab();
                if was_opened && self.terminal.as_ref().is_some_and(|t| t.visible) {
                    self.session.status = "Terminal restarted".into();
                }
            } else if let Some(term) = &mut self.terminal {
                term.visible = !term.visible;
                term.focused = term.visible && !term.ended.load(std::sync::atomic::Ordering::Acquire);
            }
            return;
        }
        if let Some(term) = &mut self.terminal {
            if term.visible && term.focused {
                // Ctrl+Shift+Tab returns focus to Forge without hiding PTY.
                if key.code == KeyCode::BackTab
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    term.focused = false;
                } else {
                    if let Err(error) = term.send(key) {
                        self.session.status = format!("Terminal input failed: {error}");
                        term.focused = false;
                    }
                }
                return;
            }
        }

        if self.preview_state.find.open {
            if matches!(key.code, KeyCode::F(1) | KeyCode::F(2)) {
                self.close_file_find();
            } else if self.preview_state.find.input_active
                || matches!(key.code, KeyCode::Esc | KeyCode::F(3))
            {
                self.handle_file_find_key(key);
                return;
            }
        }
        if self.blame.popup.is_some() {
            self.handle_blame_key(key);
            return;
        }
        if self.keys.open {
            self.keys.handle(key);
            return;
        }
        if key.code == KeyCode::F(4) {
            self.keys.open = true;
            return;
        }
        if self.editing.active() {
            self.handle_editor_key(key);
            return;
        }
        if self.tools.panel.is_some() {
            self.handle_tools_key(key);
            return;
        }
        if self.session.help_open {
            self.handle_help_key(key);
            return;
        }
        if self.scm.pending_write.is_none() {
            self.session.write_status = None;
        }
        // Shift-F10 / Context Menu key opens the same contextual actions as
        // a right click. This is deliberately unavailable in editors, modal
        // inputs and help; those modes handled keys above.
        if self.session.context_menu.is_none()
            && (key.code == KeyCode::Menu
                || (key.code == KeyCode::F(10) && key.modifiers.contains(KeyModifiers::SHIFT)))
        {
            let x = self.session.layout.list_inner.x.saturating_add(3);
            let y = self.session.layout.list_inner.y.saturating_add(1);
            if self.session.focus == PaneFocus::Preview {
                let x = self.session.layout.preview_inner.x.saturating_add(3);
                let y = self.session.layout.preview_inner.y.saturating_add(1);
                self.open_diff_context_menu(x, y);
            } else if self.in_git_sidebar() {
                match self.selected_scm_row() {
                    Some(ScmRow::Change { column, index, .. }) => {
                        self.open_staging_context_menu(column, index, x, y);
                    }
                    Some(ScmRow::Folder { .. }) => {
                        self.open_staging_folder_menu(self.scm.scm_selected, x, y);
                    }
                    Some(ScmRow::Section { .. }) => {
                        self.open_staging_section_menu(self.scm.scm_selected, x, y);
                    }
                    _ => {}
                }
            } else if self.files.visible.get(self.files.selected).is_some() {
                self.open_file_context_menu(self.files.selected, x, y);
            }
            return;
        }
        // Context menu captures navigation / confirm / dismiss.
        if self.session.context_menu.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.dismiss_menu();
                    self.session.status = self.hint_bar();
                }
                KeyCode::Char('?') => {
                    self.dismiss_menu();
                    self.session.help_open = true;
                    self.session.help_reference = false;
                    self.session.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(menu) = self.session.context_menu.as_mut() {
                        menu.move_sel(-1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(menu) = self.session.context_menu.as_mut() {
                        menu.move_sel(1);
                    }
                }
                KeyCode::Enter => {
                    let action = self
                        .session
                        .context_menu
                        .as_ref()
                        .and_then(|m| m.current_action());
                    if let Some(action) = action {
                        self.apply_context_action(action);
                    }
                }
                KeyCode::Char('q') => {
                    self.dismiss_menu();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl-C must never terminate Forge, including in a context menu.
                    self.dismiss_menu();
                    self.session.status = self.hint_bar();
                }
                _ => {}
            }
            return;
        }

        let Some(key) = self.keys.remap(key, false) else {
            return;
        };
        match key.code {
            KeyCode::F(1) => {
                if self.in_git_sidebar() {
                    self.leave_git_sidebar();
                }
            }
            KeyCode::F(2) => {
                if !self.in_git_sidebar() {
                    self.enter_git_sidebar();
                }
            }
            KeyCode::F(3) => self.set_markdown_code(!self.preview_state.markdown_code),
            KeyCode::F(6) => self.open_blame_popup(),
            KeyCode::F(7) if self.session.focus == PaneFocus::Preview => {
                if !self.close_inline_peek() {
                    self.toggle_inline_peek();
                }
            }
            KeyCode::Esc
                if self.session.focus == PaneFocus::Preview && self.close_inline_peek() => {}
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_tools(tools::PanelKind::Files)
            }
            KeyCode::Char('f' | 'F') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.open_tools(tools::PanelKind::Content);
                } else if !self.in_git_sidebar()
                    && matches!(self.preview_state.preview_mode, PreviewMode::File)
                    && self
                        .preview_state
                        .preview
                        .as_ref()
                        .is_some_and(|p| matches!(p.content, PreviewContent::Text { .. }))
                {
                    self.open_file_find();
                } else {
                    self.open_tools(tools::PanelKind::Content);
                }
            }
            KeyCode::Char(':') => self.open_tools(tools::PanelKind::Commands),
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_tools(tools::PanelKind::Commands)
            }
            KeyCode::Char('v') if self.sidebar_focused() => self.toggle_mark(),
            KeyCode::Char('V') if self.sidebar_focused() => {
                self.run_command(tools::Command::MarkAll)
            }
            KeyCode::Char('e') => {
                if let Some(path) = self.editor_target() {
                    self.open_text_editor(path, self.preview_source_line());
                }
            }
            KeyCode::Char('E') => self.run_command(tools::Command::Editor),
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.open_tools(tools::PanelKind::Workspaces)
            }
            KeyCode::Char('o') => self.open_selected_worktree_file(),
            KeyCode::F(9) => self.open_tools(tools::PanelKind::Settings),
            KeyCode::Char(',') => self.open_tools(tools::PanelKind::Settings),
            KeyCode::Char('H') => self.run_command(tools::Command::FileHistory),
            KeyCode::Char('L') => self.run_command(tools::Command::History),
            KeyCode::Char('G') => self.show_history(),
            KeyCode::Char('I') => self.run_command(tools::Command::Lines),
            KeyCode::Char('F') => self.run_command(tools::Command::Filters),
            KeyCode::Char('B') => self.run_command(tools::Command::Branches),
            KeyCode::Char('Z') => self.run_command(tools::Command::Stashes),
            KeyCode::Char('S') => self.run_command(tools::Command::Sync),
            KeyCode::Char('W') => self.run_command(tools::Command::Worktrees),
            KeyCode::Char('x') => self.run_command(tools::Command::Discard),
            KeyCode::Delete => self.run_command(tools::Command::Delete),
            KeyCode::Esc
                if self.preview_state.selection.active()
                    && self.session.focus == PaneFocus::Preview =>
            {
                self.preview_state.selection.clear();
                self.session.status = "Preview selection cleared".into();
            }
            KeyCode::Esc if !self.tools.marks.is_empty() => {
                self.tools.marks.clear();
                self.session.status = "Selection cleared".into();
            }
            KeyCode::Char('?') => {
                self.session.help_open = true;
                self.session.help_reference = false;
                self.session.help_scroll = 0;
            }
            KeyCode::Char('q') => self.session.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.session.focus == PaneFocus::Preview && self.preview_state.selection.active()
                {
                    self.copy_preview_selection();
                } else {
                    self.session.status = "Use q to quit · Ctrl-C does not exit Forge".into();
                }
            }
            KeyCode::Esc => {
                if self.in_git_sidebar() && self.sidebar_focused() {
                    self.leave_git_sidebar();
                } else if self.in_git_sidebar() && matches!(self.session.focus, PaneFocus::Preview)
                {
                    // From diff preview back to SCM list (keep diff loaded).
                    self.session.focus = PaneFocus::List;
                } else if matches!(self.preview_state.preview_mode, PreviewMode::Diff) {
                    self.preview_state.preview_mode = PreviewMode::File;
                    self.clear_diff_view();
                    self.request_preview_for_selection();
                    self.session.focus = PaneFocus::List;
                } else if self.session.focus == PaneFocus::Preview {
                    self.session.focus = PaneFocus::List;
                }
            }
            KeyCode::Char('c') => self.open_commit(),
            KeyCode::Char('s') if self.in_git_sidebar() => {
                if self.session.focus == PaneFocus::Preview {
                    self.request_hunk_write(true);
                } else {
                    self.batch_stage(true);
                }
            }
            KeyCode::Char('u') if self.in_git_sidebar() => {
                if self.session.focus == PaneFocus::Preview {
                    self.request_hunk_write(false);
                } else {
                    self.batch_stage(false);
                }
            }
            KeyCode::Char('n')
                if self.session.focus == PaneFocus::Preview
                    && matches!(self.preview_state.preview_mode, PreviewMode::Diff)
                    && !self.preview_state.preview_loading =>
            {
                self.move_hunk(1);
            }
            KeyCode::Char('N')
                if self.session.focus == PaneFocus::Preview
                    && matches!(self.preview_state.preview_mode, PreviewMode::Diff)
                    && !self.preview_state.preview_loading =>
            {
                self.move_hunk(-1);
            }
            KeyCode::Char('p') if self.git_sync_shortcut_enabled() => self.request_pull(),
            KeyCode::Char('P') if self.git_sync_shortcut_enabled() => self.request_push(),
            KeyCode::Char('d') if self.in_git_sidebar() && self.sidebar_focused() => {
                self.request_diff_for_scm_selection();
                self.session.focus = PaneFocus::Preview;
            }
            KeyCode::Char('d') => {
                self.diff_selected_file();
            }
            KeyCode::Char('g') => {
                self.toggle_scm_sidebar();
            }
            KeyCode::Char('[') => {
                self.expand_staging();
                let first = self.scm_slices().0.first().map(|e| e.path.clone());
                if let Some(path) = first {
                    expand_ancestors_for_path(&mut self.scm.scm_expanded_unstaged, &path);
                }
                let rows = self.scm_rows();
                self.scm.pending_transfer = None;
                self.scm.scm_selected = scm_jump_to_column(&rows, StagingColumn::Unstaged);
                self.session.focus = PaneFocus::List;
                self.request_diff_for_scm_selection();
            }
            KeyCode::Char(']') => {
                self.expand_staging();
                let first = self.scm_slices().1.first().map(|e| e.path.clone());
                if let Some(path) = first {
                    expand_ancestors_for_path(&mut self.scm.scm_expanded_staged, &path);
                }
                let rows = self.scm_rows();
                self.scm.pending_transfer = None;
                self.scm.scm_selected = scm_jump_to_column(&rows, StagingColumn::Staged);
                self.session.focus = PaneFocus::List;
                self.request_diff_for_scm_selection();
            }
            KeyCode::Char('j') | KeyCode::Down => match self.session.focus {
                PaneFocus::Preview => self.scroll_preview(1),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                    self.session.focus = PaneFocus::List;
                    self.move_scm(1);
                }
                _ => {
                    self.session.focus = PaneFocus::List;
                    self.move_sel(1);
                }
            },
            KeyCode::Char('k') | KeyCode::Up => match self.session.focus {
                PaneFocus::Preview => self.scroll_preview(-1),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                    self.session.focus = PaneFocus::List;
                    self.move_scm(-1);
                }
                _ => {
                    self.session.focus = PaneFocus::List;
                    self.move_sel(-1);
                }
            },
            KeyCode::Char(' ') => {
                if matches!(self.session.focus, PaneFocus::List | PaneFocus::Staging)
                    && self.in_git_sidebar()
                {
                    self.toggle_scm_expand_at(self.scm.scm_selected);
                } else if matches!(self.session.focus, PaneFocus::List) && !self.in_git_sidebar() {
                    self.toggle_expand_at(self.files.selected);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if self.sidebar_focused() => {
                if self.in_git_sidebar() {
                    self.scm_open_or_expand();
                } else {
                    self.open_selected();
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                if key.modifiers.contains(KeyModifiers::ALT)
                    && self.session.focus == PaneFocus::Preview
                    && !self.config.wrap_code
                    && matches!(self.preview_state.preview_mode, PreviewMode::File | PreviewMode::Diff) =>
            {
                let max_x = if matches!(self.preview_state.preview_mode, PreviewMode::Diff) {
                    let visible = (self.session.layout.preview_inner.width as usize / 2).saturating_sub(7);
                    if let Some(diff) = &self.preview_state.file_diff {
                        self.preview_state.split_cache.max_code_columns(&diff.lines).saturating_sub(visible)
                    } else { 0 }
                } else {
                    let visible = self.session.layout.preview_inner.width.saturating_sub(5) as usize;
                    self.preview_state.text_rows.max_code_columns().saturating_sub(visible)
                };
                let previous_x = self.preview_state.preview_scroll_x;
                let offset = &mut self.preview_state.preview_scroll_x;
                *offset = match key.code {
                    KeyCode::Left => offset.saturating_sub(8),
                    KeyCode::Right => offset.saturating_add(8).min(max_x),
                    KeyCode::Home => 0,
                    KeyCode::End => max_x,
                    _ => *offset,
                };
                if previous_x != *offset {
                    // Old visual selection coordinates refer to a different horizontal window.
                    self.preview_state.selection.clear();
                }
            }
            KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Left => {
                if matches!(self.session.focus, PaneFocus::Preview) {
                    self.session.focus = PaneFocus::List;
                } else if self.in_git_sidebar() {
                    self.scm_tree_back();
                } else {
                    self.tree_back();
                }
            }
            KeyCode::Char('r') => {
                self.request_list();
                self.request_status();
                if self.in_git_sidebar() && self.history.expanded {
                    self.request_history();
                }
            }
            KeyCode::Tab => {
                // Sidebar <-> Preview only (Git reuses sidebar via activity).
                self.session.focus = match self.session.focus {
                    PaneFocus::List | PaneFocus::Staging => PaneFocus::Preview,
                    PaneFocus::Preview => PaneFocus::List,
                };
            }
            KeyCode::BackTab => {
                self.session.focus = match self.session.focus {
                    PaneFocus::List | PaneFocus::Staging => PaneFocus::Preview,
                    PaneFocus::Preview => PaneFocus::List,
                };
            }
            KeyCode::Home => {
                if self.session.focus == PaneFocus::Preview {
                    self.preview_state.preview_scroll = 0;
                } else {
                    self.session.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.scm.pending_transfer = None;
                        self.scm.follow_selection = true;
                        self.scm.scm_selected = 0;
                        self.request_diff_for_scm_selection();
                    } else {
                        self.files.follow_selection = true;
                        self.files.selected = 0;
                        self.request_preview_for_selection();
                    }
                }
            }
            KeyCode::End => {
                if self.session.focus == PaneFocus::Preview {
                    self.preview_state.preview_scroll = self
                        .preview_total_lines()
                        .saturating_sub(self.session.layout.preview_inner.height as usize);
                    self.maybe_request_preview_page();
                } else {
                    self.session.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.scm.pending_transfer = None;
                        self.scm.follow_selection = true;
                        self.scm.scm_selected = self.scm_rows().len().saturating_sub(1);
                        self.request_diff_for_scm_selection();
                        self.maybe_page_history();
                    } else {
                        self.files.follow_selection = true;
                        self.files.selected = self.files.visible.len().saturating_sub(1);
                        self.request_preview_for_selection();
                    }
                }
            }
            KeyCode::PageDown | KeyCode::PageUp => {
                let sign = if key.code == KeyCode::PageDown { 1 } else { -1 };
                match self.session.focus {
                    PaneFocus::Preview => {
                        let page = self.session.layout.preview_inner.height.saturating_sub(2).max(1) as isize;
                        self.scroll_preview(sign * page);
                    }
                    PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                        let page = self.session.layout.list_inner.height.saturating_sub(2).max(1) as isize;
                        self.move_scm(sign * page);
                    }
                    _ => {
                        let page = self.session.layout.list_inner.height.saturating_sub(2).max(1) as isize;
                        self.move_sel(sign * page);
                    }
                }
            },
            _ => {}
        }
    }

    fn handle_help_mouse(&mut self, mouse: MouseEvent) {
        let point = ratatui::layout::Position::new(mouse.column,mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = self.session.help_area.contains(point);
                if self.session.help_close_rect.contains(point) || !inside {
                    self.handle_help_key(KeyEvent::new(KeyCode::Esc,KeyModifiers::NONE));
                } else if self.session.help_overview_rect.contains(point) {
                    self.session.help_reference = false;
                    self.session.help_scroll = 0;
                } else if self.session.help_reference_rect.contains(point) {
                    self.session.help_reference = true;
                    self.session.help_scroll = 0;
                }
            }
            MouseEventKind::ScrollDown => self.scroll_help(3),
            MouseEventKind::ScrollUp => self.scroll_help(-3),
            _ => {}
        }
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        let point = ratatui::layout::Position::new(mouse.column,mouse.row);
        if self.session.help_open {
            self.handle_help_mouse(mouse);
            return;
        }
        // Input ownership must follow the clicked pane, not visibility of
        // the embedded terminal. Popups always take priority over the PTY.
        let modal = self.blame.popup.is_some()
            || self.keys.open || self.editing.active()
            || self.tools.panel.is_some() || self.session.help_open
            || self.session.commit_open || self.session.context_menu.is_some();
        if !modal && matches!(mouse.kind,MouseEventKind::Down(MouseButton::Left)) {
            let tab_strip = self.terminal_tab_bar.contains(point);
            let terminal_area = self.terminal.as_ref()
                .is_some_and(|t|t.visible && t.area.contains(point));
            let workspace_click = self.session.layout.contains_list(mouse.column,mouse.row)
                || self.session.layout.contains_preview(mouse.column,mouse.row)
                || self.session.layout.files_tab.contains(point)
                || self.session.layout.git_tab.contains(point);
            if workspace_click && !tab_strip && !terminal_area {
                if let Some(term) = &mut self.terminal {term.focused=false;}
            }
        }
        // Route terminal mouse events to the child when it enabled tracking.
        // Otherwise scroll/drag select in the host and copy on release.
        // Shift always overrides child tracking for host text selection.
        if !modal {
            if let Some(term) = &mut self.terminal {
                if term.visible && term.area.contains(point) {
                    // Also focus when the child has enabled mouse reporting;
                    // that mode does not pass through the host selection path.
                    if matches!(mouse.kind,MouseEventKind::Down(MouseButton::Left)) {
                        term.focused = !term.ended.load(std::sync::atomic::Ordering::Acquire);
                    }
                    match term.mouse(mouse) {
                    Ok(Some(text)) if !text.is_empty() => self.copy_terminal_text(&text),
                    Err(error) => self.session.status = format!("Terminal mouse: {error}"),
                    _ => {}
                }
                    return;
                }
            }
        }
        // The tab strip is part of Forge, not of the nested terminal's PTY.
        // Hit boxes are produced by the same frame renderer that paints them.
        if !modal && self.terminal.as_ref().is_some_and(|term| term.visible)
            && self.tools.panel.is_none() && !self.keys.open
            && !self.session.help_open && !self.editing.active()
            && self.terminal_tab_bar.contains(ratatui::layout::Position::new(mouse.column,mouse.row))
        {
            let action = self.terminal_tab_hits.iter().find(|(rect,_)|
                rect.contains(ratatui::layout::Position::new(mouse.column,mouse.row))
            ).map(|(_, action)| *action);
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(action) = action { self.handle_terminal_tab_action(action); }
                }
                MouseEventKind::Down(MouseButton::Middle) => {
                    if let Some(TerminalTabAction::Activate(id) | TerminalTabAction::Close(id)) = action {
                        self.close_terminal_tab_by_id(id);
                    }
                }
                MouseEventKind::ScrollUp => self.switch_terminal_tab(true),
                MouseEventKind::ScrollDown => self.switch_terminal_tab(false),
                _ => {},
            }
            return;
        }
        if self.scrollbar_drag.is_some() && self.handle_scrollbar_mouse(mouse) {
            return;
        }
        if self.blame.popup.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollDown => self.scroll_blame(3),
                MouseEventKind::ScrollUp => self.scroll_blame(-3),
                _ => {}
            }
            return;
        }
        if self.keys.open {
            return;
        }
        if self.editing.active() {
            self.handle_editor_mouse(mouse);
            return;
        }
        if self.tools.panel.is_some() {
            self.handle_tools_mouse(mouse);
            return;
        }
        if self.session.help_open {
            match mouse.kind {
                MouseEventKind::ScrollDown => self.scroll_help(3),
                MouseEventKind::ScrollUp => self.scroll_help(-3),
                _ => {}
            }
            return;
        }
        if self.preview_state.find.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            let rect = self.session.layout.file_find_rect;
            if rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row)) {
                if mouse.row == rect.y + 1 {
                    self.preview_state.find.input_active = true;
                } else if mouse.row == rect.y + 2 {
                    let x = mouse.column.saturating_sub(rect.x);
                    let w = rect.width;
                    if w >= 34 && x >= w.saturating_sub(3) {
                        self.close_file_find();
                    } else if w >= 34 && x >= w.saturating_sub(9) {
                        self.navigate_file_find(true);
                    } else if w >= 34 && x >= w.saturating_sub(16) {
                        self.navigate_file_find(false);
                    } else if (2..6).contains(&x) {
                        self.preview_state.find.match_case = !self.preview_state.find.match_case;
                        self.start_file_find();
                    } else if (7..11).contains(&x) {
                        self.preview_state.find.whole_word = !self.preview_state.find.whole_word;
                        self.start_file_find();
                    } else if (12..17).contains(&x) {
                        self.preview_state.find.regex = !self.preview_state.find.regex;
                        self.start_file_find();
                    }
                }
                return;
            }
            self.preview_state.find.input_active = false;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && !self.inline_popup_contains(mouse.column, mouse.row)
            && self
                .session
                .layout
                .preview_inner
                .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
        {
            if let Some(path) = self.blame_path() {
                let scroll = self.preview_state.preview_scroll;
                let row = scroll
                    + mouse
                        .row
                        .saturating_sub(self.session.layout.preview_inner.y)
                        as usize;
                self.blame.cursor = Some((path, row, scroll));
            }
        }
        let col = mouse.column;
        let row = mouse.row;

        // Context menu: consume clicks while open.
        if let Some(menu) = self.session.context_menu.clone() {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Down(MouseButton::Right) => {
                    if menu.contains(col, row) {
                        if let Some((idx, action)) = self
                            .session
                            .context_menu
                            .as_ref()
                            .and_then(|m| m.action_at_row(col, row))
                        {
                            if let Some(m) = self.session.context_menu.as_mut() {
                                m.selected = idx;
                            }
                            let enabled = self
                                .session
                                .context_menu
                                .as_ref()
                                .and_then(|m| m.items.get(idx))
                                .map(|i| i.enabled)
                                .unwrap_or(false);
                            if enabled {
                                self.apply_context_action(action);
                            }
                        }
                    } else {
                        self.dismiss_menu();
                        // Fall through so a left click still selects underneath.
                        if matches!(
                            mouse.kind,
                            MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
                        ) {
                            self.handle_mouse_content(mouse);
                        }
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    // Ignore wheel over menu / dismiss on wheel outside.
                    if !menu.contains(col, row) {
                        self.dismiss_menu();
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(m) = self.session.context_menu.as_mut() {
                        m.hover_at(col, row);
                    }
                }
                _ => {}
            }
            return;
        }

        self.handle_mouse_content(mouse);
    }

    pub(super) fn handle_mouse_content(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let point = ratatui::layout::Position::new(col, row);
            if self.session.layout.git_sync_status.contains(point) {
                // Clicking the branch / divergence indicator opens the
                // existing Sync panel. It must never silently Push/Pull.
                self.run_command(tools::Command::Sync);
                return;
            }
            if self.session.layout.files_tab.contains(point) {
                if self.in_git_sidebar() {
                    self.leave_git_sidebar();
                } else {
                    self.session.focus = PaneFocus::List;
                }
                return;
            }
            if self.session.layout.git_tab.contains(point) {
                if !self.in_git_sidebar() {
                    self.enter_git_sidebar();
                } else {
                    self.session.focus = PaneFocus::List;
                }
                return;
            }
            if self.session.layout.open_file_tab.contains(point) {
                self.open_selected_worktree_file();
                return;
            }
            if self.session.layout.markdown_preview_tab.contains(point) {
                self.set_markdown_code(false);
                self.session.focus = PaneFocus::Preview;
                return;
            }
            if self.session.layout.markdown_code_tab.contains(point) {
                self.set_markdown_code(true);
                self.session.focus = PaneFocus::Preview;
                return;
            }
        }
        if self.handle_scrollbar_mouse(mouse) {
            return;
        }
        let over_list = self.session.layout.contains_list(col, row);
        let over_preview = self.session.layout.contains_preview(col, row);
        let git = self.in_git_sidebar();
        // The terminal is in mouse-capture mode, so native OS text selection
        // cannot work. Preview selection owns left drag events instead.
        let region = self.session.layout.preview_inner;
        let ruler = self.session.layout.overview_ruler;
        if self.config.show_overview_ruler
            && ruler.contains(ratatui::layout::Position::new(col, row))
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left))
        {
            if let Some(target) = self.overview_click_target(
                row.saturating_sub(ruler.y) as usize, ruler.height as usize)
            {
                self.session.focus = PaneFocus::Preview;
                self.preview_state.pending_overview_line = Some(target);
                self.advance_overview_jump();
            }
            return;
        }
        if !self.preview_state.selection.is_dragging() && self.inline_mouse(mouse) {
            return;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) && self.preview_state.selection.is_dragging()
        {
            let release = matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left));
            self.preview_state.selection.move_to(
                region,
                self.preview_state.preview_scroll,
                col,
                row,
                release,
            );
            if release && self.preview_state.selection.active() {
                self.session.status = "Preview selection ready · Ctrl-C Copy · Esc Clear".into();
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if over_preview && matches!(self.preview_state.preview_mode, PreviewMode::Diff) {
                    self.open_diff_context_menu(col, row);
                } else if over_list && git {
                    let rows = self.scm_rows();
                    if let Some(flat) = sticky_index_at(
                        self.session.layout.list_inner,
                        self.scm.scroll_scm,
                        &self.scm.sticky_headers,
                        rows.len(),
                        col,
                        row,
                    ) {
                        if let Some(ScmRow::Change { column, index, .. }) = rows.get(flat) {
                            self.open_staging_context_menu(*column, *index, col, row);
                        } else if let Some(ScmRow::Folder { .. }) = rows.get(flat) {
                            self.open_staging_folder_menu(flat, col, row);
                        } else if let Some(ScmRow::Section { .. }) = rows.get(flat) {
                            self.open_staging_section_menu(flat, col, row);
                        }
                    }
                } else if over_list {
                    if let Some(index) = sticky_index_at(
                        self.session.layout.list_inner,
                        self.files.scroll,
                        &self.files.sticky_headers,
                        self.files.visible.len(),
                        col,
                        row,
                    ) {
                        self.open_file_context_menu(index, col, row);
                    } else {
                        self.open_file_root_menu(col, row);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if over_list && git {
                    self.session.focus = PaneFocus::List;
                    let rows = self.scm_rows();
                    if let Some(flat) = sticky_index_at(
                        self.session.layout.list_inner,
                        self.scm.scroll_scm,
                        &self.scm.sticky_headers,
                        rows.len(),
                        col,
                        row,
                    ) {
                        let pinned = ((row - self.session.layout.list_inner.y) as usize)
                            < self.scm.sticky_headers.len();
                        let same = flat == self.scm.scm_selected;
                        self.scm.scm_selected = flat;
                        self.scm.pending_transfer = None;
                        self.scm.follow_selection = true;
                        if pinned {
                            // Clicking a sticky title navigates to its real
                            // location; it must not collapse the subtree.
                            self.scm.scroll_scm = flat;
                            return;
                        }
                        match self.selected_scm_row() {
                            Some(ScmRow::Change { .. } | ScmRow::CommitFile { .. }) => {
                                let now = Instant::now();
                                let double = self.scm.last_scm_click.is_some_and(|(when, i)| {
                                    i == flat && now.duration_since(when) <= DOUBLE_CLICK
                                });
                                self.scm.last_scm_click =
                                    if double { None } else { Some((now, flat)) };
                                if double {
                                    self.open_selected_worktree_file();
                                } else {
                                    self.request_diff_for_scm_selection();
                                }
                            }
                            Some(ScmRow::Folder { .. }) => {
                                // Like the Files tree, one click on a folder
                                // toggles it immediately. Previously a click
                                // on any other selected row only moved the
                                // highlight and required a second click.
                                self.toggle_scm_expand_at(flat);
                            }
                            Some(ScmRow::Commit { .. }) => {
                                self.request_diff_for_scm_selection();
                                if same {
                                    self.toggle_history_row(flat);
                                }
                            }
                            Some(ScmRow::CommitFolder { .. }) => {
                                // History directory nodes share the Git
                                // single-click expansion behavior.
                                self.toggle_history_row(flat);
                            }
                            Some(ScmRow::HistorySection { .. }) => {
                                if same {
                                    self.toggle_history_row(flat);
                                }
                            }
                            _ => {
                                self.preview_state.preview_mode = PreviewMode::Diff;
                                self.clear_diff_view();
                                self.preview_state.preview_loading = false;
                                self.services.jobs.cancel_key("git:diff");
                                self.preview_state.pending_diff = None;
                                self.session.status = "No changes selected".into();
                            }
                        }
                    }
                } else if over_list {
                    self.session.focus = PaneFocus::List;
                    // Stay in Files when clicking the file tree.
                    if !matches!(self.session.activity, ActivityTab::Files) {
                        self.session.activity = ActivityTab::Files;
                        self.preview_state.preview_mode = PreviewMode::File;
                    }
                    if let Some(index) = sticky_index_at(
                        self.session.layout.list_inner,
                        self.files.scroll,
                        &self.files.sticky_headers,
                        self.files.visible.len(),
                        col,
                        row,
                    ) {
                        let pinned = ((row - self.session.layout.list_inner.y) as usize)
                            < self.files.sticky_headers.len();
                        if pinned {
                            self.files.follow_selection = true;
                            self.files.selected = index;
                            self.files.scroll = index;
                            // Directory previews are intentionally unchanged.
                            return;
                        }
                        self.files.follow_selection = true;
                        let now = Instant::now();
                        let is_double = self
                            .files
                            .last_list_click
                            .map(|(t, i)| i == index && now.duration_since(t) <= DOUBLE_CLICK)
                            .unwrap_or(false);
                        self.files.last_list_click = Some((now, index));

                        let is_dir = self
                            .files
                            .visible
                            .get(index)
                            .is_some_and(|r| matches!(r.entry.kind, EntryKind::Directory));

                        if index != self.files.selected {
                            self.files.selected = index;
                            self.request_preview_for_selection();
                        }

                        if is_double {
                            if !is_dir {
                                self.open_selected();
                            }
                            self.files.last_list_click = None;
                        } else if is_dir {
                            self.toggle_expand_at(index);
                        }
                    }
                } else if over_preview {
                    self.session.focus = PaneFocus::Preview;
                    self.preview_state.selection.begin(
                        region,
                        self.preview_state.preview_scroll,
                        col,
                        row,
                    );
                }
            }
            // The wheel scrolls the hovered pane without taking keyboard focus.
            MouseEventKind::ScrollUp => {
                if over_list || (!over_preview && self.sidebar_focused()) {
                    if self.in_git_sidebar() {
                        self.wheel_scroll_git(-3);
                    } else {
                        self.wheel_scroll_files(-3);
                    }
                } else if over_preview || matches!(self.session.focus, PaneFocus::Preview) {
                    self.scroll_preview(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if over_list || (!over_preview && self.sidebar_focused()) {
                    if self.in_git_sidebar() {
                        self.wheel_scroll_git(3);
                    } else {
                        self.wheel_scroll_files(3);
                    }
                } else if over_preview || matches!(self.session.focus, PaneFocus::Preview) {
                    self.scroll_preview(3);
                }
            }
            _ => {}
        }
    }

    pub(super) fn wheel_scroll_files(&mut self, delta: isize) {
        if self.files.visible.is_empty() {
            return;
        }
        self.files.follow_selection = false;
        self.files.scroll = self
            .files
            .scroll
            .saturating_add_signed(delta)
            .min(self.files.visible.len() - 1);
    }

    pub(super) fn wheel_scroll_git(&mut self, delta: isize) {
        let count = self.scm_rows().len();
        if count == 0 {
            return;
        }
        self.scm.follow_selection = false;
        self.scm.scroll_scm = self
            .scm
            .scroll_scm
            .saturating_add_signed(delta)
            .min(count - 1);
    }

    pub(super) fn diff_selected_file(&mut self) {
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if matches!(entry.kind, EntryKind::Directory) {
            self.session.status = "select a file to diff".into();
            return;
        }
        let Ok(rel) = entry.path.strip_prefix(&repo) else {
            self.session.status = "file outside repository".into();
            return;
        };
        let rela = forge_model::path_to_rela(rel);
        let side = if let Some(snap) = &self.scm.git_status {
            if snap.unstaged.iter().any(|c| c.path == rela) {
                DiffSide::Unstaged
            } else if snap.staged.iter().any(|c| c.path == rela) {
                DiffSide::Staged
            } else {
                self.session.status = "This file has no Git changes".into();
                return;
            }
        } else {
            DiffSide::Unstaged
        };
        self.enter_git_sidebar();
        let column = if side == DiffSide::Unstaged {
            StagingColumn::Unstaged
        } else {
            StagingColumn::Staged
        };
        expand_ancestors_for_path(self.scm_expanded_mut(column), &rela);
        let (unstaged, staged) = self.scm_slices();
        if let Some(index) = self.scm_rows().iter().position(|r| {
            scm_entry(r, unstaged, staged)
                .is_some_and(|entry| entry.path == rela && entry.side == side)
        }) {
            self.scm.scm_selected = index;
        }
        self.request_diff_for_scm_selection();
        self.session.focus = PaneFocus::Preview;
    }

    pub(super) fn copy_preview_selection(&mut self) {
        let Some(text) = self.preview_state.selection.selected_text() else {
            self.session.status = "Select visible preview text by dragging first".into();
            return;
        };
        if text.is_empty() {
            self.session.status = "Select preview text first".into();
            return;
        }
        if text.len() > 256 * 1024 {
            self.session.status = "Selection exceeds 256 KiB; select a smaller region".into();
            return;
        }
        #[cfg(not(test))]
        {
            let native = copy_path_clipboard(&text).is_ok();
            let terminal = crossterm::execute!(
                std::io::stdout(),
                crossterm::clipboard::CopyToClipboard::to_clipboard_from(&text)
            )
            .is_ok();
            self.session.status = if native {
                format!("Copied {} characters to clipboard", text.chars().count())
            } else if terminal {
                format!(
                    "Sent {} characters via terminal clipboard (OSC 52)",
                    text.chars().count()
                )
            } else {
                "Clipboard unavailable in this terminal".into()
            };
        }
        #[cfg(test)]
        {
            self.session.status = format!("Selected {} characters for copy", text.chars().count());
        }
    }

    pub(super) fn scrollbar(&self, target: ScrollbarTarget) -> VerticalScrollbar {
        match target {
            ScrollbarTarget::Preview => VerticalScrollbar::new(
                self.session.layout.preview_scrollbar,
                self.preview_state.preview_scroll,
                self.preview_total_lines(),
            ),
            ScrollbarTarget::Sidebar => {
                let (first, total) = if self.in_git_sidebar() {
                    (self.scm.scroll_scm, self.scm_rows().len())
                } else {
                    (self.files.scroll, self.files.visible.len())
                };
                VerticalScrollbar::new(self.session.layout.sidebar_scrollbar, first, total)
            }
        }
    }

    pub(super) fn seek_scrollbar(&mut self, target: ScrollbarTarget, row: u16, drag: ScrollbarDrag) {
        let Some(scroll) = self.scrollbar(target).scroll_at(row, drag) else { return };
        match target {
            ScrollbarTarget::Preview => {
                let old = self.preview_state.preview_scroll;
                self.preview_state.preview_scroll = scroll;
                self.session.focus = PaneFocus::Preview;
                if scroll > old { self.maybe_request_preview_page(); }
            }
            ScrollbarTarget::Sidebar => {
                self.session.focus = PaneFocus::List;
                if self.in_git_sidebar() {
                    self.scm.follow_selection = false;
                    self.scm.scroll_scm = scroll;
                    self.maybe_page_history();
                } else {
                    self.files.follow_selection = false;
                    self.files.scroll = scroll;
                }
            }
        }
    }

    /// Capture drag across the whole TUI window, not just the original
    /// one-cell lane. Pointer movements outside the terminal window itself
    /// cannot be delivered by the terminal emulator.
    pub(super) fn handle_scrollbar_mouse(&mut self, mouse: MouseEvent) -> bool {
        if let Some((target, drag)) = self.scrollbar_drag {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.seek_scrollbar(target, mouse.row, drag);
                    return true;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.seek_scrollbar(target, mouse.row, drag);
                    self.scrollbar_drag = None;
                    return true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    // Some terminals lose the Up event when the pointer exits
                    // their window. A new click begins a fresh interaction.
                    self.scrollbar_drag = None;
                }
                _ => {}
            }
        }
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        for target in [ScrollbarTarget::Preview, ScrollbarTarget::Sidebar] {
            let bar = self.scrollbar(target);
            if let Some(drag) = bar.begin_drag(mouse.column, mouse.row) {
                self.scrollbar_drag = Some((target, drag));
                self.seek_scrollbar(target, mouse.row, drag);
                return true;
            }
        }
        false
    }

    pub(super) fn scroll_preview(&mut self, delta: isize) {
        let total = self.preview_total_lines();
        let viewport = self.session.layout.preview_inner.height as usize;
        let max_scroll = total.saturating_sub(viewport);
        let next = (self.preview_state.preview_scroll as isize)
            .saturating_add(delta)
            .clamp(0, max_scroll as isize) as usize;
        self.preview_state.preview_scroll = next;
        if delta > 0 {
            self.maybe_request_preview_page();
        }
    }

    pub(super) fn move_sel(&mut self, delta: isize) {
        self.files.compact_expand_pending = None;
        self.files.follow_selection = true;
        self.files.external_anchor = None;
        if self.files.visible.is_empty() {
            return;
        }
        let len = self.files.visible.len() as isize;
        let next = (self.files.selected as isize + delta).clamp(0, len - 1) as usize;
        if next != self.files.selected {
            self.files.selected = next;
            self.request_preview_for_selection();
        }
    }

    pub(super) fn move_scm(&mut self, delta: isize) {
        self.scm.pending_transfer = None;
        self.scm.follow_selection = true;
        let len = self.scm_rows().len();
        if len == 0 {
            return;
        }
        let next = (self.scm.scm_selected as isize + delta).clamp(0, (len as isize) - 1) as usize;
        if next != self.scm.scm_selected {
            self.scm.scm_selected = next;
            self.request_diff_for_scm_selection();
            self.maybe_page_history();
        }
    }

    /// Continue a compact-folder expansion through cached single-child
    /// directories. Missing listings are requested by request_missing_expanded
    /// and resumed on arrival; one click is enough even on a cold tree.
    pub(super) fn advance_compact_folder_expansion(&mut self) -> Option<PathBuf> {
        let mut path = self.files.compact_expand_pending.take()?;
        for _ in 0..128 {
            if !self.files.expanded.contains(&path) {
                break;
            }
            let Some(children) = self.files.children.get(&path) else {
                self.files.compact_expand_pending = Some(path.clone());
                break;
            };
            if children.len() != 1 || !matches!(children[0].kind, EntryKind::Directory) {
                break;
            }
            let next = children[0].path.clone();
            if !next.starts_with(&path) || next == path {
                break;
            }
            self.files.expanded.insert(next.clone());
            path = next;
        }
        Some(path)
    }

    pub(super) fn toggle_expand_at(&mut self, index: usize) {
        self.files.follow_selection = true;
        let Some(row) = self.files.visible.get(index) else {
            return;
        };
        if !matches!(row.entry.kind, EntryKind::Directory) {
            return;
        }
        let path = row.entry.path.clone();
        let was_expanded = self.files.expanded.contains(&path);
        // A later listing must not continue an expansion the user canceled.
        self.files.compact_expand_pending = None;
        let target = if was_expanded {
            self.files.expanded.remove(&path);
            // Collapsing a compact row also clears any hidden descendants,
            // so the next expansion starts from a predictable state.
            self.files
                .expanded
                .retain(|entry| !entry.starts_with(&path));
            path.clone()
        } else {
            self.files.expanded.insert(path.clone());
            if !self.files.children.contains_key(&path) {
                self.request_dir(path.clone());
            }
            if self.config.compact_folders {
                self.files.compact_expand_pending = Some(path.clone());
                self.advance_compact_folder_expansion()
                    .unwrap_or(path.clone())
            } else {
                path.clone()
            }
        };
        self.rebuild_visible();
        self.request_missing_expanded();
        if let Some(i) = self
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == target)
        {
            self.files.selected = i;
        }
        self.session.status = self.hint_bar();
        if matches!(self.preview_state.preview_mode, PreviewMode::File) {
            self.request_preview_for_selection();
        }
    }

    pub(super) fn open_selected(&mut self) {
        let Some(row) = self.files.visible.get(self.files.selected) else {
            return;
        };
        if matches!(row.entry.kind, EntryKind::Directory) {
            self.toggle_expand_at(self.files.selected);
            return;
        }
        self.session.status = format!("previewing {}", row.entry.name);
        self.request_preview_for_selection();
    }

    /// `h` / Backspace: collapse expanded dir, else select parent, else re-root up.
    pub(super) fn tree_back(&mut self) {
        self.files.follow_selection = true;
        let Some(row) = self.files.visible.get(self.files.selected).cloned() else {
            self.re_root_up();
            return;
        };
        if matches!(row.entry.kind, EntryKind::Directory) && row.expanded {
            self.toggle_expand_at(self.files.selected);
            return;
        }
        if row.depth > 0 {
            if let Some(parent_idx) = find_parent_row(&self.files.visible, self.files.selected) {
                self.files.selected = parent_idx;
                self.request_preview_for_selection();
            }
            return;
        }
        self.re_root_up();
    }

    pub(super) fn re_root_up(&mut self) {
        let Some(parent) = self.files.tree_root.parent() else {
            self.session.status = "already at root".into();
            return;
        };
        let parent = parent.to_path_buf();
        if parent == self.files.tree_root {
            self.session.status = "already at root".into();
            return;
        }
        let came_from = self
            .files
            .tree_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned());
        self.files.tree_root = parent;
        self.files.expanded.clear();
        self.files.children.clear();
        self.files.visible.clear();
        self.files.selected = 0;
        self.files.scroll = 0;
        self.files.sticky_headers.clear();
        self.preview_state.preview_scroll = 0;
        self.files.restore_select = came_from;
        self.files.last_list_click = None;
        self.session.focus = PaneFocus::List;
        self.request_list();
        self.request_status();
        if let Some(name) = &self.files.restore_select {
            self.session.status = format!("up - restoring {name}");
        }
    }
}

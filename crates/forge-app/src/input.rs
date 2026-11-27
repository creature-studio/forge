//! Keyboard, mouse, scrollbar and directory-navigation interactions.
//! Business state is still owned by App; moving these methods changes no behavior.
use super::*;
use crate::embedded_terminal::MouseRouting;
use crate::mouse_hit::{
    terminal_at, FindHit, HeaderHit, HitGeometry, ListHitGeometry, PaneHit, RowHit, TerminalHit,
};
use crate::splitter::{Axis as SplitterAxis, Gesture as SplitterGesture};

impl App {
    fn activity_action(&mut self, action: ActivityAction) {
        match action {
            ActivityAction::Files => {
                if !self.in_git_sidebar() && self.layout_state.sidebar_visible() {
                    self.set_sidebar_visible(false);
                } else {
                    if self.in_git_sidebar() {
                        self.leave_git_sidebar();
                    }
                    self.set_sidebar_visible(true);
                }
            }
            ActivityAction::Git => {
                if self.in_git_sidebar() && self.layout_state.sidebar_visible() {
                    self.set_sidebar_visible(false);
                } else {
                    if !self.in_git_sidebar() {
                        self.enter_git_sidebar();
                    }
                    self.set_sidebar_visible(true);
                }
            }
            ActivityAction::Search => self.open_tools(tools::PanelKind::Content),
            ActivityAction::Terminal => self.toggle_terminal_panel(),
            ActivityAction::Settings => self.open_tools(tools::PanelKind::Settings),
            ActivityAction::Collapse => self.set_sidebar_visible(false),
        }
    }

    fn toggle_terminal_panel(&mut self) {
        let restart = self.terminals.active.as_ref().is_none_or(|term| {
            !term.is_visible() && term.ended.load(std::sync::atomic::Ordering::Acquire)
        });
        if restart {
            let was_opened = self.terminals.active.is_some();
            self.open_terminal_tab();
            if was_opened
                && self
                    .terminals
                    .active
                    .as_ref()
                    .is_some_and(|t| t.is_visible())
            {
                self.session.status = "Terminal restarted".into();
            }
        } else if let Some(term) = &mut self.terminals.active {
            if term.is_visible() {
                term.set_visible(false);
            } else {
                term.activate();
            }
        }
        self.reconcile_terminal_focus();
    }

    fn open_terminal_tab(&mut self) {
        let local;
        let profiles = if let Some(profiles) = &self.shell_profiles {
            profiles
        } else {
            local = embedded_terminal::shell::local_profiles(&self.config.terminal);
            &local
        };
        match embedded_terminal::shell::resolve_default(&self.config.terminal, profiles) {
            Ok(profile) => self.open_terminal_with_profile(profile.as_ref()),
            Err(error) => {
                self.session.status = error.to_string();
                self.open_shell_profiles(false);
            }
        }
    }

    pub(super) fn open_shell_profiles(&mut self, set_default: bool) {
        use std::sync::{Arc, Mutex};
        if self.shell_profiles.is_none() && self.shell_discovery.is_none() {
            let result = Arc::new(Mutex::new(None));
            self.shell_discovery = Some(result.clone());
            let config = self.config.terminal.clone();
            let notify = self.terminal_notify.clone();
            std::thread::spawn(move || {
                let profiles = embedded_terminal::shell::discover(&config);
                if let Ok(mut slot) = result.lock() {
                    *slot = Some(profiles);
                }
                notify.notify_one();
            });
        }
        self.render_shell_profiles_menu(set_default);
    }

    fn render_shell_profiles_menu(&mut self, set_default: bool) {
        use forge_ui::ContextMenuItem;
        let mut items = Vec::new();
        let mut labels = std::collections::HashMap::new();
        if let Some(profiles) = &self.shell_profiles {
            let default =
                embedded_terminal::shell::resolve_default(&self.config.terminal, profiles)
                    .ok()
                    .flatten()
                    .map(|p| p.id);
            for (i, profile) in profiles.iter().enumerate() {
                let action = ContextAction::ShellProfile(i);
                items.push(ContextMenuItem {
                    action,
                    enabled: true,
                });
                labels.insert(
                    action,
                    format!(
                        "{}{}",
                        profile.name,
                        if default.as_ref() == Some(&profile.id) {
                            " ✓"
                        } else {
                            ""
                        }
                    ),
                );
            }
        } else {
            items.push(ContextMenuItem {
                action: ContextAction::ShellLoading,
                enabled: false,
            });
        }
        if !set_default {
            items.push(ContextMenuItem {
                action: ContextAction::ShellDefaultMenu,
                enabled: true,
            });
        }
        items.push(ContextMenuItem {
            action: ContextAction::ShellConfigure,
            enabled: true,
        });
        let bar = self.terminal_tab_bar;
        let x = self
            .terminal_tab_hits
            .iter()
            .find_map(|(area, action)| (*action == TerminalTabAction::Profiles).then_some(area.x))
            .unwrap_or_else(|| bar.right().saturating_sub(25));
        let y = bar.bottom();
        self.session.context_menu =
            ContextMenu::new(items, x, y, ContextTarget::TerminalProfiles { set_default });
        if let Some(menu) = &mut self.session.context_menu {
            menu.labels = labels;
        }
    }

    pub(super) fn poll_shell_discovery(&mut self) {
        let result = self
            .shell_discovery
            .as_ref()
            .and_then(|r| r.lock().ok()?.take());
        if let Some(profiles) = result {
            self.shell_discovery = None;
            self.shell_profiles = Some(profiles);
            if let Some(ContextTarget::TerminalProfiles { set_default }) =
                self.session.context_menu.as_ref().map(|m| &m.target)
            {
                let set_default = *set_default;
                self.render_shell_profiles_menu(set_default);
            }
        }
    }

    pub(super) fn choose_shell_profile(&mut self, index: usize, set_default: bool) {
        let Some(profile) = self
            .shell_profiles
            .as_ref()
            .and_then(|p| p.get(index))
            .cloned()
        else {
            return;
        };
        if set_default {
            let old = self
                .config
                .terminal
                .default_profile
                .replace(profile.id.clone());
            if let Err(error) = self.config.save() {
                self.config.terminal.default_profile = old;
                self.session.status = format!("Cannot save shell profile: {error}");
                return;
            }
            self.session.status = format!("Default terminal: {}", profile.name);
        } else {
            self.open_terminal_with_profile(Some(&profile));
        }
    }

    pub(super) fn configure_shell_profiles(&mut self) {
        let Some(path) = forge_config::Config::path() else {
            self.session.status = "Configuration path is unavailable".into();
            return;
        };
        if !path.exists() {
            if let Err(error) = self.config.save() {
                self.session.status = format!("Cannot create config: {error}");
                return;
            }
        }
        self.shell_profiles = None;
        self.shell_discovery = None;
        self.open_terminal_config_editor(path);
        self.session.status =
            "Edit [terminal.profiles] in config.toml; changes reload on save".into();
    }

    fn open_terminal_with_profile(
        &mut self,
        profile: Option<&embedded_terminal::shell::ShellProfile>,
    ) {
        if !self.terminals.can_open() {
            self.session.status = "Terminal limit: 8 tabs (Ctrl+Shift+W closes one)".into();
            return;
        }
        let next = if let Some(profile) = profile {
            embedded_terminal::EmbeddedTerminal::start_with_profile(
                self.files.root(),
                80,
                12,
                self.terminal_notify.clone(),
                Some(profile),
            )
        } else {
            embedded_terminal::EmbeddedTerminal::start(
                self.files.root(),
                80,
                12,
                self.terminal_notify.clone(),
            )
        };
        match next {
            Ok(next) => {
                self.terminals.install(next);
                self.reconcile_terminal_focus();
                self.session.status = "New terminal tab".into();
            }
            Err(error) => self.session.status = format!("Terminal: {error}"),
        }
    }

    fn activate_terminal_number(&mut self, position: usize) {
        self.terminals.activate_number(position);
        self.reconcile_terminal_focus();
    }

    fn switch_terminal_tab(&mut self, backwards: bool) {
        self.terminals.switch(backwards);
        self.reconcile_terminal_focus();
    }

    fn activate_terminal_tab(&mut self, id: u64) {
        self.terminals.activate(id);
        self.reconcile_terminal_focus();
    }

    fn close_terminal_tab_by_id(&mut self, id: u64) {
        if self.terminals.close_by_id(id) {
            self.reconcile_terminal_focus();
            self.session.status = "Terminal tab closed".into();
        }
    }

    fn open_terminal_help(&mut self) {
        self.terminal_help_return = self
            .terminals
            .active
            .as_ref()
            .is_some_and(|term| term.is_visible() && term.is_focused());
        if let Some(term) = &mut self.terminals.active {
            term.set_focused(false);
        }
        self.session.help_open = true;
        self.session.help_reference = false;
        self.session.help_scroll = 0;
    }

    fn toggle_terminal_mouse_routing(&mut self) {
        if let Some(term) = &mut self.terminals.active {
            self.session.status = match term.toggle_mouse_routing() {
                MouseRouting::SelectText => {
                    "Terminal mouse: select text (Ctrl+drag for native drag)".into()
                }
                MouseRouting::Native => {
                    "Terminal mouse: native TUI drag (Shift+drag selects text)".into()
                }
            };
        }
    }

    fn handle_terminal_tab_action(&mut self, action: TerminalTabAction) {
        match action {
            TerminalTabAction::Activate(id) => self.activate_terminal_tab(id),
            TerminalTabAction::Close(id) => self.close_terminal_tab_by_id(id),
            TerminalTabAction::New => self.open_terminal_tab(),
            TerminalTabAction::Profiles => self.open_shell_profiles(false),
            TerminalTabAction::Previous => self.switch_terminal_tab(true),
            TerminalTabAction::Next => self.switch_terminal_tab(false),
            TerminalTabAction::Maximize => {
                if let Some(term) = &mut self.terminals.active {
                    term.maximized = !term.maximized;
                }
            }
            TerminalTabAction::ToggleMouseRouting => self.toggle_terminal_mouse_routing(),
            TerminalTabAction::Help => self.open_terminal_help(),
            TerminalTabAction::Hide => {
                if let Some(term) = &mut self.terminals.active {
                    term.set_visible(false);
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
            self.show_terminal_copy_notice("Selection too large", true);
            return;
        }
        #[cfg(not(test))]
        {
            let native = copy_path_clipboard(text).is_ok();
            let osc52 = if native {
                false
            } else {
                crossterm::execute!(
                    std::io::stdout(),
                    crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)
                )
                .is_ok()
            };
            self.show_terminal_copy_notice(
                if native {
                    "Copied to clipboard"
                } else if osc52 {
                    "Copy sent to clipboard"
                } else {
                    "Clipboard unavailable"
                },
                !native && !osc52,
            );
            self.session.status = if native {
                format!("Copied {} terminal characters", text.chars().count())
            } else if osc52 {
                format!(
                    "Terminal copied via OSC 52 ({} chars)",
                    text.chars().count()
                )
            } else {
                "Terminal clipboard unavailable".into()
            };
        }
        #[cfg(test)]
        {
            self.session.status = format!("Terminal selected {} characters", text.chars().count());
            self.show_terminal_copy_notice("Copied to clipboard", false);
        }
    }

    fn close_terminal_tab(&mut self) {
        if self.terminals.close_active() {
            self.reconcile_terminal_focus();
            self.session.status = "Terminal tab closed".into();
        }
    }

    fn handle_context_menu_key(&mut self, key: KeyEvent) {
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
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        // Keyboard input cancels unfinished pointer activation. In particular,
        // Esc followed by mouse movement/release must not finish a stale
        // Files/Git Activity Bar click.
        self.session.activity_pressed = None;
        let owner = self.key_owner(key);
        if self.pointer_capture().is_some()
            && (self.modal_open()
                || (owner != input_router::KeyOwner::Terminal
                    && matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::F(2)))
                || matches!(key.code, KeyCode::F(8) | KeyCode::F(11) | KeyCode::F(12)))
        {
            self.cancel_pointer_capture();
        }
        // Resolve the keyboard owner before PTY shortcuts or workspace commands.
        match owner {
            input_router::KeyOwner::Commit => return, // Component-local editor.
            input_router::KeyOwner::Shortcuts => {
                self.keys.handle(key);
                return;
            }
            input_router::KeyOwner::Help => {
                self.handle_help_key(key);
                return;
            }
            input_router::KeyOwner::Blame => {
                self.handle_blame_key(key);
                return;
            }
            input_router::KeyOwner::Editor => {
                self.handle_editor_key(key);
                return;
            }
            input_router::KeyOwner::Tools => {
                self.handle_tools_key(key);
                return;
            }
            input_router::KeyOwner::Menu => {
                self.handle_context_menu_key(key);
                return;
            }
            input_router::KeyOwner::Find => {
                if key.code == KeyCode::F(4) && key.modifiers.is_empty() {
                    self.keys.open = true;
                } else {
                    self.handle_file_find_key(key);
                }
                return;
            }
            input_router::KeyOwner::Terminal | input_router::KeyOwner::Workspace => {}
        }
        // Passive Find leaves workspace navigation available.
        if self.preview_state.find.open && matches!(key.code, KeyCode::F(1) | KeyCode::F(2)) {
            self.close_file_find();
        }
        // Alt+1..8 selects the *visible tab number* from anywhere in the
        // workspace while Terminal is shown. Do not steal modal/editor keys.
        if self
            .terminals
            .active
            .as_ref()
            .is_some_and(|term| term.is_visible())
            && !self.keys.open
            && !self.session.help_open
            && self.tools.panel.is_none()
            && !self.editing.active()
            && key.modifiers == KeyModifiers::ALT
        {
            if let KeyCode::Char(d @ '1'..='8') = key.code {
                self.activate_terminal_number(d as usize - '1' as usize);
                return;
            }
        }
        // The VS Code palette chord is an explicit host shortcut when the
        // PTY is focused too. Honor user bindings instead of stealing a chord
        // which was rebound to another action.
        if key
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('p' | 'P'))
            && self.keys.remap(key, false).is_some_and(|mapped| {
                mapped.code == KeyCode::Char('k') && mapped.modifiers == KeyModifiers::CONTROL
            })
        {
            self.open_tools(tools::PanelKind::Commands);
            return;
        }
        // F4 in Terminal opens the in-app help with a dedicated Terminal guide.
        if key.code == KeyCode::F(4)
            && key.modifiers.is_empty()
            && self
                .terminals
                .active
                .as_ref()
                .is_some_and(|term| term.is_visible() && term.is_focused())
        {
            self.open_terminal_help();
            return;
        }
        if key.code == KeyCode::F(4) && key.modifiers.is_empty() {
            self.keys.open = true;
            return;
        }
        // Function-key alternatives work even in host terminals that swallow
        // Ctrl+Shift combinations or report them as plain Ctrl.
        if key.code == KeyCode::F(8)
            && key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        {
            self.open_shell_profiles(false);
            return;
        }
        if key.code == KeyCode::F(8) && key.modifiers == KeyModifiers::SHIFT {
            self.open_terminal_tab();
            return;
        }
        if key.code == KeyCode::F(12)
            && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            && self
                .terminals
                .active
                .as_ref()
                .is_some_and(|t| t.is_visible())
        {
            self.switch_terminal_tab(key.modifiers.contains(KeyModifiers::SHIFT));
            return;
        }
        // Host shortcuts are intercepted before the child PTY receives keys.
        let both = key
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        if both {
            match key.code {
                KeyCode::Char('N' | 'n') => {
                    self.open_terminal_tab();
                    return;
                }
                KeyCode::Char('W' | 'w') if self.terminals.active.is_some() => {
                    self.close_terminal_tab();
                    return;
                }
                KeyCode::Char('M' | 'm')
                    if self
                        .terminals
                        .active
                        .as_ref()
                        .is_some_and(|t| t.is_visible() && t.is_focused()) =>
                {
                    self.toggle_terminal_mouse_routing();
                    return;
                }
                KeyCode::Left if self.terminals.active.is_some() => {
                    self.switch_terminal_tab(true);
                    return;
                }
                KeyCode::Right if self.terminals.active.is_some() => {
                    self.switch_terminal_tab(false);
                    return;
                }
                KeyCode::Char('C' | 'c') if self.terminals.active.is_some() => {
                    let selected = self
                        .terminals
                        .active
                        .as_ref()
                        .and_then(|t| t.selected_text());
                    if let Some(selected) = selected.filter(|s| !s.is_empty()) {
                        self.copy_terminal_text(&selected);
                    } else {
                        self.session.status = "Select terminal text with the mouse first".into();
                    }
                    return;
                }
                KeyCode::Char('V' | 'v')
                    if self
                        .terminals
                        .active
                        .as_ref()
                        .is_some_and(|t| t.is_visible()) =>
                {
                    let pasted = arboard::Clipboard::new().and_then(|mut cb| cb.get_text());
                    match pasted {
                        Ok(text) => {
                            if let Some(term) = &mut self.terminals.active {
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
        if let Some(term) = &mut self.terminals.active {
            if term.is_visible()
                && term.is_focused()
                && !term.on_alternate_screen()
                && key.modifiers.contains(KeyModifiers::SHIFT)
            {
                match key.code {
                    KeyCode::PageUp => {
                        term.scroll(-10);
                        return;
                    }
                    KeyCode::PageDown => {
                        term.scroll(10);
                        return;
                    }
                    KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        term.scroll(isize::MIN);
                        return;
                    }
                    KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        term.scroll_to_bottom();
                        return;
                    }
                    _ => {}
                }
            }
        }
        // F11 toggles terminal maximize while preserving the PTY and its
        // shell session. Ctrl+Shift+Enter is supported when distinguishable.
        if (key.code == KeyCode::F(11) && key.modifiers.is_empty())
            || (key.code == KeyCode::Enter
                && key.modifiers == KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        {
            if let Some(term) = &mut self.terminals.active {
                if term.is_visible() {
                    term.maximized = !term.maximized;
                    self.focus_terminal_input();
                    return;
                }
            }
        }
        // F8 is dependable across terminal emulators; Ctrl+` is a convenience
        // where the host reports it separately from Ctrl+Space.
        if (key.code == KeyCode::F(8) && key.modifiers.is_empty())
            || (key.code == KeyCode::Char('`') && key.modifiers == KeyModifiers::CONTROL)
        {
            self.toggle_terminal_panel();
            return;
        }
        if let Some(term) = &mut self.terminals.active {
            if term.is_visible() && term.is_focused() {
                // Ctrl+Shift+Tab returns focus to Forge without hiding PTY.
                if key.code == KeyCode::BackTab && key.modifiers.contains(KeyModifiers::CONTROL) {
                    term.set_focused(false);
                } else if let Err(error) = term.send(key) {
                    self.session.status = format!("Terminal input failed: {error}");
                    term.set_focused(false);
                }
                return;
            }
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
            let x = self.layout_state.geometry.list_inner.x.saturating_add(3);
            let y = self.layout_state.geometry.list_inner.y.saturating_add(1);
            if self.session.focus == PaneFocus::Preview {
                let x = self.layout_state.geometry.preview_inner.x.saturating_add(3);
                let y = self.layout_state.geometry.preview_inner.y.saturating_add(1);
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
        // Physical file shortcuts belong only to the Files tree, never to
        // the editor, preview selection, Git or an embedded child terminal.
        if self.sidebar_focused() && !self.in_git_sidebar() {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c' | 'C'), KeyModifiers::CONTROL) => {
                    self.capture_file_clipboard(true);
                    return;
                }
                (KeyCode::Char('x' | 'X'), KeyModifiers::CONTROL) => {
                    self.capture_file_clipboard(false);
                    return;
                }
                (KeyCode::Char('v' | 'V'), KeyModifiers::CONTROL) => {
                    self.paste_files(None);
                    return;
                }
                (KeyCode::Char('a' | 'A'), KeyModifiers::CONTROL) => {
                    self.run_command(tools::Command::MarkAll);
                    return;
                }
                (KeyCode::Delete, KeyModifiers::NONE) => {
                    self.trash_selected_files();
                    return;
                }
                (KeyCode::Delete, KeyModifiers::SHIFT) => {
                    self.permanently_delete_selected_files();
                    return;
                }
                (KeyCode::Up | KeyCode::Down, KeyModifiers::SHIFT) => {
                    let old = self.files.selected;
                    self.move_sel(if key.code == KeyCode::Up { -1 } else { 1 });
                    self.mark_file_range(old, self.files.selected, false);
                    return;
                }
                (KeyCode::Esc, KeyModifiers::NONE) => {
                    self.tools.file_selection_anchor = None;
                }
                (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE) => {
                    self.tools.file_selection_anchor = None;
                }
                _ => {}
            }
        }
        let Some(key) = self.keys.remap(key, false) else {
            return;
        };
        // A commit-detail link has explicit keyboard focus. Tab enters the
        // link list only while this preview is shown; normal previews keep
        // Tab's workspace-pane switching behavior.
        if self.session.focus == PaneFocus::Preview {
            let links = crate::view::commit_detail::file_link_count(self);
            if links > 0 {
                match key.code {
                    KeyCode::Tab if key.modifiers.is_empty() => {
                        self.preview_state.commit_link_focus = Some(
                            self.preview_state
                                .commit_link_focus
                                .map_or(0, |index| (index + 1) % links),
                        );
                        return;
                    }
                    KeyCode::BackTab
                        if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                    {
                        match self.preview_state.commit_link_focus {
                            Some(0) | None => {
                                self.preview_state.commit_link_focus = None;
                                self.session.focus = PaneFocus::List;
                            }
                            Some(index) => self.preview_state.commit_link_focus = Some(index - 1),
                        }
                        return;
                    }
                    KeyCode::Enter if self.preview_state.commit_link_focus.is_some() => {
                        let index = self.preview_state.commit_link_focus.take().unwrap();
                        self.jump_commit_detail_file(index);
                        return;
                    }
                    _ => {}
                }
            } else {
                self.preview_state.commit_link_focus = None;
            }
            // Shift+arrows select exact rendered preview characters, including
            // Unicode and wrapped rows. A single physical Shift+Right selects
            // the first character; Ctrl+C reuses mouse selection's copy.
            if key.modifiers == KeyModifiers::SHIFT
                && matches!(
                    key.code,
                    KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Home
                        | KeyCode::End
                )
            {
                let scroll = self.preview_state.presentation.preview_scroll;
                if let Some(row) = self
                    .preview_state
                    .presentation
                    .selection
                    .keyboard_extend(key.code, scroll)
                {
                    let height = self.layout_state.geometry.preview_inner.height.max(1) as usize;
                    if row < scroll {
                        self.preview_state.presentation.preview_scroll = row;
                    } else if row >= scroll + height {
                        self.preview_state.presentation.preview_scroll =
                            row.saturating_sub(height - 1);
                        self.maybe_request_preview_page();
                    }
                }
                return;
            }
        }
        if !input_router::workspace_modifier_allowed(key) {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down
                if key.modifiers == KeyModifiers::ALT
                    && self.session.focus == PaneFocus::Preview =>
            {
                self.jump_overview_marker(key.code == KeyCode::Down);
            }
            KeyCode::F(1) => {
                if self.in_git_sidebar() {
                    self.leave_git_sidebar();
                }
                self.set_sidebar_visible(true);
            }
            KeyCode::F(2) => {
                if !self.in_git_sidebar() {
                    self.enter_git_sidebar();
                }
                self.set_sidebar_visible(true);
            }
            KeyCode::Char('b' | 'B') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_sidebar_visible(!self.layout_state.sidebar_visible());
            }
            KeyCode::Left
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.layout_state.adjust_sidebar(-2);
            }
            KeyCode::Right
                if key
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.layout_state.adjust_sidebar(2);
            }
            KeyCode::F(3) => self.set_markdown_code(!self.preview_state.markdown_code),
            KeyCode::F(6) => self.open_blame_popup(),
            KeyCode::F(7) if self.session.focus == PaneFocus::Preview => {
                if !self.close_inline_peek() {
                    self.toggle_inline_peek();
                }
            }
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
            KeyCode::F(5) => {
                self.session.help_open = true;
                self.session.help_reference = false;
                self.session.help_scroll = 0;
            }
            KeyCode::Char('q') => self.session.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.session.focus == PaneFocus::Preview
                    && self.preview_state.presentation.selection.active()
                {
                    self.copy_preview_selection();
                } else {
                    self.session.status = "Use q to quit · Ctrl-C does not exit Forge".into();
                }
            }
            KeyCode::Esc => self.handle_workspace_escape(),
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
                    && matches!(
                        self.preview_state.preview_mode,
                        PreviewMode::File | PreviewMode::Diff
                    ) =>
            {
                let max_x = self
                    .preview_state
                    .max_horizontal_offset(self.layout_state.geometry.preview_inner.width);
                let previous_x = self.preview_state.presentation.preview_scroll_x;
                let offset = &mut self.preview_state.presentation.preview_scroll_x;
                *offset = match key.code {
                    KeyCode::Left => offset.saturating_sub(8),
                    KeyCode::Right => offset.saturating_add(8).min(max_x),
                    KeyCode::Home => 0,
                    KeyCode::End => max_x,
                    _ => *offset,
                };
                if previous_x != *offset {
                    // Old visual selection coordinates refer to a different horizontal window.
                    self.preview_state.presentation.selection.clear();
                }
            }
            KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Left => {
                if matches!(self.session.focus, PaneFocus::Preview) {
                    // A collapsed sidebar must be made visible before it can
                    // receive keyboard focus.
                    self.set_sidebar_visible(true);
                } else if self.in_git_sidebar() {
                    self.scm_tree_back();
                } else {
                    self.tree_back();
                }
            }
            KeyCode::Char('r' | 'R') if key.modifiers == KeyModifiers::CONTROL => {
                self.request_list();
                self.request_status();
                if self.in_git_sidebar() && self.history.expanded() {
                    self.request_history();
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.preview_state.commit_link_focus = None;
                // A collapsed Sidebar must not receive invisible keyboard focus.
                if self.layout_state.sidebar_visible() {
                    self.session.focus = match self.session.focus {
                        PaneFocus::List | PaneFocus::Staging => PaneFocus::Preview,
                        PaneFocus::Preview => PaneFocus::List,
                    };
                } else {
                    self.session.focus = PaneFocus::Preview;
                }
            }
            KeyCode::Home => {
                if self.session.focus == PaneFocus::Preview {
                    self.preview_state.presentation.preview_scroll = 0;
                } else {
                    self.session.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.scm.cancel_transfer();
                        self.scm.follow_selection = true;
                        self.scm.scm_selected = 0;
                        self.request_diff_for_scm_selection();
                    } else {
                        self.navigation.cancel_reveal();
                        self.files.follow_selection = true;
                        self.files.selected = 0;
                        self.request_preview_for_selection();
                    }
                }
            }
            KeyCode::End => {
                if self.session.focus == PaneFocus::Preview {
                    self.preview_state.presentation.preview_scroll = self
                        .preview_total_lines()
                        .saturating_sub(self.layout_state.geometry.preview_inner.height as usize);
                    self.maybe_request_preview_page();
                } else {
                    self.session.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.scm.cancel_transfer();
                        self.scm.follow_selection = true;
                        self.scm.scm_selected = self.scm_rows().len().saturating_sub(1);
                        self.request_diff_for_scm_selection();
                        self.maybe_page_history();
                    } else {
                        self.navigation.cancel_reveal();
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
                        let page = self
                            .layout_state
                            .geometry
                            .preview_inner
                            .height
                            .saturating_sub(2)
                            .max(1) as isize;
                        self.scroll_preview(sign * page);
                    }
                    PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                        let page = self
                            .layout_state
                            .geometry
                            .list_inner
                            .height
                            .saturating_sub(2)
                            .max(1) as isize;
                        self.move_scm(sign * page);
                    }
                    _ => {
                        let page = self
                            .layout_state
                            .geometry
                            .list_inner
                            .height
                            .saturating_sub(2)
                            .max(1) as isize;
                        self.move_sel(sign * page);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_help_mouse(&mut self, mouse: MouseEvent) {
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = self.session.help_area.contains(point);
                if self.session.help_close_rect.contains(point) || !inside {
                    self.handle_help_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
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

    /// Resolve Activity Bar buttons at painted coordinates. Prefer the shared
    /// hit targets, falling back to the same layout rectangles while a frame
    /// is being rebuilt. Pointer motion never calls the action itself.
    fn activity_at(&self, column: u16, row: u16) -> Option<ActivityAction> {
        let point = ratatui::layout::Position::new(column, row);
        self.activity_hits
            .action_at(column, row)
            .or_else(|| HitGeometry::new(&self.layout_state.geometry).activity(point))
    }

    fn pointer_capture(&self) -> Option<input_router::PointerCapture> {
        input_router::PointerCapture::resolve(
            self.layout_state.splitter.captured(),
            self.terminals
                .active
                .as_ref()
                .is_some_and(|t| t.is_visible() && t.captures_left_drag()),
            self.scrollbar_drag.is_some(),
            self.preview_state.presentation.selection.is_dragging(),
            self.session.activity_pressed.is_some(),
        )
    }

    fn cancel_pointer_capture(&mut self) {
        self.layout_state.splitter.cancel();
        self.scrollbar_drag = None;
        self.preview_state.presentation.selection.end_drag();
        self.session.activity_pressed = None;
        if let Some(term) = &mut self.terminals.active {
            if term.captures_left_drag() {
                term.cancel_pointer_capture();
            }
        }
    }

    fn captured_preview_mouse(&mut self, mouse: MouseEvent) {
        let release = matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left));
        self.preview_state.presentation.selection.move_to(
            self.layout_state.geometry.preview_inner,
            self.preview_state.presentation.preview_scroll,
            mouse.column,
            mouse.row,
            release,
        );
        if release && self.preview_state.presentation.selection.active() {
            self.session.status = "Preview selection ready · Ctrl-C Copy · Esc Clear".into();
        }
    }

    /// Deliver continuation events before fresh hit testing. A drag/release
    /// is never routed by the pane under the current pointer.
    fn route_captured_mouse(&mut self, mouse: MouseEvent, modal: bool) -> bool {
        use input_router::PointerCapture::*;
        let Some(owner) = self.pointer_capture() else {
            return false;
        };
        if modal || matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.cancel_pointer_capture();
            return false;
        }
        let continuation = matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        );
        match owner {
            Splitter => false, // Existing splitter handler owns its gesture.
            Terminal => {
                if continuation
                    || matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) && self
                        .terminals
                        .active
                        .as_ref()
                        .is_some_and(|t| t.captures_host_selection())
                {
                    if let Some(term) = &mut self.terminals.active {
                        let result = term.mouse(mouse);
                        if term.selection_autoscroll_active() {
                            self.selection_scroll_notify.notify_one();
                        }
                        match result {
                            Ok(Some(text)) if !text.is_empty() => self.copy_terminal_text(&text),
                            Err(error) => self.session.status = format!("Terminal mouse: {error}"),
                            _ => {}
                        }
                    }
                    true
                } else {
                    false
                }
            }
            Scrollbar if continuation => self.handle_scrollbar_mouse(mouse),
            PreviewSelection if continuation => {
                self.captured_preview_mouse(mouse);
                true
            }
            _ => false, // Activity release/drag handled by its click policy.
        }
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        if matches!(mouse.kind, MouseEventKind::Moved) {
            if let Some(pressed) = self.session.activity_pressed {
                if self.activity_at(mouse.column, mouse.row) != Some(pressed) {
                    self.session.activity_pressed = None;
                }
            }
            // A pointer move is presentation-only. Never let Hover events
            // reach the generic click/drag/selection router: after repeated
            // Esc the cursor moving must not silently switch pane or Activity.
            self.terminal_tab_hits.hover_at(mouse.column, mouse.row);
            self.activity_hits.hover_at(mouse.column, mouse.row);
            self.commit_file_links.hover_at(mouse.column, mouse.row);
            if let Some(menu) = self.session.context_menu.as_mut() {
                menu.hover_at(mouse.column, mouse.row);
            }
            // Preserve xterm mouse-tracking for an embedded PTY, but it owns
            // its own focus; motion alone cannot focus it or the workspace.
            let modal = self.modal_open();
            self.layout_state.splitter.hover(
                &self.layout_state.geometry,
                self.layout_state.sidebar_visible(),
                point,
                modal,
            );
            if !modal {
                if let Some(term) = &mut self.terminals.active {
                    if term.is_visible() && term.area.contains(point) {
                        match term.mouse(mouse) {
                            Ok(Some(text)) if !text.is_empty() => self.copy_terminal_text(&text),
                            Err(error) => self.session.status = format!("Terminal mouse: {error}"),
                            _ => {}
                        }
                    }
                }
            }
            return;
        }
        let modal = self.modal_open();
        if self.route_captured_mouse(mouse, modal) {
            return;
        }
        if self.modal_owner() == Some(input_router::KeyOwner::Help) {
            self.terminal_tab_hits.clear_hover();
            self.handle_help_mouse(mouse);
            return;
        }
        // Input ownership must follow the clicked pane, not visibility of
        // the embedded terminal. Popups always take priority over the PTY.
        // Both dividers share one pointer-capturing splitter component.
        // It owns a gesture until release (including outside the hit region)
        // and prevents accidental file clicks or PTY mouse injection.
        let splitter_gesture = self.layout_state.splitter.mouse(
            mouse,
            &self.layout_state.geometry,
            self.layout_state.sidebar_visible(),
            modal,
        );
        match splitter_gesture {
            SplitterGesture::Started => {
                self.session.activity_pressed = None;
                return;
            }
            SplitterGesture::Moved(SplitterAxis::Horizontal, row)
            | SplitterGesture::Released(SplitterAxis::Horizontal, row) => {
                self.layout_state.resize_terminal_at(row);
                return;
            }
            SplitterGesture::Moved(SplitterAxis::Vertical, column)
            | SplitterGesture::Released(SplitterAxis::Vertical, column) => {
                self.layout_state.resize_sidebar_at(column);
                return;
            }
            SplitterGesture::None => {}
        }
        // Activity Bar is a click target, never an on-press navigation target.
        // A stray press reported during mouse movement must not switch Files/Git.
        // Only release on the same control commits an action; dragging cancels.
        if !modal {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.session.activity_pressed = self.activity_at(mouse.column, mouse.row);
                    if self.session.activity_pressed.is_some() {
                        return;
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if let Some(pressed) = self.session.activity_pressed.take() {
                        if self.activity_at(mouse.column, mouse.row) == Some(pressed) {
                            if pressed != ActivityAction::Terminal {
                                self.focus_workspace_input();
                            }
                            self.activity_action(pressed);
                        }
                        return;
                    }
                }
                MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                    if self.session.activity_pressed.take().is_some() =>
                {
                    return;
                }
                _ => {}
            }
        } else {
            self.session.activity_pressed = None;
        }
        let terminal_hit = terminal_at(
            point,
            self.terminals
                .active
                .as_ref()
                .filter(|term| term.is_visible())
                .map(|term| term.area),
            self.terminal_tab_bar,
        );
        if !modal && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            if terminal_hit == TerminalHit::Outside
                && HitGeometry::new(&self.layout_state.geometry).workspace(point)
            {
                if self.preview_state.find.open
                    && HitGeometry::new(&self.layout_state.geometry)
                        .find(point)
                        .is_some()
                {
                    self.release_terminal_input();
                } else {
                    self.focus_workspace_input();
                }
            } else if terminal_hit == TerminalHit::Body {
                self.focus_terminal_input();
            }
        }
        // Route terminal mouse events to the child when it enabled tracking.
        // Otherwise scroll/drag select in the host and copy on release.
        // Shift always overrides child tracking for host text selection.
        if !modal && terminal_hit == TerminalHit::Body {
            if let Some(term) = &mut self.terminals.active {
                match term.mouse(mouse) {
                    Ok(Some(text)) if !text.is_empty() => self.copy_terminal_text(&text),
                    Err(error) => self.session.status = format!("Terminal mouse: {error}"),
                    _ => {}
                }
                return;
            }
        }
        // The tab strip is part of Forge, not of the nested terminal's PTY.
        // Hit boxes are produced by the same frame renderer that paints them.
        if !modal && terminal_hit == TerminalHit::Tabs {
            let action = self.terminal_tab_hits.action_at(mouse.column, mouse.row);
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(action) = action {
                        self.handle_terminal_tab_action(action);
                    }
                }
                MouseEventKind::Down(MouseButton::Middle) => {
                    if let Some(TerminalTabAction::Activate(id) | TerminalTabAction::Close(id)) =
                        action
                    {
                        self.close_terminal_tab_by_id(id);
                    }
                }
                MouseEventKind::ScrollUp => self.switch_terminal_tab(true),
                MouseEventKind::ScrollDown => self.switch_terminal_tab(false),
                _ => {}
            }
            return;
        }
        match self.modal_owner() {
            Some(input_router::KeyOwner::Commit | input_router::KeyOwner::Shortcuts) => return,
            Some(input_router::KeyOwner::Help) => {
                self.handle_help_mouse(mouse);
                return;
            }
            Some(input_router::KeyOwner::Blame) => {
                match mouse.kind {
                    MouseEventKind::ScrollDown => self.scroll_blame(3),
                    MouseEventKind::ScrollUp => self.scroll_blame(-3),
                    _ => {}
                }
                return;
            }
            Some(input_router::KeyOwner::Editor) => {
                self.handle_editor_mouse(mouse);
                return;
            }
            Some(input_router::KeyOwner::Tools) => {
                self.handle_tools_mouse(mouse);
                return;
            }
            _ => {}
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
                _ => {}
            }
            return;
        }

        if self.preview_state.find.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            if let Some(hit) = HitGeometry::new(&self.layout_state.geometry).find(point) {
                match hit {
                    FindHit::Input => self.focus_find_input(),
                    FindHit::Close => self.close_file_find(),
                    FindHit::Next => self.navigate_file_find(true),
                    FindHit::Previous => self.navigate_file_find(false),
                    FindHit::MatchCase => {
                        self.preview_state.find.match_case = !self.preview_state.find.match_case;
                        self.start_file_find();
                    }
                    FindHit::WholeWord => {
                        self.preview_state.find.whole_word = !self.preview_state.find.whole_word;
                        self.start_file_find();
                    }
                    FindHit::Regex => {
                        self.preview_state.find.regex = !self.preview_state.find.regex;
                        self.start_file_find();
                    }
                    FindHit::Background => {}
                }
                return;
            }
            self.blur_find_input();
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && !self.inline_popup_contains(mouse.column, mouse.row)
            && self
                .layout_state
                .geometry
                .preview_inner
                .contains(ratatui::layout::Position::new(mouse.column, mouse.row))
        {
            if let Some(path) = self.blame_path() {
                let scroll = self.preview_state.presentation.preview_scroll;
                let row = scroll
                    + mouse
                        .row
                        .saturating_sub(self.layout_state.geometry.preview_inner.y)
                        as usize;
                self.blame.cursor = Some((path, row, scroll));
            }
        }
        self.handle_mouse_content(mouse);
    }

    pub(super) fn handle_mouse_content(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let point = ratatui::layout::Position::new(col, row);
            if let Some(file) = self.commit_file_links.action_at(col, row) {
                self.jump_commit_detail_file(file);
                return;
            }
            if let Some(hit) = HitGeometry::new(&self.layout_state.geometry).header(point) {
                match hit {
                    HeaderHit::Sync => self.run_command(tools::Command::Sync),
                    HeaderHit::OpenFile => self.open_selected_worktree_file(),
                    HeaderHit::MarkdownPreview | HeaderHit::MarkdownCode => {
                        self.set_markdown_code(hit == HeaderHit::MarkdownCode);
                        self.session.focus = PaneFocus::Preview;
                    }
                }
                return;
            }
        }
        if self.handle_scrollbar_mouse(mouse) {
            return;
        }
        let point = ratatui::layout::Position::new(col, row);
        let hit = HitGeometry::new(&self.layout_state.geometry).pane(point);
        let over_list = hit == PaneHit::List;
        let over_preview = hit == PaneHit::Preview;
        let git = self.in_git_sidebar();
        // The terminal is in mouse-capture mode, so native OS text selection
        // cannot work. Preview selection owns left drag events instead.
        let region = self.layout_state.geometry.preview_inner;
        if matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) && self.preview_state.presentation.selection.is_dragging()
        {
            self.captured_preview_mouse(mouse);
            return;
        }
        if self.config.show_overview_ruler
            && matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
            )
        {
            if let Some((offset, height)) =
                HitGeometry::new(&self.layout_state.geometry).overview(point)
            {
                if let Some(target) = self.overview_click_target(offset, height) {
                    self.session.focus = PaneFocus::Preview;
                    self.preview_state.navigation.pending_overview_line = Some(target);
                    self.advance_overview_jump();
                }
                return;
            }
        }
        if self.inline_mouse(mouse) {
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if over_preview && matches!(self.preview_state.preview_mode, PreviewMode::Diff) {
                    self.open_diff_context_menu(col, row);
                } else if over_list && git {
                    let rows = self.scm_rows();
                    if let Some(RowHit {
                        index: flat,
                        pinned: _,
                    }) = (ListHitGeometry {
                        area: self.layout_state.geometry.list_inner,
                        scroll: self.scm.scroll_scm,
                        sticky: &self.scm.sticky_headers,
                        len: rows.len(),
                    })
                    .row(point)
                    {
                        if let Some(ScmRow::Change { column, index, .. }) = rows.get(flat) {
                            self.open_staging_context_menu(*column, *index, col, row);
                        } else if let Some(ScmRow::Folder { .. }) = rows.get(flat) {
                            self.open_staging_folder_menu(flat, col, row);
                        } else if let Some(ScmRow::Section { .. }) = rows.get(flat) {
                            self.open_staging_section_menu(flat, col, row);
                        }
                    }
                } else if over_list {
                    if let Some(RowHit { index, pinned: _ }) = (ListHitGeometry {
                        area: self.layout_state.geometry.list_inner,
                        scroll: self.files.scroll,
                        sticky: &self.files.sticky_headers,
                        len: self.files.visible.len(),
                    })
                    .row(point)
                    {
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
                    if let Some(RowHit {
                        index: flat,
                        pinned,
                    }) = (ListHitGeometry {
                        area: self.layout_state.geometry.list_inner,
                        scroll: self.scm.scroll_scm,
                        sticky: &self.scm.sticky_headers,
                        len: rows.len(),
                    })
                    .row(point)
                    {
                        let same = flat == self.scm.scm_selected;
                        self.scm.scm_selected = flat;
                        self.scm.cancel_transfer();
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
                                    if self.selected_scm_entry().is_some_and(|entry| {
                                        entry.status == forge_git::GitStatus::Conflicted
                                    }) {
                                        self.scm_open_or_expand();
                                    } else {
                                        self.open_selected_worktree_file();
                                    }
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
                    self.navigation.cancel_reveal();
                    // Stay in Files when clicking the file tree.
                    if !matches!(self.session.activity, ActivityTab::Files) {
                        self.session.activity = ActivityTab::Files;
                        self.preview_state.preview_mode = PreviewMode::File;
                    }
                    if let Some(RowHit { index, pinned }) = (ListHitGeometry {
                        area: self.layout_state.geometry.list_inner,
                        scroll: self.files.scroll,
                        sticky: &self.files.sticky_headers,
                        len: self.files.visible.len(),
                    })
                    .row(point)
                    {
                        if mouse
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                        {
                            let old = self.files.selected;
                            self.files.selected = index;
                            self.files.follow_selection = true;
                            self.files.last_list_click = None;
                            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                self.mark_file_range(
                                    old,
                                    index,
                                    mouse.modifiers.contains(KeyModifiers::CONTROL),
                                );
                            } else {
                                if self.tools.marks.files.is_empty() && old != index {
                                    if let Some(previous) = self.files.visible.get(old) {
                                        self.tools.marks.files.insert(previous.entry.path.clone());
                                    }
                                }
                                self.tools.file_selection_anchor = self.current_path();
                                self.toggle_mark();
                            }
                            self.request_preview_for_selection();
                            return;
                        }
                        self.tools.file_selection_anchor =
                            self.files.visible.get(index).map(|r| r.entry.path.clone());
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
                    self.preview_state.presentation.selection.begin(
                        region,
                        self.preview_state.presentation.preview_scroll,
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
        let side = if let Some(snap) = self.scm.status() {
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
        // A diff opened from Files is a preview submode, not an Activity
        // switch. Keep the selected file, tree expansion and scroll intact.
        // Esc closes this submode first, then returns focus to Files.
        self.close_file_find();
        self.request_diff(rela, side);
        self.session.focus = PaneFocus::Preview;
    }

    pub(super) fn copy_preview_selection(&mut self) {
        let Some(text) = self.preview_state.presentation.selection.selected_text() else {
            self.session.status =
                "Select preview text with Shift+Arrows or mouse drag first".into();
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

    pub(super) fn scrollbar(&self, target: ScrollbarTarget) -> Option<VerticalScrollbar> {
        match target {
            ScrollbarTarget::Preview => Some(VerticalScrollbar::new(
                self.layout_state.geometry.preview_scrollbar,
                self.preview_state.presentation.preview_scroll,
                self.preview_total_lines(),
            )),
            ScrollbarTarget::Sidebar => {
                let (first, total) = if self.in_git_sidebar() {
                    (self.scm.scroll_scm, self.scm_rows().len())
                } else {
                    (self.files.scroll, self.files.visible.len())
                };
                Some(VerticalScrollbar::new(
                    self.layout_state.geometry.sidebar_scrollbar,
                    first,
                    total,
                ))
            }
            ScrollbarTarget::PreviewHorizontal => None,
        }
    }

    pub(super) fn horizontal_scrollbar(&self) -> HorizontalScrollbar {
        let area = self.layout_state.geometry.preview_horizontal_scrollbar;
        HorizontalScrollbar::new(
            area,
            self.preview_state.presentation.preview_scroll_x,
            (area.width as usize).saturating_add(self.layout_state.geometry.preview_horizontal_max),
        )
    }

    pub(super) fn seek_scrollbar(
        &mut self,
        target: ScrollbarTarget,
        column: u16,
        row: u16,
        drag: ScrollbarDrag,
    ) {
        let scroll = match target {
            ScrollbarTarget::PreviewHorizontal => {
                self.horizontal_scrollbar().scroll_at(column, drag)
            }
            _ => self
                .scrollbar(target)
                .and_then(|bar| bar.scroll_at(row, drag)),
        };
        let Some(scroll) = scroll else { return };
        match target {
            ScrollbarTarget::PreviewHorizontal => {
                self.preview_state.presentation.preview_scroll_x = scroll;
                self.session.focus = PaneFocus::Preview;
            }
            ScrollbarTarget::Preview => {
                let old = self.preview_state.presentation.preview_scroll;
                self.preview_state.presentation.preview_scroll = scroll;
                self.session.focus = PaneFocus::Preview;
                if scroll > old {
                    self.maybe_request_preview_page();
                }
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
                    self.seek_scrollbar(target, mouse.column, mouse.row, drag);
                    return true;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.seek_scrollbar(target, mouse.column, mouse.row, drag);
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
        // Reject non-scrollbar clicks using geometry before computing any
        // domain-dependent row counts or preview lengths.
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        let candidates = HitGeometry::new(&self.layout_state.geometry).scrollbar_candidates(point);
        for target in candidates {
            let drag = match target {
                ScrollbarTarget::PreviewHorizontal => self
                    .horizontal_scrollbar()
                    .begin_drag(mouse.column, mouse.row),
                _ => self
                    .scrollbar(target)
                    .and_then(|bar| bar.begin_drag(mouse.column, mouse.row)),
            };
            if let Some(drag) = drag {
                self.scrollbar_drag = Some((target, drag));
                self.seek_scrollbar(target, mouse.column, mouse.row, drag);
                return true;
            }
        }
        false
    }

    pub(super) fn scroll_preview(&mut self, delta: isize) {
        let total = self.preview_total_lines();
        let viewport = self.layout_state.geometry.preview_inner.height as usize;
        let max_scroll = total.saturating_sub(viewport);
        let next = (self.preview_state.presentation.preview_scroll as isize)
            .saturating_add(delta)
            .clamp(0, max_scroll as isize) as usize;
        self.preview_state.presentation.preview_scroll = next;
        if delta > 0 {
            self.maybe_request_preview_page();
        }
    }

    pub(super) fn move_sel(&mut self, delta: isize) {
        self.navigation.cancel_reveal();
        if self.files.move_selection(delta) {
            self.request_preview_for_selection();
        }
    }

    pub(super) fn move_scm(&mut self, delta: isize) {
        self.scm.cancel_transfer();
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
        self.files.advance_compact_folder_expansion()
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
        self.files.cancel_compact_expansion();
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
            if !self.files.has_listing(&path) {
                self.request_dir(path.clone());
            }
            if self.config.compact_folders {
                self.files.begin_compact_expansion(path.clone());
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
        self.navigation.cancel_reveal();
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

    /// Left / h / Backspace: collapse an expanded directory or select its
    /// parent within the current workspace. The workspace root never changes
    /// as a side effect of tree navigation.
    pub(super) fn tree_back(&mut self) {
        self.navigation.cancel_reveal();
        self.files.follow_selection = true;
        let Some(row) = self.files.visible.get(self.files.selected).cloned() else {
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
        }
        // depth == 0: no-op. Parent directory is outside the workspace.
    }
}

#[cfg(test)]
mod keyboard_navigation_tests {
    use super::*;
    use ratatui::layout::Rect;

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn nested_files_tree() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let folder = root.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside.txt"), "inside").unwrap();
        std::fs::write(root.join("top.txt"), "top").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.clone());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.files.install_listing(
            root.clone(),
            vec![
                Entry {
                    id: forge_model::PathId(1),
                    name: "folder".into(),
                    path: folder.clone(),
                    kind: EntryKind::Directory,
                },
                Entry {
                    id: forge_model::PathId(2),
                    name: "top.txt".into(),
                    path: root.join("top.txt"),
                    kind: EntryKind::File,
                },
            ],
        );
        app.files.install_listing(
            folder.clone(),
            vec![Entry {
                id: forge_model::PathId(3),
                name: "inside.txt".into(),
                path: folder.clone().join("inside.txt"),
                kind: EntryKind::File,
            }],
        );
        app.files.expanded.insert(folder);
        app.rebuild_visible();
        (dir, app)
    }

    #[tokio::test]
    async fn left_h_and_backspace_stop_at_workspace_root() {
        let (dir, mut app) = nested_files_tree();
        let root = dir.path().to_path_buf();
        let file = root.join("top.txt");
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == file)
            .unwrap();
        let original = app.files.selected;
        let original_rows = app.files.visible.clone();
        for code in [KeyCode::Left, KeyCode::Char('h'), KeyCode::Backspace] {
            for _ in 0..3 {
                app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
                assert_eq!(app.files.root(), root, "{code:?} changed workspace root");
                assert_eq!(
                    app.files.selected, original,
                    "{code:?} moved top-level selection"
                );
                assert_eq!(
                    app.files.visible, original_rows,
                    "{code:?} rebuilt the file tree"
                );
            }
        }
    }

    #[tokio::test]
    async fn left_collapses_folder_and_selects_parent_without_rerooting() {
        let (dir, mut app) = nested_files_tree();
        let root = dir.path().to_path_buf();
        let folder = root.join("folder");
        let inner = folder.join("inside.txt");
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == inner)
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.files.visible[app.files.selected].entry.path, folder);
        assert!(app.files.expanded.contains(&folder));

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(!app.files.expanded.contains(&folder));
        assert_eq!(app.files.visible[app.files.selected].entry.path, folder);
        assert_eq!(app.files.root(), root);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.files.root(), root);
        assert_eq!(app.files.visible[app.files.selected].entry.path, folder);
    }

    #[tokio::test]
    async fn left_on_empty_tree_and_hidden_sidebar_never_changes_root() {
        let (dir, mut app) = nested_files_tree();
        let root = dir.path().to_path_buf();
        app.files.visible.clear();
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.files.root(), root);
        assert!(app.files.visible.is_empty());

        app.session.focus = PaneFocus::Preview;
        app.layout_state.set_sidebar_visible(false);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.files.root(), root);
        assert!(app.layout_state.sidebar_visible());
        assert_eq!(app.session.focus, PaneFocus::List);
    }

    #[tokio::test]
    async fn files_diff_never_switches_activity_and_escape_unwinds_one_layer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let result = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Forge Test"]);
        git(&["config", "user.email", "esc@example.test"]);
        let path = root.join("one.txt");
        std::fs::write(
            &path,
            "original
",
        )
        .unwrap();
        git(&["add", "one.txt"]);
        git(&["commit", "-qm", "base"]);
        std::fs::write(
            &path,
            "original
changed
",
        )
        .unwrap();

        let mut app = create_workspace_at(ColorMode::Ansi16, root.to_path_buf());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.files.install_listing(
            root.to_path_buf(),
            vec![Entry {
                id: forge_model::PathId(41),
                name: "one.txt".into(),
                path: path.clone(),
                kind: EntryKind::File,
            }],
        );
        app.rebuild_visible();
        app.scm
            .set_status(Some(app.services.git.status(root).unwrap()));

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(matches!(app.preview_state.preview_mode, PreviewMode::Diff));
        assert_eq!(app.files.selected, 0);

        tokio::time::timeout(Duration::from_secs(5), async {
            while app.preview_state.pending_diff.is_some() {
                app.poll_pending_diff();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            app.preview_state.file_diff.is_some(),
            "Files diff should actually load"
        );

        app.handle_key(esc());
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(matches!(app.preview_state.preview_mode, PreviewMode::File));
        assert_eq!(app.files.root(), root);
        app.handle_key(esc());
        assert_eq!(
            app.session.focus,
            PaneFocus::Preview,
            "Esc must not change pane focus after dismissing Diff"
        );
        assert_eq!(app.session.activity, ActivityTab::Files);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.session.focus, PaneFocus::List);
        assert_eq!(app.files.visible[app.files.selected].entry.path, path);
    }

    #[tokio::test]
    async fn esc_closes_files_diff_before_returning_to_file_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.session.activity = ActivityTab::Files;
        app.session.focus = PaneFocus::Preview;
        app.layout_state.set_sidebar_visible(false);
        app.preview_state.preview_mode = PreviewMode::Diff;
        assert_eq!(
            app.workspace_escape(),
            input_router::WorkspaceEscape::CloseFileDiff
        );

        app.handle_key(esc());
        assert!(matches!(app.preview_state.preview_mode, PreviewMode::File));
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(
            !app.layout_state.sidebar_visible(),
            "first Esc only closes the diff"
        );

        app.handle_key(esc());
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(
            !app.layout_state.sidebar_visible(),
            "Esc must not expand the hidden sidebar"
        );

        // Ctrl+B explicitly restores and focuses the hidden sidebar.
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.layout_state.sidebar_visible());
        assert_eq!(app.session.focus, PaneFocus::List);
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.handle_key(esc());
        assert!(
            matches!(app.preview_state.preview_mode, PreviewMode::Diff),
            "Esc from the list must not change an unfocused preview"
        );
    }

    #[tokio::test]
    async fn esc_scopes_selection_to_active_list_and_preview_returns_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        let path = dir.path().join("selected.txt");
        app.tools.marks.files.insert(path.clone());
        app.tools.marks.git.insert(tools::GitMark {
            path: path.clone(),
            side: DiffSide::Unstaged,
        });
        // Esc in Files must not clear Git marks.
        app.handle_key(esc());
        assert!(app.tools.marks.files.is_empty());
        assert_eq!(app.tools.marks.git.len(), 1);

        // Background marks must not hijack the Git preview. Esc is not
        // pane navigation; Tab must explicitly focus the Git list first.
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.layout_state.set_sidebar_visible(true);
        app.handle_key(esc());
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert_eq!(app.tools.marks.git.len(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.session.focus, PaneFocus::List);
        app.handle_key(esc());
        assert!(app.tools.marks.git.is_empty());
        assert_eq!(app.session.activity, ActivityTab::Git);

        app.session.activity = ActivityTab::Files;
        app.session.focus = PaneFocus::List;
        app.tools.marks.git.insert(tools::GitMark {
            path,
            side: DiffSide::Staged,
        });
        assert_eq!(app.workspace_escape(), input_router::WorkspaceEscape::None);
        app.handle_key(esc());
        assert_eq!(app.tools.marks.git.len(), 1);
    }

    #[tokio::test]
    async fn passive_file_find_does_not_steal_terminal_escape() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = false;
        assert_eq!(app.key_owner(esc()), input_router::KeyOwner::Terminal);
        app.handle_key(esc());
        assert!(
            app.preview_state.find.open,
            "passive Find must not receive PTY Esc"
        );
        app.preview_state.find.input_active = true;
        assert_eq!(app.key_owner(esc()), input_router::KeyOwner::Find);
        app.handle_key(esc());
        assert!(!app.preview_state.find.open);
    }

    #[tokio::test]
    async fn preview_and_scrollbar_capture_survive_drag_into_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let preview = Rect::new(30, 2, 60, 15);
        let terminal = Rect::new(30, 20, 60, 10);
        app.terminals.active.as_mut().unwrap().area = terminal;
        app.layout_state.geometry.preview = preview;
        app.layout_state.geometry.preview_inner = preview;
        app.layout_state.geometry.preview_scrollbar = Rect::new(90, 2, 1, 15);
        let path = dir.path().join("long.txt");
        app.preview_state.preview = Some(Preview {
            path,
            content: PreviewContent::Text {
                lines: (0..200).map(|i| format!("line {i}")).collect(),
                truncated: false,
                bytes_read: 1000,
            },
        });
        let event = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        app.handle_mouse(event(down, 36, 5));
        assert_eq!(
            app.pointer_capture(),
            Some(input_router::PointerCapture::PreviewSelection)
        );
        app.handle_mouse(event(drag, 65, 25));
        app.handle_mouse(event(up, 65, 25));
        assert!(!app.preview_state.presentation.selection.is_dragging());
        assert!(!app.terminals.active.as_ref().unwrap().captures_left_drag());
        app.handle_mouse(event(down, 90, 5));
        assert_eq!(
            app.pointer_capture(),
            Some(input_router::PointerCapture::Scrollbar)
        );
        app.handle_mouse(event(drag, 60, 28));
        app.handle_mouse(event(up, 60, 28));
        assert!(app.scrollbar_drag.is_none());
        assert!(!app.terminals.active.as_ref().unwrap().captures_left_drag());
        assert!(app.preview_state.presentation.preview_scroll > 0);
        // A new press supersedes a lost release and transfers focus exactly once.
        app.handle_mouse(event(down, 36, 5));
        assert!(app.preview_state.presentation.selection.is_dragging());
        app.handle_mouse(event(down, 36, 25));
        assert!(!app.preview_state.presentation.selection.is_dragging());
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Terminal);
        app.handle_mouse(event(up, 36, 25));
        assert!(app.pointer_capture().is_none());
    }

    #[tokio::test]
    async fn terminal_splitter_resizes_without_leaking_mouse_to_other_panes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        app.layout_state.geometry.window = Rect::new(0, 0, 120, 43);
        app.layout_state.geometry.footer = Rect::new(0, 40, 120, 3);
        app.layout_state.geometry.terminal_divider = Rect::new(0, 21, 120, 1);
        app.layout_state.geometry.preview = Rect::new(35, 1, 85, 20);
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        let send = |app: &mut App, kind, column, row| {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        };
        assert_eq!(terminal_panel_height(43, None), 14);
        send(&mut app, down, 55, 21);
        assert!(app.layout_state.splitter.dragging(SplitterAxis::Horizontal));
        assert!(
            app.layout_state.terminal_height().is_none(),
            "press only captures the grip"
        );
        send(&mut app, drag, 2, 14); // over sidebar, still owns splitter
        assert_eq!(app.layout_state.terminal_height(), Some(25));
        send(&mut app, drag, 100, 0); // protect minimum workspace height
        assert_eq!(app.layout_state.terminal_height(), Some(34));
        send(&mut app, up, 55, 150); // release far outside and clamp
        assert_eq!(app.layout_state.terminal_height(), Some(5));
        assert!(!app.layout_state.splitter.dragging(SplitterAxis::Horizontal));
        assert!(!app.session.should_quit);
        assert!(!app.terminals.active.as_ref().unwrap().captures_left_drag());
        // A stray release after capture ends cannot resize the panel.
        send(&mut app, up, 35, 3);
        assert_eq!(app.layout_state.terminal_height(), Some(5));
        // Hover is visual-only, never activates the child or changes focus.
        app.layout_state.geometry.terminal_divider = Rect::new(0, 21, 120, 1);
        send(&mut app, MouseEventKind::Moved, 50, 21);
        assert!(app
            .layout_state
            .splitter
            .highlighted(SplitterAxis::Horizontal));
        send(&mut app, MouseEventKind::Moved, 50, 20);
        assert!(!app
            .layout_state
            .splitter
            .highlighted(SplitterAxis::Horizontal));
        // The preference survives closing and re-opening Terminal and clamps
        // safely when the outer terminal is resized smaller.
        assert_eq!(
            terminal_panel_height(15, app.layout_state.terminal_height()),
            5
        );
        assert_eq!(terminal_panel_height(30, Some(50)), 21);
        assert_eq!(
            terminal_panel_height(13, app.layout_state.terminal_height()),
            0
        );
    }

    #[tokio::test]
    async fn activity_bar_requires_press_and_release_on_same_icon() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        let files = Rect::new(1, 2, 3, 1);
        let git = Rect::new(1, 4, 3, 1);
        app.layout_state.geometry.files_tab = files;
        app.layout_state.geometry.git_tab = git;
        let event = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let down = MouseEventKind::Down(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        for _ in 0..10 {
            app.handle_key(esc());
        }
        // Esc cancels an unfinished press even when its delayed MouseUp
        // arrives over the same Git icon.
        app.handle_mouse(event(down, git.x + 1, git.y));
        assert!(app.session.activity_pressed.is_some());
        app.handle_key(esc());
        assert!(app.session.activity_pressed.is_none());
        app.handle_mouse(event(up, git.x + 1, git.y));
        assert!(!app.in_git_sidebar());

        // A press (or stray synthetic press) is not a click.
        app.handle_mouse(event(down, git.x + 1, git.y));
        assert!(!app.in_git_sidebar());
        app.handle_mouse(event(MouseEventKind::Moved, 15, 10));
        app.handle_mouse(event(up, 15, 10));
        assert!(!app.in_git_sidebar(), "release outside cancels click");

        // Dragging through the Git icon does not activate it.
        app.handle_mouse(event(down, files.x + 1, files.y));
        app.handle_mouse(event(
            MouseEventKind::Drag(MouseButton::Left),
            git.x + 1,
            git.y,
        ));
        app.handle_mouse(event(up, git.x + 1, git.y));
        assert!(!app.in_git_sidebar());

        // A complete deliberate click still activates Git.
        app.handle_mouse(event(down, git.x + 1, git.y));
        assert!(!app.in_git_sidebar(), "press alone cannot select Git");
        app.handle_mouse(event(up, git.x + 1, git.y));
        assert!(app.in_git_sidebar(), "matching release selects Git");
        // And Files has the same behavior.
        app.handle_mouse(event(down, files.x + 1, files.y));
        assert!(app.in_git_sidebar());
        app.handle_mouse(event(up, files.x + 1, files.y));
        assert!(!app.in_git_sidebar());
    }

    #[tokio::test]
    async fn repeated_escape_then_pointer_motion_never_changes_git_preview_focus() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.layout_state.set_sidebar_visible(true);
        app.layout_state.geometry.activity = Rect::new(0, 1, 5, 26);
        app.layout_state.geometry.list = Rect::new(5, 1, 25, 26);
        app.layout_state.geometry.list_inner = Rect::new(6, 3, 23, 23);
        app.layout_state.geometry.preview = Rect::new(31, 1, 69, 26);
        app.layout_state.geometry.preview_inner = Rect::new(33, 3, 65, 23);
        app.preview_state.preview_mode = PreviewMode::Diff;
        // Simulate the precise sequence: multiple Esc, then moving the
        // pointer over the Git tree, divider, preview and activity bar.
        for _ in 0..12 {
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        for (column, row) in [
            (15, 6),
            (15, 20),
            (30, 12),
            (65, 8),
            (2, 4),
            (40, 23),
            (98, 26),
            (6, 3),
            (9, 9),
        ] {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
            assert_eq!(
                app.session.activity,
                ActivityTab::Git,
                "Hover at ({column}, {row}) switched Activity"
            );
            assert_eq!(
                app.session.focus,
                PaneFocus::Preview,
                "Hover at ({column}, {row}) took keyboard focus"
            );
            assert!(
                app.layout_state.sidebar_visible(),
                "Hover at ({column}, {row}) changed sidebar"
            );
            assert!(matches!(app.preview_state.preview_mode, PreviewMode::Diff));
        }
        // An actual click remains the deliberate way to change focus.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.session.focus, PaneFocus::List);
    }

    #[tokio::test]
    async fn keyboard_preview_selection_copies_real_rendered_cells_without_mouse() {
        use ratatui::{backend::TestBackend, widgets::Paragraph, Terminal};
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.session.focus = PaneFocus::Preview;
        let area = Rect::new(0, 0, 16, 3);
        app.layout_state.geometry.preview_inner = area;
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("A中B"), area);
                app.preview_state.presentation.selection.observe(
                    frame,
                    area,
                    0,
                    &forge_theme::Theme::modern_ide(),
                    "test:keyboard",
                    false,
                );
            })
            .unwrap();
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        app.handle_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(
            app.preview_state
                .presentation
                .selection
                .selected_text()
                .as_deref(),
            Some("A中")
        );
        app.handle_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            app.session.status.contains("Selected 2 characters"),
            "Ctrl+C must copy the keyboard-created selection: {}",
            app.session.status
        );
        app.handle_key(esc());
        assert!(!app.preview_state.presentation.selection.active());
        assert_eq!(app.workspace_escape(), input_router::WorkspaceEscape::None);
    }

    #[tokio::test]
    async fn damaged_terminal_escape_bytes_cannot_dispatch_navigation_or_git_writes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        // ESC[...CSI and ESC O ... SS3 can leak these bytes as standalone
        // characters after a broken escape prefix. They must not navigate
        // across activities or execute high-impact Git actions.
        const FRAGMENTS: &str = "[]?<;~0123456789ABCFHIOPQSZMGNLWEDVRsup:";
        for activity in [ActivityTab::Files, ActivityTab::Git] {
            app.session.activity = activity;
            app.session.focus = PaneFocus::List;
            for ch in FRAGMENTS.chars() {
                app.handle_key(esc());
                app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                assert_eq!(
                    app.session.activity, activity,
                    "raw terminal fragment {ch:?} switched Activity"
                );
                assert!(
                    app.tools.panel.is_none(),
                    "raw terminal fragment {ch:?} opened a tool"
                );
                assert!(
                    !app.session.help_open,
                    "raw terminal fragment {ch:?} opened Help"
                );
                assert!(
                    app.scm.pending_write.is_none(),
                    "raw terminal fragment {ch:?} started a Git write"
                );
                assert!(
                    !app.session.commit_open,
                    "raw terminal fragment {ch:?} opened Commit"
                );
                assert!(!app.session.should_quit);
            }
        }

        // Physical, deliberate chords still work with the same canonical
        // workspace actions. No plain-letter compatibility aliases remain.
        app.session.activity = ActivityTab::Files;
        app.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
        assert!(app.session.help_open);
        app.handle_key(esc());
        assert!(!app.session.help_open);
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(app.session.activity, ActivityTab::Git);
        app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!(app.session.activity, ActivityTab::Files);
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(app
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, tools::PanelKind::Commands)));
    }

    #[tokio::test]
    async fn escape_followed_by_brackets_and_mouse_motion_cannot_open_git_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.layout_state.geometry.list = Rect::new(5, 2, 27, 28);
        app.layout_state.geometry.preview = Rect::new(33, 2, 60, 28);
        app.session.focus = PaneFocus::List;

        // Reproduce both observed incidents: Esc followed ~8 ms later by
        // what crossterm classified as a Git-column character. The pointer
        // motion that follows should never make Files become Git.
        for bracket in ['[', ']'] {
            app.handle_key(esc());
            app.handle_key(KeyEvent::new(KeyCode::Char(bracket), KeyModifiers::NONE));
            for (col, row) in [(18, 6), (18, 14), (18, 24)] {
                app.handle_mouse(MouseEvent {
                    kind: MouseEventKind::Moved,
                    column: col,
                    row,
                    modifiers: KeyModifiers::NONE,
                });
                assert_eq!(
                    app.session.activity,
                    ActivityTab::Files,
                    "Esc followed by '{bracket}' and pointer move entered Git"
                );
                assert_eq!(app.session.focus, PaneFocus::List);
            }
        }
        // Intentional Git navigation stays available through F2, and once
        // in Git the bracket shortcuts still select the respective columns.
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(app.session.activity, ActivityTab::Git);
        for bracket in ['[', ']'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(bracket), KeyModifiers::NONE));
            assert_eq!(app.session.activity, ActivityTab::Git);
            assert_eq!(app.session.focus, PaneFocus::List);
        }
    }

    #[tokio::test]
    async fn repeated_escape_then_pointer_motion_never_switches_files_to_git() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
        app.layout_state.geometry.list = Rect::new(5, 2, 25, 27);
        app.layout_state.geometry.preview = Rect::new(31, 2, 70, 27);
        app.layout_state.geometry.preview_inner = Rect::new(33, 3, 65, 24);
        app.session.focus = PaneFocus::Preview;
        for _ in 0..12 {
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        for (column, row) in [(3, 4), (15, 5), (40, 6), (90, 25)] {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
            assert_eq!(app.session.activity, ActivityTab::Files);
            assert_eq!(app.session.focus, PaneFocus::Preview);
        }
    }

    #[tokio::test]
    async fn esc_from_git_root_does_not_switch_activity_or_reselect_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        app.layout_state.set_sidebar_visible(true);
        app.handle_key(esc());
        assert!(app.in_git_sidebar());
        assert_eq!(app.session.focus, PaneFocus::List);

        app.session.focus = PaneFocus::Preview;
        app.handle_key(esc());
        assert!(app.in_git_sidebar());
        assert_eq!(
            app.session.focus,
            PaneFocus::Preview,
            "Esc from Git Diff must not jump to the Git list"
        );
        app.handle_key(esc());
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(
            app.in_git_sidebar(),
            "root Esc must not silently switch to Files"
        );
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.session.focus,
            PaneFocus::List,
            "Tab is the explicit way to focus Git list"
        );
    }

    #[tokio::test]
    async fn modal_keeps_terminal_host_chords_from_stealing_the_keyboard() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let term = app
            .terminals
            .active
            .as_ref()
            .expect("terminal should open")
            .id;
        assert!(app.terminals.active.as_ref().unwrap().is_focused());
        app.open_tools(tools::PanelKind::Settings);

        app.handle_key(KeyEvent::new(
            KeyCode::Char('W'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, term);
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        assert!(
            app.terminals.active.as_ref().unwrap().is_visible(),
            "dialog owns F8"
        );
        assert!(
            app.tools.panel.is_some(),
            "host chords must not dismiss the dialog"
        );

        app.handle_key(esc());
        assert!(app.tools.panel.is_none());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        assert!(
            !app.terminals.active.as_ref().unwrap().is_visible(),
            "host shortcut works after modal closes"
        );
    }

    #[tokio::test]
    async fn command_palette_alias_overrides_terminal_only_for_registered_chord() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        assert!(app.terminals.active.as_ref().unwrap().is_focused());
        for letter in ['p', 'P'] {
            app.handle_key(KeyEvent::new(
                KeyCode::Char(letter),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ));
            assert!(matches!(
                app.tools.panel.as_ref().map(|p| &p.kind),
                Some(tools::PanelKind::Commands)
            ));
            app.handle_key(esc());
            assert!(app.tools.panel.is_none());
            assert!(app.terminals.active.as_ref().unwrap().is_focused());
        }
    }

    #[tokio::test]
    async fn ctrl_modified_letters_cannot_quit_or_discard_workspace() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        let initial = app.session.activity;
        for ch in ['q', 'x', 'd', 's', 'w'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL));
            assert!(!app.session.should_quit, "Ctrl+{ch} must not exit");
            assert!(
                app.tools.panel.is_none(),
                "Ctrl+{ch} must not open a prompt"
            );
            assert!(!app.session.commit_open, "Ctrl+{ch} must not open Commit");
            assert_eq!(app.session.activity, initial);
        }
    }

    #[tokio::test]
    async fn f4_shortcuts_overlay_restores_file_find_after_esc() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = true;
        app.handle_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
        assert!(app.keys.open);
        assert!(app.preview_state.find.open);
        app.handle_key(esc());
        assert!(!app.keys.open);
        assert!(app.preview_state.find.open);
        assert!(app.preview_state.find.input_active);
        app.handle_key(esc());
        assert!(!app.preview_state.find.open);
    }

    #[tokio::test]
    async fn git_parent_navigation_never_switches_to_files() {
        let root = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, root.path().to_path_buf());
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
    }

    #[tokio::test]
    async fn find_terminal_pointer_handoff_has_one_typing_owner() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let id = app.terminals.active.as_ref().unwrap().id;
        app.terminals.active.as_mut().unwrap().area = Rect::new(0, 24, 100, 10);
        app.layout_state.geometry.file_find_rect = Rect::new(40, 2, 35, 4);
        app.preview_state.find.open = true;
        app.focus_find_input();
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Find);
        assert!(!app.terminals.active.as_ref().unwrap().is_focused());
        assert!(app.handle_paste("needle"));

        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(click(10, 26));
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Terminal);
        assert!(app.preview_state.find.open);
        assert!(!app.preview_state.find.input_active);
        assert!(app.handle_paste("terminal-only"));
        assert_eq!(app.preview_state.find.query, "needle");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click(10, 26)
        });

        app.handle_mouse(click(45, 3));
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Find);
        assert!(!app.terminals.active.as_ref().unwrap().is_focused());
        assert!(app.handle_paste("-again"));
        assert_eq!(app.preview_state.find.query, "needle-again");
        assert_eq!(app.terminals.active.as_ref().unwrap().id, id);
        app.activity_action(ActivityAction::Terminal);
        assert!(!app.terminals.active.as_ref().unwrap().is_visible());
        app.activity_action(ActivityAction::Terminal);
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Terminal);
        assert!(!app.preview_state.find.input_active);
        assert_eq!(app.terminals.active.as_ref().unwrap().id, id);
    }

    #[tokio::test]
    async fn modal_mouse_cannot_focus_find_behind_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = false;
        app.layout_state.geometry.file_find_rect = Rect::new(40, 2, 35, 4);
        app.session.commit_open = true;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 45,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.typing_owner(), input_router::KeyOwner::Commit);
        assert!(!app.preview_state.find.input_active);
        assert!(!app.handle_paste("ignored by workspace"));
        assert!(app.preview_state.find.query.is_empty());
    }

    #[tokio::test]
    async fn file_find_takes_esc_before_focused_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = true;
        app.handle_key(esc());
        assert!(!app.preview_state.find.open);
        assert!(app.terminals.active.as_ref().unwrap().is_visible());
    }
}

//! Tool panel key and row actions; does not own Git or filesystem mutations.
use super::*;

impl App {
    pub(crate) fn handle_tools_key(&mut self, key: KeyEvent) {
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        if key.code == KeyCode::Esc {
            if matches!(panel.kind, PanelKind::Confirm(_)) {
                // A cancelled confirmation must never retain a deferred
                // "after operation" redirect for some later action.
                self.tools.after = None;
            }
            self.back_tools();
            return;
        }
        if key.code == KeyCode::F(4) && key.modifiers.is_empty() {
            self.keys.open = true;
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.close_tools();
            return;
        }
        // Ctrl+R is Refresh in Forge; Alt+R toggles regex locally in search.
        if key.code == KeyCode::Char('r')
            && key.modifiers == KeyModifiers::ALT
            && matches!(panel.kind, PanelKind::Content)
        {
            panel.regex = !panel.regex;
            panel.title = format!(
                "Search contents · {}",
                if panel.regex { "regex" } else { "literal" }
            );
            self.reload_tools();
            return;
        }
        let is_refresh = !matches!(panel.kind, PanelKind::Prompt(_) | PanelKind::Confirm(_))
            && self.keys.remap(key, false).is_some_and(|remapped| {
                matches!(remapped.code, KeyCode::Char('r' | 'R'))
                    && remapped.modifiers == KeyModifiers::CONTROL
            });
        if is_refresh {
            self.request_list();
            self.request_status();
            self.reload_tools();
            return;
        }
        let input_mode = panel.is_input();
        if matches!(key.code, KeyCode::Up | KeyCode::Down)
            || (!input_mode && matches!(key.code, KeyCode::Char('j') | KeyCode::Char('k')))
        {
            let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
                -1
            } else {
                1
            };
            let previous = panel.selected;
            panel.select_row(panel.selected.saturating_add_signed(delta));
            if matches!(panel.kind, PanelKind::Content) && panel.selected != previous {
                panel.search_selection_touched = true;
            }
            self.request_search_context();
            return;
        }
        if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
            let delta: isize = if key.code == KeyCode::PageDown { 1 } else { -1 };
            if key.modifiers.contains(KeyModifiers::CONTROL) || panel.rows.is_empty() {
                if delta > 0 {
                    panel.scroll = panel.scroll.saturating_add(10).min(panel.scroll_limit);
                } else {
                    panel.scroll = panel.scroll.saturating_sub(10);
                }
            } else {
                let previous = panel.selected;
                panel.select_row(panel.selected.saturating_add_signed(
                    delta * panel.page_size().saturating_sub(1).max(1) as isize,
                ));
                if matches!(panel.kind, PanelKind::Content) && panel.selected != previous {
                    panel.search_selection_touched = true;
                }
                self.request_search_context();
            }
            return;
        }
        if !input_mode && matches!(key.code, KeyCode::Home | KeyCode::End) {
            let next = if key.code == KeyCode::Home {
                0
            } else {
                panel.rows.len().saturating_sub(1)
            };
            let previous = panel.selected;
            panel.select_row(next);
            if matches!(panel.kind, PanelKind::Content) && panel.selected != previous {
                panel.search_selection_touched = true;
            }
            self.request_search_context();
            return;
        }
        let kind = panel.kind.clone();
        let selected = panel.rows.get(panel.selected).map(|r| r.value.clone());
        if key.code == KeyCode::Char(' ') && matches!(kind, PanelKind::Settings) {
            if let Some(value) = selected {
                self.activate_tool_row(kind, value);
            }
            return;
        }
        if key.code == KeyCode::Char('O') && matches!(kind, PanelKind::Workspaces) {
            self.prompt(
                PromptAction::OpenWorkspace,
                "Open workspace directory",
                String::new(),
                "Absolute path, ~/path, or relative to current workspace · Enter open",
            );
            return;
        }
        if key.code == KeyCode::Enter {
            match kind {
                PanelKind::Prompt(action) => {
                    let value = panel.input.value().to_owned();
                    self.submit_prompt(action, value);
                }
                PanelKind::Confirm(operation) => {
                    let after = self.tools.after.take();
                    self.start_tool_operation(operation, after);
                }
                _ => {
                    if let Some(value) = selected {
                        self.activate_tool_row(kind, value);
                    }
                }
            }
            return;
        }
        if input_mode {
            let old = panel.input.value().to_owned();
            panel.input.handle_event(&crossterm::event::Event::Key(key));
            if panel.input.value() != old && !matches!(kind, PanelKind::Prompt(_)) {
                self.reload_tools();
            }
            return;
        }
        // Do not reinterpret an unhandled Ctrl/Alt chord as a plain letter
        // action (e.g. Ctrl+Shift+W must not trigger a Worktree action).
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        self.tool_action_key(kind, selected, key.code);
    }

    pub(crate) fn activate_tool_row(&mut self, kind: PanelKind, value: RowValue) {
        match value {
            RowValue::Command(c) => self.run_command(c),
            RowValue::File(path, line) => {
                self.close_tools();
                self.reveal_file(path, line);
            }
            RowValue::Line(_) => {}
            RowValue::Workspace(path) => {
                if self.write_busy() {
                    if let Some(panel) = &mut self.tools.panel {
                        panel.message = "Wait for the current write to finish".into();
                    }
                } else if !path.is_dir() {
                    if let Some(panel) = &mut self.tools.panel {
                        panel.message =
                            "Directory no longer exists · press O for another path".into();
                    }
                } else {
                    self.close_tools();
                    self.switch_workspace(path);
                }
            }
            RowValue::Git(item) => match kind {
                PanelKind::Git(GitQuery::History(_) | GitQuery::HistoryPage { .. }) => {
                    self.open_history_child(PanelKind::Git(GitQuery::Commit(item.id)))
                }
                PanelKind::Git(GitQuery::Commit(oid)) => {
                    if let Some(path) = item.path {
                        self.open_history_child(PanelKind::Git(GitQuery::CommitFile { oid, path }));
                    }
                }
                PanelKind::Git(GitQuery::PublishRemotes(branch)) => {
                    let remote = item.id.clone();
                    self.confirm(
                        Operation::Git(GitAction::PublishBranch {
                            remote: remote.clone(),
                            branch: branch.clone(),
                        }),
                        format!(
                            "Publish local branch {branch} to {remote}/{branch}?\n\nRuns git push --set-upstream {remote} {branch}.\nSets {remote}/{branch} as the tracking branch.\nNo force-push. Remote changes may reject the push."
                        ),
                        Some(PanelKind::Git(GitQuery::Sync)),
                    );
                }
                PanelKind::Git(GitQuery::Branches) => {
                    if item.id == self.scm.branch {
                        if let Some(panel) = &mut self.tools.panel {
                            panel.message = format!("Already on branch {}", item.id);
                        }
                    } else {
                        self.confirm(
                            Operation::Git(GitAction::SwitchBranch(item.id.clone())),
                            format!("Switch this repository to branch {}?", item.id),
                            Some(PanelKind::Git(GitQuery::Branches)),
                        );
                    }
                }
                PanelKind::Git(GitQuery::Stashes) => {
                    // Keep the selected stash list in the navigation stack.
                    // Enter views the patch; Esc returns to this list, not
                    // directly to the Files/Git workspace.
                    self.open_history_child(PanelKind::Git(GitQuery::StashDiff(item.id)))
                }
                PanelKind::Git(GitQuery::Conflicts) => {
                    if let Some(path) = item.path {
                        if let Some(root) = self.repo_root() {
                            self.close_tools();
                            self.open_conflict_editor(root.join(path));
                        }
                    }
                }
                PanelKind::Git(GitQuery::Worktrees) => {
                    self.close_tools();
                    self.switch_workspace(PathBuf::from(item.id));
                }
                _ => {}
            },
        }
    }

    pub(crate) fn tool_action_key(
        &mut self,
        kind: PanelKind,
        selected: Option<RowValue>,
        key: KeyCode,
    ) {
        if key == KeyCode::Char(':') {
            self.open_history_child(PanelKind::Commands);
            return;
        }
        if matches!(key, KeyCode::Char('[') | KeyCode::Char(']')) {
            let history = match &kind {
                PanelKind::Git(GitQuery::History(path)) => Some((path.clone(), 0)),
                PanelKind::Git(GitQuery::HistoryPage { path, skip }) => Some((path.clone(), *skip)),
                _ => None,
            };
            if let Some((path, skip)) = history {
                if self.tools.pending_read.is_some() {
                    return;
                }
                if key == KeyCode::Char(']')
                    && !self
                        .tools
                        .panel
                        .as_ref()
                        .is_some_and(|p| p.history_has_older)
                {
                    return;
                }
                if key == KeyCode::Char('[') && skip == 0 {
                    return;
                }
                self.open_tools(PanelKind::Git(GitQuery::HistoryPage {
                    path,
                    skip: if key == KeyCode::Char(']') {
                        skip + 200
                    } else {
                        skip.saturating_sub(200)
                    },
                }));
                return;
            }
        }
        let kind = match kind {
            PanelKind::Git(GitQuery::HistoryPage { path, .. }) => {
                PanelKind::Git(GitQuery::History(path))
            }
            other => other,
        };
        match (&kind, key) {
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Branch,
                    "Create branch",
                    String::new(),
                    "Creates a branch at HEAD; switch using Enter",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Stash,
                    "Save stash",
                    String::new(),
                    "Stash message · includes untracked files · confirmation follows",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Worktrees), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Worktree,
                    "Create worktree",
                    String::new(),
                    "Path · Git creates a branch named after the directory",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('F')) => {
                self.run_command(Command::Fetch);
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('p')) => {
                self.close_tools();
                self.request_pull();
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('P')) => {
                let status = self.tools.sync.lines().next().unwrap_or_default().trim();
                if status.starts_with("## No commits yet")
                    || status.starts_with("## Initial commit")
                {
                    if let Some(panel) = &mut self.tools.panel {
                        panel.message =
                            "Create the first commit before publishing this branch".into();
                    }
                    return;
                }
                if self.tools.pending_read.is_some() {
                    if let Some(panel) = &mut self.tools.panel {
                        panel.message =
                            "Checking branch tracking status; retry after loading".into();
                    }
                    return;
                }
                if let Some(branch) = self.unpublished_branch() {
                    self.open_history_child(PanelKind::Git(GitQuery::PublishRemotes(branch)));
                } else {
                    self.close_tools();
                    self.request_push();
                }
                return;
            }
            _ => {}
        }
        let Some(selected) = selected else {
            return;
        };
        if let (PanelKind::Lines(diff), RowValue::Line(index)) = (&kind, &selected) {
            let stage = diff.side == DiffSide::Unstaged;
            if key == KeyCode::Char(if stage { 's' } else { 'u' }) {
                self.start_tool_operation(
                    Operation::Line {
                        diff: diff.clone(),
                        index: *index,
                    },
                    None,
                );
            }
            return;
        }
        let RowValue::Git(item) = selected else {
            return;
        };
        let after = Some(kind.clone());
        match (kind, key) {
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char(' ')) => {
                self.tools.pinned = Some(item.clone());
                if let Some(p) = &mut self.tools.panel {
                    p.message = format!("Pinned {} · select second version, then D", item.label);
                }
            }
            (PanelKind::Git(GitQuery::History(path)), KeyCode::Char('D')) => {
                if let Some(old) = self.tools.pinned.clone() {
                    self.open_history_child(PanelKind::Git(GitQuery::Compare {
                        old: old.id,
                        new: item.id,
                        old_path: if path.is_some() { old.path } else { None },
                        new_path: if path.is_some() { item.path } else { None },
                    }));
                } else if let Some(p) = &mut self.tools.panel {
                    p.message = "Pin the first version with Space".into();
                }
            }
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char('C')) => self.confirm(
                Operation::Git(GitAction::CherryPick(item.id.clone())),
                format!(
                    "Cherry-pick {} into this branch? Conflicts may require resolution.",
                    item.label
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char('R')) => self.confirm(
                Operation::Git(GitAction::Revert(item.id.clone())),
                format!("Create a commit reverting {}?", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('r')) => self.prompt(
                PromptAction::RenameBranch(item.id.clone()),
                "Rename branch",
                item.id,
                "New branch name",
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('X')) => self.confirm(
                Operation::Git(GitAction::DeleteBranch(item.id.clone())),
                format!(
                    "Delete branch {}? Git refuses unmerged or checked-out branches.",
                    item.id
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('D')) => {
                self.open_history_child(PanelKind::Git(GitQuery::BranchDiff(item.id)))
            }
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('a')) => self.confirm(
                Operation::Git(GitAction::ApplyStash(item.id.clone())),
                format!("Apply {}? The stash is kept.", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('I')) => self.confirm(
                Operation::Git(GitAction::ApplyStashIndexed(item.id.clone())),
                format!("Apply {} and restore the original staged/index state? Stash is kept. Git may reject index conflicts.", item.label),
                after.clone(),
            ),
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('p')) => self.confirm(
                Operation::Git(GitAction::PopStash(item.id.clone())),
                format!("Pop {}? Applies changes and permanently removes this stash on success. On conflicts the stash is retained.", item.label),
                after.clone(),
            ),
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('X')) => self.confirm(
                Operation::Git(GitAction::DropStash(item.id.clone())),
                format!("Permanently drop {}?", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('o') | KeyCode::Char('t')) => self
                .confirm(
                    Operation::Git(GitAction::Resolve {
                        paths: vec![item.id.clone()],
                        ours: key == KeyCode::Char('o'),
                    }),
                    format!(
                        "Replace {} with {} and stage the resolution?",
                        item.id,
                        if key == KeyCode::Char('o') {
                            "ours"
                        } else {
                            "theirs"
                        }
                    ),
                    after,
                ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('s')) => self.confirm(
                Operation::Git(GitAction::MarkResolved(vec![item.id.clone()])),
                format!(
                    "Stage {} as resolved? Verify manual edits and remove conflict markers first.",
                    item.id
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('e')) => {
                if let Some(root) = self.repo_root() {
                    self.close_tools();
                    self.open_conflict_editor(root.join(item.id));
                }
            }
            (PanelKind::Git(GitQuery::Worktrees), KeyCode::Char('X')) => {
                let path = PathBuf::from(&item.id);
                if self.files.root().starts_with(&path) {
                    if let Some(p) = &mut self.tools.panel {
                        p.message = "Switch away before removing the current worktree".into();
                    }
                } else {
                    self.confirm(
                        Operation::Git(GitAction::RemoveWorktree(path)),
                        format!(
                            "Remove worktree {}? Git refuses dirty or locked worktrees.",
                            item.id
                        ),
                        after,
                    );
                }
            }
            _ => {}
        }
    }
}

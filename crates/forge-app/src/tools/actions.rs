//! User-confirmed file and Git actions; keep writes separate from tool panel navigation.
use super::*;

impl App {
    pub(crate) fn run_command(&mut self, command: Command) {
        use Command::*;
        let paths = self.operation_paths();
        let current = self.current_path();
        let relative = |p: &Path| {
            p.strip_prefix(&self.files.tree_root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        };
        let parent = current
            .as_ref()
            .map(|p| {
                if p.is_dir() {
                    p.clone()
                } else {
                    p.parent().unwrap_or(p).to_owned()
                }
            })
            .unwrap_or_else(|| self.files.tree_root.clone());
        let prefix = if parent == self.files.tree_root {
            String::new()
        } else {
            format!("{}/", relative(&parent))
        };
        match command {
            ToggleCodeWrap => {
                let mut next = self.config.clone();
                next.wrap_code = !next.wrap_code;
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        self.preview_state.text_rows.clear();
                        self.preview_state.preview_scroll_x = 0;
                        self.preview_state.preview_scroll = 0;
                        self.reset_blame();
                        self.reset_inline_diff();
                        self.session.status = format!("Code auto wrap: {} · saved", if self.config.wrap_code { "On" } else { "Off" });
                        let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                        self.reload_tools();
                        if let Some(p) = &mut self.tools.panel { p.selected = selected.min(p.rows.len().saturating_sub(1)); }
                    }
                    Err(err) => {
                        self.session.status = format!("Code auto wrap not saved: {err}");
                        if let Some(p) = &mut self.tools.panel { p.message = self.session.status.clone(); }
                    }
                }
            }
            ToggleOverviewRuler => {
                let mut next = self.config.clone();
                next.show_overview_ruler = !next.show_overview_ruler;
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        // A changed preview width is measured on the next draw.
                        self.preview_state.text_rows.clear();
                        self.blame.cursor = None;
                        self.session.status = format!("Focus Overview: {} · saved",
                            if self.config.show_overview_ruler { "On" } else { "Off" });
                        let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                        self.reload_tools();
                        if let Some(p) = &mut self.tools.panel {
                            p.selected = selected.min(p.rows.len().saturating_sub(1));
                        }
                    }
                    Err(err) => {
                        self.session.status = format!("Focus Overview not saved: {err}");
                        if let Some(p) = &mut self.tools.panel { p.message = self.session.status.clone(); }
                    }
                }
            }
            ToggleStagedGutter | ToggleUnstagedGutter => {
                let mut next = self.config.clone();
                let staged = matches!(command, ToggleStagedGutter);
                if staged { next.git.show_staged_gutter = !next.git.show_staged_gutter; }
                else { next.git.show_unstaged_gutter = !next.git.show_unstaged_gutter; }
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        self.close_inline_peek();
                        self.session.status = format!("Git {} gutter · saved", if staged { "staged" } else { "unstaged" });
                        let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                        self.reload_tools();
                        if let Some(p) = &mut self.tools.panel { p.selected = selected.min(p.rows.len().saturating_sub(1)); }
                    }
                    Err(err) => {
                        self.session.status = format!("Git gutter settings not saved: {err}");
                        if let Some(p) = &mut self.tools.panel { p.message = self.session.status.clone(); }
                    }
                }
            }
            CycleTheme => {
                let mut next = self.config.clone();
                let theme = match next.theme.as_deref().unwrap_or("current") {
                    "current" => "vscode-dark",
                    "vscode-dark" => "vscode-black",
                    "vscode-black" => "github-dark",
                    "github-dark" => "nord",
                    _ => "current",
                };
                next.theme = Some(theme.to_owned());
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        self.preview_state.text_rows.clear();
                        self.session.status = format!("Theme: {theme} · saved");
                        let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                        self.reload_tools();
                        if let Some(panel) = &mut self.tools.panel {
                            panel.selected = selected.min(panel.rows.len().saturating_sub(1));
                        }
                    }
                    Err(err) => {
                        self.session.status = format!("Theme not saved: {err}");
                        if let Some(panel) = &mut self.tools.panel {
                            panel.message = self.session.status.clone();
                        }
                    }
                }
            }
            Settings => self.open_tools(PanelKind::Settings),
            ShortcutSettings => {
                self.close_tools();
                self.keys.open = true;
            }
            CompactFolders => {
                let mut next = self.config.clone();
                next.compact_folders = !next.compact_folders;
                let in_settings = self.tools.panel.as_ref()
                    .is_some_and(|p| matches!(p.kind, PanelKind::Settings));
                match next.save() {
                    Ok(()) => {
                        let current = self.selected_scm_row();
                        self.config = next;
                        self.files.compact_expand_pending = None;
                        self.rebuild_visible_preserving_selection();
                        let rows = self.scm_rows();
                        let target = current.as_ref().and_then(|old| {
                            rows.iter().position(|row| match (old, row) {
                                (ScmRow::Folder { column: a, path: ap, .. },
                                 ScmRow::Folder { column: b, path: bp, .. }) =>
                                    a == b && (ap == bp || bp.starts_with(&format!("{ap}/"))),
                                (ScmRow::CommitFolder { commit: a, path: ap, .. },
                                 ScmRow::CommitFolder { commit: b, path: bp, .. }) =>
                                    a == b && (ap == bp || bp.starts_with(&format!("{ap}/"))),
                                (ScmRow::Change { column: a, index: ai, .. },
                                 ScmRow::Change { column: b, index: bi, .. }) =>
                                    a == b && ai == bi,
                                (ScmRow::CommitFile { commit: a, index: ai, .. },
                                 ScmRow::CommitFile { commit: b, index: bi, .. }) =>
                                    a == b && ai == bi,
                                (ScmRow::Commit { index: a }, ScmRow::Commit { index: b }) => a == b,
                                (ScmRow::Section { column: a, .. },
                                 ScmRow::Section { column: b, .. }) => a == b,
                                (ScmRow::HistorySection { .. }, ScmRow::HistorySection { .. }) => true,
                                _ => false,
                            })
                        });
                        self.scm.scm_selected = target
                            .unwrap_or(self.scm.scm_selected.min(rows.len().saturating_sub(1)));
                        self.files.sticky_headers.clear();
                        self.scm.sticky_headers.clear();
                        self.session.status = format!("Compact Folders: {} · saved",
                            if self.config.compact_folders { "On" } else { "Off" });
                        if in_settings {
                            let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                            self.reload_tools();
                            if let Some(p) = &mut self.tools.panel {
                                p.selected = selected.min(p.rows.len().saturating_sub(1));
                            }
                        } else { self.close_tools(); }
                    }
                    Err(error) => {
                        self.session.status = format!("Compact Folders not saved: {error}");
                        if let Some(panel) = &mut self.tools.panel {
                            panel.message = self.session.status.clone();
                        }
                    }
                }
            }
            TreeStickyScroll => {
                let mut next = self.config.clone();
                next.tree_sticky_scroll = !next.tree_sticky_scroll;
                let was_settings = self.tools.panel.as_ref()
                    .is_some_and(|p| matches!(p.kind, PanelKind::Settings));
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        self.files.sticky_headers.clear();
                        self.scm.sticky_headers.clear();
                        self.session.status = format!("Tree Sticky Scroll: {} · saved",
                            if self.config.tree_sticky_scroll { "On" } else { "Off" });
                        if was_settings {
                            let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                            self.reload_tools();
                            if let Some(p) = &mut self.tools.panel {
                                p.selected = selected.min(p.rows.len().saturating_sub(1));
                            }
                        } else { self.close_tools(); }
                    }
                    Err(error) => {
                        self.session.status = format!("Tree Sticky Scroll not saved: {error}");
                        if let Some(panel) = &mut self.tools.panel {
                            panel.message = self.session.status.clone();
                        }
                    }
                }
            }
            FindFile => self.open_tools(PanelKind::Files), FindContent => self.open_tools(PanelKind::Content),
            OpenFile => { self.close_tools(); self.open_selected_worktree_file(); },
            NewFile => self.prompt(PromptAction::NewFile,"New file",prefix,"Path relative to workspace · existing files are never overwritten"),
            NewDirectory => self.prompt(PromptAction::NewDirectory,"New directory",prefix,"Path relative to workspace · parent must exist"),
            Rename => if let Some(path) = current { let value = relative(&path); self.prompt(PromptAction::Rename(path),"Rename",value,"New path relative to workspace · no overwrite"); },
            Copy | Move if !paths.is_empty() => {
                let n = paths.len(); let action = if matches!(command,Copy) { PromptAction::Copy(paths) } else { PromptAction::Move(paths) };
                self.prompt(action, if matches!(command,Copy) { "Copy items" } else { "Move items" },".".into(),&format!("Destination directory for {n} selected items · no overwrite"));
            }
            Delete if !paths.is_empty() => {
                let detail = format!("Permanently delete {} items and directory contents?\n{}",paths.len(),paths.iter().map(|p| relative(p)).collect::<Vec<_>>().join("\n"));
                self.confirm(Operation::Files(FileOperation::Delete(paths)),detail,None);
            }
            Mark => { self.close_tools(); self.toggle_mark(); }, MarkAll => { self.close_tools(); self.mark_all(); }, ClearMarks => { self.tools.marks.clear(); self.close_tools(); },
            Stage | Unstage => { self.close_tools(); self.batch_stage(matches!(command,Stage)); },
            Lines if self.in_git_sidebar() => {
                if !self.preview_state.preview_loading && self.preview_state.pending_diff.is_none() {
                    if let Some(diff) = self.preview_state.file_diff.clone().filter(|d| self.diff_matches_selection(d)) { self.open_tools(PanelKind::Lines(diff)); }
                    else { self.session.status = "Open a current diff first with d".into(); }
                }
            }
            FileHistory => if let (Some(path),Some(root)) = (current,self.repo_root()) {
                self.tools.pinned = None;
                if !path.is_dir() { if let Ok(path) = path.strip_prefix(root) { self.open_tools(PanelKind::Git(GitQuery::History(Some(forge_model::path_to_rela(path))))); } }
            },
            History => { self.tools.pinned = None; self.open_tools(PanelKind::Git(GitQuery::History(None))); },
            Graph => {
                self.close_tools();
                self.show_history();
            }
            Editor => { let target = self.editor_target(); self.close_tools(); self.open_editor(target,self.preview_source_line()); },
            EditText => {let target = self.editor_target();self.close_tools();if let Some(path)=target{self.open_text_editor(path,self.preview_source_line());}},
            Filters => self.open_tools(PanelKind::Filters),
            Hidden => {
                let mut next = self.config.clone();
                next.show_hidden_files = !next.show_hidden_files;
                let was_settings = self.tools.panel.as_ref()
                    .is_some_and(|p| matches!(p.kind, PanelKind::Settings));
                match next.save() {
                    Ok(()) => {
                        self.config = next;
                        self.tools.filter.show_hidden = self.config.show_hidden_files;
                        self.files.revealed_path = None;
                        self.rebuild_visible_preserving_selection();
                        self.request_missing_expanded();
                        self.request_list();
                        self.session.status = format!("Show hidden files: {} · saved",
                            if self.config.show_hidden_files { "On" } else { "Off" });
                        if was_settings {
                            let selected = self.tools.panel.as_ref().map_or(0, |p| p.selected);
                            self.reload_tools();
                            if let Some(p) = &mut self.tools.panel {
                                p.selected = selected.min(p.rows.len().saturating_sub(1));
                            }
                        } else { self.close_tools(); }
                    }
                    Err(error) => {
                        self.session.status = format!("Show hidden files not saved: {error}");
                        if let Some(panel) = &mut self.tools.panel {
                            panel.message = self.session.status.clone();
                        }
                    }
                }
            }
            All | Modified | Untracked => {
                // Explicit filter changes supersede any one-off Reveal override.
                self.files.revealed_path = None;
                match command { All => { self.tools.filter.status = 0; self.tools.filter.extension.clear(); }, Modified => self.tools.filter.status = 1, Untracked => self.tools.filter.status = 2, _=>{} }
                self.rebuild_visible_preserving_selection(); self.close_tools(); self.request_preview_for_selection();
            }
            Extension => self.prompt(PromptAction::Extension,"Filter extension",self.tools.filter.extension.clone(),"Extension without wildcard, e.g. rs or ts · empty clears"),
            Exclusions => self.prompt(PromptAction::Exclusions,"Excluded directory names",if self.tools.filter.excluded.is_empty() { "target,node_modules".into() } else { self.tools.filter.excluded.clone() },"Comma-separated names · .git is always hidden · use a comma alone to clear defaults"),
            Discard => self.discard_selection(),
            Branches => self.open_tools(PanelKind::Git(GitQuery::Branches)), Stashes => self.open_tools(PanelKind::Git(GitQuery::Stashes)), Conflicts => self.open_tools(PanelKind::Git(GitQuery::Conflicts)),
            Amend => self.prompt(PromptAction::Amend,"Amend latest commit",String::new(),"New commit message · includes ALL staged changes · rewrites HEAD · confirmation follows"),
            Sync => self.open_tools(PanelKind::Git(GitQuery::Sync)), Fetch => self.start_tool_operation(Operation::Git(GitAction::Fetch),Some(PanelKind::Git(GitQuery::Sync))),
            Worktrees => self.open_tools(PanelKind::Git(GitQuery::Worktrees)),
            Workspaces => self.open_tools(PanelKind::Workspaces),
            AbortMerge | AbortCherryPick | AbortRevert | ContinueCherryPick | ContinueRevert => {
                let action = match command { AbortMerge=>GitAction::AbortMerge, AbortCherryPick=>GitAction::AbortCherryPick, AbortRevert=>GitAction::AbortRevert, ContinueCherryPick=>GitAction::ContinueCherryPick, _=>GitAction::ContinueRevert };
                self.confirm(Operation::Git(action),"Apply the selected recovery command to this repository? Abort restores the pre-operation state.".into(),Some(PanelKind::Git(GitQuery::Conflicts)));
            }
            _ => { self.session.status = "Select files or directories first".into(); }
        }
    }

    pub(crate) fn submit_prompt(&mut self, action: PromptAction, value: String) {
        use PromptAction::*;
        if matches!(action, OpenWorkspace) {
            let input = value.trim();
            if input.is_empty() {
                if let Some(panel) = &mut self.tools.panel { panel.message = "Directory path is required".into(); }
                return;
            }
            let path = if input == "~" || input.starts_with("~/") {
                match std::env::var_os("HOME") {
                    Some(home) => PathBuf::from(home).join(input.strip_prefix("~/").unwrap_or("")),
                    None => {
                        if let Some(panel) = &mut self.tools.panel { panel.message = "HOME is unavailable".into(); }
                        return;
                    }
                }
            } else {
                let path = PathBuf::from(input);
                if path.is_absolute() { path } else { self.files.tree_root.join(path) }
            };
            let path = match path.canonicalize() {
                Ok(path) if path.is_dir() => path,
                Ok(_) => {
                    if let Some(panel) = &mut self.tools.panel { panel.message = "Path is not a directory".into(); }
                    return;
                }
                Err(error) => {
                    if let Some(panel) = &mut self.tools.panel { panel.message = format!("Cannot open directory: {error}"); }
                    return;
                }
            };
            if self.write_busy() {
                if let Some(panel) = &mut self.tools.panel { panel.message = "Wait for the current write to finish".into(); }
                return;
            }
            self.close_tools();
            self.switch_workspace(path);
            return;
        }
        if matches!(action, Extension | Exclusions) {
            self.files.revealed_path = None;
            if matches!(action, Extension) {
                self.tools.filter.extension = value.trim().trim_start_matches('.').into();
            } else {
                self.tools.filter.excluded = value;
            }
            self.close_tools();
            self.rebuild_visible_preserving_selection();
            self.request_preview_for_selection();
            return;
        }
        if value.trim().is_empty() {
            if let Some(p) = &mut self.tools.panel {
                p.message = "Value is required".into();
            }
            return;
        }
        let path = PathBuf::from(&value);
        let operation = match action {
            NewFile => Operation::Files(FileOperation::NewFile(path)),
            NewDirectory => Operation::Files(FileOperation::NewDirectory(path)),
            Rename(from) => Operation::Files(FileOperation::Rename {
                path: from,
                destination: path,
            }),
            Copy(paths) => Operation::Files(FileOperation::Copy {
                paths,
                destination: path,
            }),
            Move(paths) => Operation::Files(FileOperation::Move {
                paths,
                destination: path,
            }),
            Branch => Operation::Git(GitAction::CreateBranch(value.clone())),
            RenameBranch(old) => Operation::Git(GitAction::RenameBranch {
                old,
                new: value.clone(),
            }),
            Stash => Operation::Git(GitAction::SaveStash(value.clone())),
            StashUnstaged => Operation::Git(GitAction::SaveStashUnstaged(value.clone())),
            Amend => Operation::Git(GitAction::Amend(value.clone())),
            Worktree => Operation::Git(GitAction::AddWorktree(if path.is_absolute() {
                path
            } else {
                self.files.tree_root.join(path)
            })),
            _ => return,
        };
        let after = match &operation {
            Operation::Git(GitAction::CreateBranch(_) | GitAction::RenameBranch { .. }) => {
                Some(PanelKind::Git(GitQuery::Branches))
            }
            Operation::Git(GitAction::SaveStash(_) | GitAction::SaveStashUnstaged(_)) => {
                Some(PanelKind::Git(GitQuery::Stashes))
            }
            Operation::Git(GitAction::AddWorktree(_)) => Some(PanelKind::Git(GitQuery::Worktrees)),
            _ => None,
        };
        if matches!(
            operation,
            Operation::Git(
                GitAction::Amend(_) | GitAction::SaveStash(_) | GitAction::SaveStashUnstaged(_)
            )
        ) {
            let detail = if matches!(operation, Operation::Git(GitAction::Amend(_))) {
                format!("Rewrite the latest commit with ALL staged changes?\nNew message: {value}\nThis changes the commit ID.")
            } else if matches!(operation, Operation::Git(GitAction::SaveStashUnstaged(_))) {
                format!("Stash unstaged and untracked changes?\nMessage: {value}\nStaged/index changes remain in place.")
            } else {
                format!("Save all tracked and untracked changes in a stash?\nMessage: {value}\nSaved edits will be removed from the worktree until the stash is applied.")
            };
            self.confirm(operation, detail, after);
        } else {
            self.start_tool_operation(operation, after);
        }
    }

    pub(crate) fn batch_stage(&mut self, stage: bool) {
        if !self.in_git_sidebar() {
            self.session.status = "Use the Git tab (F2) for Stage / Unstage".into();
            return;
        }
        if self.tools.marks.is_empty() {
            self.request_write_stage(stage);
            return;
        }
        let side = if stage {
            DiffSide::Unstaged
        } else {
            DiffSide::Staged
        };
        let selected = self.tools.marks.git_paths(side);
        if selected.is_empty() {
            self.session.status = format!(
                "No {} selection; mark rows on the {} side or use v on the row",
                side.label(),
                if stage { "CHANGES" } else { "STAGED" }
            );
            return;
        }
        let Some(snapshot) = &self.scm.git_status else {
            self.session.status = "Refresh Git status first".into();
            return;
        };
        let source = if stage {
            &snapshot.unstaged
        } else {
            &snapshot.staged
        };
        let paths: Vec<_> = source
            .iter()
            .filter(|e| {
                selected
                    .iter()
                    .any(|p| snapshot.repo_root.join(&e.path).starts_with(p))
            })
            .map(|e| PathBuf::from(&e.path))
            .collect();
        if paths.is_empty() {
            self.session.status = format!(
                "No matching {} changes in the selected items; the other side is not included",
                side.label()
            );
            return;
        }
        self.start_tool_operation(Operation::Stage { paths, stage }, None);
    }

    /// Navigate to a working-tree file, leaving Git's Diff view intact until
    /// the file exists and can be revealed. Historical/deleted paths may not
    /// exist in the current checkout and must not create files.
    pub(crate) fn open_selected_worktree_file(&mut self) {
        let path = if self.in_git_sidebar() {
            match self.selected_scm_row() {
                Some(ScmRow::Change { .. }) => self
                    .selected_scm_entry()
                    .and_then(|entry| self.repo_root().map(|root| root.join(&entry.path))),
                Some(ScmRow::CommitFile { commit, index, .. }) => self
                    .history
                    .commits
                    .get(commit)
                    .and_then(|c| self.history.files.get(&c.id))
                    .and_then(|files| files.get(index))
                    .and_then(|entry| self.repo_root().map(|root| root.join(&entry.path))),
                _ if self.session.focus == PaneFocus::Preview
                    && matches!(self.preview_state.preview_mode, PreviewMode::Diff) =>
                {
                    self.preview_state
                        .file_diff
                        .as_ref()
                        .filter(|d| self.diff_matches_selection(d))
                        .map(|diff| diff.repo_root.join(&diff.path))
                }
                _ => None,
            }
        } else if self.session.focus == PaneFocus::Preview
            && matches!(self.preview_state.preview_mode, PreviewMode::Diff)
        {
            self.preview_state
                .file_diff
                .as_ref()
                .filter(|d| self.diff_matches_selection(d))
                .map(|diff| diff.repo_root.join(&diff.path))
        } else {
            self.current_path()
        };
        let Some(path) = path else {
            self.session.status = "Select a changed file or a Diff first".into();
            return;
        };
        if !path.is_file() {
            self.session.status = format!(
                "Cannot open {}: file is missing from this checkout",
                path.display()
            );
            return;
        }
        if self.in_git_sidebar() {
            self.leave_git_sidebar();
        }
        self.close_tools();
        // Loading a preview normally focuses the right pane. For Open file
        // retain the Files tree so the expanded/selected path stays visible,
        // particularly on narrow terminals showing only one pane.
        self.tools.return_focus = Some(PaneFocus::List);
        self.reveal_file(path.clone(), 1);
        self.session.status = format!("Opening {}", path.display());
    }

    /// Discard only tracked, unstaged, non-conflicted files under an exact file
    /// or directory path. The index and untracked files remain untouched.
    pub(crate) fn has_discardable_changes_under(&self, scope: &Path) -> bool {
        self.scm.git_status.as_ref().is_some_and(|snap| {
            scope.starts_with(&snap.repo_root)
                && snap.unstaged.iter().any(|e| {
                    !matches!(
                        e.status,
                        forge_git::GitStatus::Untracked | forge_git::GitStatus::Conflicted
                    ) && snap.repo_root.join(&e.path).starts_with(scope)
                })
        })
    }

    pub(crate) fn discard_scope(&mut self, selected: Vec<PathBuf>) {
        let Some(snapshot) = &self.scm.git_status else {
            self.session.status = "Refresh Git status first".into();
            return;
        };
        let selected: Vec<_> = selected
            .into_iter()
            .filter(|p| p.starts_with(&snapshot.repo_root))
            .collect();
        let mut paths: Vec<_> = snapshot
            .unstaged
            .iter()
            .filter(|e| {
                !matches!(
                    e.status,
                    forge_git::GitStatus::Untracked | forge_git::GitStatus::Conflicted
                ) && selected
                    .iter()
                    .any(|p| snapshot.repo_root.join(&e.path).starts_with(p))
            })
            .map(|e| e.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            self.session.status =
                "No tracked unstaged changes in this scope; untracked/conflicted files are protected".into();
            return;
        }
        let detail = format!(
            "Discard UNSTAGED worktree changes in {} tracked files?\nStaged changes, untracked files and conflicts are preserved.\n\n{}{}",
            paths.len(),
            paths.iter().take(40).cloned().collect::<Vec<_>>().join("\n"),
            if paths.len() > 40 { format!("\n… and {} more", paths.len() - 40) } else { String::new() }
        );
        self.confirm(Operation::Git(GitAction::Discard(paths)), detail, None);
    }

    pub(crate) fn discard_selection(&mut self) {
        if self.session.focus == PaneFocus::Preview {
            if let Some(diff) = self.preview_state.file_diff.clone().filter(|d| {
                d.side == DiffSide::Unstaged
                    && self.diff_matches_selection(d)
                    && self.preview_state.pending_diff.is_none()
                    && !self.preview_state.preview_loading
            }) {
                if let Some(hunk) = self
                    .preview_state
                    .hunk_index
                    .and_then(|i| diff.hunks().get(i).cloned())
                {
                    let detail = format!(
                        "Discard this hunk in {}? Staged changes are preserved.\n\n{}",
                        diff.path,
                        diff.hunk_patch(&hunk)
                    );
                    self.confirm(Operation::DiscardHunk { diff, hunk }, detail, None);
                    return;
                }
            }
            self.session.status = "Select an unstaged hunk first".into();
            return;
        }
        let selected = if self.in_git_sidebar() {
            if !self.tools.marks.git.is_empty() {
                self.tools
                    .marks
                    .git
                    .iter()
                    .filter(|mark| mark.side == DiffSide::Unstaged)
                    .map(|mark| mark.path.clone())
                    .collect()
            } else {
                match self.selected_scm_row() {
                    Some(ScmRow::Folder {
                        column: StagingColumn::Unstaged,
                        path,
                        ..
                    }) => self
                        .repo_root()
                        .map(|root| vec![root.join(path)])
                        .unwrap_or_default(),
                    Some(ScmRow::Change {
                        column: StagingColumn::Unstaged,
                        ..
                    }) => self
                        .selected_scm_entry()
                        .and_then(|e| self.repo_root().map(|root| vec![root.join(&e.path)]))
                        .unwrap_or_default(),
                    _ => {
                        self.session.status =
                            "Discard applies to CHANGES, not staged/history rows".into();
                        return;
                    }
                }
            }
        } else {
            self.operation_paths()
        };
        self.discard_scope(selected);
    }

    pub(crate) fn start_tool_operation(&mut self, operation: Operation, after: Option<PanelKind>) {
        if self.write_busy() {
            if let Some(p) = &mut self.tools.panel {
                p.message = self.session.status.clone();
            }
            return;
        }
        self.cancel_tool_read();
        let root = self.files.tree_root.clone();
        let repo = self.repo_root().unwrap_or_else(|| root.clone());
        let git = self.services.git;
        let fetch = matches!(operation, Operation::Git(GitAction::Fetch));
        self.tools.after = after;
        self.tools.pending_task = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("tools:write".into()),
            async move {
                let message = match operation {
                    Operation::Files(op) => {
                        tokio::task::spawn_blocking(move || forge_fs::workspace::operate(&root, op))
                            .await??
                    }
                    Operation::Git(action) => git.write.action(&repo, action).await?,
                    Operation::Stage { paths, stage } => {
                        if stage {
                            git.stage_paths(&repo, &paths).await?;
                        } else {
                            git.unstage_paths(&repo, &paths).await?;
                        }
                        format!(
                            "{} {} files",
                            if stage { "Staged" } else { "Unstaged" },
                            paths.len()
                        )
                    }
                    Operation::Line { diff, index } => {
                        git.write.apply_line(&repo, &diff, index).await?;
                        format!(
                            "{} line {} in {}",
                            if diff.side == DiffSide::Unstaged {
                                "Staged"
                            } else {
                                "Unstaged"
                            },
                            index + 1,
                            diff.path
                        )
                    }
                    Operation::DiscardHunk { diff, hunk } => {
                        git.write.discard_hunk(&repo, &diff, &hunk).await?;
                        format!("Discarded hunk in {}", diff.path)
                    }
                };
                Ok(if fetch {
                    format!("Fetch completed: {message}")
                } else if message.is_empty() {
                    "Operation completed".into()
                } else {
                    message
                })
            },
        ));
        let mut panel = Panel::new(PanelKind::Text, "Working");
        panel.message = "Operation in progress… · Esc closes this view; operation continues".into();
        self.tools.panel = Some(panel);
    }

}

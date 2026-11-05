//! Shared action availability for keyboard input, menus and contextual help.
use super::*;

#[cfg(test)]
mod tests;

pub(super) struct StageTarget {
    pub path: PathBuf,
    pub hunk: Option<usize>,
    pub paths: Vec<PathBuf>,
}

pub(super) struct Shortcut {
    pub key: &'static str,
    pub label: &'static str,
}

pub(super) const HELP_TEXT: &str = "Git help
\nStage: add changes to the staging area for the next commit.
Unstage: remove changes from the next commit; keep your edits.
Commit: save ALL staged changes in this repository, including files outside the selected folder.
\nFiles / Git tabs
Click the visible tabs or press F1 for Files, F2 for Git.
Files: browse, search, create, rename, copy, move, delete and edit text.
Right-click a folder to create inside it; right-click blank space to create at root.
Right-click operations target that item. Commands use marked items for batches.
Stage / Unstage are available only in Git.
\nGit file list (left)
j/k or arrows: select a file. Its diff loads automatically in Git.
s: stage the selected file, or all changes under the selected folder.
u: unstage the selected file or folder; keep working-tree edits.
v: select the exact row side. The same file can be selected in CHANGES and
STAGED independently; the header count includes each side once.
s uses only marked CHANGES rows; u uses only marked STAGED rows. Folder
marks are recursive. The other side is never staged or unstaged by mistake.
Enter or d: focus the selected Git file's diff.
[ / ]: jump to Changes / Staged.
\nDiff (right)
Tab: switch focus between the list and preview.
s: stage only the current unstaged hunk (change block).
u: unstage only the current staged hunk.
n / N: next / previous hunk. j/k: scroll.
No editable hunks: Tab back to the list for whole-file actions.
\nFolders and group headings
Enter or Space: expand/collapse a folder. h or Left: parent.
s on a Changes folder: stage its changes, including subfolders.
u on a Staged folder: unstage its changes, including subfolders.
Folder actions include collapsed children, additions and deletions.
Only the selected folder is affected; group headings are navigation only.
Select a file inside the folder to review its diff.
\nRepository actions
c: open commit editor. Enter submits; Esc cancels.
p: pull repository changes. P: push repository commits.
Pull and push always affect the repository, not just the selection.
r: refresh files and Git status. g: open Git from Files.
Esc: return from diff to the list, then from Git to Files.
q: quit the workspace. Ctrl-C: quit, including from this help.
\nHelp
? opens help outside editors. Esc, ? or q closes help.
j/k, arrows, PageUp/PageDown or mouse wheel: scroll help.
Typing ?, q, s or other shortcuts in the commit editor inserts text.
\nSearch and workspace commands
: opens searchable Commands; type a command name, Enter runs it.
Ctrl-P: fuzzy filename search. Ctrl-F: global content search.
Ctrl-R in content search toggles regex. Enter jumps to the result line.
Search respects ignore files; skips binary files, files over 2 MiB and .git.
F: filters for Git status, extension, hidden files and excluded directories.
Filtering keeps the selected path, expansion and focus; if that path is
hidden it falls back to the nearest visible parent folder, not its old index.
Default exclusions: target and node_modules. .git is always protected.
e or right-click Edit text: open the built-in UTF-8 text editor (up to 2 MiB).
Ctrl-S save; Ctrl-Z/Y undo/redo. Arrows, mouse, multiline typing and paste.
Esc closes; unsaved changes: Ctrl-S save & close, Enter discard, Esc keep editing.
Saving preserves line endings and permissions; external changes retain your draft.
E: open the selected file using $EDITOR (fallback $VISUAL, then vi).
The TUI releases the terminal and restores selection/scroll after editing.
\nSelection and file operations
v: toggle selected file/folder. V: select all visible items.
Esc: clear selection. Marked paths stay selected across directory expansion.
Files marks are path-only. Switching Files <-> Git clears multi-selection.
s/u in the Git list: Stage/Unstage marked items on the matching side only.
: Commands includes New file/directory, Rename, Copy, Move and Delete.
Paths are relative to the workspace. Copy/Move needs an existing directory.
No operation overwrites an existing destination. Delete requires confirmation.
\nIndividual diff lines and discard
I: open changed lines from the current diff. Up/Down selects a line.
s stages that line in an unstaged diff; u unstages it in a staged diff.
All other changes remain unchanged. A changed diff requires reopening it.
x: discard selected tracked files; in the right diff, discard current hunk.
Discard requires confirmation and preserves the index. Delete removes untracked files.
\nHistory and repository management
H: selected file history, following renames. L: repository commit history.
Enter: commit details, then an affected file's diff. [/] pages through history.
Space: pin a history version. D: compare pinned version to selected version.
C/R in history: Cherry-pick/Revert with confirmation. Amend is in Commands.
B: branches. Enter switch, A create, r rename, X delete, D compare to HEAD.
Z: stashes. Enter diff, A save, a apply, X drop. Save includes untracked files.
S: ahead/behind and remote results. F fetch, p pull --ff-only, P push.
W: worktrees. Enter switch workspace, A create, X remove clean worktree.
Commands → Conflicts lists conflicts: e editor, o ours, t theirs, s mark resolved.
Commands also includes continue/abort for cherry-pick and revert, and abort merge.
Dialogs capture shortcuts. Esc closes; PgUp/PgDn scrolls details.";

impl App {
    pub(super) fn diff_matches_selection(&self, diff: &FileDiff) -> bool {
        if self.repo_root().as_deref() != Some(diff.repo_root.as_path()) {
            return false;
        }
        if self.in_git_sidebar() {
            self.selected_scm_entry()
                .is_some_and(|e| e.path == diff.path && e.side == diff.side)
        } else {
            self.selected_entry()
                .is_some_and(|e| e.path == diff.repo_root.join(&diff.path))
        }
    }

    pub(super) fn scroll_help(&mut self, delta: i32) {
        self.session.help_scroll = (i32::from(self.session.help_scroll) + delta)
            .clamp(0, i32::from(self.session.help_scroll_limit))
            as u16;
    }

    pub(super) fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                self.session.help_open = false
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.session.should_quit = true
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll_help(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_help(-1),
            KeyCode::PageDown => self.scroll_help(10),
            KeyCode::PageUp => self.scroll_help(-10),
            KeyCode::Home => self.session.help_scroll = 0,
            KeyCode::End => self.session.help_scroll = self.session.help_scroll_limit,
            _ => {}
        }
    }
    pub(super) fn stage_target(&self, action: ContextAction) -> Result<StageTarget, &'static str> {
        if !self.in_git_sidebar() {
            return Err("Stage / Unstage are available in the Git tab (F2)");
        }
        if self.scm.pending_write.is_some()
            || self.tools.pending_task.is_some()
            || self.editing.saving()
        {
            return Err("Git operation in progress; wait for it to finish");
        }
        let repo = self.repo_root().ok_or("Not a Git repository")?;
        let stage = matches!(
            action,
            ContextAction::Stage | ContextAction::StageFolder | ContextAction::StageHunk
        );
        if matches!(
            action,
            ContextAction::StageHunk | ContextAction::UnstageHunk
        ) {
            if self.preview_state.preview_loading || self.preview_state.pending_diff.is_some() {
                return Err("Diff is loading; wait before operating on a hunk");
            }
            if !matches!(self.preview_state.preview_mode, PreviewMode::Diff) {
                return Err("Open a diff to operate on a hunk");
            }
            let diff = self
                .preview_state
                .file_diff
                .as_ref()
                .ok_or("Select a file to view its diff")?;
            // A loaded patch must still belong to the selected file and side.
            if !self.diff_matches_selection(diff) {
                return Err("Selection changed; reload the selected file's diff");
            }
            if stage != matches!(diff.side, DiffSide::Unstaged) {
                return Err(if stage {
                    "Select an unstaged diff to stage a hunk"
                } else {
                    "Select a staged diff to unstage a hunk"
                });
            }
            let index = self
                .preview_state
                .hunk_index
                .filter(|&i| i < diff.hunks().len())
                .ok_or("No editable hunk in this diff; use the file action in the sidebar")?;
            return Ok(StageTarget {
                path: PathBuf::from(&diff.path),
                hunk: Some(index),
                paths: Vec::new(),
            });
        }
        if matches!(
            action,
            ContextAction::StageFolder | ContextAction::UnstageFolder
        ) {
            let path = if self.in_git_sidebar() {
                let Some(ScmRow::Folder { path, column, .. }) = self.selected_scm_row() else {
                    return Err("Select a folder first");
                };
                if stage != (column == StagingColumn::Unstaged) {
                    return Err(if stage {
                        "Select a folder in Changes to stage"
                    } else {
                        "Select a folder in Staged to unstage"
                    });
                }
                PathBuf::from(path)
            } else {
                let e = self
                    .selected_entry()
                    .filter(|e| e.kind == EntryKind::Directory)
                    .ok_or("Select a folder first")?;
                e.path
                    .strip_prefix(&repo)
                    .map_err(|_| "Folder outside repository")?
                    .to_path_buf()
            };
            let paths = self.folder_change_paths(&repo.join(&path), stage);
            if paths.is_empty() {
                return Err(if stage {
                    "No unstaged changes in this folder"
                } else {
                    "No staged changes in this folder"
                });
            }
            return Ok(StageTarget {
                path,
                hunk: None,
                paths,
            });
        }
        let (path, can_stage, can_unstage) = if self.in_git_sidebar() {
            let e = self
                .selected_scm_entry()
                .ok_or("Select a file or folder; group headings are navigation only")?;
            (
                PathBuf::from(&e.path),
                e.side == DiffSide::Unstaged,
                e.side == DiffSide::Staged,
            )
        } else {
            let e = self.selected_entry().ok_or("Select a file first")?;
            if matches!(e.kind, EntryKind::Directory) {
                return Err("Use the folder action to stage or unstage this directory");
            }
            let path = e
                .path
                .strip_prefix(&repo)
                .map_err(|_| "File outside repository")?
                .to_path_buf();
            let (s, u) = self.file_stage_flags(&e.path);
            (path, s, u)
        };
        if !(if stage { can_stage } else { can_unstage }) {
            return Err(if stage {
                "No unstaged changes for this file; use [ to view Changes"
            } else {
                "No staged changes for this file; use ] to view Staged"
            });
        }
        Ok(StageTarget {
            paths: vec![path.clone()],
            path,
            hunk: None,
        })
    }

    fn folder_change_paths(&self, abs: &Path, stage: bool) -> Vec<PathBuf> {
        let Some(snap) = &self.scm.git_status else {
            return Vec::new();
        };
        let Ok(rel) = abs.strip_prefix(&snap.repo_root) else {
            return Vec::new();
        };
        let changes = if stage { &snap.unstaged } else { &snap.staged };
        changes
            .iter()
            .map(|c| PathBuf::from(&c.path))
            .filter(|p| p.starts_with(rel))
            .collect()
    }

    pub(super) fn selection_stage_action(&self, stage: bool) -> ContextAction {
        let folder = if self.in_git_sidebar() {
            matches!(self.selected_scm_row(), Some(ScmRow::Folder { .. }))
        } else {
            self.selected_entry()
                .is_some_and(|e| e.kind == EntryKind::Directory)
        };
        match (stage, folder) {
            (true, true) => ContextAction::StageFolder,
            (false, true) => ContextAction::UnstageFolder,
            (true, false) => ContextAction::Stage,
            (false, false) => ContextAction::Unstage,
        }
    }

    fn stage_actions(&self) -> [ContextAction; 2] {
        if self.session.focus == PaneFocus::Preview {
            [ContextAction::StageHunk, ContextAction::UnstageHunk]
        } else {
            [
                self.selection_stage_action(true),
                self.selection_stage_action(false),
            ]
        }
    }

    pub(super) fn context_shortcuts(&self) -> Vec<Shortcut> {
        if !self.in_git_sidebar() {
            let mut keys = vec![];
            if self.session.focus == PaneFocus::Preview {
                keys.push(Shortcut {
                    key: "j/k",
                    label: "Scroll",
                });
            }
            keys.push(
                if self
                    .selected_entry()
                    .is_some_and(|e| e.kind == EntryKind::Directory)
                {
                    Shortcut {
                        key: "Enter",
                        label: "Toggle folder",
                    }
                } else {
                    Shortcut {
                        key: "e",
                        label: "Edit text",
                    }
                },
            );
            keys.extend([
                Shortcut {
                    key: ":",
                    label: "Commands",
                },
                Shortcut {
                    key: "Ctrl-P",
                    label: "Find file",
                },
                Shortcut {
                    key: "Ctrl-F",
                    label: "Search",
                },
                Shortcut {
                    key: "v",
                    label: "Select",
                },
                Shortcut {
                    key: "F2",
                    label: "Git tab",
                },
                Shortcut {
                    key: "r",
                    label: "Refresh",
                },
            ]);
            return keys;
        }
        let mut keys = Vec::new();
        if !self.tools.marks.is_empty()
            && self.session.focus != PaneFocus::Preview
        {
            keys.extend([
                Shortcut {
                    key: "s",
                    label: "Stage selection",
                },
                Shortcut {
                    key: "u",
                    label: "Unstage selection",
                },
            ]);
        }
        for (key, action) in ["s", "u"].into_iter().zip(self.stage_actions()) {
            if (self.tools.marks.is_empty() || self.session.focus == PaneFocus::Preview)
                && self.stage_target(action).is_ok()
            {
                keys.push(Shortcut {
                    key,
                    label: action.label(),
                });
            }
        }
        let preview = self.session.focus == PaneFocus::Preview;
        keys.push(Shortcut {
            key: ":",
            label: "Commands",
        });
        keys.push(Shortcut {
            key: "Ctrl-P",
            label: "Find file",
        });
        keys.push(Shortcut {
            key: "Ctrl-F",
            label: "Search",
        });
        if !preview {
            keys.push(Shortcut {
                key: "v",
                label: "Select",
            });
        }
        if preview {
            keys.push(Shortcut {
                key: "j/k",
                label: "Scroll",
            });
            if matches!(self.preview_state.preview_mode, PreviewMode::Diff)
                && self.preview_state.hunk_index.is_some()
                && !self.preview_state.preview_loading
            {
                keys.push(Shortcut {
                    key: "n/N",
                    label: "Next/prev hunk",
                });
            }
        } else if self.in_git_sidebar() {
            keys.push(match self.selected_scm_row() {
                Some(ScmRow::Folder { expanded: true, .. }) => Shortcut {
                    key: "Enter",
                    label: "Collapse folder",
                },
                Some(ScmRow::Folder { .. }) => Shortcut {
                    key: "Enter",
                    label: "Expand folder",
                },
                Some(ScmRow::Change { .. }) => Shortcut {
                    key: "Enter",
                    label: "Focus diff",
                },
                _ => Shortcut {
                    key: "j/k",
                    label: "Select a file",
                },
            });
        } else {
            let directory = self
                .selected_entry()
                .is_some_and(|e| e.kind == EntryKind::Directory);
            keys.push(Shortcut {
                key: "Enter",
                label: if directory { "Toggle folder" } else { "Open" },
            });
        }
        keys.push(Shortcut {
            key: "Tab",
            label: if preview {
                "Focus list"
            } else {
                "Focus preview"
            },
        });
        if self.in_git_sidebar() {
            keys.push(Shortcut {
                key: "[/]",
                label: "Changes/Staged",
            });
        } else {
            keys.push(Shortcut {
                key: "g",
                label: "Git",
            });
            if self.selected_entry().is_some_and(|e| {
                self.file_stage_flags(&e.path) != (false, false) && e.kind != EntryKind::Directory
            }) {
                keys.push(Shortcut {
                    key: "d",
                    label: "View diff",
                });
            }
        }
        if self.scm.git_status.is_some() && self.scm.pending_write.is_none() {
            keys.extend([
                Shortcut {
                    key: "c",
                    label: "Commit staged",
                },
                Shortcut {
                    key: "p",
                    label: "Pull",
                },
                Shortcut {
                    key: "P",
                    label: "Push",
                },
            ]);
        }
        if preview || self.in_git_sidebar() {
            keys.push(Shortcut {
                key: "Esc",
                label: if preview { "Back to list" } else { "Files" },
            });
        }
        keys.extend([
            Shortcut {
                key: "r",
                label: "Refresh",
            },
            Shortcut {
                key: "q",
                label: "Quit",
            },
        ]);
        keys
    }

    pub(super) fn context_description(&self) -> String {
        if !self.in_git_sidebar() {
            if !self.tools.marks.is_empty() {
                return format!(
                    "{} selected · : Copy / Move / Delete · Esc Clear",
                    self.tools.marks.len()
                );
            }
            return self
                .selected_entry()
                .map(|e| {
                    format!(
                        "{} · {}",
                        e.name,
                        if e.kind == EntryKind::Directory {
                            "Right-click: new file / folder, rename, delete"
                        } else {
                            "e Edit text · Right-click: file actions"
                        }
                    )
                })
                .unwrap_or_else(|| "Right-click to create a file or folder".into());
        }
        if self.session.focus == PaneFocus::Preview {
            if self.preview_state.preview_loading {
                return "Loading preview...".into();
            }
            if let Some(diff) = self
                .preview_state
                .file_diff
                .as_ref()
                .filter(|_| matches!(self.preview_state.preview_mode, PreviewMode::Diff))
            {
                return format!(
                    "{} · {} · {}",
                    diff.path,
                    diff.side.label(),
                    self.preview_state
                        .hunk_index
                        .map(|i| format!("hunk {}/{}", i + 1, diff.hunks().len()))
                        .unwrap_or_else(|| "No editable hunks; Tab for file actions".into())
                );
            }
            return "Preview · Tab returns to file actions".into();
        }
        if self.in_git_sidebar() {
            match self.selected_scm_row() {
                Some(ScmRow::Folder {
                    path,
                    file_count,
                    column,
                    ..
                }) => format!(
                    "{path}/ · {file_count} changed files · {} folder including subfolders",
                    if column == StagingColumn::Unstaged {
                        "s Stage"
                    } else {
                        "u Unstage"
                    }
                ),
                Some(ScmRow::Section {
                    column: StagingColumn::Unstaged,
                    count,
                    ..
                }) => format!("Changes · {count} files not yet staged · Select a file to review"),
                Some(ScmRow::Section { count, .. }) => {
                    format!("Staged · {count} files ready for the next commit")
                }
                Some(ScmRow::Change { .. }) => self
                    .selected_scm_entry()
                    .map(|e| format!("{} · {} · whole file", e.path, e.side.label()))
                    .unwrap_or_default(),
                None => "Select a file to review its changes".into(),
            }
        } else {
            self.selected_entry()
                .map(|e| {
                    let (s, u) = self.file_stage_flags(&e.path);
                    format!(
                        "{} · {}",
                        e.name,
                        if e.kind == EntryKind::Directory {
                            "folder · Git actions include all subfolders"
                        } else if s && u {
                            "staged + unstaged · whole file"
                        } else if s {
                            "unstaged · whole file"
                        } else if u {
                            "staged · whole file"
                        } else {
                            "Press g to review Git changes"
                        }
                    )
                })
                .unwrap_or_else(|| "No files · r Refresh · ? Help".into())
        }
    }

    pub(super) fn empty_diff_hint(&self) -> &'static str {
        if self
            .scm
            .git_status
            .as_ref()
            .is_some_and(|s| s.unstaged.is_empty() && s.staged.is_empty())
        {
            "Working tree clean. Edit a file, then refresh with r."
        } else {
            match self.selected_scm_row() {
                Some(ScmRow::Folder { column: StagingColumn::Unstaged, .. }) => "s stages all changes in this folder and its subfolders, including additions and deletions. Expand with Enter or Space to review individual files.",
                Some(ScmRow::Folder { .. }) => "u unstages all changes in this folder and its subfolders; working-tree edits are kept. Expand with Enter or Space to review individual files.",
                Some(ScmRow::Section { column: StagingColumn::Staged, .. }) => "Staged files are ready for the next commit. Select a file to review it; u removes it from the staging area without discarding edits.",
                _ => "Changes are not yet staged. Select a file with j/k to review its diff; s stages the whole file. Press ? for help.",
            }
        }
    }

    pub(super) fn update_stage_menu(&self, items: &mut [forge_ui::ContextMenuItem]) {
        for item in items {
            if matches!(
                item.action,
                ContextAction::Stage
                    | ContextAction::Unstage
                    | ContextAction::StageFolder
                    | ContextAction::UnstageFolder
                    | ContextAction::StageHunk
                    | ContextAction::UnstageHunk
            ) {
                item.enabled = self.stage_target(item.action).is_ok();
            }
            if matches!(
                item.action,
                ContextAction::Commit | ContextAction::Push | ContextAction::Pull
            ) {
                item.enabled &= self.scm.pending_write.is_none();
            }
        }
    }

    pub(super) fn open_commit(&mut self) {
        if self.write_busy() {
            return;
        }
        if self.repo_root().is_none() {
            self.session.status = "not a git repository".into();
        } else {
            self.session.commit_open = true;
            self.session.status = "commit message...".into();
        }
    }

    pub(super) fn write_busy(&mut self) -> bool {
        if self.scm.pending_write.is_some()
            || self.tools.pending_task.is_some()
            || self.editing.saving()
        {
            self.session.status = "Operation in progress; wait for it to finish".into();
            true
        } else {
            false
        }
    }
}

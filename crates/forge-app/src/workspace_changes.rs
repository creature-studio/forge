//! Workbench coordination for changes to the shared filesystem and Git state.
//! Producers report completed operations; this module owns refresh policy.
use super::*;

pub(super) enum WorkspaceChange {
    FileSaved {
        path: PathBuf,
    },
    GitWrite {
        preserve_diff: bool,
    },
    ToolWrite,
    External {
        git_metadata_changed: bool,
        changed_paths: Vec<PathBuf>,
        overflowed: bool,
    },
}

/// Coalesce notifications while a local write is in flight. A completion
/// event is not a substitute for an unrelated external filesystem change.
#[derive(Default)]
pub(super) struct WorkspaceChanges {
    deferred: Option<ExternalChanges>,
}

#[derive(Default)]
struct ExternalChanges {
    git_metadata_changed: bool,
    changed_paths: HashSet<PathBuf>,
    overflowed: bool,
}

impl ExternalChanges {
    fn merge(&mut self, git_metadata_changed: bool, changed_paths: Vec<PathBuf>, overflowed: bool) {
        self.git_metadata_changed |= git_metadata_changed;
        self.overflowed |= overflowed;
        if !self.overflowed {
            self.changed_paths.extend(changed_paths);
            self.overflowed = self.changed_paths.len() > 512;
        }
        if self.overflowed {
            self.changed_paths.clear();
        }
    }
}

impl WorkspaceChanges {
    pub(super) fn clear(&mut self) {
        self.deferred = None;
    }
}

/// Only the presentation facts that affect refresh policy; no App, services,
/// mutable domain state or I/O is available to the decision function.
#[derive(Clone, Copy, Default)]
struct RefreshContext {
    git_sidebar: bool,
    history_expanded: bool,
    has_diff: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum RefreshAction {
    InvalidateSearch,
    Status,
    History,
    List,
    SelectedDiff,
    ExistingDiff,
    RevealSaved(PathBuf),
}

fn refresh_actions(change: WorkspaceChange, context: RefreshContext) -> Vec<RefreshAction> {
    use RefreshAction::*;
    match change {
        WorkspaceChange::FileSaved { path } => vec![
            InvalidateSearch,
            Status,
            if context.git_sidebar {
                SelectedDiff
            } else {
                RevealSaved(path)
            },
        ],
        WorkspaceChange::GitWrite { preserve_diff } => {
            let mut actions = vec![Status];
            if context.git_sidebar && context.history_expanded {
                actions.push(History);
            }
            actions.push(if preserve_diff && context.has_diff {
                ExistingDiff
            } else {
                List
            });
            actions
        }
        WorkspaceChange::ToolWrite => vec![List, Status],
        WorkspaceChange::External { .. } => {
            unreachable!("external changes are coalesced before planning")
        }
    }
}

impl WorkspaceChanges {
    fn defer(&mut self, git_metadata_changed: bool, changed_paths: Vec<PathBuf>, overflowed: bool) {
        self.deferred
            .get_or_insert_with(ExternalChanges::default)
            .merge(git_metadata_changed, changed_paths, overflowed);
    }

    fn take_ready(&mut self, write_pending: bool) -> Option<ExternalChanges> {
        if write_pending {
            None
        } else {
            self.deferred.take()
        }
    }
}

impl App {
    pub(super) fn flush_workspace_changes(&mut self) {
        let write_pending = self.editing.saving()
            || self.scm.pending_write.is_some()
            || self.tools.pending_task.is_some();
        let Some(changes) = self.workspace_changes.take_ready(write_pending) else {
            return;
        };
        self.refresh_external_changes(changes.git_metadata_changed || changes.overflowed);
        if changes.overflowed {
            if let Some(path) = self
                .preview_state
                .pending_preview
                .as_ref()
                .map(|p| p.path.clone())
                .or_else(|| self.preview_state.preview.as_ref().map(|p| p.path.clone()))
            {
                self.refresh_preview_if_affected(&[path]);
            }
        } else {
            let paths = changes.changed_paths.into_iter().collect::<Vec<_>>();
            self.refresh_preview_if_affected(&paths);
        }
    }

    pub(super) fn on_workspace_change(&mut self, change: WorkspaceChange) {
        if let WorkspaceChange::External {
            git_metadata_changed,
            changed_paths,
            overflowed,
        } = change
        {
            self.workspace_changes
                .defer(git_metadata_changed, changed_paths, overflowed);
            self.flush_workspace_changes();
            return;
        }
        let context = RefreshContext {
            git_sidebar: self.in_git_sidebar(),
            history_expanded: self.history.expanded(),
            has_diff: self.preview_state.file_diff.is_some(),
        };
        for action in refresh_actions(change, context) {
            match action {
                RefreshAction::InvalidateSearch => self.invalidate_search_cache(),
                RefreshAction::Status => self.request_status(),
                RefreshAction::History => self.request_history(),
                RefreshAction::List => self.request_list(),
                RefreshAction::SelectedDiff => self.request_diff_for_scm_selection(),
                RefreshAction::ExistingDiff => {
                    if let Some(diff) = &self.preview_state.file_diff {
                        self.request_diff(diff.path.clone(), diff.side);
                    }
                }
                RefreshAction::RevealSaved(path) => {
                    self.navigation
                        .request(path, self.preview_source_line(), self.session.focus);
                    self.request_preview_for_selection();
                }
            }
        }
    }

    /// Refresh the directory cache and Git snapshot without dropping focus,
    /// expansion, or a loaded diff/preview. Editing drafts are never touched.
    ///
    /// `git_metadata_changed` is true when the watcher saw HEAD, packed-refs
    /// or `refs/` move; only then can the commit graph differ from what the
    /// sidebar already holds. Worktree writes (saves, builds, external edits)
    /// refresh status and the listing but leave history alone.
    pub(super) fn refresh_external_changes(&mut self, git_metadata_changed: bool) {
        self.invalidate_search_cache();
        self.reset_inline_diff();
        self.record_refresh_anchor();
        self.refresh_tree_in_place();
        self.request_status();
        if git_metadata_changed && self.history.expanded() {
            self.request_history();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_policy_is_independent_of_history_and_existing_diff() {
        use RefreshAction::*;
        let path = PathBuf::from("saved.txt");
        for git_sidebar in [false, true] {
            for history_expanded in [false, true] {
                for has_diff in [false, true] {
                    let context = RefreshContext {
                        git_sidebar,
                        history_expanded,
                        has_diff,
                    };
                    let expected = vec![
                        InvalidateSearch,
                        Status,
                        if git_sidebar {
                            SelectedDiff
                        } else {
                            RevealSaved(path.clone())
                        },
                    ];
                    assert_eq!(
                        refresh_actions(WorkspaceChange::FileSaved { path: path.clone() }, context),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn git_and_tool_refresh_policy_uses_only_relevant_inputs() {
        use RefreshAction::*;
        for git_sidebar in [false, true] {
            for history_expanded in [false, true] {
                for has_diff in [false, true] {
                    let context = RefreshContext {
                        git_sidebar,
                        history_expanded,
                        has_diff,
                    };
                    assert_eq!(
                        refresh_actions(WorkspaceChange::ToolWrite, context),
                        vec![List, Status]
                    );
                    for preserve_diff in [false, true] {
                        let mut expected = vec![Status];
                        if git_sidebar && history_expanded {
                            expected.push(History);
                        }
                        expected.push(if preserve_diff && has_diff {
                            ExistingDiff
                        } else {
                            List
                        });
                        assert_eq!(
                            refresh_actions(WorkspaceChange::GitWrite { preserve_diff }, context),
                            expected
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn deferred_changes_remain_owned_until_writes_complete() {
        let mut changes = WorkspaceChanges::default();
        changes.defer(false, vec![PathBuf::from("a")], false);
        assert!(changes.take_ready(true).is_none());
        changes.defer(true, vec![PathBuf::from("b")], false);
        let ready = changes.take_ready(false).unwrap();
        assert!(ready.git_metadata_changed);
        assert_eq!(ready.changed_paths.len(), 2);
        assert!(changes.take_ready(false).is_none());
    }

    #[test]
    fn external_notifications_merge_paths_and_metadata_without_unbounded_growth() {
        let mut changes = ExternalChanges::default();
        let path = PathBuf::from("a");
        changes.merge(false, vec![path.clone()], false);
        changes.merge(true, vec![path, PathBuf::from("b")], false);
        assert_eq!(changes.changed_paths.len(), 2);
        assert!(changes.git_metadata_changed);
        changes.merge(
            false,
            (0..513).map(|n| PathBuf::from(n.to_string())).collect(),
            false,
        );
        assert!(changes.overflowed);
        assert!(changes.changed_paths.is_empty());
    }

    #[tokio::test]
    async fn unrelated_external_change_is_not_lost_during_local_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        let busy = app.services.jobs.spawn(JobPriority::Normal, None, async {
            std::future::pending::<Result<String>>().await
        });
        app.tools.pending_task = Some(busy);
        let path = dir.path().join("outside-write.txt");
        std::fs::write(&path, "external").unwrap();
        app.on_workspace_change(WorkspaceChange::External {
            git_metadata_changed: true,
            changed_paths: vec![path.clone()],
            overflowed: false,
        });
        let deferred = app
            .workspace_changes
            .deferred
            .as_ref()
            .expect("must queue external change");
        assert!(deferred.changed_paths.contains(&path));
        assert!(deferred.git_metadata_changed);
        let job = app.tools.pending_task.take().unwrap();
        app.services.jobs.cancel(job.id);
        app.flush_workspace_changes();
        assert!(app.workspace_changes.deferred.is_none());
        assert!(!app.files.pending_lists.is_empty());
    }
}

//! Workbench navigation owns asynchronous reveal and focus handoffs.
use super::workspace_filter::FileFilter;
use super::*;

#[derive(Clone)]
struct RevealRequest {
    path: PathBuf,
    line: usize,
    focus: PaneFocus,
}

#[derive(Default)]
pub(super) struct NavigationState {
    pending: Option<RevealRequest>,
    resume_diff: Option<DiffResume>,
    worktrees: HashMap<PathBuf, WorkspaceNavigation>,
}

impl NavigationState {
    pub(super) fn request(&mut self, path: PathBuf, line: usize, focus: PaneFocus) {
        self.pending = Some(RevealRequest { path, line, focus });
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn take_for(&mut self, path: &Path) -> Option<(usize, PaneFocus)> {
        if self
            .pending
            .as_ref()
            .is_some_and(|request| request.path == path)
        {
            return self
                .pending
                .take()
                .map(|request| (request.line, request.focus));
        }
        None
    }

    pub(super) fn cancel_reveal(&mut self) {
        self.pending = None;
    }

    pub(super) fn request_diff_resume(&mut self, resume: DiffResume) {
        self.resume_diff = Some(resume);
    }

    pub(super) fn diff_resume(&self) -> Option<&DiffResume> {
        self.resume_diff.as_ref()
    }

    pub(super) fn cancel_diff_resume(&mut self) {
        self.resume_diff = None;
    }

    /// Preserve the existing one-shot semantics: a completed diff that
    /// does not match the intended working-tree diff abandons the resume.
    pub(super) fn take_diff_resume(
        &mut self,
        path: &str,
        source: &DiffSource,
    ) -> Option<DiffResume> {
        self.resume_diff.take().filter(|r| {
            r.path == path && matches!(source, DiffSource::Working(side) if *side == r.side)
        })
    }

    pub(super) fn clear_pending(&mut self) {
        self.cancel_reveal();
        self.resume_diff = None;
    }
}

/// Navigation state is local to a worktree. Never reuse a directory's
/// expansion/selection in another worktree with similar filenames.
#[derive(Clone)]
pub(super) struct WorkspaceNavigation {
    files: files_state::FileNavigation,
    scm: scm_state::ScmNavigation,
    filter: FileFilter,
    pub(super) activity: ActivityTab,
    pub(super) focus: PaneFocus,
}

pub(super) struct DiffResume {
    pub path: String,
    pub side: DiffSide,
    pub scroll: usize,
    pub focus: PaneFocus,
    pub hunk: Option<usize>,
}

impl App {
    /// A user visibility command includes an explicit focus handoff. Layout
    /// geometry and repaint never dispatch this application-level command.
    pub(super) fn set_sidebar_visible(&mut self, visible: bool) {
        self.layout_state.set_sidebar_visible(visible);
        self.session.focus = if visible {
            PaneFocus::List
        } else {
            PaneFocus::Preview
        };
    }

    pub(super) fn reveal_file(&mut self, path: PathBuf, line: usize) {
        self.reveal_file_with_focus(path, line, PaneFocus::Preview);
    }

    pub(super) fn reveal_file_with_focus(&mut self, path: PathBuf, line: usize, focus: PaneFocus) {
        // Git may discover a repository above the current Files root. The
        // target cannot appear in that narrower tree until it is rooted at
        // the repository, so switch only when the target is actually outside.
        if !path.starts_with(self.files.root()) {
            let repo = self.repo_root();
            if let Some(repo) = repo.filter(|repo| path.starts_with(repo)) {
                if self.write_busy() {
                    self.session.status = "Cannot navigate while a write is in progress".into();
                    return;
                }
                self.switch_workspace(repo);
            } else {
                self.session.status = "File is outside the current workspace".into();
                return;
            }
        }
        self.session.activity = ActivityTab::Files;
        self.session.focus = PaneFocus::List;
        self.files.begin_reveal(&path);
        self.navigation.request(path, line, focus);
        self.request_missing_expanded();
        self.rebuild_visible();
        self.try_reveal();
    }

    pub(super) fn try_reveal(&mut self) {
        let Some(RevealRequest { path, line, focus }) = self.navigation.pending.clone() else {
            return;
        };
        let viewport = self.layout_state.geometry.list_inner.height as usize;
        if !self.files.select_revealed_path(&path, viewport) {
            if self.files.pending_lists.is_empty() {
                self.navigation.cancel_reveal();
                self.session.status = format!("Cannot locate {}", path.display());
            }
            return;
        }
        let pending_target = self
            .preview_state
            .pending_preview
            .as_ref()
            .is_some_and(|pending| pending.path == path);
        let already_displayed = matches!(self.preview_state.preview_mode, PreviewMode::File)
            && self
                .preview_state
                .preview
                .as_ref()
                .is_some_and(|preview| preview.path == path)
            && self.preview_state.pending_preview.is_none();
        if already_displayed {
            // No asynchronous preview completion will follow. Complete the
            // explicit reveal now, including its focus/scroll handoff, or a
            // later reveal could accidentally inherit this operation's focus.
            self.navigation.cancel_reveal();
            let markdown_available = self.markdown_available();
            self.preview_state
                .reveal_source_line(line, markdown_available);
            self.session.focus = focus;
        } else if !pending_target {
            // Resolve the target once in Files, then send its identity to
            // Preview. Preview does not consult selection/filter state.
            if let Some(entry) = self.files.selected_entry() {
                self.request_file_preview(Some((path, entry.kind)));
            }
        }
    }

    pub(super) fn switch_workspace(&mut self, path: PathBuf) {
        if self.write_busy() || path == self.files.root() {
            return;
        }
        // Capture identity (absolute path) rather than a fragile row index.
        let current_root = self.files.root().to_path_buf();
        self.config
            .recent_workspaces
            .retain(|p| p != &path && p != &current_root);
        self.config
            .recent_workspaces
            .insert(0, current_root.clone());
        self.config.recent_workspaces.insert(0, path.clone());
        self.config.recent_workspaces.truncate(12);
        // Test fixtures switch workspaces often; never rewrite the developer's
        // real ~/.config/forge/config.toml from a unit test.
        if !cfg!(test) {
            if let Err(error) = self.config.save() {
                self.session.status = format!("Workspace history not saved: {error}");
            }
        }
        let navigation = WorkspaceNavigation {
            files: self.files.navigation_snapshot(),
            scm: self.scm.navigation_snapshot(),
            filter: self.workspace_filter.clone(),
            activity: self.session.activity,
            focus: self.session.focus,
        };
        self.navigation.worktrees.insert(current_root, navigation);
        let restore = self.navigation.worktrees.get(&path).cloned();
        if self
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, tools::PanelKind::Files | tools::PanelKind::Content))
        {
            self.close_tools();
        }
        self.tools.marks.clear();
        self.tools.file_selection_anchor = None;
        self.workspace_filter = restore
            .as_ref()
            .map(|n| n.filter.clone())
            .unwrap_or_default();
        if restore.is_none() {
            self.workspace_filter.show_hidden = self.config.show_hidden_files;
        }
        self.tools.sync.clear();
        self.tools.remote_result.clear();
        self.tools.last_fetch.clear();
        self.navigation.clear_pending();
        self.workspace_changes.clear();
        self.files
            .restore_workspace(path, restore.as_ref().map(|n| &n.files));
        self.files_preview = None;
        self.scm.restore_workspace(restore.as_ref().map(|n| &n.scm));
        // Graph/commit details from the previous repository are invalid.
        self.history = history::HistoryState::default();
        self.session.activity = restore
            .as_ref()
            .map(|n| n.activity)
            .unwrap_or(ActivityTab::Files);
        self.session.focus = restore.as_ref().map(|n| n.focus).unwrap_or(PaneFocus::List);
        self.initialize_workspace_listing();
        self.request_status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_reveal_owns_line_and_focus_and_ignores_old_completion() {
        let mut nav = NavigationState::default();
        let old = PathBuf::from("old.rs");
        let new = PathBuf::from("new.rs");
        nav.request(old.clone(), 8, PaneFocus::List);
        nav.request(new.clone(), 42, PaneFocus::Preview);
        assert_eq!(nav.take_for(&old), None);
        assert_eq!(nav.take_for(&new), Some((42, PaneFocus::Preview)));
        assert_eq!(nav.take_for(&new), None);
    }

    #[test]
    fn workspace_change_clears_pending_navigation() {
        let mut nav = NavigationState::default();
        let path = PathBuf::from("old.rs");
        nav.request(path.clone(), 8, PaneFocus::List);
        nav.resume_diff = Some(DiffResume {
            path: "old.rs".into(),
            side: DiffSide::Unstaged,
            scroll: 5,
            focus: PaneFocus::Preview,
            hunk: None,
        });
        nav.clear_pending();
        assert!(!nav.has_pending());
        assert_eq!(nav.take_for(&path), None);
        assert!(nav.resume_diff.is_none());
    }
}

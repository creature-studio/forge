//! Commit history for the Git sidebar (the HISTORY accordion).
//!
//! Reads come from `forge-git-gix` through the [`GitService`] facade and run on
//! the shared job scheduler, so expanding history never blocks the event loop.

use std::collections::{HashMap, HashSet};

use forge_git::{CommitChange, GitReadError, GraphCommit, GraphSnapshot};
use forge_jobs::{JobError, JobHandle};
use forge_model::JobId;
use forge_ui::{dir_key, layout_graph, GraphLayout, HistoryView, ScmRow};

use super::*;

/// Start small, then expand in bounded chunks when the user nears the
/// oldest currently loaded commit. Layout requires the earlier graph rows
/// for lane continuity, so each larger read is an asynchronous snapshot.
const HISTORY_PAGE_SIZE: usize = 200;
const HISTORY_MAX_COMMITS: usize = 5_000;

pub(super) struct PendingHistory {
    #[allow(dead_code)]
    id: JobId,
    handle: JobHandle<Result<GraphSnapshot, GitReadError>>,
}

pub(super) struct PendingCommitFiles {
    oid: String,
    handle: JobHandle<Result<Vec<CommitChange>, GitReadError>>,
}

type CommitDetail = (String, Vec<forge_git::CommitFileStat>);

pub(super) struct HistoryState {
    /// The HISTORY accordion itself (independent of per-commit expansion).
    pub expanded: bool,
    /// Commits in display order (children before parents).
    pub commits: Vec<GraphCommit>,
    /// Lane glyphs, one row per commit.
    pub layout: GraphLayout,
    pub expanded_commits: HashSet<String>,
    pub expanded_dirs: HashSet<String>,
    pub files: HashMap<String, Vec<CommitChange>>,
    /// Status relative to the current branch's configured fetch upstream.
    pub upstream: forge_git::UpstreamStatus,
    pub unpushed: HashSet<String>,
    pub incoming: HashSet<String>,
    pub ahead: usize,
    pub behind: usize,
    pub counts_truncated: bool,
    pending_detail: Option<(String, JobHandle<Result<CommitDetail>>)>,
    pub detail_files: Option<(String, Vec<forge_git::CommitFileStat>)>,
    pending_graph: Option<PendingHistory>,
    pending_files: Vec<PendingCommitFiles>,
    /// Deferred click on a commit-detail file whose history tree is still loading.
    pending_link: Option<(String, String)>,
    pub branch: String,
    pub message: String,
    pub loaded: bool,
    /// Requested graph size; keep it per active worktree.
    pub limit: usize,
    /// Last query returned fewer commits than requested.
    pub exhausted: bool,
    /// Select the newest commit as soon as the list arrives (`G`).
    focus_pending: bool,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            // History is the point of the section, so it starts open.
            expanded: true,
            commits: Vec::new(),
            layout: GraphLayout::default(),
            expanded_commits: HashSet::new(),
            expanded_dirs: HashSet::new(),
            files: HashMap::new(),
            upstream: forge_git::UpstreamStatus::default(),
            unpushed: HashSet::new(),
            incoming: HashSet::new(),
            ahead: 0,
            behind: 0,
            counts_truncated: false,
            pending_detail: None,
            detail_files: None,
            pending_graph: None,
            pending_files: Vec::new(),
            pending_link: None,
            branch: String::new(),
            message: String::new(),
            loaded: false,
            limit: HISTORY_PAGE_SIZE,
            exhausted: false,
            focus_pending: false,
        }
    }
}

impl HistoryState {
    /// Install a snapshot without touching git (used by tests).
    #[cfg(test)]
    pub(super) fn seed(&mut self, commits: Vec<GraphCommit>) {
        self.commits = commits;
        self.layout = layout_graph(&self.commits);
        self.loaded = true;
    }
}

impl App {
    /// Borrowed history for the SCM sidebar renderer.
    pub(super) fn history_view(&self) -> HistoryView<'_> {
        HistoryView {
            expanded: self.history.expanded,
            loading: self.history.pending_graph.is_some(),
            commits: &self.history.commits,
            layout: &self.history.layout,
            expanded_commits: &self.history.expanded_commits,
            expanded_dirs: &self.history.expanded_dirs,
            files: &self.history.files,
            upstream: &self.history.upstream,
            current_branch: &self.history.branch,
            unpushed: &self.history.unpushed,
            incoming: &self.history.incoming,
            ahead: self.history.ahead,
            behind: self.history.behind,
            counts_truncated: self.history.counts_truncated,
        }
    }

    /// `G`: show the Git tab with HISTORY open and the newest commit selected.
    pub(super) fn show_history(&mut self) {
        if !self.in_git_sidebar() {
            self.enter_git_sidebar();
        }
        self.session.context_menu = None;
        self.session.commit_open = false;
        self.session.commit_cancel_pending = false;
        self.history.expanded = true;
        self.history.focus_pending = true;
        self.request_history();
        self.select_first_commit();
        self.session.status = self.hint_bar();
    }

    /// Enter / Space on the HISTORY header.
    pub(super) fn toggle_history(&mut self) {
        self.history.expanded = !self.history.expanded;
        if self.history.expanded {
            self.history.focus_pending = true;
            self.request_history();
            self.select_first_commit();
        } else {
            self.clamp_scm_selection();
        }
    }

    fn select_first_commit(&mut self) {
        let rows = self.scm_rows();
        if let Some(index) = rows.iter().position(ScmRow::is_commit) {
            self.scm.scm_selected = index;
            self.request_diff_for_scm_selection();
            self.session.status = self.hint_bar();
        }
    }

    pub(super) fn request_history(&mut self) {
        if self.history.pending_graph.is_some() {
            return;
        }
        let root = self
            .repo_root()
            .unwrap_or_else(|| self.files.tree_root.clone());
        let git = self.services.git;
        let limit = self.history.limit;
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:history".into()), async move {
                    match tokio::task::spawn_blocking(move || git.graph(&root, limit)).await {
                        Ok(result) => result,
                        Err(err) => Err(GitReadError::History(err.to_string())),
                    }
                });
        self.history.pending_graph = Some(PendingHistory {
            id: handle.id,
            handle,
        });
    }

    /// The visible SCM row is near the oldest loaded history item; load an
    /// additional page on demand. Do not repeat once exhausted or in flight.
    pub(super) fn maybe_page_history(&mut self) {
        if !self.in_git_sidebar()
            || !self.history.expanded
            || self.history.pending_graph.is_some()
            || !self.history.loaded
            || self.history.exhausted
            || self.history.limit >= HISTORY_MAX_COMMITS
            || self.history.commits.len() < self.history.limit
        {
            return;
        }
        let Some(commit) = self.selected_scm_row().and_then(|row| row.commit()) else {
            return;
        };
        if commit + 12 < self.history.commits.len() {
            return;
        }
        self.history.limit = (self.history.limit + HISTORY_PAGE_SIZE).min(HISTORY_MAX_COMMITS);
        self.request_history();
    }

    fn request_commit_files(&mut self, oid: String) {
        if self.history.files.contains_key(&oid)
            || self
                .history
                .pending_files
                .iter()
                .any(|pending| pending.oid == oid)
        {
            return;
        }
        let root = self
            .repo_root()
            .unwrap_or_else(|| self.files.tree_root.clone());
        let git = self.services.git;
        let key = format!("git:history-files:{oid}");
        let job_oid = oid.clone();
        let handle = self
            .services
            .jobs
            .spawn(JobPriority::High, Some(key), async move {
                match tokio::task::spawn_blocking(move || git.commit_changes(&root, &job_oid)).await
                {
                    Ok(result) => result,
                    Err(err) => Err(GitReadError::History(err.to_string())),
                }
            });
        self.history
            .pending_files
            .push(PendingCommitFiles { oid, handle });
    }

    /// Follow a path in the rendered commit details, not the current worktree.
    /// The History tree may not have fetched this commit's file list yet.
    pub(super) fn jump_commit_detail_file(&mut self, file_index: usize) {
        let Some((oid, path)) = crate::view::commit_detail::file_link_target(self, file_index)
        else {
            return;
        };
        self.preview_state.commit_link_focus = None;
        self.history.pending_link = Some((oid.clone(), path));
        self.history.expanded = true;
        self.history.expanded_commits.insert(oid.clone());
        self.session.focus = PaneFocus::Preview;
        if self.history.files.contains_key(&oid) {
            self.reveal_pending_commit_link();
        } else {
            self.request_commit_files(oid);
            self.session.status = "Loading historical file list…".into();
        }
    }

    /// Select the exact historical file in SCM before requesting its Diff.
    /// Keeping these in sync is important: diff_matches_selection rejects
    /// stale diffs associated with another commit or file.
    fn reveal_pending_commit_link(&mut self) {
        let Some((oid, path)) = self.history.pending_link.take() else {
            return;
        };
        let selected_oid = self.history_selected_oid();
        if !self.in_git_sidebar() || selected_oid.as_deref() != Some(oid.as_str()) {
            return; // User navigated away while the file list was loading.
        }
        let Some(commit_index) = self.history.commits.iter().position(|c| c.id == oid) else {
            return;
        };
        let Some(files) = self.history.files.get(&oid) else {
            return;
        };
        let Some(file_index) = files.iter().position(|entry| entry.path == path) else {
            self.session.status = format!("Historical file not found: {path}");
            return;
        };
        self.history.expanded_commits.insert(oid.clone());
        // Expand all ancestors so compact folders and ordinary trees both
        // reveal the target. No file is created or read from the worktree.
        let mut directory = String::new();
        for part in path
            .split('/')
            .take(path.split('/').count().saturating_sub(1))
        {
            if !directory.is_empty() {
                directory.push('/');
            }
            directory.push_str(part);
            self.history.expanded_dirs.insert(dir_key(&oid, &directory));
        }
        let target = self.scm_rows().iter().position(|row| {
            matches!(row, ScmRow::CommitFile { commit, index, .. }
                if *commit == commit_index && *index == file_index)
        });
        let Some(target) = target else {
            self.session.status = format!("Could not reveal historical file: {path}");
            return;
        };
        self.scm.scm_selected = target;
        self.scm.follow_selection = true;
        self.session.focus = PaneFocus::Preview;
        self.preview_state.preview_scroll = 0;
        self.request_diff_for_scm_selection();
    }

    pub(super) fn request_commit_details(&mut self, oid: String) {
        // A new history selection invalidates a queued link from another commit.
        if self
            .history
            .pending_link
            .as_ref()
            .is_some_and(|(id, _)| id != &oid)
        {
            self.history.pending_link = None;
        }
        self.services.jobs.cancel_key("git:commit-detail");
        self.services.jobs.cancel_key("git:diff");
        self.services.jobs.cancel_key("fs:preview");
        self.preview_state.pending_diff = None;
        self.preview_state.pending_preview = None;
        self.clear_diff_view();
        self.preview_state.preview = None;
        self.history.detail_files = None;
        self.preview_state.preview_mode = PreviewMode::File;
        self.preview_state.preview_scroll = 0;
        self.preview_state.preview_loading = true;
        let Some(root) = self.repo_root() else {
            self.preview_state.preview_loading = false;
            return;
        };
        let git = self.services.git;
        let query_oid = oid.clone();
        let handle = self.services.jobs.spawn(
            JobPriority::High,
            Some("git:commit-detail".into()),
            async move { Ok(git.write.commit_detail(&root, &query_oid).await?) },
        );
        self.history.pending_detail = Some((oid, handle));
    }

    /// Apply finished history / commit-file jobs. Returns whether anything changed.
    pub(super) fn poll_history(&mut self) -> bool {
        let mut changed = false;
        if let Some(((oid, _), result)) =
            pending_request::take_ready(&mut self.history.pending_detail, |(_, handle)| {
                handle.try_join()
            })
        {
            let selected = matches!(self.selected_scm_row(), Some(ScmRow::Commit { index })
                    if self.history.commits.get(index).is_some_and(|c| c.id == oid));
            if self.session.activity == ActivityTab::Git
                && (selected
                    || (self.scm_directory_selected()
                        && matches!(self.preview_state.preview_mode, PreviewMode::File)))
            {
                let content = match result {
                    Ok(Ok((detail, files))) => {
                        self.history.detail_files = Some((oid.clone(), files));
                        PreviewContent::Text {
                            bytes_read: detail.len(),
                            lines: detail.lines().map(str::to_owned).collect(),
                            truncated: false,
                        }
                    }
                    Ok(Err(err)) => PreviewContent::Error {
                        message: err.to_string(),
                    },
                    Err(_) => PreviewContent::Error {
                        message: "Commit details read cancelled".into(),
                    },
                };
                self.preview_state.preview_loading = false;
                self.preview_state.preview = Some(forge_fs::Preview {
                    path: PathBuf::from(&oid),
                    content,
                });
            }
            changed = true;
        }
        if let Some((_, result)) =
            pending_request::take_ready(&mut self.history.pending_graph, |pending| {
                pending.handle.try_join()
            })
        {
            self.apply_history_result(result);
            changed = true;
        }

        let finished = pending_request::drain_ready(&mut self.history.pending_files, |pending| {
            pending.handle.try_join()
        });
        for (pending, result) in finished {
            let oid = pending.oid;
            match result {
                Ok(Ok(changes)) => {
                    self.history.files.insert(oid, changes);
                    if self
                        .history
                        .pending_link
                        .as_ref()
                        .is_some_and(|(pending_oid, _)| {
                            self.history.files.contains_key(pending_oid)
                        })
                    {
                        self.reveal_pending_commit_link();
                    }
                }
                Ok(Err(err)) => {
                    if self
                        .history
                        .pending_link
                        .as_ref()
                        .is_some_and(|(pending_oid, _)| pending_oid == &oid)
                    {
                        self.history.pending_link = None;
                    }
                    self.history.message = format!("commit files: {err}");
                }
                Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
            }
            changed = true;
        }
        changed
    }

    fn apply_history_result(
        &mut self,
        result: Result<Result<GraphSnapshot, GitReadError>, JobError>,
    ) {
        let anchor = self.history_selected_oid();
        match result {
            Ok(Ok(snapshot)) => {
                self.history.branch = snapshot.branch;
                self.history.upstream = snapshot.upstream;
                self.history.unpushed = snapshot.unpushed;
                self.history.incoming = snapshot.incoming;
                self.history.ahead = snapshot.ahead;
                self.history.behind = snapshot.behind;
                self.history.counts_truncated = snapshot.counts_truncated;
                self.history.commits = snapshot.commits;
                self.history.layout = layout_graph(&self.history.commits);
                self.history.exhausted = self.history.commits.len() < self.history.limit;
                self.history.loaded = true;
                self.history.message.clear();
                self.reselect_history(anchor);
                if self.history.focus_pending {
                    self.history.focus_pending = false;
                    self.select_first_commit();
                }
            }
            Ok(Err(err)) => {
                self.history.message = err.to_string();
                self.history.loaded = true;
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    /// Commit id behind the selected history row, if any.
    fn history_selected_oid(&self) -> Option<String> {
        let row = self.selected_scm_row()?;
        self.history
            .commits
            .get(row.commit()?)
            .map(|commit| commit.id.clone())
    }

    /// Keep the same commit selected after the list changed underneath.
    fn reselect_history(&mut self, anchor: Option<String>) {
        let Some(oid) = anchor else {
            self.clamp_scm_selection();
            return;
        };
        let Some(commit) = self.history.commits.iter().position(|c| c.id == oid) else {
            self.clamp_scm_selection();
            return;
        };
        let rows = self.scm_rows();
        match rows.iter().position(|row| row.commit() == Some(commit)) {
            Some(index) => self.scm.scm_selected = index,
            None => self.clamp_scm_selection(),
        }
    }

    /// Expand / collapse the selected history row; open a file's diff.
    pub(super) fn toggle_history_row(&mut self, index: usize) {
        let Some(row) = self.scm_rows().get(index).cloned() else {
            return;
        };
        match row {
            ScmRow::HistorySection { .. } => self.toggle_history(),
            ScmRow::Commit { index: commit } => {
                let Some(oid) = self.history.commits.get(commit).map(|c| c.id.clone()) else {
                    return;
                };
                if self.history.expanded_commits.remove(&oid) {
                    self.clamp_scm_selection();
                    self.session.status = self.hint_bar();
                    return;
                }
                self.history.expanded_commits.insert(oid.clone());
                self.request_commit_files(oid);
            }
            ScmRow::CommitFolder {
                commit,
                path,
                expanded,
                ..
            } => {
                let Some(oid) = self.history.commits.get(commit).map(|c| c.id.clone()) else {
                    return;
                };
                let key = dir_key(&oid, &path);
                if expanded {
                    self.history.expanded_dirs.remove(&key);
                    let prefix = dir_key(&oid, &format!("{path}/"));
                    self.history
                        .expanded_dirs
                        .retain(|item| !item.starts_with(&prefix));
                } else {
                    let mut prefix = String::new();
                    for part in path.split('/') {
                        if !prefix.is_empty() {
                            prefix.push('/');
                        }
                        prefix.push_str(part);
                        self.history.expanded_dirs.insert(dir_key(&oid, &prefix));
                    }
                    let target = &path;
                    if let Some(i) = self.scm_rows().iter().position(|row| {
                        matches!(row, ScmRow::CommitFolder { commit: c, path: p, .. }
                            if *c == commit && p == target)
                    }) {
                        self.scm.scm_selected = i;
                    }
                }
            }
            ScmRow::CommitFile {
                commit,
                index: file,
                ..
            } => {
                let Some(commit) = self.history.commits.get(commit).cloned() else {
                    return;
                };
                let Some(change) = self
                    .history
                    .files
                    .get(&commit.id)
                    .and_then(|changes| changes.get(file))
                else {
                    self.session.status = "commit files still loading".into();
                    return;
                };
                self.request_commit_diff(commit.id, commit.short, change.path.clone());
                self.session.focus = PaneFocus::Preview;
            }
            _ => {}
        }
        self.session.status = self.hint_bar();
    }

    /// Left: collapse the selected history row, or climb to its parent.
    pub(super) fn collapse_history_row(&mut self, index: usize) {
        let rows = self.scm_rows();
        let Some(row) = rows.get(index).cloned() else {
            return;
        };
        match row {
            ScmRow::Commit { index: commit } => {
                if let Some(oid) = self.history.commits.get(commit).map(|c| c.id.clone()) {
                    self.history.expanded_commits.remove(&oid);
                    self.clamp_scm_selection();
                }
            }
            ScmRow::CommitFolder {
                commit,
                path,
                expanded,
                ..
            } => {
                if expanded {
                    if let Some(oid) = self.history.commits.get(commit).map(|c| c.id.clone()) {
                        self.history.expanded_dirs.remove(&dir_key(&oid, &path));
                    }
                } else if let Some(parent) = find_scm_parent_row(&rows, index) {
                    self.scm.scm_selected = parent;
                }
            }
            ScmRow::CommitFile { .. } => {
                if let Some(parent) = find_scm_parent_row(&rows, index) {
                    self.scm.scm_selected = parent;
                }
            }
            _ => {}
        }
        self.request_diff_for_scm_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "forge")
            .env("GIT_AUTHOR_EMAIL", "forge@example.test")
            .env("GIT_COMMITTER_NAME", "forge")
            .env("GIT_COMMITTER_EMAIL", "forge@example.test")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["checkout", "-q", "-b", "main"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "one\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "feat: base"]);
        std::fs::write(root.join("src/a.rs"), "one\ntwo\n").unwrap();
        git(root, &["commit", "-qam", "fix: add a line"]);
        dir
    }

    fn app(root: &std::path::Path) -> App {
        let mut app = create_workspace_at(forge_theme::ColorMode::TrueColor, root.to_owned());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.session.activity = ActivityTab::Git;
        app.history.expanded = true;
        app.scm.git_status = app.services.git.status(root).ok();
        app
    }

    fn commit(id: &str, subject: &str) -> GraphCommit {
        GraphCommit {
            id: id.into(),
            short: id.chars().take(7).collect(),
            parents: Vec::new(),
            refs: Vec::new(),
            author: "forge".into(),
            time: 0,
            subject: subject.into(),
        }
    }

    fn change(path: &str) -> CommitChange {
        CommitChange {
            path: path.into(),
            old_path: None,
            kind: forge_git::CommitChangeKind::Modified,
        }
    }

    /// Drive history + commit-file + diff jobs until nothing is in flight.
    async fn settle(app: &mut App) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                app.poll_history();
                app.poll_pending_diff();
                let busy = app.history.pending_graph.is_some()
                    || app.history.pending_detail.is_some()
                    || !app.history.pending_files.is_empty()
                    || app.preview_state.pending_diff.is_some();
                if !busy {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sidebar_history_loads_older_graph_pages_only_near_end() {
        let dir = repo();
        // Slightly more than the initial window: exercise real Git graph
        // integration, lane layout, asynchronous paging, and selection.
        for _ in 0..210 {
            git(
                dir.path(),
                &["commit", "-q", "--allow-empty", "-m", "older history"],
            );
        }
        let mut app = app(dir.path());
        app.request_history();
        settle(&mut app).await;
        assert_eq!(app.history.limit, HISTORY_PAGE_SIZE);
        assert_eq!(app.history.commits.len(), HISTORY_PAGE_SIZE);
        assert!(!app.history.exhausted);
        assert!(app.history.pending_graph.is_none());

        // Merely opening History or selecting a newer commit must not
        // eagerly scan the rest of a large repository.
        app.scm.scm_selected = commit_row(&app).unwrap();
        app.maybe_page_history();
        assert!(app.history.pending_graph.is_none());

        let last = app.scm_rows().iter().position(|row|
            matches!(row, ScmRow::Commit { index } if *index + 1 == app.history.commits.len())
        ).unwrap();
        app.scm.scm_selected = last;
        let selected_oid = app.history_selected_oid().unwrap();
        app.maybe_page_history();
        assert_eq!(app.history.limit, HISTORY_PAGE_SIZE * 2);
        assert!(app.history.pending_graph.is_some());
        settle(&mut app).await;
        assert_eq!(app.history.commits.len(), 212);
        assert!(app.history.exhausted);
        assert_eq!(app.history_selected_oid().unwrap(), selected_oid);
        assert!(app.history.pending_graph.is_none());

        app.maybe_page_history();
        assert!(
            app.history.pending_graph.is_none(),
            "exhausted history must not rescan"
        );
    }

    #[tokio::test]
    async fn worktree_refresh_skips_history_unless_refs_move() {
        let dir = repo();
        let mut app = app(dir.path());
        app.request_history();
        settle(&mut app).await;
        assert!(app.history.loaded);

        app.refresh_external_changes(false);
        assert!(
            app.history.pending_graph.is_none(),
            "worktree writes must not reload the commit graph"
        );

        app.refresh_external_changes(true);
        assert!(
            app.history.pending_graph.is_some(),
            "HEAD/ref changes must reload the commit graph"
        );
        settle(&mut app).await;
    }

    fn commit_row(app: &App) -> Option<usize> {
        app.scm_rows().iter().position(ScmRow::is_commit)
    }

    fn row<F: Fn(&ScmRow) -> bool>(app: &App, predicate: F) -> Option<usize> {
        app.scm_rows().iter().position(predicate)
    }

    #[tokio::test]
    async fn expanding_commit_nests_folders_and_collapses_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        let oid = "a".repeat(40);
        app.history.seed(vec![
            commit(&oid, "feat: first"),
            commit("b", "feat: second"),
        ]);
        app.history
            .files
            .insert(oid.clone(), vec![change("src/a.rs"), change("README.md")]);

        let commit_index = commit_row(&app).unwrap();
        assert_eq!(app.scm.scm_selected, 0);
        app.toggle_history_row(commit_index);
        assert!(app.history.expanded_commits.contains(&oid));
        let rows = app.scm_rows();
        assert!(rows.iter().any(
            |row| matches!(row, ScmRow::CommitFolder { name, depth: 1, .. } if name == "src")
        ));
        assert!(rows.iter().filter(|row| row.is_commit()).count() == 2);

        // Open `src` so the file shows up one level deeper.
        let folder = row(&app, |row| {
            matches!(row, ScmRow::CommitFolder { name, expanded: false, .. } if name == "src")
        })
        .unwrap();
        app.toggle_history_row(folder);
        let file = row(&app, ScmRow::is_commit_file).unwrap();
        assert!(matches!(
            app.scm_rows()[file],
            ScmRow::CommitFile { depth: 2, .. }
        ));

        // Left on the file climbs to its folder, then collapses it.
        app.scm.scm_selected = file;
        app.collapse_history_row(file);
        assert!(matches!(
            app.selected_scm_row(),
            Some(ScmRow::CommitFolder { name, .. }) if name == "src"
        ));
        app.collapse_history_row(app.scm.scm_selected);
        assert!(!app.history.expanded_dirs.contains(&dir_key(&oid, "src")));

        // Left on the collapsed folder climbs to the commit, then collapses it.
        app.collapse_history_row(app.scm.scm_selected);
        assert!(app.selected_scm_row().is_some_and(|row| row.is_commit()));
        app.collapse_history_row(app.scm.scm_selected);
        assert!(!app.history.expanded_commits.contains(&oid));
        assert_eq!(
            app.scm_rows().iter().filter(|row| row.is_commit()).count(),
            2
        );
    }

    #[tokio::test]
    async fn compact_commit_folder_expands_to_files_in_one_click() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        let oid = "a".repeat(40);
        app.history.seed(vec![commit(&oid, "one")]);
        app.history.files.insert(
            oid.clone(),
            vec![
                change("crates/forge-config/src/lib.rs"),
                change("crates/forge-config/src/mod.rs"),
            ],
        );
        app.history.expanded_commits.insert(oid.clone());
        let folder = row(&app, |row| {
            matches!(row,
            ScmRow::CommitFolder { path, expanded: false, .. }
                if path == "crates/forge-config/src")
        })
        .unwrap();
        assert!(
            matches!(&app.scm_rows()[folder], ScmRow::CommitFolder { name, .. } if name == "crates / forge-config / src")
        );
        app.toggle_history_row(folder);
        assert!(app.history.expanded_dirs.contains(&dir_key(&oid, "crates")));
        assert!(app
            .history
            .expanded_dirs
            .contains(&dir_key(&oid, "crates/forge-config")));
        assert!(app
            .history
            .expanded_dirs
            .contains(&dir_key(&oid, "crates/forge-config/src")));
        let rows = app.scm_rows();
        let merged = rows
            .iter()
            .position(|row| {
                matches!(row,
            ScmRow::CommitFolder { path, name, expanded: true, .. }
                if path == "crates/forge-config/src"
                && name == "crates / forge-config / src")
            })
            .unwrap();
        assert_eq!(app.scm.scm_selected, merged);
        assert_eq!(rows.iter().filter(|row| row.is_commit_file()).count(), 2);
        app.toggle_history_row(merged);
        assert!(
            matches!(&app.scm_rows()[merged], ScmRow::CommitFolder { path, name, expanded: false, .. } if path == "crates/forge-config/src" && name == "crates / forge-config / src")
        );
        app.config.compact_folders = false;
        assert!(app.scm_rows().iter().any(|r| matches!(r, ScmRow::CommitFolder { path, name, .. } if path == "crates" && name == "crates")));
        app.config.compact_folders = true;
        assert!(!app
            .history
            .expanded_dirs
            .contains(&dir_key(&oid, "crates/forge-config/src")));
        assert_eq!(
            app.scm_rows()
                .iter()
                .filter(|row| row.is_commit_file())
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn history_section_collapses_and_expands() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.history.seed(vec![commit("a", "one")]);
        let header = app
            .scm_rows()
            .iter()
            .position(|row| matches!(row, ScmRow::HistorySection { .. }))
            .unwrap();
        app.scm.scm_selected = header;
        app.toggle_history_row(header);
        assert!(!app.history.expanded);
        assert_eq!(
            app.scm_rows().iter().filter(|row| row.is_commit()).count(),
            0
        );
        app.toggle_history_row(header);
        assert!(app.history.expanded);
        assert_eq!(
            app.scm_rows().iter().filter(|row| row.is_commit()).count(),
            1
        );
    }

    #[tokio::test]
    async fn clicking_commit_detail_link_loads_history_and_opens_the_commit_diff() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        let oid = app.history.commits[0].id.clone();
        assert_eq!(
            app.preview_state
                .preview
                .as_ref()
                .unwrap()
                .path
                .to_string_lossy(),
            oid
        );
        assert!(
            !app.history.files.contains_key(&oid),
            "test the lazy-load path"
        );

        let mut terminal = Terminal::new(TestBackend::new(110, 28)).unwrap();
        let area = Rect::new(5, 1, 100, 24);
        terminal
            .draw(|frame| {
                app.commit_file_links.clear();
                assert!(crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette()
                ));
            })
            .unwrap();
        let (link_area, index) = app
            .commit_file_links
            .iter()
            .next()
            .copied()
            .expect("one link");
        assert_eq!(index, 0);
        assert_eq!(
            app.commit_file_links
                .action_at(link_area.x.saturating_sub(1), link_area.y),
            None
        );
        assert_eq!(
            app.commit_file_links.action_at(link_area.x, link_area.y),
            Some(0)
        );
        let before = terminal.backend().buffer()[(link_area.x, link_area.y)].bg;
        app.commit_file_links.hover_at(link_area.x, link_area.y);
        terminal
            .draw(|frame| {
                app.commit_file_links.clear();
                assert!(crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette()
                ));
            })
            .unwrap();
        assert_ne!(
            terminal.backend().buffer()[(link_area.x, link_area.y)].bg,
            before
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: link_area.x,
            row: link_area.y,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_content(click);
        assert!(app.history.pending_link.is_some());
        assert!(app.history.expanded_commits.contains(&oid));
        settle(&mut app).await;
        assert!(app.history.pending_link.is_none());
        let row = app.selected_scm_row().expect("selected history row");
        assert!(matches!(row, ScmRow::CommitFile { .. }), "{row:?}");
        assert_eq!(app.session.focus, PaneFocus::Preview);
        let diff = app
            .preview_state
            .file_diff
            .as_ref()
            .expect("historical diff");
        assert_eq!(diff.path, "src/a.rs");
        assert!(diff.lines.iter().any(|line| line.text == "+two"));
        assert!(matches!(&app.preview_state.diff_source,
            DiffSource::Commit { oid: target, .. } if target == &oid));
    }

    #[tokio::test]
    async fn keyboard_tabs_focus_commit_files_and_enter_opens_the_selected_historical_diff() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        let oid = app.history.commits[0].id.clone();
        app.session.focus = PaneFocus::Preview;
        assert_eq!(crate::view::commit_detail::file_link_count(&app), 1);
        let area = Rect::new(5, 1, 100, 24);
        let mut terminal = Terminal::new(TestBackend::new(110, 28)).unwrap();
        terminal
            .draw(|frame| {
                app.commit_file_links.clear();
                crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette(),
                );
            })
            .unwrap();
        let (link, _) = app.commit_file_links.iter().next().copied().unwrap();
        let before = terminal.backend().buffer()[(link.x, link.y)].bg;
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.preview_state.commit_link_focus, Some(0));
        assert_eq!(app.session.focus, PaneFocus::Preview);
        terminal
            .draw(|frame| {
                app.commit_file_links.clear();
                crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette(),
                );
            })
            .unwrap();
        assert_ne!(terminal.backend().buffer()[(link.x, link.y)].bg, before);
        app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.preview_state.commit_link_focus, None);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.history.pending_link.is_some());
        assert!(app.history.expanded_commits.contains(&oid));
        settle(&mut app).await;
        assert!(matches!(
            app.selected_scm_row(),
            Some(ScmRow::CommitFile { .. })
        ));
        assert_eq!(
            app.preview_state.file_diff.as_ref().unwrap().path,
            "src/a.rs"
        );
        assert_eq!(app.preview_state.commit_link_focus, None);
    }

    #[tokio::test]
    async fn commit_link_keyboard_cycles_and_scrolls_to_offscreen_files() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        for i in 0..16 {
            std::fs::write(
                dir.path().join(format!("item-{i:02}.txt")),
                format!(
                    "item {i}
"
                ),
            )
            .unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "multi-file navigation"]);
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        app.session.focus = PaneFocus::Preview;
        let count = crate::view::commit_detail::file_link_count(&app);
        assert!(count >= 16);
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        for i in 0..count {
            app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
            assert_eq!(app.preview_state.commit_link_focus, Some(i));
        }
        // Last off-screen file gains keyboard focus and becomes visible.
        let area = Rect::new(4, 1, 74, 8);
        let mut terminal = Terminal::new(TestBackend::new(84, 16)).unwrap();
        terminal
            .draw(|frame| {
                app.commit_file_links.clear();
                assert!(crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette()
                ));
            })
            .unwrap();
        assert!(app.preview_state.preview_scroll > 0);
        assert!(
            app.commit_file_links
                .iter()
                .any(|(_, index)| *index == count - 1),
            "keyboard-focused offscreen commit file must be scrolled into view"
        );
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.preview_state.commit_link_focus, Some(0));
        app.handle_key(key(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.preview_state.commit_link_focus, None);
        assert_eq!(app.session.focus, PaneFocus::List);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.session.focus, PaneFocus::Preview);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.preview_state.commit_link_focus, Some(0));
    }

    #[tokio::test]
    async fn long_history_links_reveal_exact_paths_even_with_identical_suffixes_and_renames() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        let suffix = "very_long_directory_name_for_git_stat_truncation/repeated/subdirectory/same_filename.rs";
        let old = format!("old/{suffix}");
        let new = format!("new/{suffix}");
        let other = format!("other/{suffix}");
        for path in [&old, &other] {
            let target = dir.path().join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(
                target,
                if path == &old {
                    "original\n"
                } else {
                    "different\n"
                },
            )
            .unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "add long paths"]);
        std::fs::create_dir_all(dir.path().join(&new).parent().unwrap()).unwrap();
        git(dir.path(), &["mv", &old, &new]);
        std::fs::write(dir.path().join(&other), "different\nupdated\n").unwrap();
        git(
            dir.path(),
            &["commit", "-qam", "rename and modify long paths"],
        );
        for expected in [&new, &other] {
            let mut app = app(dir.path());
            app.show_history();
            settle(&mut app).await;
            let area = Rect::new(2, 1, 50, 24);
            app.session.layout.preview = area;
            app.session.layout.preview_inner = area;
            let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
            term.draw(|frame| {
                app.commit_file_links.clear();
                assert!(crate::view::commit_detail::draw(
                    frame,
                    area,
                    &mut app,
                    crate::view::workspace_palette()
                ));
            })
            .unwrap();
            let (link, _) = app
                .commit_file_links
                .iter()
                .find(|(_, index)| {
                    crate::view::commit_detail::file_link_target(&app, *index)
                        .is_some_and(|(_, path)| path == *expected)
                })
                .copied()
                .expect("canonical long-path link");
            assert_eq!(term.backend().buffer()[(link.x, link.y)].symbol(), "…");
            app.handle_mouse_content(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: link.x,
                row: link.y,
                modifiers: KeyModifiers::NONE,
            });
            settle(&mut app).await;
            let diff = app
                .preview_state
                .file_diff
                .as_ref()
                .expect("historical diff");
            assert_eq!(&diff.path, expected);
            assert!(
                matches!(
                    app.selected_scm_row(),
                    Some(ScmRow::CommitFile { depth: 2, .. })
                ),
                "expected {expected}, compact={}, selected={:?}, rows={:?}",
                app.config.compact_folders,
                app.selected_scm_row(),
                app.scm_rows()
            );
        }
    }

    #[tokio::test]
    async fn scrolled_commit_link_clicks_the_correct_file_not_its_stats() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        for i in 0..30 {
            std::fs::write(
                dir.path().join(format!("file-{i:02}.txt")),
                format!("file {i}\n"),
            )
            .unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "feat: many files"]);
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        app.preview_state.preview_scroll = 25;
        let area = Rect::new(4, 1, 76, 9);
        app.session.layout.preview = area;
        app.session.layout.preview_inner = area;
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|frame| {
            app.commit_file_links.clear();
            assert!(crate::view::commit_detail::draw(
                frame,
                area,
                &mut app,
                crate::view::workspace_palette()
            ));
        })
        .unwrap();
        let (link, index) = app
            .commit_file_links
            .iter()
            .next()
            .copied()
            .expect("scrolled link");
        assert!(index > 0, "must use a file beyond the first visible entry");
        let (oid, expected_path) =
            crate::view::commit_detail::file_link_target(&app, index).unwrap();
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: link.y,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_content(click(area.right() - 2)); // +/- stat column
        assert!(app.history.pending_link.is_none());
        assert!(matches!(
            app.selected_scm_row(),
            Some(ScmRow::Commit { .. })
        ));
        app.handle_mouse_content(click(link.x));
        settle(&mut app).await;
        let diff = app
            .preview_state
            .file_diff
            .as_ref()
            .expect("correct historical diff");
        assert_eq!(diff.path, expected_path);
        assert!(matches!(&app.preview_state.diff_source,
            DiffSource::Commit { oid: loaded, .. } if loaded == &oid));
    }

    #[tokio::test]
    async fn commit_detail_link_respects_visible_rows_and_scroll() {
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let dir = repo();
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        let area = Rect::new(5, 1, 75, 8);
        let mut term = Terminal::new(TestBackend::new(90, 20)).unwrap();
        term.draw(|frame| {
            app.commit_file_links.clear();
            assert!(crate::view::commit_detail::draw(
                frame,
                area,
                &mut app,
                crate::view::workspace_palette()
            ));
        })
        .unwrap();
        // A short viewport scrolls the file path into view; all hit areas must
        // remain within that viewport rather than retaining unscrolled rows.
        for (region, _) in app.commit_file_links.iter() {
            assert!(region.x >= area.x && region.right() <= area.right());
            assert!(region.y >= area.y && region.bottom() <= area.bottom());
        }
        app.preview_state.preview_scroll = 100;
        term.draw(|frame| {
            app.commit_file_links.clear();
            assert!(crate::view::commit_detail::draw(
                frame,
                area,
                &mut app,
                crate::view::workspace_palette()
            ));
        })
        .unwrap();
        assert!(app
            .commit_file_links
            .iter()
            .all(|(rect, _)| rect.y >= area.y && rect.bottom() <= area.bottom()));
    }

    #[tokio::test]
    async fn commit_file_opens_a_read_only_diff_in_the_preview() {
        let dir = repo();
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        assert!(app.history.loaded, "{}", app.history.message);
        assert_eq!(app.history.commits.len(), 2);
        assert_eq!(app.history.branch, "main");
        // `G` focuses the newest commit as soon as the list arrives.
        assert!(matches!(
            app.selected_scm_row(),
            Some(ScmRow::Commit { index: 0 })
        ));

        // Expand HEAD and its src/ folder, then open the changed file.
        let commit_index = app.scm.scm_selected;
        app.toggle_history_row(commit_index);
        settle(&mut app).await;
        let oid = app.history.commits[0].id.clone();
        assert!(
            app.history.files.contains_key(&oid),
            "{}",
            app.history.message
        );
        let folder = row(
            &app,
            |row| matches!(row, ScmRow::CommitFolder { name, .. } if name == "src"),
        )
        .unwrap();
        app.toggle_history_row(folder);
        let file = row(&app, ScmRow::is_commit_file).unwrap();
        app.scm.scm_selected = file;
        app.toggle_history_row(file);
        settle(&mut app).await;

        let diff = app.preview_state.file_diff.clone().expect("diff loaded");
        assert_eq!(diff.path, "src/a.rs");
        assert!(
            diff.lines.iter().any(|line| line.text == "+two"),
            "{:?}",
            diff.lines
        );
        assert!(matches!(
            &app.preview_state.diff_source,
            DiffSource::Commit { oid: loaded, .. } if *loaded == oid
        ));
        assert_eq!(app.preview_state.diff_label, app.history.commits[0].short);
        assert!(app.diff_matches_selection(&diff));
        assert_eq!(app.session.focus, PaneFocus::Preview);

        // Staging a historical hunk is rejected instead of touching the index.
        let err = match app.stage_target(forge_ui::ContextAction::StageHunk) {
            Ok(_) => panic!("history hunk must not be stageable"),
            Err(err) => err,
        };
        assert!(err.contains("read-only"), "{err}");
        assert!(app.scm.pending_write.is_none());
    }

    #[tokio::test]
    async fn selected_commit_shows_full_message_and_ignores_stale_details() {
        let dir = repo();
        git(
            dir.path(),
            &[
                "commit",
                "--allow-empty",
                "-qm",
                "Detailed subject\n\nFull body line one\nFull body line two",
            ],
        );
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        let oid = app.history.commits[0].id.clone();
        let Some(Preview {
            content: PreviewContent::Text { lines, .. },
            ..
        }) = &app.preview_state.preview
        else {
            panic!("commit details expected");
        };
        let text = lines.join("\n");
        for expected in [
            &oid,
            "Author:",
            "Commit:",
            "AuthorDate:",
            "CommitDate:",
            "Full body line one",
            "Full body line two",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(app.preview_state.file_diff.is_none());
        // Keyboard navigation loads the next commit, never retaining HEAD's body.
        app.move_scm(1);
        settle(&mut app).await;
        let preview = app.preview_state.preview.as_ref().unwrap();
        assert_ne!(preview.path, PathBuf::from(&oid));
        // A pending detail read cannot replace a different selection's preview.
        app.request_commit_details(oid);
        app.scm.scm_selected = 0;
        app.request_diff_for_scm_selection();
        settle(&mut app).await;
        assert!(app.preview_state.file_diff.is_none());
        assert!(!app.preview_state.preview_loading);
    }

    #[tokio::test]
    async fn selecting_commit_folder_keeps_styled_commit_detail_preview() {
        use ratatui::{backend::TestBackend, Terminal};
        let dir = repo();
        let mut app = app(dir.path());
        app.show_history();
        settle(&mut app).await;
        let commit_row = app.scm.scm_selected;
        let oid = app.history.commits[0].id.clone();
        // Load children without changing the currently displayed detail.
        app.toggle_history_row(commit_row);
        settle(&mut app).await;
        let before_path = app.preview_state.preview.as_ref().unwrap().path.clone();
        assert_eq!(before_path, std::path::PathBuf::from(&oid));

        let render = |app: &mut App| {
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            let mut used_commit_renderer = false;
            term.draw(|frame| {
                used_commit_renderer = crate::view::commit_detail::draw(
                    frame,
                    ratatui::layout::Rect::new(0, 0, 100, 30),
                    app,
                    crate::view::workspace_palette(),
                );
            })
            .unwrap();
            (
                used_commit_renderer,
                term.backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<Vec<_>>()
                    .join(""),
            )
        };
        let (initial, initial_text) = render(&mut app);
        assert!(initial);
        assert!(initial_text.contains("CHANGED FILES"));

        let folder = app
            .scm_rows()
            .iter()
            .position(|row| matches!(row, ScmRow::CommitFolder { commit: 0, .. }))
            .expect("commit folder");
        app.scm.scm_selected = folder;
        app.request_diff_for_scm_selection();
        assert_eq!(
            app.preview_state.preview.as_ref().unwrap().path,
            before_path
        );
        let (folder_rendered, text) = render(&mut app);
        assert!(folder_rendered, "folder must retain commit detail renderer");
        assert!(text.contains("CHANGED FILES"));
        assert!(text.contains("fix: add a line"));

        app.toggle_history_row(folder);
        let (expanded_rendered, _) = render(&mut app);
        assert!(
            expanded_rendered,
            "expanding folder must not replace commit details"
        );
    }

    #[tokio::test]
    async fn history_is_empty_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.request_history();
        settle(&mut app).await;
        assert!(app.history.loaded);
        assert!(app.history.commits.is_empty());
        assert!(!app.history.message.is_empty());
        assert_eq!(
            app.scm_rows().iter().filter(|row| row.is_commit()).count(),
            0
        );
    }
}

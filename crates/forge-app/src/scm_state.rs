//! Git sidebar selection and asynchronous transfer ownership.
//! Inputs are Git snapshots and a borrowed history presentation, never App.
use super::*;
use forge_ui::HistoryView;
use std::sync::{Arc, Mutex};

pub(super) struct ScmState {
    pub(super) pending_status: Option<PendingStatus>,
    /// A refresh arrived while a status job was already running. Blocking
    /// workers cannot be aborted, so re-run only after the current one ends.
    status_rerun: bool,
    last_status_request: Option<Instant>,
    pub(super) pending_write: Option<PendingWrite>,
    git_status: Option<StatusSnapshot>,
    row_cache: Mutex<Option<RowCache>>,
    pub(super) branch: String,
    pub(super) scm_selected: usize,
    pub(super) scroll_scm: usize,
    pub(super) follow_selection: bool,
    /// When a whole file moves between Changes and Staged, follow its path
    /// after the asynchronous Git snapshot, not the old numeric row index.
    pending_transfer: Option<(String, StagingColumn)>,
    pub(super) last_scm_click: Option<(Instant, usize)>,
    scm_expanded_unstaged: HashSet<String>,
    scm_expanded_staged: HashSet<String>,
    pub(super) sticky_headers: Vec<usize>,
}

struct RowCache {
    history_revision: u64,
    history_loading: bool,
    compact: bool,
    rows: Arc<[ScmRow]>,
}

#[derive(Clone)]
pub(super) struct ScmNavigation {
    staged: HashSet<String>,
    unstaged: HashSet<String>,
    selected: usize,
    scroll: usize,
}

impl ScmState {
    pub(super) fn new() -> Self {
        Self {
            git_status: None,
            row_cache: Mutex::new(None),
            branch: String::new(),
            scm_selected: 0,
            scroll_scm: 0,
            follow_selection: true,
            pending_transfer: None,
            last_scm_click: None,
            scm_expanded_unstaged: HashSet::new(),
            scm_expanded_staged: HashSet::new(),
            sticky_headers: Vec::new(),
            pending_status: None,
            status_rerun: false,
            last_status_request: None,
            pending_write: None,
        }
    }

    pub(super) fn status(&self) -> &Option<StatusSnapshot> {
        &self.git_status
    }

    pub(super) fn set_status(&mut self, snapshot: Option<StatusSnapshot>) {
        self.git_status = snapshot;
        self.invalidate_rows();
    }

    #[cfg(test)]
    pub(super) fn test_status_mut(&mut self) -> &mut Option<StatusSnapshot> {
        self.invalidate_rows();
        &mut self.git_status
    }

    pub(super) fn expanded(&self, column: StagingColumn) -> &HashSet<String> {
        match column {
            StagingColumn::Unstaged => &self.scm_expanded_unstaged,
            StagingColumn::Staged => &self.scm_expanded_staged,
        }
    }

    pub(super) fn invalidate_rows(&mut self) {
        *self.row_cache.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Reuse the same flattened projection across keyboard, mouse and
    /// drawing. The only cached value is the rendered row structure.
    pub(super) fn cached_rows(
        &self,
        history: &HistoryView<'_>,
        revision: u64,
        compact: bool,
    ) -> Arc<[ScmRow]> {
        let mut cache = self.row_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cache.as_ref().filter(|c| {
            c.history_revision == revision
                && c.history_loading == history.loading
                && c.compact == compact
        }) {
            return Arc::clone(&cached.rows);
        }
        let rows: Arc<[ScmRow]> = self.rows(history, compact).into();
        *cache = Some(RowCache {
            history_revision: revision,
            history_loading: history.loading,
            compact,
            rows: Arc::clone(&rows),
        });
        rows
    }

    pub(super) fn navigation_snapshot(&self) -> ScmNavigation {
        ScmNavigation {
            staged: self.scm_expanded_staged.clone(),
            unstaged: self.scm_expanded_unstaged.clone(),
            selected: self.scm_selected,
            scroll: self.scroll_scm,
        }
    }

    pub(super) fn restore_workspace(&mut self, restore: Option<&ScmNavigation>) {
        self.scm_expanded_staged = restore.map(|n| n.staged.clone()).unwrap_or_default();
        self.scm_expanded_unstaged = restore.map(|n| n.unstaged.clone()).unwrap_or_default();
        self.scm_selected = restore.map_or(0, |n| n.selected);
        self.scroll_scm = restore.map_or(0, |n| n.scroll);
        self.follow_selection = true;
        self.sticky_headers.clear();
        self.last_scm_click = None;
        self.cancel_transfer();
        self.set_status(None);
        self.branch.clear();
        // Do not drop an in-flight status handle or its rerun flag: the
        // blocking worker must complete, then the new root is refreshed.
    }

    pub(super) fn follow_transfer(&mut self, transfer: Option<(String, StagingColumn)>) {
        self.pending_transfer = transfer;
    }

    pub(super) fn cancel_transfer(&mut self) {
        self.pending_transfer = None;
    }

    #[cfg(test)]
    pub(super) fn transfer_pending(&self) -> bool {
        self.pending_transfer.is_some()
    }

    pub(super) fn note_status_request(&mut self, now: Instant) {
        self.last_status_request = Some(now);
    }

    /// Reconcile changes outside shallow watches without depending on layout.
    /// At most one status scan per interval, never queue a periodic rerun.
    pub(super) fn periodic_status_due(&self, now: Instant) -> bool {
        self.git_status.is_some()
            && self.pending_status.is_none()
            && self
                .last_status_request
                .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(15))
    }

    pub(super) fn matches_snapshot(&self, snapshot: &StatusSnapshot, branch: &str) -> bool {
        self.branch == branch
            && self.git_status.as_ref().is_some_and(|old| {
                old.repo_root == snapshot.repo_root
                    && old.entries == snapshot.entries
                    && old.unstaged == snapshot.unstaged
                    && old.staged == snapshot.staged
            })
    }

    pub(super) fn retain_selected_row(
        &mut self,
        previous: Option<&ScmRow>,
        entry: Option<&forge_git::ChangeEntry>,
        history: &HistoryView<'_>,
        compact: bool,
    ) -> bool {
        let rows = self.rows(history, compact);
        let (u, s) = self.slices();
        let found = rows.iter().position(|row| {
            if let Some(entry) = entry {
                return scm_entry(row, u, s)
                    .is_some_and(|e| e.path == entry.path && e.side == entry.side);
            }
            match (previous, row) {
                (Some(ScmRow::Section { column: old, .. }), ScmRow::Section { column, .. }) => {
                    old == column
                }
                (
                    Some(ScmRow::Folder {
                        column: old_col,
                        path: old,
                        ..
                    }),
                    ScmRow::Folder { column, path, .. },
                ) => old_col == column && old == path,
                (Some(ScmRow::HistorySection { .. }), ScmRow::HistorySection { .. }) => true,
                (Some(old), new) => old == new,
                (None, _) => false,
            }
        });
        if let Some(index) = found {
            self.scm_selected = index;
            true
        } else {
            false
        }
    }

    pub(super) fn defer_status_refresh(&mut self) {
        self.status_rerun = true;
    }

    pub(super) fn take_deferred_status_refresh(&mut self) -> bool {
        std::mem::take(&mut self.status_rerun)
    }

    #[cfg(test)]
    pub(super) fn status_refresh_deferred(&self) -> bool {
        self.status_rerun
    }

    pub(super) fn slices(&self) -> (&[forge_git::ChangeEntry], &[forge_git::ChangeEntry]) {
        match &self.git_status {
            Some(snap) => (snap.unstaged.as_slice(), snap.staged.as_slice()),
            None => (&[], &[]),
        }
    }

    pub(super) fn rows(&self, history: &HistoryView<'_>, compact: bool) -> Vec<ScmRow> {
        let (u, s) = self.slices();
        build_scm_rows_with_compact(
            u,
            s,
            &self.scm_expanded_unstaged,
            &self.scm_expanded_staged,
            history,
            compact,
        )
    }

    #[cfg(test)]
    pub(super) fn selected_entry(
        &self,
        history: &HistoryView<'_>,
        compact: bool,
    ) -> Option<&forge_git::ChangeEntry> {
        let rows = self.rows(history, compact);
        let row = rows.get(self.scm_selected)?;
        let (u, s) = self.slices();
        scm_entry(row, u, s)
    }

    pub(super) fn clamp_selection(&mut self, history: &HistoryView<'_>, compact: bool) {
        self.scm_selected = self
            .scm_selected
            .min(self.rows(history, compact).len().saturating_sub(1));
    }

    pub(super) fn expanded_mut(&mut self, column: StagingColumn) -> &mut HashSet<String> {
        self.invalidate_rows();
        match column {
            StagingColumn::Unstaged => &mut self.scm_expanded_unstaged,
            StagingColumn::Staged => &mut self.scm_expanded_staged,
        }
    }

    pub(super) fn reveal_change(
        &mut self,
        path: &str,
        column: StagingColumn,
        history: &HistoryView<'_>,
        compact: bool,
    ) -> bool {
        expand_ancestors_for_path(self.expanded_mut(column), path);
        let rows = self.rows(history, compact);
        let (u, s) = self.slices();
        if let Some(index) = rows.iter().position(|row| {
            scm_entry(row, u, s).is_some_and(|e| e.path == path && e.side == column.side())
        }) {
            self.scm_selected = index;
            true
        } else {
            false
        }
    }

    /// A snapshot from before an outstanding write must not consume its jump.
    pub(super) fn reconcile_transfer(&mut self, history: &HistoryView<'_>, compact: bool) {
        if self.pending_write.is_some() {
            return;
        }
        let Some((path, side)) = self.pending_transfer.clone() else {
            return;
        };
        let (u, s) = self.slices();
        let has_target = u
            .iter()
            .chain(s.iter())
            .any(|e| e.path == path && e.side == side.side());
        if has_target {
            if self.reveal_change(&path, side, history, compact) {
                self.follow_selection = true;
                self.pending_transfer = None;
            }
        } else if !self.status_rerun {
            self.pending_transfer = None;
        }
    }
}

impl App {
    pub(super) fn periodic_git_status_due(&self, now: Instant) -> bool {
        !self.editing.saving()
            && self.scm.pending_write.is_none()
            && self.tools.pending_task.is_none()
            && self.scm.periodic_status_due(now)
    }
    pub(super) fn tick_git_status_at(&mut self, now: Instant) {
        if self.periodic_git_status_due(now) {
            self.request_status_with_reason(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_model::GitStatus;

    #[test]
    fn row_cache_reuses_projection_and_invalidates_all_structural_inputs() {
        let mut scm = ScmState::new();
        let mut history = history::HistoryState::default();
        let get = |scm: &ScmState, history: &history::HistoryState, compact| {
            let rows = scm.cached_rows(&history.view(), history.rows_revision(), compact);
            assert_eq!(&*rows, &scm.rows(&history.view(), compact));
            rows
        };
        let a = get(&scm, &history, true);
        scm.scm_selected = 1;
        scm.scroll_scm = 1;
        assert!(Arc::ptr_eq(&a, &get(&scm, &history, true)));
        let mut snapshot = StatusSnapshot::default();
        snapshot.push_change("folder/deep/a.txt", GitStatus::Modified, DiffSide::Unstaged);
        scm.set_status(Some(snapshot));
        let b = get(&scm, &history, true);
        assert!(!Arc::ptr_eq(&a, &b));
        scm.expanded_mut(StagingColumn::Unstaged)
            .insert("folder".into());
        let c = get(&scm, &history, true);
        assert!(!Arc::ptr_eq(&b, &c));
        history.set_expanded(false);
        let d = get(&scm, &history, true);
        assert!(!Arc::ptr_eq(&c, &d));
        let e = get(&scm, &history, false);
        assert!(!Arc::ptr_eq(&d, &e));
        scm.restore_workspace(None);
        assert!(!Arc::ptr_eq(&e, &get(&scm, &history, false)));
    }

    #[test]
    fn worktree_restore_preserves_navigation_not_status_or_pending_transfer() {
        let mut scm = ScmState::new();
        scm.scm_expanded_staged.insert("src".into());
        scm.scm_selected = 2;
        scm.scroll_scm = 1;
        scm.git_status = Some(StatusSnapshot::default());
        scm.follow_transfer(Some(("a.rs".into(), StagingColumn::Staged)));
        scm.defer_status_refresh();
        let snapshot = scm.navigation_snapshot();
        scm.restore_workspace(None);
        assert!(scm.git_status.is_none());
        assert!(!scm.transfer_pending());
        assert!(scm.status_refresh_deferred());
        assert!(scm.scm_expanded_staged.is_empty());
        scm.restore_workspace(Some(&snapshot));
        assert!(scm.scm_expanded_staged.contains("src"));
        assert_eq!(scm.scm_selected, 2);
        assert_eq!(scm.scroll_scm, 1);
        assert!(scm.git_status.is_none());
        assert!(scm.take_deferred_status_refresh());
        assert!(!scm.take_deferred_status_refresh());
    }

    #[test]
    fn transfer_tracks_path_not_row_without_app() {
        let mut scm = ScmState::new();
        let history = history::HistoryState::default();
        let path = "folder/deep/a.txt";
        let mut snapshot = StatusSnapshot::default();
        snapshot.staged.push(forge_git::ChangeEntry {
            path: path.into(),
            status: GitStatus::Modified,
            side: DiffSide::Staged,
        });
        scm.git_status = Some(snapshot);
        scm.pending_transfer = Some((path.into(), StagingColumn::Staged));
        scm.scm_selected = 999;
        scm.follow_selection = false;
        scm.reconcile_transfer(&history.view(), true);
        assert_eq!(
            scm.selected_entry(&history.view(), true).unwrap().path,
            path
        );
        assert!(scm.follow_selection);
        assert!(scm.pending_transfer.is_none());
        assert!(scm.scm_expanded_unstaged.is_empty());
    }

    #[test]
    fn stale_snapshot_retains_transfer_until_final_refresh() {
        let mut scm = ScmState::new();
        let history = history::HistoryState::default();
        scm.pending_transfer = Some(("missing.txt".into(), StagingColumn::Staged));
        scm.status_rerun = true;
        scm.reconcile_transfer(&history.view(), false);
        assert!(scm.pending_transfer.is_some());
        scm.status_rerun = false;
        scm.reconcile_transfer(&history.view(), false);
        assert!(scm.pending_transfer.is_none());
    }
}

//! Read-only git backend powered by gix (gitoxide).
//!
//! Must not depend on `forge-git-cli`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use forge_model::{
    CommitChange, CommitChangeKind, DiffSide, GitStatus, GraphCommit, GraphRef, GraphRefKind,
    GraphSnapshot, StatusSnapshot,
};
use gix::bstr::ByteSlice;
use gix::status::index_worktree::iter::Summary as IwSummary;

pub use forge_model::GitStatus as ModelGitStatus;

/// Errors from the gix read backend.
#[derive(Debug, thiserror::Error)]
pub enum GitReadError {
    #[error("not a git repository (or no worktree): {0}")]
    NotARepository(String),
    #[error("git discover: {0}")]
    Discover(#[from] Box<gix::discover::Error>),
    #[error("git status: {0}")]
    Status(String),
    #[error("git history: {0}")]
    History(String),
}

impl From<gix::discover::Error> for GitReadError {
    fn from(value: gix::discover::Error) -> Self {
        Self::Discover(Box::new(value))
    }
}

/// Read-only git backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct GixReadBackend;

/// Default worker cap for gix status. `FORGE_STATUS_THREADS` overrides it.
const STATUS_THREAD_LIMIT: usize = 2;

fn status_thread_limit() -> usize {
    std::env::var("FORGE_STATUS_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(STATUS_THREAD_LIMIT)
}

impl GixReadBackend {
    /// Resolve symbolic/detached HEAD, including packed refs and linked worktrees.
    pub fn head_oid(&self, start: &Path) -> Option<String> {
        gix::discover(start)
            .ok()?
            .head_id()
            .ok()
            .map(|id| id.to_string())
    }

    /// The worktree's index (linked worktrees have a separate index).
    pub fn index_path(&self, start: &Path) -> Option<PathBuf> {
        Some(gix::discover(start).ok()?.index_path())
    }
    pub fn new() -> Self {
        Self
    }

    /// Walk upward from `start` and return the worktree root, if any.
    pub fn find_repo_root(&self, start: impl AsRef<Path>) -> Option<PathBuf> {
        let repo = gix::discover(start.as_ref()).ok()?;
        repo.workdir().map(|p| p.to_path_buf())
    }

    /// Collect a [`StatusSnapshot`] for the repository containing `start`.
    ///
    /// Blocking / CPU+IO heavy — call from a worker (`spawn_blocking`) in the app.
    pub fn status_snapshot(&self, start: impl AsRef<Path>) -> Result<StatusSnapshot, GitReadError> {
        let start = start.as_ref();
        let repo = gix::discover(start).map_err(GitReadError::from)?;
        let workdir = repo.workdir().ok_or_else(|| {
            GitReadError::NotARepository(format!("{} (bare or missing worktree)", start.display()))
        })?;
        let repo_root = workdir.to_path_buf();

        let platform = repo
            .status(gix::progress::Discard)
            .map_err(|e| GitReadError::Status(e.to_string()))?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|opts| {
                opts.emit_untracked(gix::dir::walk::EmissionMode::Matching)
                    .emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
            })
            // gix fans the index/worktree comparison out to one thread per
            // logical CPU. Each worker is a fresh OS thread and thus a fresh
            // glibc malloc arena, and every arena keeps its own high-water
            // mark after the thread exits. On a 20-core WSL that turned each
            // status refresh into ~20 short-lived arenas and ~0.5 MiB of
            // retained RSS per arena. A small fixed limit keeps the call
            // comfortably parallel without the arena sprawl;
            // FORGE_STATUS_THREADS=1 forces the serial path if needed.
            .index_worktree_options_mut(|opts| {
                opts.thread_limit = Some(status_thread_limit());
            });

        let iter = platform
            .into_iter(None)
            .map_err(|e| GitReadError::Status(e.to_string()))?;

        let mut snap = StatusSnapshot::new(repo_root);
        for item in iter {
            let item = item.map_err(|e| GitReadError::Status(e.to_string()))?;
            apply_item(&mut snap, item);
        }
        snap.finish();
        Ok(snap)
    }

    /// Collect the commit graph reachable from every ref, newest first.
    ///
    /// Blocking / IO heavy — call from a worker (`spawn_blocking`) in the app.
    /// `limit` caps the number of commits returned.
    pub fn graph(
        &self,
        start: impl AsRef<Path>,
        limit: usize,
    ) -> Result<GraphSnapshot, GitReadError> {
        let start = start.as_ref();
        let repo = gix::discover(start).map_err(GitReadError::from)?;
        let repo_root = repo
            .workdir()
            .ok_or_else(|| {
                GitReadError::NotARepository(format!(
                    "{} (bare or missing worktree)",
                    start.display()
                ))
            })?
            .to_path_buf();

        let mut decorations: HashMap<gix::ObjectId, Vec<GraphRef>> = HashMap::new();
        let mut tips: Vec<gix::ObjectId> = Vec::new();
        let platform = repo.references().map_err(history_err)?;
        let references = platform.all().map_err(history_err)?;
        for reference in references {
            let mut reference = reference.map_err(history_err)?;
            let Some((kind, name)) = ref_display(&reference) else {
                continue;
            };
            let Ok(id) = reference.peel_to_id() else {
                continue;
            };
            add_decoration(
                &mut decorations,
                &mut tips,
                id.detach(),
                GraphRef { name, kind },
            );
        }

        let head = repo.head_id().ok().map(|id| id.detach());
        let head_id = head.map(|id| id.to_string());
        if let Some(id) = head {
            add_decoration(
                &mut decorations,
                &mut tips,
                id,
                GraphRef {
                    name: "HEAD".into(),
                    kind: GraphRefKind::Head,
                },
            );
        }
        let head_short = head_id.as_deref().map(short_hex);
        let branch = match repo.head_name().ok().flatten() {
            Some(name) => name.shorten().to_string(),
            None => head_short
                .clone()
                .map(|short| format!("detached at {short}"))
                .unwrap_or_default(),
        };

        let mut commits = Vec::new();
        if !tips.is_empty() {
            let walk = repo
                .rev_walk(tips)
                .sorting(gix::revision::walk::Sorting::BreadthFirst)
                .all()
                .map_err(history_err)?;
            for info in walk.take(limit.max(1)) {
                let info = info.map_err(history_err)?;
                let parents: Vec<String> =
                    info.parent_ids.iter().map(ToString::to_string).collect();
                let commit = repo.find_commit(info.id).map_err(history_err)?;
                let subject = commit
                    .message()
                    .map(|message| clean_line(message.title.to_str_lossy().as_ref()))
                    .unwrap_or_default();
                let author = commit
                    .author()
                    .map(|author| author.name.to_str_lossy().trim().to_string())
                    .unwrap_or_default();
                let time = commit.time().map(|time| time.seconds).unwrap_or_default();
                let id = info.id.to_string();
                let mut refs = decorations.remove(&info.id).unwrap_or_default();
                // HEAD first, then local branches, tags, remotes.
                refs.sort_by_key(|reference| match reference.kind {
                    GraphRefKind::Head => 0,
                    GraphRefKind::LocalBranch => 1,
                    GraphRefKind::Tag => 2,
                    GraphRefKind::RemoteBranch => 3,
                });
                commits.push(GraphCommit {
                    short: short_hex(&id),
                    refs,
                    id,
                    parents,
                    author,
                    time,
                    subject,
                });
            }
        }

        let commits = topological_order(commits, head_id.as_deref());

        Ok(GraphSnapshot {
            repo_root,
            head: head_short,
            branch,
            commits,
        })
    }

    /// Files changed by `oid`, compared against its first parent.
    pub fn commit_changes(
        &self,
        start: impl AsRef<Path>,
        oid: &str,
    ) -> Result<Vec<CommitChange>, GitReadError> {
        let repo = gix::discover(start.as_ref()).map_err(GitReadError::from)?;
        let oid = gix::ObjectId::from_hex(oid.as_bytes())
            .map_err(|err| GitReadError::History(err.to_string()))?;
        let commit = repo.find_commit(oid).map_err(history_err)?;
        let new_tree = commit.tree().map_err(history_err)?;
        let old_tree = match commit.parent_ids().next() {
            Some(parent) => {
                let parent = repo
                    .find_commit(parent.detach())
                    .map_err(|err| GitReadError::History(err.to_string()))?;
                Some(
                    parent
                        .tree()
                        .map_err(|err| GitReadError::History(err.to_string()))?,
                )
            }
            None => None,
        };
        let changes = repo
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(diff_options()))
            .map_err(history_err)?;
        let mut out = Vec::with_capacity(changes.len());
        for change in changes {
            use gix::object::tree::diff::ChangeDetached as Change;
            match change {
                Change::Addition {
                    location,
                    entry_mode,
                    ..
                } if entry_mode.is_no_tree() => out.push(CommitChange {
                    path: bstr_to_rela(location.as_ref()),
                    old_path: None,
                    kind: CommitChangeKind::Added,
                }),
                Change::Deletion {
                    location,
                    entry_mode,
                    ..
                } if entry_mode.is_no_tree() => out.push(CommitChange {
                    path: bstr_to_rela(location.as_ref()),
                    old_path: None,
                    kind: CommitChangeKind::Deleted,
                }),
                Change::Modification {
                    location,
                    previous_entry_mode,
                    entry_mode,
                    ..
                } if entry_mode.is_no_tree() => {
                    let kind = if previous_entry_mode.is_no_tree() {
                        CommitChangeKind::Modified
                    } else {
                        CommitChangeKind::TypeChanged
                    };
                    out.push(CommitChange {
                        path: bstr_to_rela(location.as_ref()),
                        old_path: None,
                        kind,
                    });
                }
                Change::Rewrite {
                    source_location,
                    location,
                    copy,
                    ..
                } => out.push(CommitChange {
                    path: bstr_to_rela(location.as_ref()),
                    old_path: Some(bstr_to_rela(source_location.as_ref())),
                    kind: if copy {
                        CommitChangeKind::Copied
                    } else {
                        CommitChangeKind::Renamed
                    },
                }),
                _ => {}
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

fn history_err(err: impl std::fmt::Display) -> GitReadError {
    GitReadError::History(err.to_string())
}

/// Rename tracking on, matching what `git show` displays by default.
fn diff_options() -> gix::diff::Options {
    let mut options = gix::diff::Options::default();
    options.track_path();
    options.track_rewrites(Some(gix::diff::Rewrites::default()));
    options
}

/// Order commits so every child precedes its parents (topological order).
///
/// The revision walk emits commits breadth-first from every ref tip, which can
/// place a branch tip before the merge commit that contains it. Lane layout
/// needs the strict "parent below child" invariant, so re-order here: ready
/// tips first (HEAD ahead of the rest), then newest commit time first.
fn topological_order(commits: Vec<GraphCommit>, head: Option<&str>) -> Vec<GraphCommit> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| (commit.id.as_str(), i))
        .collect();
    let mut pending_children = vec![0usize; commits.len()];
    for commit in &commits {
        for parent in &commit.parents {
            if let Some(&parent_index) = index.get(parent.as_str()) {
                pending_children[parent_index] += 1;
            }
        }
    }

    // Max-heap: HEAD first, then newest commit, then lowest index.
    let mut ready: BinaryHeap<(u8, i64, Reverse<usize>)> = BinaryHeap::new();
    for (i, pending) in pending_children.iter().enumerate() {
        if *pending == 0 {
            let is_head = u8::from(head == Some(commits[i].id.as_str()));
            ready.push((is_head, commits[i].time, Reverse(i)));
        }
    }

    let mut order = Vec::with_capacity(commits.len());
    while let Some((_, _, Reverse(i))) = ready.pop() {
        order.push(i);
        for parent in &commits[i].parents {
            if let Some(&parent_index) = index.get(parent.as_str()) {
                pending_children[parent_index] -= 1;
                if pending_children[parent_index] == 0 {
                    ready.push((0, commits[parent_index].time, Reverse(parent_index)));
                }
            }
        }
    }

    let mut slots: Vec<Option<GraphCommit>> = commits.into_iter().map(Some).collect();
    let mut ordered: Vec<GraphCommit> = order.into_iter().filter_map(|i| slots[i].take()).collect();
    // Defensive: unvisited commits (should only happen on a corrupt graph).
    ordered.extend(slots.into_iter().flatten());
    ordered
}

fn short_hex(id: &str) -> String {
    id.chars().take(7).collect()
}

/// Collapse control characters so a commit subject always renders on one row.
fn clean_line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Display name + kind for a ref, or `None` for refs the graph should skip.
fn ref_display(reference: &gix::Reference<'_>) -> Option<(GraphRefKind, String)> {
    use gix::refs::Category;
    let full = reference.name();
    let shorten = full.shorten().to_string();
    let kind = match full.category()? {
        Category::LocalBranch => GraphRefKind::LocalBranch,
        Category::RemoteBranch => GraphRefKind::RemoteBranch,
        Category::Tag => GraphRefKind::Tag,
        _ => return None,
    };
    // `refs/remotes/origin/HEAD` is a symbolic alias; the branch it points at
    // is already listed and would otherwise duplicate the badge.
    if shorten.ends_with("/HEAD") {
        return None;
    }
    Some((kind, shorten))
}

fn add_decoration(
    decorations: &mut HashMap<gix::ObjectId, Vec<GraphRef>>,
    tips: &mut Vec<gix::ObjectId>,
    id: gix::ObjectId,
    decoration: GraphRef,
) {
    let slot = decorations.entry(id).or_default();
    if !slot
        .iter()
        .any(|existing| existing.name == decoration.name && existing.kind == decoration.kind)
    {
        slot.push(decoration);
    }
    if !tips.contains(&id) {
        tips.push(id);
    }
}

fn apply_item(snap: &mut StatusSnapshot, item: gix::status::Item) {
    match item {
        gix::status::Item::IndexWorktree(iw) => apply_index_worktree(snap, iw),
        gix::status::Item::TreeIndex(change) => apply_tree_index(snap, change),
    }
}

fn apply_index_worktree(snap: &mut StatusSnapshot, item: gix::status::index_worktree::Item) {
    use gix::status::index_worktree::Item;

    match &item {
        Item::DirectoryContents { entry, .. } => {
            let path = bstr_to_rela(entry.rela_path.as_bstr());
            match entry.status {
                gix::dir::entry::Status::Untracked => {
                    snap.push_change(path, GitStatus::Untracked, DiffSide::Unstaged);
                }
                gix::dir::entry::Status::Ignored(_) => {
                    // Coloring only — staging panel skips Ignored via push_change.
                    snap.push_change(path, GitStatus::Ignored, DiffSide::Unstaged);
                }
                _ => {}
            }
        }
        Item::Modification { .. } | Item::Rewrite { .. } => {
            if let Some(summary) = item.summary() {
                let path = bstr_to_rela(item.rela_path());
                snap.push_change(path, summary_to_status(summary), DiffSide::Unstaged);
                if let Item::Rewrite { source, .. } = &item {
                    let src = bstr_to_rela(source.rela_path());
                    snap.push_change(src, GitStatus::Deleted, DiffSide::Unstaged);
                }
            }
        }
    }
}

fn apply_tree_index(snap: &mut StatusSnapshot, change: gix::diff::index::Change) {
    use gix::diff::index::Change;
    match change {
        Change::Addition { location, .. } => {
            snap.push_change(
                bstr_to_rela(location.as_ref()),
                GitStatus::Added,
                DiffSide::Staged,
            );
        }
        Change::Deletion { location, .. } => {
            snap.push_change(
                bstr_to_rela(location.as_ref()),
                GitStatus::Deleted,
                DiffSide::Staged,
            );
        }
        Change::Modification { location, .. } => {
            snap.push_change(
                bstr_to_rela(location.as_ref()),
                GitStatus::Modified,
                DiffSide::Staged,
            );
        }
        Change::Rewrite {
            source_location,
            location,
            ..
        } => {
            snap.push_change(
                bstr_to_rela(source_location.as_ref()),
                GitStatus::Deleted,
                DiffSide::Staged,
            );
            snap.push_change(
                bstr_to_rela(location.as_ref()),
                GitStatus::Renamed,
                DiffSide::Staged,
            );
        }
    }
}

fn summary_to_status(summary: IwSummary) -> GitStatus {
    match summary {
        IwSummary::Removed => GitStatus::Deleted,
        IwSummary::Added => GitStatus::Untracked,
        IwSummary::Modified | IwSummary::TypeChange => GitStatus::Modified,
        IwSummary::Renamed | IwSummary::Copied => GitStatus::Renamed,
        IwSummary::IntentToAdd => GitStatus::Added,
        IwSummary::Conflict => GitStatus::Conflicted,
    }
}

fn bstr_to_rela(path: &gix::bstr::BStr) -> String {
    path.to_str_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "forge")
            .env("GIT_AUTHOR_EMAIL", "forge@example.com")
            .env("GIT_COMMITTER_NAME", "forge")
            .env("GIT_COMMITTER_EMAIL", "forge@example.com")
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init"]);
        git(dir.path(), &["checkout", "-b", "main"]);
        dir
    }

    #[test]
    fn status_snapshot_colors_common_states() {
        let dir = init_repo();
        let root = dir.path();

        fs::write(root.join("keep.txt"), "k1\n").unwrap();
        fs::write(root.join("gone.txt"), "g\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        git(root, &["add", "keep.txt", "gone.txt", ".gitignore"]);
        git(root, &["commit", "-m", "init"]);

        fs::write(root.join("keep.txt"), "k2\n").unwrap();
        fs::remove_file(root.join("gone.txt")).unwrap();
        fs::write(root.join("staged.txt"), "s\n").unwrap();
        git(root, &["add", "staged.txt"]);
        fs::write(root.join("new.txt"), "u\n").unwrap();
        fs::write(root.join("ignored.txt"), "i\n").unwrap();

        let snap = GixReadBackend::new().status_snapshot(root).expect("status");

        let st = |name: &str| snap.status_for(&root.join(name));
        assert_eq!(st("keep.txt"), GitStatus::Modified, "{:?}", snap.entries);
        assert_eq!(st("gone.txt"), GitStatus::Deleted, "{:?}", snap.entries);
        assert_eq!(st("staged.txt"), GitStatus::Added, "{:?}", snap.entries);
        assert_eq!(st("new.txt"), GitStatus::Untracked, "{:?}", snap.entries);
        assert_eq!(st("ignored.txt"), GitStatus::Ignored, "{:?}", snap.entries);

        assert!(
            snap.unstaged.iter().any(|c| c.path == "keep.txt"),
            "unstaged={:?}",
            snap.unstaged
        );
        assert!(
            snap.staged.iter().any(|c| c.path == "staged.txt"),
            "staged={:?}",
            snap.staged
        );
        assert!(
            !snap.unstaged.iter().any(|c| c.path == "ignored.txt"),
            "ignored should not be in staging list"
        );

        fs::create_dir_all(root.join("clean_dir")).unwrap();
        assert_eq!(st("clean_dir"), GitStatus::Clean);
    }

    #[test]
    fn find_repo_root_walks_up() {
        let dir = init_repo();
        let root = dir.path();
        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        let found = GixReadBackend::new().find_repo_root(&nested).expect("find");
        assert_eq!(found.canonicalize().unwrap(), root.canonicalize().unwrap());
    }

    fn commit_file(root: &Path, name: &str, contents: &str, message: &str) {
        fs::write(root.join(name), contents).unwrap();
        git(root, &["add", name]);
        git(root, &["commit", "-m", message]);
    }

    #[test]
    fn graph_reports_parents_refs_and_merge() {
        let dir = init_repo();
        let root = dir.path();
        commit_file(root, "a.txt", "a\n", "feat: first");
        git(root, &["tag", "v1.0.0"]);
        git(root, &["checkout", "-b", "feature"]);
        commit_file(root, "b.txt", "b\n", "feat: branch work");
        git(root, &["checkout", "main"]);
        commit_file(root, "a.txt", "a2\n", "fix: main work");
        git(
            root,
            &["merge", "--no-ff", "-m", "merge feature", "feature"],
        );

        let graph = GixReadBackend::new().graph(root, 100).expect("graph");
        assert_eq!(graph.branch, "main");
        assert_eq!(graph.commits.len(), 4, "{:?}", graph.commits);

        let head = &graph.commits[0];
        assert_eq!(head.subject, "merge feature");
        assert_eq!(head.parents.len(), 2, "merge commit has two parents");
        assert!(head
            .refs
            .iter()
            .any(|r| r.kind == GraphRefKind::Head && r.name == "HEAD"));
        assert!(head
            .refs
            .iter()
            .any(|r| r.kind == GraphRefKind::LocalBranch && r.name == "main"));
        assert_eq!(head.id.len(), 40);
        assert_eq!(head.short.len(), 7);
        assert!(head.id.starts_with(&head.short));

        let tagged = graph
            .commits
            .iter()
            .find(|c| c.subject == "feat: first")
            .expect("root commit");
        assert!(tagged.parents.is_empty());
        assert!(tagged
            .refs
            .iter()
            .any(|r| r.kind == GraphRefKind::Tag && r.name == "v1.0.0"));

        // Children always appear before their parents.
        for (index, commit) in graph.commits.iter().enumerate() {
            for parent in &commit.parents {
                let parent_index = graph
                    .commits
                    .iter()
                    .position(|c| &c.id == parent)
                    .expect("parent present");
                assert!(parent_index > index, "parent must follow its child");
            }
        }
    }

    #[test]
    fn graph_respects_limit_and_empty_repository() {
        let dir = init_repo();
        let root = dir.path();
        let empty = GixReadBackend::new().graph(root, 10).expect("empty graph");
        assert!(empty.commits.is_empty());
        assert!(empty.head.is_none());

        commit_file(root, "a.txt", "1\n", "one");
        commit_file(root, "a.txt", "2\n", "two");
        commit_file(root, "a.txt", "3\n", "three");
        let limited = GixReadBackend::new().graph(root, 2).expect("graph");
        assert_eq!(limited.commits.len(), 2);
        assert_eq!(limited.commits[0].subject, "three");
    }

    #[test]
    fn commit_changes_reports_add_modify_delete_rename() {
        let dir = init_repo();
        let root = dir.path();
        commit_file(root, "keep.txt", "one\n", "base");
        commit_file(root, "gone.txt", "bye\n", "second");

        // Rename without touching the content so similarity is 100%.
        fs::rename(root.join("keep.txt"), root.join("renamed.txt")).unwrap();
        fs::write(root.join("gone.txt"), "bye bye\n").unwrap();
        fs::write(root.join("added.txt"), "new\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "churn"]);

        let graph = GixReadBackend::new().graph(root, 5).expect("graph");
        let head = graph.commits[0].id.clone();
        let changes = GixReadBackend::new()
            .commit_changes(root, &head)
            .expect("changes");
        let by_path = |path: &str| {
            changes
                .iter()
                .find(|c| c.path == path)
                .unwrap_or_else(|| panic!("missing {path} in {changes:?}"))
        };
        assert_eq!(by_path("added.txt").kind, CommitChangeKind::Added);
        assert_eq!(by_path("gone.txt").kind, CommitChangeKind::Modified);
        assert_eq!(by_path("renamed.txt").kind, CommitChangeKind::Renamed);
        assert_eq!(by_path("renamed.txt").old_path.as_deref(), Some("keep.txt"));

        let root_commit = graph
            .commits
            .iter()
            .find(|c| c.subject == "base")
            .unwrap()
            .id
            .clone();
        let first = GixReadBackend::new()
            .commit_changes(root, &root_commit)
            .expect("root changes");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, CommitChangeKind::Added);
        assert_eq!(first[0].path, "keep.txt");
    }
}

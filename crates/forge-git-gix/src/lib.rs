//! Read-only git backend powered by gix (gitoxide).
//!
//! Must not depend on `forge-git-cli`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use forge_model::{DiffSide, GitStatus, StatusSnapshot};
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
}

impl From<gix::discover::Error> for GitReadError {
    fn from(value: gix::discover::Error) -> Self {
        Self::Discover(Box::new(value))
    }
}

/// Read-only git backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct GixReadBackend;

impl GixReadBackend {
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
            GitReadError::NotARepository(format!(
                "{} (bare or missing worktree)",
                start.display()
            ))
        })?;
        let repo_root = workdir.to_path_buf();

        let platform = repo
            .status(gix::progress::Discard)
            .map_err(|e| GitReadError::Status(e.to_string()))?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|opts| {
                opts.emit_untracked(gix::dir::walk::EmissionMode::Matching)
                    .emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
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

        let snap = GixReadBackend::new()
            .status_snapshot(root)
            .expect("status");

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
        let found = GixReadBackend::new()
            .find_repo_root(&nested)
            .expect("find");
        assert_eq!(
            found.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }
}

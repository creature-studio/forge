//! Unified git service facade.
//!
//! Status and graph reads go through `forge-git-gix`; blame, commit queries,
//! write / network / file-diff through `forge-git-cli`. UI / app code should
//! depend on this crate, never on gix directly.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

pub use forge_git_cli::BlameLine;
pub use forge_git_cli::{CliWriteBackend, GitWriteError};
pub use forge_git_cli::{GitAction, GitItem, GitQuery};
pub use forge_git_gix::{GitReadError, GixReadBackend};
pub use forge_model::{
    ChangeEntry, CommitChange, CommitChangeKind, DiffHunk, DiffLine, DiffLineKind, DiffSide,
    FileDiff, GitStatus, GraphCommit, GraphRef, GraphRefKind, GraphSnapshot, StatusSnapshot,
};

/// Facade combining read + write backends.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitService {
    pub read: GixReadBackend,
    pub write: CliWriteBackend,
}

impl GitService {
    pub async fn blame(
        &self,
        repo: &Path,
        path: &Path,
        lines: usize,
    ) -> Result<Vec<BlameLine>, GitWriteError> {
        self.write.blame(repo, path, lines).await
    }
    pub fn new() -> Self {
        Self {
            read: GixReadBackend::new(),
            write: CliWriteBackend::new(),
        }
    }

    /// Worktree root for the repository containing `start`, if any.
    pub fn find_repo_root(&self, start: impl AsRef<Path>) -> Option<PathBuf> {
        self.read.find_repo_root(start)
    }

    /// Status snapshot via gix (blocking). Prefer running inside `spawn_blocking`.
    pub fn status(&self, start: impl AsRef<Path>) -> Result<StatusSnapshot, GitReadError> {
        self.read.status_snapshot(start)
    }

    /// Commit graph across every ref, newest first (blocking).
    pub fn graph(
        &self,
        start: impl AsRef<Path>,
        limit: usize,
    ) -> Result<GraphSnapshot, GitReadError> {
        self.read.graph(start, limit)
    }

    /// Files touched by one commit versus its first parent (blocking).
    pub fn commit_changes(
        &self,
        start: impl AsRef<Path>,
        oid: &str,
    ) -> Result<Vec<CommitChange>, GitReadError> {
        self.read.commit_changes(start, oid)
    }

    /// Stage paths relative to (or absolute under) the worktree.
    pub async fn stage_paths(
        &self,
        repo: impl AsRef<Path>,
        paths: &[impl AsRef<Path>],
    ) -> Result<(), GitWriteError> {
        self.write.stage_paths(repo, paths).await
    }

    /// Unstage paths (`git restore --staged`).
    pub async fn unstage_paths(
        &self,
        repo: impl AsRef<Path>,
        paths: &[impl AsRef<Path>],
    ) -> Result<(), GitWriteError> {
        self.write.unstage_paths(repo, paths).await
    }

    /// Create a commit with the given message (no editor).
    pub async fn commit(
        &self,
        repo: impl AsRef<Path>,
        message: impl AsRef<str>,
    ) -> Result<(), GitWriteError> {
        self.write.commit(repo, message).await
    }

    /// File-level unified diff (CLI `git diff` / `--cached`).
    pub async fn file_diff(
        &self,
        repo: impl AsRef<Path>,
        rela_path: impl AsRef<str>,
        side: DiffSide,
    ) -> Result<FileDiff, GitWriteError> {
        self.write.file_diff(repo, rela_path, side).await
    }

    /// One file's diff as changed by `oid` (read-only history view).
    pub async fn commit_file_diff(
        &self,
        repo: impl AsRef<Path>,
        oid: &str,
        path: &str,
    ) -> Result<FileDiff, GitWriteError> {
        self.write.commit_file_diff(repo.as_ref(), oid, path).await
    }

    /// Stage a single hunk from an unstaged [`FileDiff`].
    pub async fn stage_hunk(
        &self,
        repo: impl AsRef<Path>,
        diff: &FileDiff,
        hunk: &DiffHunk,
    ) -> Result<(), GitWriteError> {
        self.write.stage_hunk(repo, diff, hunk).await
    }

    /// Unstage a single hunk from a staged [`FileDiff`].
    pub async fn unstage_hunk(
        &self,
        repo: impl AsRef<Path>,
        diff: &FileDiff,
        hunk: &DiffHunk,
    ) -> Result<(), GitWriteError> {
        self.write.unstage_hunk(repo, diff, hunk).await
    }

    /// `git push` — credential / remote errors return [`GitWriteError::Command`].
    pub async fn push(&self, repo: impl AsRef<Path>) -> Result<String, GitWriteError> {
        self.write.push(repo).await
    }

    /// `git pull --ff-only`.
    pub async fn pull(&self, repo: impl AsRef<Path>) -> Result<String, GitWriteError> {
        self.write.pull(repo).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(cwd: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "forge")
            .env("GIT_AUTHOR_EMAIL", "forge@example.com")
            .env("GIT_COMMITTER_NAME", "forge")
            .env("GIT_COMMITTER_EMAIL", "forge@example.com")
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[test]
    fn facade_status_reports_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init"]);
        git(root, &["checkout", "-b", "main"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-m", "i"]);
        fs::write(root.join("b.txt"), "b\n").unwrap();

        let svc = GitService::new();
        let snap = svc.status(root).unwrap();
        assert_eq!(snap.status_for(&root.join("b.txt")), GitStatus::Untracked);
        assert!(svc.find_repo_root(root).is_some());
        assert!(snap.unstaged.iter().any(|c| c.path == "b.txt"));
    }
}

//! Write-path (and CLI-backed diff) git backend that shells out to `git`.
//!
//! Must not depend on gix / `forge-git-gix`.

#![forbid(unsafe_code)]

mod blame;
pub use blame::BlameLine;

use std::path::Path;
use std::process::{Output, Stdio};

use forge_model::{DiffHunk, DiffLine, DiffLineKind, DiffSide, FileDiff};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
mod workbench;
pub use workbench::{GitAction, GitItem, GitQuery};

/// Errors from the git CLI write / diff backend.
#[derive(Debug, thiserror::Error)]
pub enum GitWriteError {
    #[error("git not found or failed to spawn: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {op} failed (exit {code}): {stderr}")]
    Command {
        op: &'static str,
        code: i32,
        stderr: String,
    },
    #[error("empty commit message")]
    EmptyMessage,
    #[error("no paths provided")]
    NoPaths,
    #[error("no hunk selected")]
    NoHunk,
    #[error("empty patch")]
    EmptyPatch,
}

/// Only a bounded complete text snapshot is eligible for the full-file viewer.
/// The patch remains available even for binary/oversize content.
const FULL_DIFF_MAX_BYTES: u64 = 8 * 1024 * 1024;

async fn bounded_text_file(bytes: Vec<u8>) -> Option<Vec<String>> {
    let source = String::from_utf8(bytes).ok()?;
    if source.contains('\0') { return None; }
    let lines = source.split_terminator('\n').map(|s| s.trim_end_matches('\r').to_owned()).collect();
    Some(lines)
}

/// Read one Git blob, not a patch. A missing blob is valid for a deleted file.
async fn git_blob_lines(repo: &Path, spec: &str) -> Option<Vec<String>> {
    let mut child = Command::new("git")
        .current_dir(repo)
        .args(["--no-optional-locks", "--literal-pathspecs", "show", "--no-ext-diff", "--no-textconv", spec])
        .stdin(Stdio::null()).stderr(Stdio::null())
        .stdout(Stdio::piped()).kill_on_drop(true).spawn().ok()?;
    let mut bytes = Vec::new();
    let read = async {
        child.stdout.take()?.take(FULL_DIFF_MAX_BYTES + 1).read_to_end(&mut bytes).await.ok()?;
        if bytes.len() as u64 > FULL_DIFF_MAX_BYTES { return None; }
        let status = child.wait().await.ok()?;
        status.success().then_some(())
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), read).await.ok()??;
    bounded_text_file(bytes).await
}

async fn worktree_lines(repo: &Path, path: &str) -> Option<Vec<String>> {
    let full = repo.join(path);
    let meta = tokio::fs::metadata(&full).await.ok()?;
    if !meta.is_file() || meta.len() > FULL_DIFF_MAX_BYTES { return None; }
    bounded_text_file(tokio::fs::read(full).await.ok()?).await
}

/// Write / CLI-diff backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct CliWriteBackend;

impl CliWriteBackend {
    /// Bounded, read-only worktree/index query for the source gutter. No
    /// untracked fallback, external diff drivers, textconv or optional locks.
    pub async fn gutter_diff(&self, repo: &Path, path: &str) -> Result<FileDiff, GitWriteError> {
        self.gutter_diff_side(repo, path, DiffSide::Unstaged).await
    }

    /// Side-specific, bounded source-gutter query (never writes to the index).
    pub async fn gutter_diff_side(&self, repo: &Path, path: &str, side: DiffSide) -> Result<FileDiff, GitWriteError> {
        const LIMIT: usize = 2 * 1024 * 1024;
        let read = async {
            let mut args = vec![
                "--no-optional-locks", "--literal-pathspecs", "-c",
                "diff.suppressBlankEmpty=false", "diff", "--no-ext-diff", "--no-textconv",
                "--no-color", "--unified=3", "--output-indicator-new=+",
                "--output-indicator-old=-", "--output-indicator-context= ",
            ];
            if matches!(side, DiffSide::Staged) { args.push("--cached"); }
            args.extend(["--", path]);
            let mut child = Command::new("git")
                .current_dir(repo)
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .unwrap()
                .take((LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > LIMIT {
                return Err(std::io::Error::other("gutter diff exceeds 2 MiB"));
            }
            if !child.wait().await?.success() {
                return Err(std::io::Error::other("gutter diff failed"));
            }
            Ok(bytes)
        };
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(3), read)
            .await
            .map_err(|_| std::io::Error::other("gutter diff timed out"))??;
        let mut in_hunk = false;
        let lines = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|text| {
                let kind = if text.starts_with("@@ ") {
                    in_hunk = true;
                    DiffLineKind::Header
                } else if in_hunk {
                    match text.as_bytes().first() {
                        Some(b'+') => DiffLineKind::Add,
                        Some(b'-') => DiffLineKind::Delete,
                        Some(b' ') => DiffLineKind::Context,
                        _ => DiffLineKind::Meta,
                    }
                } else {
                    DiffLineKind::Meta
                };
                DiffLine {
                    kind,
                    text: text.to_owned(),
                }
            })
            .collect();
        Ok(FileDiff {
            repo_root: repo.to_owned(),
            path: path.to_owned(),
            side,
            lines,
            full_new_lines: None,
        })
    }

    pub fn new() -> Self {
        Self
    }

    /// `git --literal-pathspecs add -- <paths>` relative to `repo`.
    pub async fn stage_paths(
        &self,
        repo: impl AsRef<Path>,
        paths: &[impl AsRef<Path>],
    ) -> Result<(), GitWriteError> {
        let args = path_args(paths)?;
        run_git(repo.as_ref(), "add", {
            let mut v = vec!["--literal-pathspecs".into(), "add".into(), "--".into()];
            v.extend(args);
            v
        })
        .await
        .map(|_| ())
    }

    /// Unstage literal paths, preserving worktree files, including before the first commit.
    pub async fn unstage_paths(
        &self,
        repo: impl AsRef<Path>,
        paths: &[impl AsRef<Path>],
    ) -> Result<(), GitWriteError> {
        let args = path_args(paths)?;
        let has_head = run_git_raw(
            repo.as_ref(),
            "rev-parse",
            vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
        )
        .await?
        .status
        .success();
        run_git(repo.as_ref(), "unstage", {
            let mut v = if has_head {
                vec![
                    "--literal-pathspecs".into(),
                    "restore".into(),
                    "--staged".into(),
                    "--".into(),
                ]
            } else {
                // An unborn branch has no HEAD to restore from. Remove only index entries.
                vec![
                    "--literal-pathspecs".into(),
                    "rm".into(),
                    "--cached".into(),
                    "-f".into(),
                    "--".into(),
                ]
            };
            v.extend(args);
            v
        })
        .await
        .map(|_| ())
    }

    /// `git commit -m <message>` (no editor).
    pub async fn commit(
        &self,
        repo: impl AsRef<Path>,
        message: impl AsRef<str>,
    ) -> Result<(), GitWriteError> {
        let message = message.as_ref().trim();
        if message.is_empty() {
            return Err(GitWriteError::EmptyMessage);
        }
        run_git(
            repo.as_ref(),
            "commit",
            vec!["commit".into(), "-m".into(), message.into()],
        )
        .await
        .map(|_| ())
    }

    /// File-level unified diff via `git diff` / `git diff --cached`.
    ///
    /// Blocking-friendly async wrapper; empty diff returns `lines: []`.
    pub async fn file_diff(
        &self,
        repo: impl AsRef<Path>,
        rela_path: impl AsRef<str>,
        side: DiffSide,
    ) -> Result<FileDiff, GitWriteError> {
        let repo = repo.as_ref();
        let path = rela_path.as_ref();
        let mut args: Vec<String> = vec![
            "--literal-pathspecs".into(),
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--no-color".into(),
            "--unified=3".into(),
        ];
        if matches!(side, DiffSide::Staged) {
            args.push("--cached".into());
        }
        // Untracked files: `git diff` shows nothing — try `/dev/null` style via
        // `git diff --no-index` only when unstaged path has no index entry.
        // For MVP: also pass `--` path; caller may fall back to "no diff".
        args.push("--".into());
        args.push(path.into());

        let output = run_git_raw(repo, "diff", args).await?;
        // diff exits 1 when differences exist — treat as success.
        let code = output.status.code().unwrap_or(1);
        if code != 0 && code != 1 {
            return Err(GitWriteError::Command {
                op: "diff",
                code,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = parse_unified_diff(&text);

        // Untracked: empty unstaged diff → synthesize a simple "new file" view.
        if lines.is_empty() && matches!(side, DiffSide::Unstaged) {
            if let Ok(extra) = untracked_as_diff(repo, path).await {
                lines = extra;
            }
        }

        Ok(FileDiff {
            repo_root: repo.to_path_buf(),
            path: path.to_string(),
            side,
            full_new_lines: if matches!(side, DiffSide::Staged) {
                git_blob_lines(repo, &format!(":{path}")).await
            } else {
                worktree_lines(repo, path).await
            },
            lines,
        })
    }

    /// Stage one hunk from an unstaged file diff (`git apply --cached`).
    pub async fn stage_hunk(
        &self,
        repo: impl AsRef<Path>,
        diff: &FileDiff,
        hunk: &DiffHunk,
    ) -> Result<(), GitWriteError> {
        if !matches!(diff.side, DiffSide::Unstaged) {
            return Err(GitWriteError::Command {
                op: "stage-hunk",
                code: -1,
                stderr: "hunk stage requires an unstaged diff (open with d from changes)".into(),
            });
        }
        let patch = diff.hunk_patch(hunk);
        self.apply_cached_patch(repo.as_ref(), &patch, false).await
    }

    /// Unstage one hunk from a staged file diff (`git apply --cached --reverse`).
    pub async fn unstage_hunk(
        &self,
        repo: impl AsRef<Path>,
        diff: &FileDiff,
        hunk: &DiffHunk,
    ) -> Result<(), GitWriteError> {
        if !matches!(diff.side, DiffSide::Staged) {
            return Err(GitWriteError::Command {
                op: "unstage-hunk",
                code: -1,
                stderr: "hunk unstage requires a staged diff (open with d from staged)".into(),
            });
        }
        let patch = diff.hunk_patch(hunk);
        self.apply_cached_patch(repo.as_ref(), &patch, true).await
    }

    /// Apply a unified patch to the index (`--cached`), optionally reversed.
    pub async fn apply_cached_patch(
        &self,
        repo: &Path,
        patch: &str,
        reverse: bool,
    ) -> Result<(), GitWriteError> {
        if patch.trim().is_empty() {
            return Err(GitWriteError::EmptyPatch);
        }
        let mut args: Vec<String> = vec![
            "apply".into(),
            "--cached".into(),
            "--whitespace=nowarn".into(),
        ];
        if reverse {
            args.push("--reverse".into());
        }
        args.push("-".into());

        let mut child = Command::new("git")
            .args(&args)
            .current_dir(repo)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(patch.as_bytes()).await?;
            stdin.flush().await?;
            drop(stdin);
        }
        let output = child.wait_with_output().await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitWriteError::Command {
                op: if reverse {
                    "unstage-hunk"
                } else {
                    "stage-hunk"
                },
                code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// `git push` (non-interactive). Returns trimmed stdout/stderr for the status bar.
    pub async fn push(&self, repo: impl AsRef<Path>) -> Result<String, GitWriteError> {
        let output = run_git_raw(repo.as_ref(), "push", vec!["push".into()]).await?;
        summarize_remote(output, "push")
    }

    /// `git pull` (non-interactive). Credential / divergent-history errors surface in status.
    pub async fn pull(&self, repo: impl AsRef<Path>) -> Result<String, GitWriteError> {
        let output = run_git_raw(
            repo.as_ref(),
            "pull",
            vec!["pull".into(), "--ff-only".into()],
        )
        .await?;
        summarize_remote(output, "pull")
    }
}

fn summarize_remote(output: Output, op: &'static str) -> Result<String, GitWriteError> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        let msg = if !stdout.is_empty() {
            stdout
        } else if !stderr.is_empty() {
            stderr
        } else {
            format!("{op} ok")
        };
        // Collapse to a single status-bar line.
        Ok(msg.lines().next().unwrap_or("ok").to_string())
    } else {
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        Err(GitWriteError::Command {
            op,
            code: output.status.code().unwrap_or(-1),
            stderr: detail,
        })
    }
}

fn path_args(paths: &[impl AsRef<Path>]) -> Result<Vec<String>, GitWriteError> {
    if paths.is_empty() {
        return Err(GitWriteError::NoPaths);
    }
    Ok(paths
        .iter()
        .map(|p| p.as_ref().to_string_lossy().into_owned())
        .collect())
}

async fn run_git(
    repo: &Path,
    op: &'static str,
    args: Vec<String>,
) -> Result<Output, GitWriteError> {
    let output = run_git_raw(repo, op, args).await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitWriteError::Command {
            op,
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

async fn run_git_raw(
    repo: &Path,
    _op: &'static str,
    args: Vec<String>,
) -> Result<Output, GitWriteError> {
    let output = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .kill_on_drop(true)
        // Prefer repo-local config; fall back so CI / ephemeral envs can commit.
        .env(
            "GIT_AUTHOR_NAME",
            std::env::var("GIT_AUTHOR_NAME").unwrap_or_else(|_| "forge".into()),
        )
        .env(
            "GIT_AUTHOR_EMAIL",
            std::env::var("GIT_AUTHOR_EMAIL").unwrap_or_else(|_| "forge@localhost".into()),
        )
        .env(
            "GIT_COMMITTER_NAME",
            std::env::var("GIT_COMMITTER_NAME").unwrap_or_else(|_| "forge".into()),
        )
        .env(
            "GIT_COMMITTER_EMAIL",
            std::env::var("GIT_COMMITTER_EMAIL").unwrap_or_else(|_| "forge@localhost".into()),
        )
        .output()
        .await?;
    Ok(output)
}

fn parse_unified_diff(text: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let kind = if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("similarity")
            || line.starts_with("rename ")
            || line.starts_with("@@")
        {
            if line.starts_with("@@") {
                DiffLineKind::Header
            } else {
                DiffLineKind::Meta
            }
        } else if let Some(rest) = line.strip_prefix('+') {
            let _ = rest;
            DiffLineKind::Add
        } else if let Some(rest) = line.strip_prefix('-') {
            let _ = rest;
            DiffLineKind::Delete
        } else {
            DiffLineKind::Context
        };
        out.push(DiffLine {
            kind,
            text: line.to_string(),
        });
    }
    out
}

/// For untracked files, invent a minimal unified-diff-like preview.
async fn untracked_as_diff(repo: &Path, rela: &str) -> Result<Vec<DiffLine>, GitWriteError> {
    let abs = repo.join(rela);
    if !abs.is_file() {
        return Ok(Vec::new());
    }
    let tracked = run_git_raw(
        repo,
        "ls-files",
        vec![
            "--literal-pathspecs".into(),
            "ls-files".into(),
            "--error-unmatch".into(),
            "--".into(),
            rela.into(),
        ],
    )
    .await?;
    if tracked.status.success() {
        return Ok(Vec::new());
    }
    // `git diff --no-index -- /dev/null path` exits 1 with a diff.
    let output = Command::new("git")
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-index",
            "--unified=3",
            "--",
            "/dev/null",
            rela,
        ])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    let code = output.status.code().unwrap_or(1);
    if code != 0 && code != 1 {
        return Ok(Vec::new());
    }
    Ok(parse_unified_diff(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;

    fn git(cwd: &Path, args: &[&str]) {
        let st = StdCommand::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "forge")
            .env("GIT_AUTHOR_EMAIL", "forge@example.com")
            .env("GIT_COMMITTER_NAME", "forge")
            .env("GIT_COMMITTER_EMAIL", "forge@example.com")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn stage_unstage_commit_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init"]);
        git(root, &["checkout", "-b", "main"]);
        fs::write(root.join("a.txt"), "one\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-m", "init"]);

        fs::write(root.join("a.txt"), "two\n").unwrap();
        fs::write(root.join("b.txt"), "new\n").unwrap();

        let cli = CliWriteBackend::new();
        cli.stage_paths(root, &["a.txt", "b.txt"]).await.unwrap();

        let staged = cli
            .file_diff(root, "a.txt", DiffSide::Staged)
            .await
            .unwrap();
        assert!(
            staged
                .lines
                .iter()
                .any(|l| matches!(l.kind, DiffLineKind::Add)),
            "{:?}",
            staged.lines
        );

        assert_eq!(staged.full_new_lines.as_deref(), Some(&["two".to_owned()][..]));

        cli.unstage_paths(root, &["a.txt"]).await.unwrap();
        let unstaged = cli
            .file_diff(root, "a.txt", DiffSide::Unstaged)
            .await
            .unwrap();
        assert_eq!(unstaged.full_new_lines.as_deref(), Some(&["two".to_owned()][..]));
        assert!(
            unstaged
                .lines
                .iter()
                .any(|l| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete)),
            "{:?}",
            unstaged.lines
        );

        // Restage and commit.
        cli.stage_paths(root, &["a.txt", "b.txt"]).await.unwrap();
        cli.commit(root, "update").await.unwrap();
    }

    #[tokio::test]
    async fn stage_single_hunk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init"]);
        git(root, &["checkout", "-b", "main"]);
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(root, &["commit", "-m", "init"]);

        fs::write(root.join("a.txt"), "one\nTWO\nthree\nFOUR\n").unwrap();
        let cli = CliWriteBackend::new();
        let diff = cli
            .file_diff(root, "a.txt", DiffSide::Unstaged)
            .await
            .unwrap();
        let hunks = diff.hunks();
        assert!(!hunks.is_empty(), "expected hunks in {:?}", diff.lines);
        // Stage first hunk only.
        cli.stage_hunk(root, &diff, &hunks[0]).await.unwrap();
        let staged = cli
            .file_diff(root, "a.txt", DiffSide::Staged)
            .await
            .unwrap();
        assert!(
            staged
                .lines
                .iter()
                .any(|l| matches!(l.kind, DiffLineKind::Add)),
            "expected staged adds: {:?}",
            staged.lines
        );
    }
}

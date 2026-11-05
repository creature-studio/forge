//! Shared domain types for the forge workspace.
//!
//! Keep this crate free of Tokio, Ratatui, and git backends.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Opaque identifier for a filesystem entry within the app model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(pub u64);

/// High-level entry kind shown in the file manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// A directory entry as seen by the UI / FS layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: PathId,
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Coarse git working-tree / index status for coloring / badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum GitStatus {
    #[default]
    Clean,
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
    Ignored,
}

impl GitStatus {
    /// Severity used when merging multiple statuses for one path / directory.
    pub fn severity(self) -> u8 {
        match self {
            GitStatus::Clean => 0,
            GitStatus::Ignored => 1,
            GitStatus::Untracked => 2,
            GitStatus::Added => 3,
            GitStatus::Renamed => 4,
            GitStatus::Modified => 5,
            GitStatus::Deleted => 6,
            GitStatus::Conflicted => 7,
        }
    }

    /// Keep the higher-severity status.
    pub fn merge(self, other: Self) -> Self {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }

    /// Short porcelain-like badge (2 chars) for dense lists / staging.
    pub fn badge(self) -> &'static str {
        match self {
            GitStatus::Clean => "  ",
            GitStatus::Modified => " M",
            GitStatus::Added => " A",
            GitStatus::Deleted => " D",
            GitStatus::Renamed => " R",
            GitStatus::Untracked => "??",
            GitStatus::Conflicted => "UU",
            GitStatus::Ignored => " !",
        }
    }

    /// Quiet single-char status mark (ASCII). Prefer coloring the name in explorers
    /// instead of showing this glyph; staging may still use it.
    pub fn gutter(self) -> &'static str {
        match self {
            GitStatus::Clean => " ",
            GitStatus::Ignored => ".",
            GitStatus::Untracked => "U",
            GitStatus::Modified => "M",
            GitStatus::Added => "A",
            GitStatus::Deleted => "D",
            GitStatus::Renamed => "R",
            GitStatus::Conflicted => "C",
        }
    }
}

/// Which side of the index a change / diff refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSide {
    /// Worktree vs index (`git diff`).
    Unstaged,
    /// Index vs HEAD (`git diff --cached`).
    Staged,
}

impl DiffSide {
    pub fn label(self) -> &'static str {
        match self {
            DiffSide::Unstaged => "unstaged",
            DiffSide::Staged => "staged",
        }
    }
}

/// One line inside a unified diff preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
    Header,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// File-level unified diff payload for the preview pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub repo_root: PathBuf,
    /// Path relative to the worktree (`/` separators).
    pub path: String,
    pub side: DiffSide,
    pub lines: Vec<DiffLine>,
}

/// One `@@ … @@` hunk inside a [`FileDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// Inclusive start index into [`FileDiff::lines`] (the `@@` header).
    pub start: usize,
    /// Exclusive end index into [`FileDiff::lines`].
    pub end: usize,
    pub header: String,
}

impl FileDiff {
    /// Split unified-diff lines into hunks (by `@@` headers).
    pub fn hunks(&self) -> Vec<DiffHunk> {
        parse_diff_hunks(&self.lines)
    }

    /// Build a standalone patch for one hunk, suitable for `git apply --cached`.
    pub fn hunk_patch(&self, hunk: &DiffHunk) -> String {
        build_hunk_patch(self, hunk)
    }

    /// Build a patch from the current index to the index with exactly one change
    /// staged/unstaged. Other changes in the hunk become context or are omitted.
    pub fn line_patch(&self, index: usize) -> Option<String> {
        let hunk = self
            .hunks()
            .into_iter()
            .find(|h| index > h.start && index < h.end)?;
        if !matches!(
            self.lines.get(index)?.kind,
            DiffLineKind::Add | DiffLineKind::Delete
        ) {
            return None;
        }
        let reverse = self.side == DiffSide::Staged;
        let ranges: Vec<_> = hunk.header.split_whitespace().collect();
        let range = ranges.get(if reverse { 2 } else { 1 })?;
        let start: usize = range.get(1..)?.split(',').next()?.parse().ok()?;
        let mut body = String::new();
        let (mut old_count, mut new_count) = (0, 0);
        let mut kept = false;
        for (offset, line) in self.lines[hunk.start + 1..hunk.end].iter().enumerate() {
            if line.text.starts_with("\\ No newline") {
                if kept {
                    body.push_str(&line.text);
                    body.push('\n');
                }
                kept = false;
                continue;
            }
            let selected = hunk.start + 1 + offset == index;
            let prefix = match (&line.kind, reverse, selected) {
                (DiffLineKind::Context, _, _) => Some(' '),
                (DiffLineKind::Add, false, true) | (DiffLineKind::Delete, true, true) => Some('+'),
                (DiffLineKind::Delete, false, true) | (DiffLineKind::Add, true, true) => Some('-'),
                (DiffLineKind::Delete, false, false) | (DiffLineKind::Add, true, false) => {
                    Some(' ')
                }
                _ => None,
            };
            if let Some(prefix) = prefix {
                if prefix != '+' {
                    old_count += 1;
                }
                if prefix != '-' {
                    new_count += 1;
                }
                body.push(prefix);
                body.push_str(line.text.get(1..)?);
                body.push('\n');
                kept = true;
            } else {
                kept = false;
            }
        }
        let old_header = self
            .lines
            .iter()
            .find(|l| l.text.starts_with("--- "))?
            .text
            .clone();
        let new_header = self
            .lines
            .iter()
            .find(|l| l.text.starts_with("+++ "))?
            .text
            .clone();
        let new_file = old_header == "--- /dev/null";
        let deleted_file = new_header == "+++ /dev/null";
        let base_missing = if reverse { deleted_file } else { new_file };
        let target_missing = if reverse {
            new_file && new_count == 0
        } else {
            deleted_file && new_count == 0
        };
        // Preserve git's quoted path spelling, including whitespace and Unicode.
        let a = if new_file {
            new_header[4..].replacen("b/", "a/", 1)
        } else {
            old_header[4..].into()
        };
        let b = if deleted_file {
            old_header[4..].replacen("a/", "b/", 1)
        } else {
            new_header[4..].into()
        };
        let diff_header = self
            .lines
            .iter()
            .find(|l| l.text.starts_with("diff --git "))?;
        let mode = self
            .lines
            .iter()
            .find_map(|l| {
                l.text
                    .strip_prefix("new file mode ")
                    .or_else(|| l.text.strip_prefix("deleted file mode "))
            })
            .unwrap_or("100644");
        let mut patch = format!("{}\n", diff_header.text);
        if base_missing {
            patch.push_str(&format!("new file mode {mode}\n"));
        }
        if target_missing {
            patch.push_str(&format!("deleted file mode {mode}\n"));
        }
        patch.push_str(&format!(
            "--- {}\n+++ {}\n",
            if base_missing { "/dev/null" } else { &a },
            if target_missing { "/dev/null" } else { &b }
        ));
        let old_start = if old_count == 0 { 0 } else { start.max(1) };
        let new_start = if new_count == 0 { 0 } else { start.max(1) };
        patch.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n{body}"
        ));
        Some(patch)
    }
}

/// Locate `@@` hunks in a parsed unified diff.
pub fn parse_diff_hunks(lines: &[DiffLine]) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let is_hunk_hdr =
            matches!(lines[i].kind, DiffLineKind::Header) && lines[i].text.starts_with("@@");
        if is_hunk_hdr {
            let start = i;
            i += 1;
            while i < lines.len() {
                let next_hunk = matches!(lines[i].kind, DiffLineKind::Header)
                    && lines[i].text.starts_with("@@");
                let next_file = matches!(lines[i].kind, DiffLineKind::Meta)
                    && lines[i].text.starts_with("diff ");
                if next_hunk || next_file {
                    break;
                }
                i += 1;
            }
            hunks.push(DiffHunk {
                start,
                end: i,
                header: lines[start].text.clone(),
            });
        } else {
            i += 1;
        }
    }
    hunks
}

fn build_hunk_patch(diff: &FileDiff, hunk: &DiffHunk) -> String {
    let mut header = String::new();
    for line in &diff.lines {
        if matches!(line.kind, DiffLineKind::Header) && line.text.starts_with("@@") {
            break;
        }
        // Keep diff meta (`diff --git`, `index`, `---`, `+++`, mode lines).
        if matches!(line.kind, DiffLineKind::Meta)
            || line.text.starts_with("---")
            || line.text.starts_with("+++")
        {
            header.push_str(&line.text);
            header.push('\n');
        }
    }
    if !header.contains("diff --git") {
        let path = &diff.path;
        header = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
    }
    let mut out = header;
    let end = hunk.end.min(diff.lines.len());
    let start = hunk.start.min(end);
    for line in &diff.lines[start..end] {
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

/// A single change listed in the staging workbench.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    /// Path relative to the worktree (`/` separators).
    pub path: String,
    pub status: GitStatus,
    pub side: DiffSide,
}

/// Snapshot of path → status relative to a repository worktree root.
#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    /// Absolute path to the worktree root.
    pub repo_root: PathBuf,
    /// Paths relative to [`Self::repo_root`], using `/` separators (git style).
    /// Merged (highest severity) status for file-list coloring.
    pub entries: HashMap<String, GitStatus>,
    /// Unstaged / untracked changes for the staging panel (ignored omitted).
    pub unstaged: Vec<ChangeEntry>,
    /// Staged (index vs HEAD) changes for the staging panel.
    pub staged: Vec<ChangeEntry>,
}

impl StatusSnapshot {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            entries: HashMap::new(),
            unstaged: Vec::new(),
            staged: Vec::new(),
        }
    }

    pub fn insert(&mut self, rela_path: impl Into<String>, status: GitStatus) {
        let key = rela_path.into();
        match self.entries.get(&key).copied() {
            Some(prev) => {
                self.entries.insert(key, prev.merge(status));
            }
            None => {
                self.entries.insert(key, status);
            }
        }
    }

    /// Record a change for the staging lists and update the merged map.
    pub fn push_change(&mut self, rela_path: impl Into<String>, status: GitStatus, side: DiffSide) {
        let path = rela_path.into();
        self.insert(path.clone(), status);
        // Ignored files are useful for coloring but not for staging.
        if matches!(status, GitStatus::Ignored) {
            return;
        }
        let entry = ChangeEntry { path, status, side };
        match side {
            DiffSide::Unstaged => self.unstaged.push(entry),
            DiffSide::Staged => self.staged.push(entry),
        }
    }

    /// Sort staging lists for stable UI.
    pub fn finish(&mut self) {
        self.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
        self.staged.sort_by(|a, b| a.path.cmp(&b.path));
        // Dedup same path+side keeping higher severity.
        dedup_changes(&mut self.unstaged);
        dedup_changes(&mut self.staged);
    }

    /// Status for an absolute filesystem path (file or directory).
    ///
    /// Directories inherit the highest-severity status of any recorded child.
    pub fn status_for(&self, abs_path: &Path) -> GitStatus {
        let Ok(rel) = abs_path.strip_prefix(&self.repo_root) else {
            return GitStatus::Clean;
        };
        if rel.as_os_str().is_empty() {
            return self.aggregate_prefix("");
        }
        let key = path_to_rela(rel);
        if let Some(status) = self.entries.get(&key).copied() {
            return status;
        }
        // Directory bubble-up: any child under `key/`.
        self.aggregate_prefix(&key)
    }

    fn aggregate_prefix(&self, dir_rela: &str) -> GitStatus {
        let prefix = if dir_rela.is_empty() {
            String::new()
        } else {
            format!("{dir_rela}/")
        };
        let mut best = GitStatus::Clean;
        for (path, status) in &self.entries {
            let matches = if prefix.is_empty() {
                true
            } else {
                path.starts_with(&prefix)
            };
            if matches {
                best = best.merge(*status);
            }
        }
        best
    }
}

fn dedup_changes(list: &mut Vec<ChangeEntry>) {
    if list.is_empty() {
        return;
    }
    let mut out: Vec<ChangeEntry> = Vec::with_capacity(list.len());
    for entry in list.drain(..) {
        if let Some(last) = out.last_mut() {
            if last.path == entry.path {
                last.status = last.status.merge(entry.status);
                continue;
            }
        }
        out.push(entry);
    }
    *list = out;
}

/// Convert a relative `Path` into a git-style rela path (`/` separators).
pub fn path_to_rela(path: &Path) -> String {
    let mut out = String::new();
    for (i, comp) in path.components().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(&comp.as_os_str().to_string_lossy());
    }
    out
}

/// Identifier for a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(pub u64);

/// Job scheduling priority (High is preferred by the scheduler).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobPriority {
    /// Interactive work (directory lists, key-driven updates).
    High = 2,
    /// Default background work (git status, etc.).
    Normal = 1,
    /// Prefetch / warm caches.
    Low = 0,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_inherits_child_status() {
        let mut snap = StatusSnapshot::new(PathBuf::from("/repo"));
        snap.insert("crates/forge/src/main.rs", GitStatus::Modified);
        snap.insert("README.md", GitStatus::Untracked);
        assert_eq!(
            snap.status_for(Path::new("/repo/crates")),
            GitStatus::Modified
        );
        assert_eq!(
            snap.status_for(Path::new("/repo/README.md")),
            GitStatus::Untracked
        );
        assert_eq!(
            snap.status_for(Path::new("/repo/clean.txt")),
            GitStatus::Clean
        );
    }

    #[test]
    fn merge_prefers_conflict() {
        assert_eq!(
            GitStatus::Modified.merge(GitStatus::Conflicted),
            GitStatus::Conflicted
        );
        assert_eq!(
            GitStatus::Untracked.merge(GitStatus::Added),
            GitStatus::Added
        );
    }

    #[test]
    fn parse_hunks_splits_on_at_headers() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "diff --git a/a b/a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "--- a/a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "+++ b/a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -1,2 +1,3 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                text: " a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+b".into(),
            },
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -10,1 +11,1 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Delete,
                text: "-c".into(),
            },
        ];
        let hunks = parse_diff_hunks(&lines);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].start, 3);
        assert_eq!(hunks[0].end, 6);
        assert_eq!(hunks[1].start, 6);
        assert_eq!(hunks[1].end, 8);
    }

    #[test]
    fn push_change_builds_staging_lists() {
        let mut snap = StatusSnapshot::new(PathBuf::from("/repo"));
        snap.push_change("a.txt", GitStatus::Modified, DiffSide::Unstaged);
        snap.push_change("b.txt", GitStatus::Added, DiffSide::Staged);
        snap.push_change("c.txt", GitStatus::Ignored, DiffSide::Unstaged);
        snap.finish();
        assert_eq!(snap.unstaged.len(), 1);
        assert_eq!(snap.staged.len(), 1);
        assert_eq!(snap.entries.get("c.txt"), Some(&GitStatus::Ignored));
    }
}

//! Workspace search and explicit, non-overwriting file operations.
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

mod search;
pub use search::{search, search_stream, ContentContext, ContentMatcher, FileIndex};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
    pub score: usize,
    /// Filename and content matches have distinct display semantics.
    /// An empty content match (e.g. regex ^$) still has a real line number.
    pub content: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    pub content: bool,
    pub regex: bool,
    pub show_hidden: bool,
    pub extension: String,
    pub allowed: Option<HashSet<PathBuf>>,
    pub excluded: HashSet<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            content: false,
            regex: false,
            show_hidden: false,
            extension: String::new(),
            allowed: None,
            excluded: DEFAULT_EXCLUSIONS.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

pub const DEFAULT_EXCLUSIONS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
];

/// Subsequence matching, favoring consecutive characters and short basenames.
pub fn fuzzy_score(query: &str, name: &str) -> Option<usize> {
    let name = name.to_lowercase();
    let mut chars = query.chars().flat_map(char::to_lowercase).peekable();
    let mut score = 0;
    let mut gap = 0;
    for ch in name.chars() {
        if chars.peek() == Some(&ch) {
            chars.next();
            score += gap;
            gap = 0;
        } else {
            gap += 1;
        }
        if chars.peek().is_none() {
            return Some(score + name.len());
        }
    }
    if chars.peek().is_none() {
        Some(name.len())
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub enum FileOperation {
    NewFile(PathBuf),
    NewDirectory(PathBuf),
    Copy {
        paths: Vec<PathBuf>,
        destination: PathBuf,
    },
    Move {
        paths: Vec<PathBuf>,
        destination: PathBuf,
    },
    Rename {
        path: PathBuf,
        destination: PathBuf,
    },
    Delete(Vec<PathBuf>),
}

/// Validate lexical boundaries and every existing parent, without following the final symlink.
pub fn checked_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let relative = path
        .strip_prefix(&root)
        .context("path must be inside the workspace")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|p| matches!(p, Component::ParentDir) || p.as_os_str() == ".git")
    {
        bail!("workspace root, parent traversal and .git are protected");
    }
    let mut parent = path.parent().context("path has no parent")?;
    while !parent.exists() {
        parent = parent.parent().context("missing workspace parent")?;
    }
    if !parent.canonicalize()?.starts_with(root) {
        bail!("symlink parent leaves the workspace");
    }
    Ok(path)
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub fn unique_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !out.iter().any(|parent| p.starts_with(parent)) {
            out.push(p);
        }
    }
    out
}

pub fn operate(root: &Path, operation: FileOperation) -> Result<String> {
    match operation {
        FileOperation::NewFile(path) => {
            let path = checked_path(root, &path)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok(format!("Created {}", path.display()))
        }
        FileOperation::NewDirectory(path) => {
            let path = checked_path(root, &path)?;
            fs::create_dir(&path)?;
            Ok(format!("Created {}", path.display()))
        }
        FileOperation::Delete(paths) => {
            let paths = unique_roots(&paths);
            let paths = paths
                .iter()
                .map(|p| checked_path(root, p))
                .collect::<Result<Vec<_>>>()?;
            for path in &paths {
                fs::symlink_metadata(path)?;
            }
            for (index, path) in paths.iter().enumerate() {
                let result = if fs::symlink_metadata(path)?.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
                if let Err(error) = result {
                    return Err(batch_failure(
                        "Delete",
                        &paths,
                        index,
                        path,
                        &error,
                        "The failed item may also be partially deleted.",
                    ));
                }
            }
            Ok(format!("Deleted {} items", paths.len()))
        }
        FileOperation::Rename { path, destination } => {
            let path = checked_path(root, &path)?;
            let destination = checked_path(root, &destination)?;
            transfer(&path, &destination, false)?;
            Ok(format!("Renamed to {}", destination.display()))
        }
        FileOperation::Copy { paths, destination } => {
            transfer_many(root, &paths, &destination, true)
        }
        FileOperation::Move { paths, destination } => {
            transfer_many(root, &paths, &destination, false)
        }
    }
}

pub fn transfer_many(
    root: &Path,
    paths: &[PathBuf],
    destination: &Path,
    copy: bool,
) -> Result<String> {
    let destination = if destination.is_absolute() {
        destination.to_owned()
    } else {
        root.join(destination)
    };
    if !destination.is_dir() {
        bail!("destination must be an existing directory");
    }
    let mut pairs = Vec::new();
    let mut targets = HashSet::new();
    for path in unique_roots(paths) {
        let from = checked_path(root, &path)?;
        let to = checked_path(
            root,
            &destination.join(from.file_name().context("missing filename")?),
        )?;
        fs::symlink_metadata(&from)?;
        if exists(&to) || !targets.insert(to.clone()) {
            bail!("destination already exists: {}", to.display());
        }
        if to.starts_with(&from) {
            bail!("cannot move or copy a directory into itself");
        }
        pairs.push((from, to));
    }
    for (index, (from, to)) in pairs.iter().enumerate() {
        if let Err(error) = transfer(from, to, copy) {
            let sources: Vec<_> = pairs.iter().map(|(source, _)| source.clone()).collect();
            return Err(batch_failure(
                if copy { "Copy" } else { "Move" },
                &sources,
                index,
                from,
                &error,
                if copy {
                    "Earlier items were copied; the failed item has not been published."
                } else {
                    "Earlier items were moved and will not be automatically rolled back."
                },
            ));
        }
    }
    Ok(format!(
        "{} {} items",
        if copy { "Copied" } else { "Moved" },
        pairs.len()
    ))
}

/// Keep the fact of partial execution visible to callers. A failure cannot
/// imply that previous files are unchanged. This is also shown in the TUI's
/// Operation failed details.
fn batch_failure(
    kind: &str,
    paths: &[PathBuf],
    failed_index: usize,
    failed: &Path,
    error: &dyn std::fmt::Display,
    note: &str,
) -> anyhow::Error {
    let completed = paths[..failed_index]
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    anyhow::anyhow!(
        "{kind} partially completed: {} of {} succeeded; failed: {} ({error}); {} not attempted.{} {}",
        completed.len(),
        paths.len(),
        failed.display(),
        paths.len() - failed_index - 1,
        if completed.is_empty() {
            String::new()
        } else {
            format!(" Completed: {}.", completed.join(", "))
        },
        note
    )
}

fn transfer(from: &Path, to: &Path, copy: bool) -> Result<()> {
    if exists(to) {
        bail!("destination already exists: {}", to.display());
    }
    if to.starts_with(from) {
        bail!("destination is inside source");
    }
    if !copy {
        fs::rename(from, to)?;
        return Ok(());
    }

    let metadata = fs::symlink_metadata(from)?;
    let file_type = metadata.file_type();
    let parent = to.parent().context("destination has no parent")?;
    if file_type.is_symlink() {
        // Creating a symlink is itself atomic and refuses an existing name.
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        #[cfg(not(unix))]
        bail!("copying symlinks is unsupported on this platform");
    } else if file_type.is_dir() {
        // Never expose an incomplete directory at its final destination.
        // TempDir cleans staging recursively on *any* recursive copy error.
        let stage = tempfile::Builder::new()
            .prefix(".forge-copy-")
            .tempdir_in(parent)?;
        copy_directory_contents(from, stage.path(), metadata.permissions())?;
        if exists(to) {
            bail!("destination appeared while copying: {}", to.display());
        }
        fs::rename(stage.path(), to)?;
    } else if file_type.is_file() {
        // Validate/open source before allocating the destination. Persist
        // with no-clobber semantics so a racing writer cannot be overwritten.
        let source = fs::File::open(from)?;
        if !source.metadata()?.is_file() {
            bail!("source changed into a non-regular file: {}", from.display());
        }
        let mut stage = tempfile::Builder::new()
            .prefix(".forge-copy-")
            .tempfile_in(parent)?;
        std::io::copy(&mut &source, stage.as_file_mut())?;
        stage
            .as_file_mut()
            .set_permissions(metadata.permissions())?;
        stage.as_file_mut().sync_all()?;
        stage.persist_noclobber(to)?;
    } else {
        bail!(
            "unsupported file type (FIFO/socket/device): {}",
            from.display()
        );
    }
    Ok(())
}

/// This path is *only* used inside a private staging directory. The contents
/// become visible at the final name only after the whole tree succeeds.
fn copy_directory_contents(from: &Path, to: &Path, permissions: fs::Permissions) -> Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            bail!("copying Git metadata is not supported");
        }
        let src = entry.path();
        let dst = to.join(entry.file_name());
        transfer(&src, &dst, true)
            .map_err(|error| anyhow::anyhow!("copying {}: {error:#}", src.display()))?;
    }
    fs::set_permissions(to, permissions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn fuzzy_and_regex_search_respect_ignores_limits_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        for (path, text) in [
            ("src/nested/application.rs", "first\nlet count = 42;\n"),
            ("target/build.rs", "count = 42"),
            ("ignored.txt", "count = 42"),
            (".secret", "count = 42"),
            ("binary.dat", "\0count = 42"),
            ("readme.md", "count = 12"),
        ] {
            fs::write(root.join(path), text).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut options = SearchOptions::default();
        let hits = search(root, "aprs", &options, &cancel).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("application.rs"));
        options.content = true;
        options.regex = true;
        let hits = search(root, r"count = \d{2}", &options, &cancel).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.line == 2));
        options.extension = "rs".into();
        let hits = search(root, "count", &options, &cancel).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(search(root, "[", &options, &cancel).is_err());
        options.regex = false;
        cancel.store(true, Ordering::Relaxed);
        assert!(search(root, "count", &options, &cancel).unwrap().is_empty());
    }

    #[test]
    fn regex_empty_line_hit_has_content_origin_and_correct_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("hello.txt"), "first\n\nthird\n").unwrap();
        let mut options = SearchOptions::default();
        options.content = true;
        options.regex = true;
        let cancel = AtomicBool::new(false);
        let hits = search(root, "^$", &options, &cancel).unwrap();
        assert!(hits.iter().any(|hit| hit.content
            && hit.text.is_empty()
            && hit.line == 2
            && hit.path.ends_with("hello.txt")));
        options.content = false;
        let names = search(root, "hello", &options, &cancel).unwrap();
        assert!(names
            .iter()
            .any(|hit| !hit.content && hit.line == 1 && hit.path.ends_with("hello.txt")));
    }

    #[test]
    fn file_operations_are_recursive_and_never_overwrite_or_cross_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        operate(root, FileOperation::NewDirectory("src".into())).unwrap();
        operate(root, FileOperation::NewDirectory("dest".into())).unwrap();
        operate(root, FileOperation::NewDirectory("src/nested".into())).unwrap();
        operate(root, FileOperation::NewFile("src/nested/a.txt".into())).unwrap();
        fs::write(root.join("src/nested/a.txt"), "keep").unwrap();
        assert!(operate(root, FileOperation::NewFile("src/nested/a.txt".into())).is_err());
        operate(
            root,
            FileOperation::Copy {
                paths: vec![root.join("src"), root.join("src/nested")],
                destination: "dest".into(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("dest/src/nested/a.txt")).unwrap(),
            "keep"
        );
        assert!(operate(
            root,
            FileOperation::Copy {
                paths: vec![root.join("src")],
                destination: "dest".into()
            }
        )
        .is_err());
        operate(
            root,
            FileOperation::Rename {
                path: "src".into(),
                destination: "renamed".into(),
            },
        )
        .unwrap();
        operate(
            root,
            FileOperation::Move {
                paths: vec![root.join("renamed")],
                destination: "dest".into(),
            },
        )
        .unwrap();
        assert!(!root.join("renamed").exists());
        assert!(root.join("dest/renamed/nested/a.txt").exists());
        assert!(operate(root, FileOperation::NewFile("../escape".into())).is_err());
        assert!(operate(root, FileOperation::Delete(vec![root.to_owned()])).is_err());
        assert!(operate(root, FileOperation::NewFile(".git/config".into())).is_err());
        operate(root, FileOperation::Delete(vec![root.join("dest")])).unwrap();
        assert!(!root.join("dest").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copying_fifo_fails_fast_without_publishing_any_destination() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("dest")).unwrap();
        let fifo = root.join("source.pipe");
        // mkfifo command is available on Linux/WSL. A blocking File::open on
        // this path would hang the test indefinitely; metadata must gate it.
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let now = std::time::Instant::now();
        let error = operate(
            root,
            FileOperation::Copy {
                paths: vec![fifo],
                destination: root.join("dest"),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(now.elapsed() < std::time::Duration::from_secs(2));
        assert!(error.contains("unsupported file type"), "{error}");
        assert!(!root.join("dest/source.pipe").exists());
        assert!(!fs::read_dir(root.join("dest")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .as_bytes()
            .starts_with(b".forge-copy-")));
    }

    #[cfg(unix)]
    #[test]
    fn directory_copy_failure_cleans_staging_and_never_publishes_partial_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.txt"), "completed staging file").unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(root.join("src/z.pipe"))
            .status()
            .unwrap()
            .success());
        fs::create_dir(root.join("dest")).unwrap();
        let error = operate(
            root,
            FileOperation::Copy {
                paths: vec![root.join("src")],
                destination: root.join("dest"),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unsupported file type"), "{error}");
        assert!(!root.join("dest/src").exists());
        assert!(
            fs::read_dir(root.join("dest")).unwrap().next().is_none(),
            "failed recursive copy must clean all staging artifacts"
        );
        assert_eq!(
            fs::read_to_string(root.join("src/a.txt")).unwrap(),
            "completed staging file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn batch_copy_reports_partial_success_and_does_not_create_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a-good.txt"), "safe complete copy").unwrap();
        let fifo = root.join("z-bad.pipe");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        fs::create_dir(root.join("dest")).unwrap();
        let error = operate(
            root,
            FileOperation::Copy {
                paths: vec![fifo, root.join("a-good.txt")],
                destination: root.join("dest"),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("1 of 2 succeeded"), "{error}");
        assert!(error.contains("z-bad.pipe"), "{error}");
        assert!(error.contains("Completed:"), "{error}");
        assert_eq!(
            fs::read_to_string(root.join("dest/a-good.txt")).unwrap(),
            "safe complete copy"
        );
        assert!(!root.join("dest/z-bad.pipe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_source_cannot_leave_a_zero_byte_destination() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("dest")).unwrap();
        let from = root.join("unreadable.txt");
        fs::write(&from, "original private bytes").unwrap();
        fs::set_permissions(&from, fs::Permissions::from_mode(0o000)).unwrap();
        let failure = operate(
            root,
            FileOperation::Copy {
                paths: vec![from.clone()],
                destination: root.join("dest"),
            },
        );
        fs::set_permissions(&from, fs::Permissions::from_mode(0o600)).unwrap();
        // Root can read permission-000 files; the ordinary user must reject.
        if let Err(error) = failure {
            assert!(error.to_string().contains("Permission denied"));
            assert!(
                fs::read_dir(root.join("dest")).unwrap().next().is_none(),
                "no empty target and no staging junk on read failure"
            );
        }
    }

    #[test]
    fn batch_delete_reports_successful_items_before_a_later_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "will delete").unwrap();
        fs::create_dir(root.join("z")).unwrap();
        // Force a failure inside second target by making its contents
        // inaccessible on Unix. Ordinary permission semantics vary with uid;
        // the Copy/FIFO cases above cover deterministic failure separately.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(root.join("z/inside.txt"), "keep").unwrap();
            fs::set_permissions(root.join("z"), fs::Permissions::from_mode(0o000)).unwrap();
            let result = operate(
                root,
                FileOperation::Delete(vec![root.join("z"), root.join("a.txt")]),
            );
            fs::set_permissions(root.join("z"), fs::Permissions::from_mode(0o700)).ok();
            if let Err(error) = result {
                let message = error.to_string();
                assert!(message.contains("1 of 2 succeeded"), "{message}");
                assert!(message.contains("z"), "{message}");
                assert!(!root.join("a.txt").exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_copied_as_links_and_cannot_escape_via_parent() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        assert!(operate(root, FileOperation::NewFile("escape/pwn".into())).is_err());
        fs::create_dir(root.join("copy")).unwrap();
        operate(
            root,
            FileOperation::Copy {
                paths: vec![root.join("escape")],
                destination: "copy".into(),
            },
        )
        .unwrap();
        assert!(fs::symlink_metadata(root.join("copy/escape"))
            .unwrap()
            .is_symlink());
        operate(root, FileOperation::Delete(vec![root.join("escape")])).unwrap();
        assert!(outside.path().exists());
    }
}

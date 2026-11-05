//! Workspace search and explicit, non-overwriting file operations.
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
    pub score: usize,
}

#[derive(Clone, Debug)]
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
            excluded: ["target", "node_modules"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

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

/// Ignore-aware, bounded search. Cancellation is checked between files and lines.
pub fn search(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    if options.content && query.is_empty() {
        return Ok(Vec::new());
    }
    let regex = if options.content && options.regex {
        Some(regex::Regex::new(query)?)
    } else {
        None
    };
    let excluded = options.excluded.clone();
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(!options.show_hidden)
        .follow_links(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0
                || (name != ".git"
                    && !(e.file_type().is_some_and(|t| t.is_dir())
                        && excluded.contains(name.as_ref())))
        });
    let mut hits = Vec::new();
    for entry in walk.build() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if !options.extension.is_empty()
            && path.extension().and_then(|x| x.to_str())
                != Some(options.extension.trim_start_matches('.'))
        {
            continue;
        }
        if options
            .allowed
            .as_ref()
            .is_some_and(|set| !set.contains(path))
        {
            continue;
        }
        if !options.content {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if let Some(score) = fuzzy_score(query, &name) {
                hits.push(SearchHit {
                    path: path.to_owned(),
                    line: 1,
                    text: String::new(),
                    score,
                });
            }
        } else {
            // Never load an unbounded or binary file into the search worker.
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 2 * 1024 * 1024 || bytes.contains(&0) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if regex
                    .as_ref()
                    .map_or_else(|| line.contains(query), |r| r.is_match(line))
                {
                    hits.push(SearchHit {
                        path: path.to_owned(),
                        line: index + 1,
                        text: line.chars().take(400).collect(),
                        score: 0,
                    });
                    if hits.len() >= 2000 {
                        break;
                    }
                }
            }
        }
        if hits.len() >= 2000 {
            break;
        }
    }
    hits.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    Ok(hits)
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
            for path in &paths {
                if fs::symlink_metadata(path)?.is_dir() {
                    fs::remove_dir_all(path)?;
                } else {
                    fs::remove_file(path)?;
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
    for (from, to) in &pairs {
        transfer(from, to, copy)?;
    }
    Ok(format!(
        "{} {} items",
        if copy { "Copied" } else { "Moved" },
        pairs.len()
    ))
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
    let meta = fs::symlink_metadata(from)?;
    if meta.is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        #[cfg(not(unix))]
        bail!("copying symlinks is unsupported on this platform");
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                bail!("copying Git metadata is not supported");
            }
            transfer(&entry.path(), &to.join(entry.file_name()), true)?;
        }
        fs::set_permissions(to, meta.permissions())?;
    } else {
        // create_new prevents accidental replacement if a target appears during copying.
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        std::io::copy(&mut fs::File::open(from)?, &mut out)?;
        fs::set_permissions(to, meta.permissions())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

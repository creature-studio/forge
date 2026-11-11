//! Bounded, in-process search. No external ripgrep executable is required.
use super::{fuzzy_score, SearchHit, SearchOptions, DEFAULT_EXCLUSIONS};
use anyhow::Result;
use ignore::{WalkBuilder, WalkState};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

const MAX_HITS: usize = 2000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PATHS: usize = 100_000;
const MAX_INDEX_BYTES: usize = 32 * 1024 * 1024;

/// The same bounded UTF-8 text domain used by content search.
pub struct ContentContext {
    pub lines: Vec<String>,
}

impl ContentContext {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self> {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Context cancelled");
        anyhow::ensure!(
            fs::metadata(path)?.is_file(),
            "Context is not a regular file"
        );
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "Context is not a regular file");
        anyhow::ensure!(metadata.len() <= MAX_FILE_BYTES, "Context exceeds 2 MiB");
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "Context exceeds 2 MiB"
        );
        anyhow::ensure!(!bytes.contains(&0), "Binary file: context unavailable");
        let text = std::str::from_utf8(&bytes)?;
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Context cancelled");
        Ok(Self {
            lines: text.lines().map(str::to_owned).collect(),
        })
    }
}

/// Identical case-sensitive literal / regex semantics to the scanner.
pub struct ContentMatcher(regex::Regex);

impl ContentMatcher {
    pub fn new(query: &str, regex: bool) -> Result<Self> {
        Ok(Self(regex::Regex::new(&if regex {
            query.to_owned()
        } else {
            regex::escape(query)
        })?))
    }

    pub fn ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        self.0
            .find_iter(text)
            .take(512)
            .map(|m| m.start()..m.end())
            .collect()
    }
}

fn effective_options(options: &SearchOptions) -> SearchOptions {
    let mut options = options.clone();
    options.content = false;
    options.regex = false;
    options.extension = options.extension.trim_start_matches('.').into();
    options
        .excluded
        .extend(DEFAULT_EXCLUSIONS.iter().map(|s| (*s).into()));
    options
}

fn walker(root: &Path, options: &SearchOptions) -> WalkBuilder {
    let excluded = effective_options(options).excluded;
    let mut walk = WalkBuilder::new(root);
    walk.hidden(!options.show_hidden)
        .follow_links(false)
        .threads(
            std::thread::available_parallelism()
                .map_or(2, usize::from)
                .clamp(1, 4),
        )
        .filter_entry(move |entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git"
                && (entry.depth() == 0
                    || !(entry.file_type().is_some_and(|t| t.is_dir())
                        && excluded.contains(name.as_ref())))
        });
    walk
}

fn eligible(path: &Path, options: &SearchOptions) -> bool {
    (options.extension.is_empty()
        || path.extension().and_then(|s| s.to_str())
            == Some(options.extension.trim_start_matches('.')))
        && options
            .allowed
            .as_ref()
            .is_none_or(|set| set.contains(path))
}

/// A single workspace snapshot. Queries never touch the filesystem.
/// The owner invalidates this snapshot on watcher events and workspace changes.
pub struct FileIndex {
    root: PathBuf,
    options: SearchOptions,
    paths: Vec<PathBuf>,
    pub truncated: bool,
    pub skipped: usize,
}

impl FileIndex {
    pub fn matches(&self, root: &Path, options: &SearchOptions) -> bool {
        self.root == root && self.options == effective_options(options)
    }

    pub fn build(root: &Path, options: &SearchOptions, cancel: &AtomicBool) -> Result<Self> {
        // Report an inaccessible root, but tolerate individual disappearing/unreadable files.
        fs::read_dir(root)?;
        let paths = Mutex::new((Vec::new(), 0usize, false));
        let skipped = AtomicUsize::new(0);
        walker(root, options).build_parallel().run(|| {
            Box::new(|entry| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        return WalkState::Continue;
                    }
                };
                if entry.file_type().is_some_and(|t| t.is_file()) && eligible(entry.path(), options)
                {
                    let mut state = paths.lock().unwrap();
                    let bytes = entry.path().as_os_str().len() + std::mem::size_of::<PathBuf>();
                    if state.0.len() >= MAX_PATHS || state.1 + bytes > MAX_INDEX_BYTES {
                        state.2 = true;
                        return WalkState::Quit;
                    }
                    state.1 += bytes;
                    state.0.push(entry.into_path());
                }
                WalkState::Continue
            })
        });
        let (mut paths, _, truncated) = paths.into_inner().unwrap();
        paths.sort();
        Ok(Self {
            root: root.to_owned(),
            options: effective_options(options),
            paths,
            truncated,
            skipped: skipped.into_inner(),
        })
    }

    pub fn query(&self, query: &str, cancel: &AtomicBool) -> Vec<SearchHit> {
        // Keep only the best 2,000 rows, even for an empty or very broad query.
        let mut hits = std::collections::BinaryHeap::new();
        for (index, path) in self.paths.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            if let Some(score) = fuzzy_score(
                query,
                &path.file_name().unwrap_or_default().to_string_lossy(),
            ) {
                hits.push((score, index));
                if hits.len() > MAX_HITS {
                    hits.pop();
                }
            }
        }
        hits.into_sorted_vec()
            .into_iter()
            .map(|(score, index)| SearchHit {
                path: self.paths[index].clone(),
                line: 1,
                text: String::new(),
                score,
                content: false,
            })
            .collect()
    }
}

pub fn search(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    if !options.content {
        return Ok(FileIndex::build(root, options, cancel)?.query(query, cancel));
    }
    search_stream(root, query, options, cancel, |_| {})
}

/// Parallel fallback using the existing regex engine and ignore walker.
/// Each worker reuses a bounded read buffer; the shared hit budget stops traversal early.
/// The callback must be fast and nonblocking. It receives at most 2,000 hits.
pub fn search_stream(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
    on_hit: impl Fn(&SearchHit) + Sync,
) -> Result<Vec<SearchHit>> {
    if query.is_empty() || cancel.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    let regex = if options.regex {
        Some(regex::Regex::new(query)?)
    } else {
        None
    };
    fs::read_dir(root)?;
    let hits = Mutex::new(Vec::new());
    let full = AtomicBool::new(false);
    walker(root, options).build_parallel().run(|| {
        let mut bytes = Vec::new();
        let hits = &hits;
        let full = &full;
        let on_hit = &on_hit;
        // Regex scratch space is per worker instead of contended across threads.
        let regex = regex.clone();
        Box::new(move |entry| {
            if cancel.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) || !eligible(entry.path(), options) {
                return WalkState::Continue;
            }
            let path = entry.path();
            let Ok(file) = fs::File::open(path) else {
                return WalkState::Continue;
            };
            if file.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
                return WalkState::Continue;
            }
            bytes.clear();
            if file
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_FILE_BYTES
                || bytes.contains(&0)
            {
                return WalkState::Continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                return WalkState::Continue;
            };
            for (index, line) in text.lines().enumerate() {
                if cancel.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                if regex
                    .as_ref()
                    .map_or_else(|| line.contains(query), |r| r.is_match(line))
                {
                    let mut hits = hits.lock().unwrap();
                    if hits.len() == MAX_HITS || cancel.load(Ordering::Relaxed) {
                        return WalkState::Quit;
                    }
                    let hit = SearchHit {
                        path: path.to_owned(),
                        line: index + 1,
                        text: line.chars().take(400).collect(),
                        score: 0,
                        content: true,
                    };
                    on_hit(&hit);
                    hits.push(hit);
                    if hits.len() == MAX_HITS {
                        full.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                }
            }
            WalkState::Continue
        })
    });
    let mut hits = hits.into_inner().unwrap();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn context_is_bounded_and_reports_unreadable_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context");
        let cancel = AtomicBool::new(false);
        assert!(ContentContext::load(&path, &cancel).is_err());
        for bytes in [
            b"binary\0".to_vec(),
            b"invalid\xff".to_vec(),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(ContentContext::load(&path, &cancel).is_err());
        }
        fs::write(&path, "before\r\n界needle[42]\r\nafter").unwrap();
        let context = ContentContext::load(&path, &cancel).unwrap();
        assert_eq!(context.lines, ["before", "界needle[42]", "after"]);
        assert_eq!(
            ContentMatcher::new("needle[42]", false)
                .unwrap()
                .ranges(&context.lines[1]),
            [3..13]
        );
        assert_eq!(
            ContentMatcher::new(r"界needle\[\d+\]", true)
                .unwrap()
                .ranges(&context.lines[1]),
            [0..13]
        );
        assert!(ContentMatcher::new("NEEDLE", false)
            .unwrap()
            .ranges(&context.lines[1])
            .is_empty());
        assert!(ContentMatcher::new("[", true).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(ContentContext::load(&path, &cancel).is_err());
    }

    #[test]
    fn custom_exclusions_extend_defaults_and_git_is_always_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for folder in DEFAULT_EXCLUSIONS
            .iter()
            .copied()
            .chain([".git", "custom", "src"])
        {
            fs::create_dir(root.join(folder)).unwrap();
            fs::write(root.join(folder).join("match.txt"), "needle").unwrap();
        }
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "needle").unwrap();
        fs::write(root.join(".hidden"), "needle").unwrap();
        let mut options = SearchOptions {
            show_hidden: true,
            excluded: ["custom".into()].into(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let index = FileIndex::build(root, &options, &cancel).unwrap();
        let hits = index.query("match", &cancel);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, root.join("src/match.txt"));
        options.content = true;
        let hits = search(root, "needle", &options, &cancel).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|hit| hit.path == root.join(".hidden")));
    }

    #[test]
    fn cached_queries_do_not_enumerate_or_read_disk_and_keys_include_options() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("application.rs"), "hello").unwrap();
        let cancel = AtomicBool::new(false);
        let options = SearchOptions::default();
        let index = FileIndex::build(&root, &options, &cancel).unwrap();
        assert_eq!(index.query("app", &cancel).len(), 1);
        // Even the directory no longer exists: a second query must be memory-only.
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(index.query("aprs", &cancel).len(), 1);
        assert!(index.matches(&root, &options));
        assert!(!index.matches(dir.path(), &options));
        let mut changed = options.clone();
        changed.show_hidden = true;
        assert!(!index.matches(&root, &changed));
        changed = options.clone();
        changed.allowed = Some(HashSet::new());
        assert!(!index.matches(&root, &changed));
        cancel.store(true, Ordering::Relaxed);
        assert!(index.query("app", &cancel).is_empty());
    }

    #[test]
    fn parallel_content_stream_preserves_lines_literals_limits_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("code.txt"),
            format!("first\r\nneedle[42]\r\n{}\n", "界".repeat(600)),
        )
        .unwrap();
        fs::write(root.join("binary"), b"needle\0").unwrap();
        fs::write(root.join("invalid"), b"needle\xff").unwrap();
        fs::write(root.join("large"), vec![b'n'; MAX_FILE_BYTES as usize + 1]).unwrap();
        let cancel = AtomicBool::new(false);
        let mut options = SearchOptions {
            content: true,
            ..Default::default()
        };
        let streamed = Mutex::new(Vec::new());
        let hits = search_stream(root, "needle[42]", &options, &cancel, |h| {
            streamed.lock().unwrap().push(h.line)
        })
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        assert_eq!(*streamed.lock().unwrap(), [2]);
        assert_eq!(
            search(root, "界", &options, &cancel).unwrap()[0]
                .text
                .chars()
                .count(),
            400
        );
        options.regex = true;
        assert_eq!(
            search(root, r"needle\[\d+\]", &options, &cancel)
                .unwrap()
                .len(),
            1
        );
        assert!(search(root, "[", &options, &cancel).is_err());
        assert!(search(root, "", &options, &cancel).unwrap().is_empty());
        options.regex = false;
        fs::write(root.join("many"), "needle\n".repeat(3000)).unwrap();
        assert_eq!(
            search(root, "needle", &options, &cancel).unwrap().len(),
            MAX_HITS
        );
        let calls = AtomicUsize::new(0);
        search_stream(root, "needle", &options, &cancel, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap();
        // A worker can already be waiting for the hit lock when cancellation occurs.
        assert!(calls.load(Ordering::Relaxed) <= 4);
        assert!(search(root, "needle", &options, &cancel)
            .unwrap()
            .is_empty());
        assert!(
            FileIndex::build(&root.join("missing"), &options, &AtomicBool::new(false)).is_err()
        );
    }

    #[test]
    fn filename_ranking_considers_files_after_the_result_limit() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        // Construct a snapshot directly to isolate ranking from traversal order.
        let mut paths: Vec<_> = (0..2500)
            .map(|n| dir.path().join(format!("a{n:04}-very-long.rs")))
            .collect();
        paths.push(dir.path().join("z.rs"));
        let index = FileIndex {
            root: dir.path().into(),
            options: SearchOptions::default(),
            paths,
            truncated: false,
            skipped: 0,
        };
        let hits = index.query("rs", &cancel);
        assert_eq!(hits.len(), MAX_HITS);
        assert!(hits[0].path.ends_with("z.rs"));
    }

    #[test]
    #[ignore = "explicit timing fixture; run with --ignored --nocapture"]
    fn search_timings() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..4000 {
            fs::write(
                dir.path().join(format!("file-{n:04}.rs")),
                "line one\nlet needle = 42;\n".repeat(100),
            )
            .unwrap();
        }
        let cancel = AtomicBool::new(false);
        let options = SearchOptions::default();
        let start = std::time::Instant::now();
        let index = FileIndex::build(dir.path(), &options, &cancel).unwrap();
        let indexed = start.elapsed();
        let start = std::time::Instant::now();
        for query in ["f", "fi", "file", "file3", "file39"] {
            index.query(query, &cancel);
        }
        let queried = start.elapsed();
        let start = std::time::Instant::now();
        let content = SearchOptions {
            content: true,
            ..options
        };
        let hits = search(dir.path(), "needle", &content, &cancel).unwrap();
        let matched = start.elapsed();
        let start = std::time::Instant::now();
        search(dir.path(), "absent-pattern", &content, &cancel).unwrap();
        println!("4,000 files / 10 MB: index={indexed:?}; five cached queries={queried:?}; content cap={} in {matched:?}; full no-match scan={:?}", hits.len(), start.elapsed());
    }
}

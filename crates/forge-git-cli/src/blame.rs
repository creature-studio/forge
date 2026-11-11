//! Bounded, read-only blame of the working file (including index/worktree edits).
use super::*;
use std::time::Duration;

pub const MAX_BLAME_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub oid: String,
    pub author: String,
    pub time: i64,
    pub summary: String,
    /// Path in this commit, which can predate a rename.
    pub path: String,
}

impl BlameLine {
    pub fn uncommitted(&self) -> bool {
        self.oid.bytes().all(|b| b == b'0')
    }
}

impl CliWriteBackend {
    /// Only attribute the lines the preview actually loaded, at most 400.
    /// A deadline and file cap bound cost even for pathological histories.
    pub async fn blame(
        &self,
        repo: &Path,
        path: &Path,
        line_count: usize,
    ) -> Result<Vec<BlameLine>, GitWriteError> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let relative = path
                .strip_prefix(repo)
                .map_err(|e| GitWriteError::Command {
                    op: "blame",
                    code: -1,
                    stderr: e.to_string(),
                })?;
            let metadata = tokio::fs::symlink_metadata(path).await?;
            if !metadata.is_file() || metadata.len() > MAX_BLAME_BYTES || line_count == 0 {
                return Ok(Vec::new());
            }
            let count = line_count.min(400);
            let path_arg = relative.to_string_lossy().into_owned();
            let tracked = run_git(
                repo,
                "blame",
                vec![
                    "--literal-pathspecs".into(),
                    "ls-files".into(),
                    "-z".into(),
                    "--".into(),
                    path_arg.clone(),
                ],
            )
            .await?;
            // An index addition in an unborn repository has no commit either.
            let head = run_git_raw(
                repo,
                "blame",
                vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
            )
            .await?;
            let in_head = if head.status.success() {
                run_git(
                    repo,
                    "blame",
                    vec![
                        "--literal-pathspecs".into(),
                        "ls-tree".into(),
                        "-z".into(),
                        "HEAD".into(),
                        "--".into(),
                        path_arg.clone(),
                    ],
                )
                .await?
                .stdout
            } else {
                Vec::new()
            };
            if tracked.stdout.is_empty() || in_head.is_empty() {
                return Ok(vec![
                    BlameLine {
                        oid: "0".repeat(40),
                        author: String::new(),
                        time: 0,
                        summary: "Uncommitted changes".into(),
                        path: path_arg,
                    };
                    count
                ]);
            }
            let text = run_git(
                repo,
                "blame",
                vec![
                    "--literal-pathspecs".into(),
                    "-c".into(),
                    "core.quotePath=false".into(),
                    "blame".into(),
                    "--line-porcelain".into(),
                    "-L".into(),
                    format!("1,{count}"),
                    "--".into(),
                    path_arg,
                ],
            )
            .await?;
            Ok(parse_porcelain(&String::from_utf8_lossy(&text.stdout)))
        })
        .await
        .map_err(|_| GitWriteError::Command {
            op: "blame",
            code: -1,
            stderr: "Blame timed out after 5 seconds".into(),
        })?
    }
}

fn parse_porcelain(text: &str) -> Vec<BlameLine> {
    let mut result = Vec::new();
    let mut record = BlameLine {
        oid: String::new(),
        author: String::new(),
        time: 0,
        summary: String::new(),
        path: String::new(),
    };
    for line in text.lines() {
        if line.starts_with('\t') {
            if !record.oid.is_empty() {
                result.push(record.clone());
            }
        } else if let Some(value) = line.strip_prefix("author ") {
            record.author = value.into();
        } else if let Some(value) = line.strip_prefix("author-time ") {
            record.time = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("summary ") {
            record.summary = value.into();
        } else if let Some(value) = line.strip_prefix("filename ") {
            record.path = unquote_path(value);
        } else {
            let oid = line.split(' ').next().unwrap_or("");
            if matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                record = BlameLine {
                    oid: oid.into(),
                    author: String::new(),
                    time: 0,
                    summary: String::new(),
                    path: String::new(),
                };
            }
        }
    }
    result
}

/// Git C-quotes filenames containing tabs, newlines, quotes and non-UTF8 bytes.
fn unquote_path(value: &str) -> String {
    let Some(value) = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return value.into();
    };
    let mut bytes = Vec::new();
    let mut input = value.bytes().peekable();
    while let Some(b) = input.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match input.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b @ b'0'..=b'7') => {
                let mut n = (b - b'0') as u16;
                for _ in 0..2 {
                    if input.peek().is_some_and(|b| matches!(b, b'0'..=b'7')) {
                        n = n * 8 + (input.next().unwrap() - b'0') as u16;
                    }
                }
                bytes.push(n as u8);
            }
            Some(b) => bytes.push(b),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn porcelain_preserves_authors_repeated_commits_and_uncommitted_lines() {
        let oid = "a".repeat(40);
        let text = format!("{oid} 1 1 1\nauthor 张三\nauthor-time 42\nsummary first\nfilename old.rs\n\t中文\n{oid} 2 2 1\nauthor 张三\nauthor-time 42\nsummary first\nfilename old.rs\n\tsecond\n{} 3 3 1\nauthor Not Committed Yet\nsummary version\nfilename new.rs\n\tedit\n", "0".repeat(40));
        let lines = parse_porcelain(&text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].author, "张三");
        assert_eq!(lines[1].oid, oid);
        assert!(lines[2].uncommitted());
        assert_eq!(unquote_path(r#""a\t\344\270\255.rs""#), "a\t中.rs");
    }

    fn git(root: &Path, args: &[&str]) {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Blame Author")
            .env("GIT_AUTHOR_EMAIL", "blame@example.invalid")
            .env("GIT_COMMITTER_NAME", "Blame Author")
            .env("GIT_COMMITTER_EMAIL", "blame@example.invalid")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[tokio::test]
    async fn working_tree_blame_tracks_commit_edits_additions_and_rename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        let file = root.join("-中文 file.txt");
        std::fs::write(
            &file,
            "first\n原始内容\nthird unchanged padding for rename detection\n",
        )
        .unwrap();
        git(root, &["add", "--", "-中文 file.txt"]);
        git(
            root,
            &["commit", "-qm", "original subject", "-m", "full body"],
        );
        let backend = CliWriteBackend::new();
        let original = backend.blame(root, &file, 3).await.unwrap();
        assert_eq!(original.len(), 3);
        assert_eq!(original[1].author, "Blame Author");
        assert_eq!(original[1].summary, "original subject");
        std::fs::write(
            &file,
            "first\nchanged\nthird unchanged padding for rename detection\n",
        )
        .unwrap();
        let edited = backend.blame(root, &file, 3).await.unwrap();
        assert_eq!(edited[0].oid, original[0].oid);
        assert!(edited[1].uncommitted());
        assert!(!edited[2].uncommitted());
        // Index-only content is also attributed against HEAD.
        git(root, &["add", "--", "-中文 file.txt"]);
        assert!(backend.blame(root, &file, 3).await.unwrap()[1].uncommitted());
        let new = root.join("new.txt");
        std::fs::write(&new, "new\n").unwrap();
        assert!(backend.blame(root, &new, 1).await.unwrap()[0].uncommitted());
        git(root, &["add", "new.txt"]);
        assert!(backend.blame(root, &new, 1).await.unwrap()[0].uncommitted());
        git(root, &["mv", "--", "-中文 file.txt", "renamed.txt"]);
        git(root, &["commit", "-qm", "rename and edit"]);
        let renamed = backend
            .blame(root, &root.join("renamed.txt"), 3)
            .await
            .unwrap();
        assert_eq!(renamed[0].oid, original[0].oid);
        assert_eq!(renamed[0].path, "-中文 file.txt");
        assert_eq!(renamed[1].summary, "rename and edit");
        let (_, detail) = backend
            .query(root, GitQuery::Commit(original[0].oid.clone()))
            .await
            .unwrap();
        assert!(detail.contains(&original[0].oid) && detail.contains("full body"));
    }

    #[tokio::test]
    async fn unborn_empty_and_oversized_files_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        let file = root.join("new.txt");
        std::fs::write(&file, "first\n").unwrap();
        git(root, &["add", "new.txt"]);
        let backend = CliWriteBackend::new();
        assert!(backend.blame(root, &file, 1).await.unwrap()[0].uncommitted());
        std::fs::write(&file, "").unwrap();
        assert!(backend.blame(root, &file, 0).await.unwrap().is_empty());
        std::fs::File::create(&file)
            .unwrap()
            .set_len(MAX_BLAME_BYTES + 1)
            .unwrap();
        assert!(backend.blame(root, &file, 400).await.unwrap().is_empty());
    }
}

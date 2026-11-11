//! Source-line attribution and an independent, read-only commit overlay.
use super::*;
use forge_git::{BlameLine, GitQuery};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const CACHE_FILES: usize = 8;
const MAX_BYTES: usize = 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    head: Option<String>,
    index: Option<(std::time::SystemTime, u64)>,
    content: u64,
}

#[derive(Clone)]
struct Cached {
    key: CacheKey,
    lines: Result<Arc<Vec<BlameLine>>, String>,
}

struct ReadResult {
    cached: Cached,
    refreshed: Option<Preview>,
}

#[derive(Default)]
pub(super) struct BlameState {
    cache: Vec<Cached>,
    active: Option<Cached>,
    pending: Option<(PathBuf, JobHandle<Result<ReadResult>>)>,
    checked: Option<(PathBuf, Instant)>,
    /// Clicked visual row; scrolling makes the top visible row current again.
    pub cursor: Option<(PathBuf, usize, usize)>,
    pub popup: Option<CommitPopup>,
}

pub(super) struct CommitPopup {
    pub oid: String,
    pub path: String,
    pub repo: PathBuf,
    pub title: String,
    pub text: String,
    pub scroll: u16,
    pub message: String,
    pub max_scroll: u16,
    pending: Option<JobHandle<Result<String>>>,
}

fn hash(value: impl Hash) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut state);
    state.finish()
}

fn source_hash(preview: &Preview) -> u64 {
    match &preview.content {
        PreviewContent::Text { lines, .. } => hash(lines),
        _ => 0,
    }
}

fn revision(
    git: GitService,
    repo: &Path,
) -> (Option<String>, Option<(std::time::SystemTime, u64)>) {
    let head = git.read.head_oid(repo);
    let index = git
        .read
        .index_path(repo)
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
    (head, index)
}

async fn read_blame(
    git: GitService,
    repo: PathBuf,
    path: PathBuf,
    expected: u64,
    cached: Option<Cached>,
) -> Result<ReadResult> {
    use tokio::io::AsyncReadExt;
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= MAX_BYTES as u64,
        "Blame skipped: file exceeds 1 MiB or is not a regular file"
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(&path)
        .await?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Blame skipped: file exceeds 1 MiB"
    );
    let root = repo.clone();
    let (head, index) = tokio::task::spawn_blocking(move || revision(git, &root)).await?;
    let key = CacheKey {
        path: path.clone(),
        head,
        index,
        content: hash(&bytes),
    };
    let fresh = forge_fs::preview_from_bytes(
        &path,
        &bytes[..bytes.len().min(forge_fs::PREVIEW_MAX_BYTES)],
        metadata.len(),
    );
    let refreshed = (source_hash(&fresh) != expected).then_some(fresh.clone());
    if let Some(cached) = cached.filter(|c| c.key == key) {
        return Ok(ReadResult { cached, refreshed });
    }
    let count = match fresh.content {
        PreviewContent::Text { ref lines, .. } => lines.len(),
        _ => 0,
    };
    let lines = git
        .blame(&repo, &path, count)
        .await
        .map(Arc::new)
        .map_err(|e| e.to_string());
    // Do not install attribution if a save or a branch change raced the read.
    let mut after = Vec::new();
    tokio::fs::File::open(&path)
        .await?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut after)
        .await?;
    anyhow::ensure!(
        hash(&after) == key.content,
        "File changed during blame; retrying"
    );
    let revision_after = tokio::task::spawn_blocking(move || revision(git, &repo)).await?;
    anyhow::ensure!(
        (key.head.clone(), key.index) == revision_after,
        "HEAD or index changed during blame; retrying"
    );
    Ok(ReadResult {
        cached: Cached { key, lines },
        refreshed,
    })
}

impl App {
    /// File content is the only eligible target. A directory selection does
    /// not create a preview or initiate a blame request for that directory.
    pub(super) fn blame_path(&self) -> Option<PathBuf> {
        if !self.config.git.blame.enabled
            || self.in_git_sidebar()
            || self.preview_state.preview_loading
            || !matches!(self.preview_state.preview_mode, PreviewMode::File)
        {
            return None;
        }
        let preview = self.preview_state.preview.as_ref()?;
        matches!(preview.content, PreviewContent::Text { .. }).then(|| preview.path.clone())
    }

    pub(super) fn reset_blame(&mut self) {
        self.services.jobs.cancel_key("git:blame");
        self.blame.pending = None;
        self.blame.active = None;
        self.blame.checked = None;
        self.blame.cursor = None;
    }

    /// Called after preview completion and on the one-second background tick.
    /// Validations read bounded bytes and HEAD through gix; cache hits spawn no git.
    pub(super) fn tick_blame(&mut self) {
        if !self.config.git.blame.show_inline && !self.config.git.blame.show_popup {
            return;
        }
        if self.blame.popup.is_some() || self.blame.pending.is_some() {
            return;
        }
        let Some(path) = self.blame_path() else {
            return;
        };
        if self
            .blame
            .checked
            .as_ref()
            .is_some_and(|(p, t)| p == &path && t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let Some(repo) = self.repo_root().filter(|root| path.starts_with(root)) else {
            return;
        };
        let cached = self
            .blame
            .cache
            .iter()
            .find(|c| c.key.path == path)
            .cloned();
        let expected = source_hash(self.preview_state.preview.as_ref().unwrap());
        let git = self.services.git;
        let target = path.clone();
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::Low, Some("git:blame".into()), async move {
                    read_blame(git, repo, target, expected, cached).await
                });
        self.blame.checked = Some((path.clone(), Instant::now()));
        self.blame.pending = Some((path, handle));
    }

    pub(super) fn poll_blame(&mut self) {
        let done = self
            .blame
            .pending
            .as_mut()
            .and_then(|(path, handle)| handle.try_join().map(|result| (path.clone(), result)));
        if let Some((path, result)) = done {
            self.blame.pending = None;
            if self.blame_path().as_ref() == Some(&path) {
                match result {
                    Ok(Ok(read)) => {
                        if let Some(fresh) = read.refreshed {
                            // Preserve visual position while refreshing the viewed file.
                            // This also works when a folder is selected in the sidebar.
                            self.preview_state.preview = Some(fresh);
                            self.preview_state.markdown = MarkdownPreview::default();
                            self.preview_state.text_rows.clear();
                            self.preview_state.selection.clear();
                            self.blame.cursor = None;
                        }
                        self.blame.cache.retain(|c| c.key.path != path);
                        if self.blame.cache.len() >= CACHE_FILES {
                            self.blame.cache.remove(0);
                        }
                        self.blame.cache.push(read.cached.clone());
                        self.blame.active = Some(read.cached);
                    }
                    _ => self.blame.active = None,
                }
            }
        }
        if let Some(popup) = &mut self.blame.popup {
            if let Some(result) = popup.pending.as_mut().and_then(|h| h.try_join()) {
                popup.pending = None;
                popup.text = match result {
                    Ok(Ok(text)) => text,
                    Ok(Err(e)) => format!("Could not read commit: {e}"),
                    Err(e) => e.to_string(),
                };
            }
        }
    }

    pub(super) fn current_blame(&self) -> Option<(&BlameLine, usize)> {
        let path = self.blame_path()?;
        // Formatted Markdown hides source syntax and does not provide source
        // offsets. Only its Code tab can safely attribute individual lines.
        if self.markdown_available() && !self.preview_state.markdown_code {
            return None;
        }
        let row = self
            .blame
            .cursor
            .as_ref()
            .filter(|(p, _, scroll)| p == &path && *scroll == self.preview_state.preview_scroll)
            .map_or(self.preview_state.preview_scroll, |(_, row, _)| *row);
        let line = self.preview_state.text_rows.source_line_at(row)?;
        let cached = self.blame.active.as_ref().filter(|c| c.key.path == path)?;
        let blame = cached.lines.as_ref().ok()?.get(line.saturating_sub(1))?;
        if blame.uncommitted() && !self.config.git.blame.show_uncommitted {
            return None;
        }
        Some((blame, line))
    }

    pub(super) fn open_blame_popup(&mut self) {
        if !self.config.git.blame.enabled || !self.config.git.blame.show_popup {
            return;
        }
        let Some((line, _)) = self.current_blame() else {
            self.session.status = if self.markdown_available() && !self.preview_state.markdown_code
            {
                "Line blame is available in Markdown Code (F3)".into()
            } else {
                "No line blame available yet".into()
            };
            self.tick_blame();
            return;
        };
        let line = line.clone();
        let Some(repo) = self.repo_root() else {
            return;
        };
        self.blame.popup = Some(CommitPopup {
            oid: line.oid.clone(),
            path: line.path.clone(),
            repo,
            title: "Commit details".into(),
            text: "Uncommitted changes".into(),
            scroll: 0,
            pending: None,
            message: String::new(),
            max_scroll: 0,
        });
        if !line.uncommitted() {
            self.query_blame_popup(GitQuery::Commit(line.oid));
        }
    }

    fn query_blame_popup(&mut self, query: GitQuery) {
        let Some(popup) = &mut self.blame.popup else {
            return;
        };
        let root = popup.repo.clone();
        let git = self.services.git;
        let history = matches!(query, GitQuery::History(_));
        popup.title = match &query {
            GitQuery::CommitFile { .. } => "Commit diff",
            GitQuery::History(_) => "File history",
            _ => "Commit details",
        }
        .into();
        popup.scroll = 0;
        popup.text = "Loading…".into();
        popup.pending = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("git:blame-popup".into()),
            async move {
                let (items, mut detail) =
                    tokio::time::timeout(Duration::from_secs(5), git.write.query(&root, query))
                        .await??;
                if history {
                    detail = items
                        .into_iter()
                        .map(|item| format!("{}  {}", item.id, item.label))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                const MAX_POPUP_BYTES: usize = 256 * 1024;
                if detail.len() > MAX_POPUP_BYTES {
                    let mut end = MAX_POPUP_BYTES;
                    while !detail.is_char_boundary(end) {
                        end -= 1;
                    }
                    detail.truncate(end);
                    detail.push_str("\n… display truncated at 256 KiB");
                }
                Ok(detail)
            },
        ));
    }

    pub(super) fn handle_blame_key(&mut self, key: KeyEvent) {
        let Some(popup) = self.blame.popup.as_ref() else {
            return;
        };
        let oid = popup.oid.clone();
        let path = popup.path.clone();
        let committed = !oid.bytes().all(|b| b == b'0');
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.services.jobs.cancel_key("git:blame-popup");
                self.blame.popup = None;
            }
            KeyCode::Char('c') if committed => {
                self.copy_to_clipboard(&oid);
                if let Some(popup) = &mut self.blame.popup {
                    popup.message = self.session.status.clone();
                }
            }
            KeyCode::Char('d') if committed => {
                self.query_blame_popup(GitQuery::CommitFile { oid, path })
            }
            KeyCode::Char('h') if committed => {
                let path = self
                    .blame_path()
                    .and_then(|p| {
                        self.repo_root().and_then(|r| {
                            p.strip_prefix(r)
                                .ok()
                                .map(|p| p.to_string_lossy().into_owned())
                        })
                    })
                    .unwrap_or(path);
                self.query_blame_popup(GitQuery::History(Some(path)));
            }
            KeyCode::F(6) if committed => self.query_blame_popup(GitQuery::Commit(oid)),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_blame(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_blame(-1),
            KeyCode::PageDown => self.scroll_blame(10),
            KeyCode::PageUp => self.scroll_blame(-10),
            KeyCode::Home => {
                if let Some(p) = &mut self.blame.popup {
                    p.scroll = 0;
                }
            }
            KeyCode::End => self.scroll_blame(i32::MAX),
            _ => {}
        }
    }

    pub(super) fn scroll_blame(&mut self, delta: i32) {
        if let Some(popup) = &mut self.blame.popup {
            popup.scroll =
                (popup.scroll as i64 + delta as i64).clamp(0, popup.max_scroll as i64) as u16;
        }
    }
}

pub(super) fn relative_time(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let age = now.saturating_sub(time).max(0);
    for (seconds, suffix) in [
        (31_536_000, "y"),
        (2_592_000, "mo"),
        (86_400, "d"),
        (3_600, "h"),
        (60, "m"),
    ] {
        if age >= seconds {
            return format!("{}{} ago", age / seconds, suffix);
        }
    }
    "just now".into()
}

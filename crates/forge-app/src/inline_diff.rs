//! Source-only gutter and an overlay: source rows are never inserted/reordered.
use super::*;
use forge_model::DiffLineKind;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::hash::{Hash, Hasher};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Added,
    Modified,
    Deleted,
}
impl Change {
    fn color(self) -> Color {
        match self {
            Self::Added => Color::Green,
            Self::Modified => Color::Yellow,
            Self::Deleted => Color::Red,
        }
    }
}

#[derive(Clone, Debug)]
struct Hunk {
    header: String,
    lines: Vec<(DiffLineKind, String)>,
    marks: Vec<(usize, Change)>,
}

fn hunks(diff: &FileDiff, count: usize) -> Vec<Hunk> {
    let mut out = Vec::new();
    for range in diff.hunks() {
        let mut parts = range.header.split_whitespace();
        if parts.next() != Some("@@") {
            continue;
        }
        let Some(old) = parts.next().and_then(|s| s.strip_prefix('-')) else {
            continue;
        };
        let Some(new) = parts.next().and_then(|s| s.strip_prefix('+')) else {
            continue;
        };
        let parse = |s: &str| -> Option<(usize, usize)> {
            let (start, len) = s.split_once(',').unwrap_or((s, "1"));
            Some((start.parse().ok()?, len.parse().ok()?))
        };
        let (Some((_, _)), Some((mut line, _))) = (parse(old), parse(new)) else {
            continue;
        };
        if parts.next() != Some("@@") {
            continue;
        }
        let mut hunk = Hunk {
            header: range.header,
            lines: Vec::new(),
            marks: Vec::new(),
        };
        let body = &diff.lines[range.start + 1..range.end];
        let mut i = 0;
        while i < body.len() {
            match body[i].kind {
                DiffLineKind::Context => {
                    hunk.lines
                        .push((DiffLineKind::Context, body[i].text.clone()));
                    line = line.saturating_add(1);
                    i += 1;
                }
                DiffLineKind::Add | DiffLineKind::Delete => {
                    let start = line;
                    let (mut added, mut deleted) = (0, 0);
                    while i < body.len()
                        && matches!(
                            body[i].kind,
                            DiffLineKind::Add | DiffLineKind::Delete | DiffLineKind::Meta
                        )
                    {
                        match body[i].kind {
                            DiffLineKind::Add => {
                                added += 1;
                                line = line.saturating_add(1);
                            }
                            DiffLineKind::Delete => deleted += 1,
                            _ => {
                                i += 1;
                                continue;
                            }
                        }
                        hunk.lines
                            .push((body[i].kind.clone(), body[i].text.clone()));
                        i += 1;
                    }
                    if added > 0 {
                        for n in start..start.saturating_add(added).min(count.saturating_add(1)) {
                            if n > 0 {
                                hunk.marks.push((
                                    n,
                                    if deleted > 0 {
                                        Change::Modified
                                    } else {
                                        Change::Added
                                    },
                                ));
                            }
                        }
                    } else if deleted > 0 {
                        // The next surviving line anchors a deletion; EOF uses
                        // the last line, and an empty file uses its empty row.
                        hunk.marks
                            .push((start.clamp(1, count.max(1)), Change::Deleted));
                    }
                }
                _ => i += 1,
            }
        }
        if !hunk.marks.is_empty() {
            out.push(hunk);
        }
    }
    out
}

fn fingerprint(value: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn preview_hash(preview: &Preview) -> Option<u64> {
    match &preview.content {
        PreviewContent::Text {
            lines,
            truncated: false,
            ..
        } => Some(fingerprint(lines)),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    content: u64,
    index: Option<(std::time::SystemTime, u64)>,
}
struct Read {
    key: Key,
    fresh: Preview,
    hunks: Vec<Hunk>,
}

async fn snapshot(git: GitService, repo: &Path, path: &Path) -> Result<(Key, Preview)> {
    use tokio::io::AsyncReadExt;
    let meta = tokio::fs::symlink_metadata(path).await?;
    anyhow::ensure!(
        meta.is_file() && meta.len() <= forge_fs::PREVIEW_MAX_BYTES as u64,
        "ineligible source"
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take((forge_fs::PREVIEW_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(bytes.len() <= forge_fs::PREVIEW_MAX_BYTES, "source grew");
    let root = repo.to_owned();
    let index = tokio::task::spawn_blocking(move || {
        git.read
            .index_path(&root)
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| Some((m.modified().ok()?, m.len())))
    })
    .await?;
    let fresh = forge_fs::preview_from_bytes(path, &bytes, bytes.len() as u64);
    anyhow::ensure!(preview_hash(&fresh).is_some(), "ineligible source");
    Ok((
        Key {
            path: path.to_owned(),
            content: fingerprint(&bytes),
            index,
        },
        fresh,
    ))
}

async fn read(
    git: GitService,
    repo: PathBuf,
    path: PathBuf,
    cached: Option<(Key, Vec<Hunk>)>,
) -> Result<Read> {
    let (key, fresh) = snapshot(git, &repo, &path).await?;
    if let Some((_, hunks)) = cached.filter(|(k, _)| k == &key) {
        return Ok(Read { key, fresh, hunks });
    }
    let rel = forge_model::path_to_rela(path.strip_prefix(&repo)?);
    let diff = git.write.gutter_diff(&repo, &rel).await?;
    let (after, _) = snapshot(git, &repo, &path).await?;
    anyhow::ensure!(key == after, "source or index changed during diff");
    let count = match &fresh.content {
        PreviewContent::Text { lines, .. } => lines.len(),
        _ => 0,
    };
    Ok(Read {
        key,
        fresh,
        hunks: hunks(&diff, count),
    })
}

#[derive(Default)]
pub(super) struct InlineDiff {
    active: Option<Read>,
    pending: Option<(PathBuf, u64, JobHandle<Result<Read>>)>,
    checked: Option<Instant>,
    open: Option<usize>,
    popup: Rect,
    offset: usize,
}

impl App {
    fn inline_path(&self) -> Option<PathBuf> {
        if self.preview_state.preview_loading
            || !matches!(self.preview_state.preview_mode, PreviewMode::File)
            || self.editing.active()
            || (self.markdown_available() && !self.preview_state.markdown_code)
        {
            return None;
        }
        let preview = self.preview_state.preview.as_ref()?;
        preview_hash(preview)?;
        Some(preview.path.clone())
    }

    pub(super) fn reset_inline_diff(&mut self) {
        self.services.jobs.cancel_key("git:gutter");
        self.inline_diff = InlineDiff::default();
    }

    pub(super) fn tick_inline_diff(&mut self) {
        let Some(path) = self.inline_path() else {
            return;
        };
        if self.inline_diff.pending.is_some()
            || self
                .inline_diff
                .checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let Some(repo) = self.repo_root().filter(|r| path.starts_with(r)) else {
            return;
        };
        let expected = preview_hash(self.preview_state.preview.as_ref().unwrap()).unwrap();
        let cached = self
            .inline_diff
            .active
            .as_ref()
            .map(|r| (r.key.clone(), r.hunks.clone()));
        let git = self.services.git;
        let target = path.clone();
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::Low, Some("git:gutter".into()), async move {
                    read(git, repo, target, cached).await
                });
        self.inline_diff.checked = Some(Instant::now());
        self.inline_diff.pending = Some((path, expected, handle));
    }

    pub(super) fn poll_inline_diff(&mut self) {
        let done = self
            .inline_diff
            .pending
            .as_mut()
            .and_then(|(p, hash, h)| h.try_join().map(|r| (p.clone(), *hash, r)));
        let Some((path, expected, result)) = done else {
            return;
        };
        self.inline_diff.pending = None;
        if self.inline_path().as_ref() != Some(&path)
            || self.preview_state.preview.as_ref().and_then(preview_hash) != Some(expected)
        {
            return;
        }
        match result {
            Ok(Ok(read)) => {
                if preview_hash(&read.fresh) != Some(expected) {
                    self.preview_state.preview = Some(read.fresh.clone());
                    self.preview_state.text_rows.clear();
                    self.preview_state.markdown = MarkdownPreview::default();
                    self.preview_state.selection.clear();
                    self.reset_blame();
                }
                if self.inline_diff.active.as_ref().map(|r| &r.key) != Some(&read.key) {
                    self.inline_diff.open = None;
                    self.inline_diff.popup = Rect::default();
                }
                self.inline_diff.active = Some(read);
            }
            _ => {
                self.inline_diff.active = None;
                self.inline_diff.open = None;
                self.inline_diff.popup = Rect::default();
            }
        }
    }

    fn inline_active(&self) -> Option<&Read> {
        let path = self.inline_path()?;
        self.inline_diff.active.as_ref().filter(|r| {
            r.key.path == path
                && preview_hash(&r.fresh)
                    == self.preview_state.preview.as_ref().and_then(preview_hash)
        })
    }

    fn toggle_inline_hunk(&mut self, index: usize) {
        self.inline_diff.open = if self.inline_diff.open == Some(index) {
            None
        } else {
            Some(index)
        };
        self.inline_diff.offset = 0;
        self.inline_diff.popup = Rect::default();
        self.session.focus = PaneFocus::Preview;
    }

    pub(super) fn toggle_inline_peek(&mut self) {
        if self.inline_path().is_none() {
            return;
        }
        let row = self.preview_state.preview_scroll;
        let index = self.inline_active().and_then(|r| {
            r.hunks
                .iter()
                .position(|h| {
                    h.marks
                        .iter()
                        .any(|(n, _)| self.preview_state.text_rows.source_to_row(*n) >= row)
                })
                .or_else(|| (!r.hunks.is_empty()).then_some(0))
        });
        if let Some(index) = index {
            self.toggle_inline_hunk(index);
        } else {
            self.session.status =
                "No unstaged source changes · click a colored line number to peek".into();
        }
    }

    pub(super) fn inline_diff_hint(&self) -> bool {
        self.inline_active().is_some_and(|r| !r.hunks.is_empty())
    }

    pub(super) fn inline_popup_contains(&self, column: u16, row: u16) -> bool {
        self.inline_active().is_some()
            && self.inline_diff.open.is_some()
            && self
                .inline_diff
                .popup
                .contains(ratatui::layout::Position::new(column, row))
    }

    pub(super) fn inline_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.inline_active().is_none() {
            return false;
        }
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        if self.inline_diff.open.is_some() && self.inline_diff.popup.contains(point) {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                    if mouse.row == self.inline_diff.popup.y =>
                {
                    self.inline_diff.open = None;
                    self.inline_diff.popup = Rect::default();
                }
                MouseEventKind::ScrollDown => {
                    self.inline_diff.offset = self.inline_diff.offset.saturating_add(3)
                }
                MouseEventKind::ScrollUp => {
                    self.inline_diff.offset = self.inline_diff.offset.saturating_sub(3)
                }
                _ => {}
            }
            return true;
        }
        let region = self.session.layout.preview_inner;
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && region.contains(point)
            && mouse.column < region.x.saturating_add(5).min(region.right())
        {
            let row = self.preview_state.preview_scroll + (mouse.row - region.y) as usize;
            let index = self.inline_active().and_then(|r| {
                r.hunks.iter().position(|h| {
                    h.marks
                        .iter()
                        .any(|(n, _)| self.preview_state.text_rows.source_to_row(*n) == row)
                })
            });
            if let Some(index) = index {
                self.toggle_inline_hunk(index);
                return true;
            }
        }
        false
    }

    pub(super) fn close_inline_peek(&mut self) -> bool {
        if self.inline_diff.open.take().is_some() {
            self.inline_diff.popup = Rect::default();
            true
        } else {
            false
        }
    }

    pub(super) fn draw_inline_diff(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.inline_diff.popup = Rect::default();
        let Some(active) = self.inline_active() else {
            return;
        };
        let scroll = self.preview_state.preview_scroll;
        for hunk in &active.hunks {
            for (n, change) in &hunk.marks {
                let row = self.preview_state.text_rows.source_to_row(*n);
                if row < scroll || row >= scroll + area.height as usize {
                    continue;
                }
                let y = area.y + (row - scroll) as u16;
                let width = n.to_string().len().max(4) as u16;
                for x in area.x..area.x.saturating_add(width + 1).min(area.right()) {
                    frame.buffer_mut()[(x, y)].set_style(
                        Style::default()
                            .fg(change.color())
                            .add_modifier(Modifier::BOLD),
                    );
                }
                if matches!(&active.fresh.content, PreviewContent::Text { lines, .. } if lines.is_empty())
                {
                    frame.render_widget(
                        Paragraph::new("   1│").style(Style::default().fg(change.color())),
                        Rect::new(area.x, y, area.width.min(5), 1),
                    );
                }
            }
        }
        let Some(hunk) = self.inline_diff.open.and_then(|i| active.hunks.get(i)) else {
            return;
        };
        let anchor = hunk
            .marks
            .iter()
            .map(|(n, _)| self.preview_state.text_rows.source_to_row(*n))
            .find(|r| *r >= scroll && *r < scroll + area.height as usize);
        let Some(anchor) = anchor else { return };
        if area.width < 20 || area.height < 4 {
            return;
        }
        let height = (hunk.lines.len().saturating_add(2))
            .min((area.height as usize / 2).max(3))
            .min(12) as u16;
        let y = (area.y + (anchor - scroll) as u16 + 1).min(area.bottom() - height);
        let popup = Rect::new(area.x + 5, y, area.width - 5, height);
        let offset = self.inline_diff.offset.min(
            hunk.lines
                .len()
                .saturating_sub(height.saturating_sub(2) as usize),
        );
        let lines = hunk
            .lines
            .iter()
            .skip(offset)
            .take(height.saturating_sub(2) as usize)
            .map(|(kind, text)| {
                Line::from(Span::styled(
                    text.clone(),
                    Style::default().fg(match kind {
                        DiffLineKind::Delete => Color::Red,
                        DiffLineKind::Add => Color::Green,
                        _ => Color::Gray,
                    }),
                ))
            })
            .collect::<Vec<_>>();
        let title = format!(
            " [×] Esc · {}/{} · {} ",
            offset + 1,
            hunk.lines.len(),
            hunk.header
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().bg(Color::Black).fg(Color::White))
                .block(Block::default().borders(Borders::ALL).title(title)),
            popup,
        );
        self.inline_diff.offset = offset;
        self.inline_diff.popup = popup;
    }
}

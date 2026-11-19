//! Source-only gutter and an overlay: source rows are never inserted/reordered.
use super::*;
use forge_model::DiffLineKind;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Paragraph,
    Frame,
};
use std::hash::{Hash, Hasher};

mod peek_ui;

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
    // Worktree source-line coordinate of each displayed diff row.
    row_lines: Vec<Option<usize>>,
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
            row_lines: Vec::new(),
            marks: Vec::new(),
        };
        let body = &diff.lines[range.start + 1..range.end];
        let mut i = 0;
        while i < body.len() {
            match body[i].kind {
                DiffLineKind::Context => {
                    hunk.lines
                        .push((DiffLineKind::Context, body[i].text.clone()));
                    hunk.row_lines.push(Some(line.max(1)));
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
                                hunk.row_lines.push(Some(line.max(1)));
                                added += 1;
                                line = line.saturating_add(1);
                            }
                            DiffLineKind::Delete => {
                                hunk.row_lines.push(Some(start.max(1)));
                                deleted += 1;
                            },
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
            ..
        } => Some(fingerprint(&lines[..lines.len().min(forge_fs::PREVIEW_MAX_LINES)])),
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
    /// Working tree relative to index.
    hunks: Vec<Hunk>,
    /// Index relative to HEAD, mapped onto current worktree line numbers.
    staged_hunks: Vec<Hunk>,
}

/// Project all staged Index line numbers through the unstaged patch in a
/// single ordered pass. Avoid O(staged_marks * unstaged_diff_lines) on big files.
/// Removed Index lines anchor to the next surviving worktree line.
fn map_staged_marks(diff: &FileDiff, staged: &mut [Hunk], count: usize) {
    let mut targets = staged.iter().flat_map(|h|
        h.marks.iter().map(|(n, _)| *n)
            .chain(h.row_lines.iter().flatten().copied())
    ).collect::<Vec<_>>();
    targets.sort_unstable();
    targets.dedup();
    let mut resolved = std::collections::HashMap::with_capacity(targets.len());
    let (mut old_line, mut new_line, mut cursor) = (1usize, 1usize, 0usize);
    let parse = |part: &str| -> Option<usize> {
        let raw = part.get(1..)?;
        let (number, length) = raw.split_once(',').unwrap_or((raw, "1"));
        let n = number.parse::<usize>().ok()?;
        Some(if length == "0" { n.saturating_add(1) } else { n })
    };
    for target in targets {
        let mapped = loop {
            let Some(line) = diff.lines.get(cursor) else {
                break new_line.saturating_add(target.saturating_sub(old_line));
            };
            match line.kind {
                DiffLineKind::Header => {
                    let mut words = line.text.split_whitespace();
                    if words.next() != Some("@@") { cursor += 1; continue; }
                    let (Some(old), Some(new)) = (words.next(), words.next()) else {
                        cursor += 1; continue;
                    };
                    let (Some(old), Some(new)) = (parse(old), parse(new)) else {
                        cursor += 1; continue;
                    };
                    if target < old {
                        break new_line.saturating_add(target.saturating_sub(old_line));
                    }
                    old_line = old;
                    new_line = new;
                    cursor += 1;
                }
                DiffLineKind::Context | DiffLineKind::Delete => {
                    if target == old_line { break new_line; }
                    old_line = old_line.saturating_add(1);
                    if matches!(line.kind, DiffLineKind::Context) {
                        new_line = new_line.saturating_add(1);
                    }
                    cursor += 1;
                }
                DiffLineKind::Add => { new_line = new_line.saturating_add(1); cursor += 1; }
                DiffLineKind::Meta => cursor += 1,
            }
        };
        resolved.insert(target, mapped.clamp(1, count.max(1)));
    }
    for hunk in staged {
        for (line, _) in &mut hunk.marks {
            if let Some(mapped) = resolved.get(line) { *line = *mapped; }
        }
        for line in hunk.row_lines.iter_mut().flatten() {
            if let Some(mapped) = resolved.get(line) { *line = *mapped; }
        }
    }
}

// Peek requires only a bounded source snapshot, independent of how many
// pages of a long file have been displayed.
const PEEK_MAX_BYTES: usize = 2 * 1024 * 1024;

async fn snapshot(git: GitService, repo: &Path, path: &Path) -> Result<(Key, Preview)> {
    use tokio::io::AsyncReadExt;
    let meta = tokio::fs::symlink_metadata(path).await?;
    anyhow::ensure!(
        meta.is_file() && meta.len() <= PEEK_MAX_BYTES as u64,
        "ineligible source"
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take((PEEK_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(bytes.len() <= PEEK_MAX_BYTES, "source grew");
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
    cached: Option<(Key, Vec<Hunk>, Vec<Hunk>)>,
) -> Result<Read> {
    let (key, fresh) = snapshot(git, &repo, &path).await?;
    if let Some((_, hunks, staged_hunks)) = cached.filter(|(k, _, _)| k == &key) {
        return Ok(Read { key, fresh, hunks, staged_hunks });
    }
    let rel = forge_model::path_to_rela(path.strip_prefix(&repo)?);
    let diff = git.write.gutter_diff_side(&repo, &rel, DiffSide::Unstaged).await?;
    let staged = git.write.gutter_diff_side(&repo, &rel, DiffSide::Staged).await?;
    let (after, _) = snapshot(git, &repo, &path).await?;
    anyhow::ensure!(key == after, "source or index changed during diff");
    // The preview is intentionally paginated to 400 lines. Git hunks must
    // nevertheless retain their real file-wide line positions.
    let count = overview_source_lines(&path).await?;
    let mut staged_hunks = hunks(&staged, usize::MAX);
    map_staged_marks(&diff, &mut staged_hunks, count);
    Ok(Read {
        key,
        fresh,
        hunks: hunks(&diff, count),
        staged_hunks,
    })
}

/// Independent file-wide Git marks, including files whose preview is paged.
/// The focus ruler remains available even for files beyond the Peek byte cap.
struct OverviewRead {
    path: PathBuf,
    source_lines: usize,
    marks: Vec<(usize, Color)>,
}

#[derive(Default)]
struct OverviewState {
    active: Option<OverviewRead>,
    pending: Option<(PathBuf, JobHandle<Result<OverviewRead>>)>,
    checked: Option<Instant>,
}

#[derive(Default)]
pub(super) struct InlineDiff {
    overview: OverviewState,
    active: Option<Read>,
    pending: Option<(PathBuf, u64, JobHandle<Result<Read>>)>,
    checked: Option<Instant>,
    open: Option<usize>,
    // The selected source row used to position and scroll the peek.
    anchor_line: Option<usize>,
    popup: Rect,
    offset: usize,
}

/// Count lines in bounded chunks, not by allocating the whole file or preview.
/// Large files beyond this limit retain the scrollbar without misleading marks.
async fn overview_source_lines(path: &Path) -> Result<usize> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    anyhow::ensure!(size <= 128 * 1024 * 1024, "overview source too large");
    let mut buf = [0u8; 64 * 1024];
    let mut lines = 0usize;
    let mut last = None;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 { break; }
        lines += buf[..n].iter().filter(|&&b| b == b'\n').count();
        last = Some(buf[n - 1]);
    }
    Ok(lines + usize::from(last.is_some_and(|b| b != b'\n')))
}

async fn read_overview(git: GitService, repo: PathBuf, path: PathBuf) -> Result<OverviewRead> {
    let source_lines = overview_source_lines(&path).await?;
    let rel = forge_model::path_to_rela(path.strip_prefix(&repo)?);
    let unstaged = git.write.gutter_diff_side(&repo, &rel, DiffSide::Unstaged).await?;
    let staged = git.write.gutter_diff_side(&repo, &rel, DiffSide::Staged).await?;
    let mut staged_hunks = hunks(&staged, usize::MAX);
    map_staged_marks(&unstaged, &mut staged_hunks, source_lines);
    let mut marks = Vec::new();
    for hunk in staged_hunks {
        for (line, _) in hunk.marks {
            marks.push((line, Color::Blue));
        }
    }
    for hunk in hunks(&unstaged, source_lines) {
        for (line, change) in hunk.marks {
            marks.push((line, change.color()));
        }
    }
    Ok(OverviewRead { path, source_lines, marks })
}

impl App {
    /// File-wide overview is independent of the size-limited inline gutter.
    fn overview_path(&self) -> Option<PathBuf> {
        if self.preview_state.preview_loading
            || !matches!(self.preview_state.preview_mode, PreviewMode::File)
            || self.editing.active()
            || (self.markdown_available() && !self.preview_state.markdown_code)
        { return None; }
        match &self.preview_state.preview.as_ref()?.content {
            PreviewContent::Text { .. } => Some(self.preview_state.preview.as_ref()?.path.clone()),
            _ => None,
        }
    }

    pub(super) fn overview_git_data(&self) -> Option<(usize, &[(usize, Color)])> {
        let path = self.overview_path()?;
        let data = self.inline_diff.overview.active.as_ref()?;
        (data.path == path).then_some((data.source_lines, data.marks.as_slice()))
    }

    fn tick_overview_git(&mut self) {
        // The watcher invalidates cached Git marks when source/HEAD/index
        // changes. Disabled overview must not run an expensive file-wide diff.
        if !self.config.show_overview_ruler {
            return;
        }
        let Some(path) = self.overview_path() else {
            self.inline_diff.overview.active = None;
            return;
        };
        if self.inline_diff.overview.pending.is_some()
            || self.inline_diff.overview.checked.is_some_and(|t| {
                self.inline_diff.overview.active.as_ref().is_some_and(|a| a.path == path)
                    && t.elapsed() < Duration::from_secs(30)
                || self.inline_diff.overview.active.is_none()
                    && t.elapsed() < Duration::from_secs(10)
            })
        { return; }
        let Some(repo) = self.repo_root().filter(|root| path.starts_with(root)) else { return; };
        let git = self.services.git;
        let query_path = path.clone();
        let handle = self.services.jobs.spawn(JobPriority::Low,
            Some("git:overview".into()), async move {
                read_overview(git, repo, query_path).await
            });
        self.inline_diff.overview.pending = Some((path, handle));
        self.inline_diff.overview.checked = Some(Instant::now());
    }

    fn poll_overview_git(&mut self) {
        let done = self.inline_diff.overview.pending.as_mut().and_then(|(path, job)| {
            job.try_join().map(|result| (path.clone(), result))
        });
        let Some((path, result)) = done else { return; };
        self.inline_diff.overview.pending = None;
        if self.overview_path().as_ref() != Some(&path) { return; }
        self.inline_diff.overview.active = result.ok().and_then(Result::ok);
    }

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
        self.services.jobs.cancel_key("git:overview");
        self.inline_diff = InlineDiff::default();
    }

    pub(super) fn tick_inline_diff(&mut self) {
        self.tick_overview_git();
        let Some(path) = self.inline_path() else {
            return;
        };
        if self.inline_diff.pending.is_some()
            || self.inline_diff.checked.is_some_and(|t| {
                t.elapsed() < Duration::from_secs(
                    if self.inline_active().is_some() { 30 } else { 10 })
            })
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
            .map(|r| (r.key.clone(), r.hunks.clone(), r.staged_hunks.clone()));
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
        self.poll_overview_git();
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
                    self.inline_diff.anchor_line = None;
                    self.inline_diff.popup = Rect::default();
                }
                self.inline_diff.active = Some(read);
            }
            _ => {
                self.inline_diff.active = None;
                self.inline_diff.open = None;
                self.inline_diff.anchor_line = None;
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

    fn layer_enabled(&self, side: DiffSide) -> bool {
        match side {
            DiffSide::Unstaged => self.config.git.show_unstaged_gutter,
            DiffSide::Staged => self.config.git.show_staged_gutter,
        }
    }

    fn visible_hunk_count(read: &Read) -> usize {
        read.hunks.len() + read.staged_hunks.len()
    }

    fn selected_hunk(read: &Read, index: usize) -> Option<(&Hunk, DiffSide)> {
        if index < read.hunks.len() { read.hunks.get(index).map(|h| (h, DiffSide::Unstaged)) }
        else { read.staged_hunks.get(index - read.hunks.len()).map(|h| (h, DiffSide::Staged)) }
    }

    /// Map the chosen source line into this hunk's diff rows. Scrolling a
    /// large hunk must start near the clicked line, not at its first context.
    fn peek_offset(hunk: &Hunk, target: usize) -> usize {
        let match_row = hunk
            .row_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == Some(target))
            .find(|(i, _)| matches!(hunk.lines[*i].0, DiffLineKind::Add))
            .or_else(|| {
                hunk.row_lines
                    .iter()
                    .enumerate()
                    .find(|(_, line)| **line == Some(target))
            })
            .map(|(i, _)| i);
        match_row.unwrap_or(0).saturating_sub(2)
    }

    fn toggle_inline_hunk(&mut self, index: usize, source_line: Option<usize>) {
        if self.inline_diff.open == Some(index) {
            self.inline_diff.open = None;
            self.inline_diff.anchor_line = None;
            self.inline_diff.offset = 0;
        } else {
            self.inline_diff.offset = self
                .inline_active()
                .and_then(|r| Self::selected_hunk(r, index))
                .and_then(|(h, _)| source_line.map(|source| Self::peek_offset(h, source)))
                .unwrap_or(0);
            self.inline_diff.open = Some(index);
            self.inline_diff.anchor_line = source_line;
        }
        self.inline_diff.popup = Rect::default();
        self.session.focus = PaneFocus::Preview;
    }

    pub(super) fn toggle_inline_peek(&mut self) {
        let Some(path) = self.inline_path() else {
            return;
        };
        let top = self.preview_state.preview_scroll;
        // Render tests and first-frame input may run before the viewport gets
        // measured. In that case use the remainder of the file as a fallback.
        let bottom = if self.session.layout.preview_inner.height == 0 {
            usize::MAX
        } else {
            top.saturating_add(self.session.layout.preview_inner.height as usize)
        };
        // The same source row that owns inline blame must own the F7 Peek.
        // Falling back to the viewport top is appropriate only when the user
        // has not clicked a particular line.
        let cursor = self
            .blame
            .cursor
            .as_ref()
            .filter(|(p, row, scroll)| p == &path && *scroll == top && *row >= top && *row < bottom)
            .and_then(|(_, row, _)| self.preview_state.text_rows.source_line_at(*row));
        let target_row = cursor
            .map(|line| self.preview_state.text_rows.source_to_row(line))
            .unwrap_or(top);
        let index_and_line = self.inline_active().and_then(|read| {
            let visible_marks = (0..Self::visible_hunk_count(read))
                .filter_map(|index| {
                    Self::selected_hunk(read, index)
                        .and_then(|(hunk, side)| self.layer_enabled(side).then_some((index, hunk)))
                })
                .flat_map(|(index, hunk)| hunk.marks.iter().map(move |(line, _)| (index, *line)))
                .filter(|(_, line)| {
                    let row = self.preview_state.text_rows.source_to_row(*line);
                    row >= top && row < bottom
                });
            visible_marks.min_by_key(|(index, line)| {
                let row = self.preview_state.text_rows.source_to_row(*line);
                let distance = row.abs_diff(target_row);
                let side = usize::from(*index >= read.hunks.len());
                // F7 without a selected source line keeps WORKTREE priority.
                // With a selected line, the exact row matters more than side.
                if cursor.is_some() {
                    (distance, side, *index)
                } else {
                    (side, distance, *index)
                }
            })
        });
        if let Some((index, line)) = index_and_line {
            self.toggle_inline_hunk(index, Some(line));
        } else {
            self.session.status = "No visible source changes to peek".into();
        }
    }

    pub(super) fn inline_diff_hint(&self) -> bool {
        self.inline_active().is_some_and(|r| (!r.hunks.is_empty() && self.layer_enabled(DiffSide::Unstaged))
            || (!r.staged_hunks.is_empty() && self.layer_enabled(DiffSide::Staged)))
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
                    if mouse.row == self.inline_diff.popup.y
                        && mouse.column >= self.inline_diff.popup.right().saturating_sub(3) =>
                {
                    self.inline_diff.open = None;
                    self.inline_diff.anchor_line = None;
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
            let staged_column = mouse.column == region.x;
            let index = self.inline_active().and_then(|r| {
                let matches = |h: &Hunk| h.marks.iter().any(|(n, _)|
                    self.preview_state.text_rows.source_to_row(*n) == row);
                let staged = || self.layer_enabled(DiffSide::Staged)
                    .then(|| r.staged_hunks.iter().position(matches).map(|i| r.hunks.len() + i)).flatten();
                let unstaged = || self.layer_enabled(DiffSide::Unstaged)
                    .then(|| r.hunks.iter().position(matches)).flatten();
                if staged_column { staged().or_else(unstaged) }
                else { unstaged().or_else(staged) }
            });
            if let Some(index) = index {
                let source = self.preview_state.text_rows.source_line_at(row);
                self.toggle_inline_hunk(index, source);
                return true;
            }
        }
        false
    }

    pub(super) fn inline_peek_open(&self) -> bool {
        self.inline_diff.open.is_some()
    }

    pub(super) fn close_inline_peek(&mut self) -> bool {
        if self.inline_diff.open.take().is_some() {
            self.inline_diff.anchor_line = None;
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
        let show_unstaged = self.config.git.show_unstaged_gutter;
        let show_staged = self.config.git.show_staged_gutter;
        // Unstaged keeps the existing vivid line-number decoration. Staged
        // uses a separate subdued blue lane; both remain visible on overlap.
        if show_unstaged { for hunk in &active.hunks {
            for (n, change) in &hunk.marks {
                let row = self.preview_state.text_rows.source_to_row(*n);
                if row < scroll || row >= scroll + area.height as usize { continue; }
                let y = area.y + (row - scroll) as u16;
                let width = n.to_string().len().max(4) as u16;
                for x in area.x..area.x.saturating_add(width + 1).min(area.right()) {
                    frame.buffer_mut()[(x, y)].set_style(
                        Style::default().fg(change.color()).add_modifier(Modifier::BOLD));
                }
                if matches!(&active.fresh.content, PreviewContent::Text { lines, .. } if lines.is_empty()) {
                    frame.render_widget(
                        Paragraph::new("   1│").style(Style::default().fg(change.color())),
                        Rect::new(area.x, y, area.width.min(5), 1),
                    );
                }
            }
        }
        }
        if show_staged { for hunk in &active.staged_hunks {
            for (n, _) in &hunk.marks {
                let row = self.preview_state.text_rows.source_to_row(*n);
                if row < scroll || row >= scroll + area.height as usize { continue; }
                let y = area.y + (row - scroll) as u16;
                // One blue-background cell is reserved for staged changes.
                // If no unstaged change overlaps, tint the line number too.
                let has_unstaged = active.hunks.iter().any(|h| h.marks.iter().any(|(line, _)| line == n));
                if !has_unstaged {
                    for x in area.x..area.x.saturating_add(5).min(area.right()) {
                        frame.buffer_mut()[(x, y)].set_style(Style::default().fg(Color::Blue)
                            .add_modifier(Modifier::BOLD));
                    }
                }
                if area.x < area.right() {
                    frame.buffer_mut()[(area.x, y)]
                        .set_style(Style::default().fg(Color::LightBlue).bg(Color::Blue));
                }
            }
        }
        }
        let Some((hunk, side)) = self.inline_diff.open.and_then(|i| Self::selected_hunk(active, i)) else {
            return;
        };
        if !self.layer_enabled(side) { return; }
        let visible = |n: usize| {
            let row = self.preview_state.text_rows.source_to_row(n);
            (row >= scroll && row < scroll + area.height as usize).then_some(row)
        };
        // Clicking a gutter mark must anchor the peek to that mark. The old
        // first-visible-mark rule made line 864 open over line 840 in a long
        // hunk, even though the selected blame/source line was 864.
        let anchor = self
            .inline_diff
            .anchor_line
            .filter(|line| hunk.marks.iter().any(|(n, _)| n == line))
            .and_then(visible)
            .or_else(|| hunk.marks.iter().find_map(|(n, _)| visible(*n)));
        let Some(anchor) = anchor else { return };
        let Some((popup, offset)) = peek_ui::draw(
            frame, area, anchor, scroll, hunk, side,
            self.inline_diff.open.unwrap_or(0) + 1,
            Self::visible_hunk_count(active),
            self.inline_diff.offset,
            self.inline_diff.anchor_line,
        ) else { return; };
        self.inline_diff.offset = offset;
        self.inline_diff.popup = popup;
    }
}

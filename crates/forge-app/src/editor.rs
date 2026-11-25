//! In-place multiline text editor. File I/O stays on the job scheduler.
use super::*;
use forge_fs::text::{self, TextDocument};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct EditingState {
    pub view: Option<TextEditor>,
    pending_load: Option<JobHandle<Result<TextEditor>>>,
    pending_save: Option<JobHandle<Result<TextDocument>>>,
    return_focus: Option<PaneFocus>,
    close_after_save: bool,
    terminal_config: bool,
    clipboard: String,
    clipboard_line: bool,
    system_clipboard: Option<arboard::Clipboard>,
}

impl EditingState {
    pub fn active(&self) -> bool {
        self.view.is_some() || self.pending_load.is_some()
    }
    pub fn saving(&self) -> bool {
        self.pending_save.is_some()
    }
    pub fn exit_confirmation(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.confirm_close)
    }
    pub fn has_selection(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.selection().is_some())
    }
}

/// Bounded syntax cache: cursor and selection changes do not reparse code.
/// A small lookbehind preserves common multi-line scopes while scrolling.
#[derive(Default)]
struct EditorSyntaxCache {
    start: usize,
    top: usize,
    source: Vec<String>,
    path: String,
    theme: Option<forge_theme::Theme>,
    colored: Vec<Line<'static>>,
    revision: u64,
    colored_revision: u64,
    pending: Option<JobHandle<SyntaxJobResult>>,
}

type SyntaxJobResult = Result<(u64, Vec<Line<'static>>)>;

impl EditorSyntaxCache {
    fn poll(&mut self) {
        if let Some(result) = self.pending.as_mut().and_then(JobHandle::try_join) {
            self.pending = None;
            match result {
                Ok(Ok((revision, colored))) if revision == self.revision => {
                    self.colored = colored;
                    self.colored_revision = revision;
                }
                Ok(Ok(_)) => {} // Superseded source or viewport; keep existing colors.
                _ => self.colored_revision = self.revision,
            }
        }
    }

    fn visible<'a>(
        &'a mut self,
        jobs: &JobScheduler,
        theme: &forge_theme::Theme,
        path: &str,
        lines: &[String],
        top: usize,
        end: usize,
    ) -> &'a [Line<'static>] {
        self.poll();
        let start = top.saturating_sub(48);
        // Very long lines remain editable, but do not run language parsers
        // over them (including the Rust and Markdown fallback highlighters).
        let source = || {
            lines[start..end].iter().map(|line| {
                if line.len() > 16 * 1024 {
                    ""
                } else {
                    line.as_str()
                }
            })
        };
        if self.start != start
            || self.top != top
            || self.path != path
            || self.theme.as_ref() != Some(theme)
            || !self.source.iter().map(String::as_str).eq(source())
        {
            self.revision = self.revision.wrapping_add(1);
            // Reuse existing colors while the latest source is parsed. Text
            // always comes from the live document, never from these spans.
            if self.theme.as_ref() != Some(theme) || self.path != path {
                self.colored = vec![Line::default(); end - top];
            } else if self.top != top || self.colored.len() != end - top {
                self.colored = (top..end)
                    .map(|row| {
                        row.checked_sub(self.top)
                            .and_then(|index| self.colored.get(index))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
            }
            self.start = start;
            self.top = top;
            self.path = path.to_owned();
            self.theme = Some(theme.clone());
            self.source = source().map(str::to_owned).collect();
            if matches!(
                forge_ui::TextLang::from_name(path),
                forge_ui::TextLang::Plain
            ) {
                jobs.cancel_key("editor:syntax");
                self.pending = None;
                self.colored.clear();
                self.colored.resize_with(end - top, Line::default);
                self.colored_revision = self.revision;
            }
        }
        // At most one parse is in flight. New keys only update the desired
        // revision, avoiding cancellation notifications and extra UI frames
        // for every key in a burst. The next parse uses the latest source.
        if self.pending.is_none() && self.colored_revision != self.revision {
            let source = self.source.clone();
            let theme = theme.clone();
            let path = path.to_owned();
            let revision = self.revision;
            self.pending =
                Some(
                    jobs.spawn(JobPriority::Low, Some("editor:syntax".into()), async move {
                        tokio::time::sleep(Duration::from_millis(16)).await;
                        Ok(tokio::task::spawn_blocking(move || {
                            let colored = forge_ui::highlight_code_lines(&theme, &path, &source)
                                .into_iter()
                                .skip(top - start)
                                .collect();
                            (revision, colored)
                        })
                        .await?)
                    }),
                );
        }
        &self.colored
    }
}

#[derive(Clone)]
struct Snapshot {
    // Only retain the replaced lines, plus the size of the unchanged suffix.
    // Typing in a large document must not copy the entire file for undo.
    lines: Vec<String>,
    start: usize,
    after: usize,
    row: usize,
    col: usize,
    anchor: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConflictBlock {
    start: usize,
    base: Option<usize>,
    separator: usize,
    end: usize,
}

// Complete non-nested Git markers, including diff3/zdiff3.
fn conflict_blocks(lines: &[String]) -> Vec<ConflictBlock> {
    fn marker(line: &str, ch: u8) -> Option<usize> {
        let n = line.bytes().take_while(|&b| b == ch).count();
        (n >= 7 && (line.len() == n || line.as_bytes()[n] == b' ')).then_some(n)
    }
    let mut blocks = Vec::new();
    let mut pending: Option<(usize, usize, Option<usize>, Option<usize>)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(width) = marker(line, b'<') {
            if pending.is_some() {
                return Vec::new();
            }
            pending = Some((i, width, None, None));
        } else if let Some((start, width, base, sep)) = pending.as_mut() {
            if marker(line, b'|') == Some(*width) && base.is_none() && sep.is_none() {
                *base = Some(i);
            } else if line.len() == *width && marker(line, b'=') == Some(*width) && sep.is_none() {
                *sep = Some(i);
            } else if marker(line, b'>') == Some(*width) {
                let Some(separator) = *sep else {
                    return Vec::new();
                };
                blocks.push(ConflictBlock {
                    start: *start,
                    base: *base,
                    separator,
                    end: i,
                });
                pending = None;
            } else if [b'|', b'=', b'>']
                .iter()
                .any(|&ch| marker(line, ch) == Some(*width))
            {
                return Vec::new();
            }
        }
    }
    if pending.is_some() {
        Vec::new()
    } else {
        blocks
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictAction {
    Previous,
    Next,
    Current,
    Incoming,
    Both,
}

pub(super) struct TextEditor {
    document: TextDocument,
    lines: Vec<String>,
    byte_len: usize,
    dirty_cache: OnceLock<bool>,
    row: usize,
    col: usize, // UTF-8 byte boundary
    top: usize,
    left: usize, // display cells
    viewport: Rect,
    message: String,
    confirm_close: bool,
    anchor: Option<(usize, usize)>,
    dragging: bool,
    // Wheel scrolling does not move the editing cursor.
    follow_cursor: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    syntax_cache: EditorSyntaxCache,
    conflict_cache: OnceLock<Vec<ConflictBlock>>,
    conflict_mode: bool,
    conflict_hits: Vec<(Rect, ConflictAction, Option<usize>)>,
}

impl TextEditor {
    fn new(document: TextDocument, line: usize) -> Self {
        let lines: Vec<_> = document.text.split('\n').map(str::to_owned).collect();
        let row = line.saturating_sub(1).min(lines.len() - 1);
        Self {
            byte_len: document.text.len(),
            dirty_cache: OnceLock::from(false),
            document,
            lines,
            row,
            col: 0,
            top: row.saturating_sub(3),
            left: 0,
            viewport: Rect::default(),
            message: String::new(),
            confirm_close: false,
            anchor: None,
            dragging: false,
            follow_cursor: true,
            undo: Vec::new(),
            redo: Vec::new(),
            syntax_cache: EditorSyntaxCache::default(),
            conflict_cache: OnceLock::new(),
            conflict_mode: false,
            conflict_hits: Vec::new(),
        }
    }
    fn enable_conflicts(&mut self, jump: bool) {
        self.conflict_mode = true;
        if jump {
            if let Some(start) = self.conflicts().first().map(|b| b.start) {
                self.row = start;
                self.top = start.saturating_sub(3);
            }
        }
    }

    fn conflicts(&self) -> &[ConflictBlock] {
        self.conflict_cache
            .get_or_init(|| conflict_blocks(&self.lines))
    }

    fn active_conflict(&self) -> Option<usize> {
        self.conflicts()
            .iter()
            .position(|b| b.start <= self.row && self.row <= b.end)
    }

    fn apply_conflict(&mut self, action: ConflictAction) {
        let blocks = self.conflicts();
        if blocks.is_empty() {
            return;
        }
        let active = self.active_conflict();
        if matches!(action, ConflictAction::Previous | ConflictAction::Next) {
            let index = if action == ConflictAction::Next {
                blocks.iter().position(|b| b.start > self.row).unwrap_or(0)
            } else {
                blocks
                    .iter()
                    .rposition(|b| b.end < self.row)
                    .unwrap_or(blocks.len() - 1)
            };
            self.row = blocks[index].start;
        } else {
            let Some(index) = active else {
                self.message = "Move to a conflict first (Alt+Up/Down)".into();
                return;
            };
            let block = blocks[index];
            let mut replacement = Vec::new();
            if matches!(action, ConflictAction::Current | ConflictAction::Both) {
                replacement.extend_from_slice(
                    &self.lines[block.start + 1..block.base.unwrap_or(block.separator)],
                );
            }
            if matches!(action, ConflictAction::Incoming | ConflictAction::Both) {
                replacement.extend_from_slice(&self.lines[block.separator + 1..block.end]);
            }
            self.checkpoint_range(block.start, block.end + 1);
            self.lines.splice(block.start..=block.end, replacement);
            if self.lines.is_empty() {
                self.lines.push(String::new());
            }
            self.byte_len =
                self.lines.iter().map(String::len).sum::<usize>() + self.lines.len() - 1;
            self.row = block.start.min(self.lines.len() - 1);
            let next = self
                .conflicts()
                .iter()
                .find(|b| b.start >= self.row)
                .or_else(|| self.conflicts().first())
                .map(|b| b.start);
            if let Some(next) = next {
                self.row = next;
            }
            self.message = if self.conflicts().is_empty() {
                "No complete conflict blocks remain. Review, save, then stage.".into()
            } else {
                "Conflict accepted".into()
            };
        }
        self.col = 0;
        self.left = 0;
        self.anchor = None;
        self.dragging = false;
        self.follow_cursor = true;
        self.conflict_hits.clear();
    }

    fn value(&self) -> String {
        self.lines.join("\n")
    }
    fn dirty(&self) -> bool {
        *self.dirty_cache.get_or_init(|| {
            self.byte_len != self.document.text.len()
                || !self
                    .lines
                    .iter()
                    .map(String::as_str)
                    .eq(self.document.text.split('\n'))
        })
    }
    fn snapshot(&self, start: usize, end: usize) -> Snapshot {
        Snapshot {
            lines: self.lines[start..end].to_vec(),
            start,
            after: self.lines.len() - end,
            row: self.row,
            col: self.col,
            anchor: self.anchor,
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.lines.splice(
            snapshot.start..self.lines.len() - snapshot.after,
            snapshot.lines,
        );
        self.byte_len = self.lines.iter().map(String::len).sum::<usize>() + self.lines.len() - 1;
        self.dirty_cache.take();
        self.conflict_cache.take();
        self.conflict_hits.clear();
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.anchor = snapshot.anchor;
    }
    fn checkpoint(&mut self) {
        let (start, end) = self
            .selection()
            .map_or((self.row, self.row + 1), |((sr, _), (er, _))| (sr, er + 1));
        self.checkpoint_range(start, end);
    }
    fn checkpoint_range(&mut self, start: usize, end: usize) {
        self.undo.push(self.snapshot(start, end));
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty_cache.take();
        self.conflict_cache.take();
        self.conflict_hits.clear();
        self.message.clear();
    }
    fn undo(&mut self, redo: bool) {
        let snapshot = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(snapshot) = snapshot {
            let current = self.snapshot(snapshot.start, self.lines.len() - snapshot.after);
            if redo {
                self.undo.push(current);
            } else {
                self.redo.push(current);
            }
            self.restore(snapshot);
            self.message.clear();
        }
    }
    fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.anchor?;
        let cursor = (self.row, self.col);
        (anchor != cursor).then_some((anchor.min(cursor), anchor.max(cursor)))
    }
    fn selected_text(&self) -> Option<String> {
        let ((sr, sc), (er, ec)) = self.selection()?;
        if sr == er {
            return Some(self.lines[sr][sc..ec].into());
        }
        let mut selected = vec![self.lines[sr][sc..].to_owned()];
        selected.extend(self.lines[sr + 1..er].iter().cloned());
        selected.push(self.lines[er][..ec].to_owned());
        Some(selected.join("\n"))
    }
    fn selected_len(&self) -> usize {
        let Some(((sr, sc), (er, ec))) = self.selection() else {
            return 0;
        };
        if sr == er {
            ec - sc
        } else {
            self.lines[sr].len() - sc + ec + er - sr
                + self.lines[sr + 1..er]
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
        }
    }
    // Caller checkpoints once, so replacing a selection is one undo step.
    fn remove_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection() else {
            return false;
        };
        self.byte_len -= self.selected_len();
        let tail = self.lines[er][ec..].to_owned();
        self.lines[sr].truncate(sc);
        self.lines[sr].push_str(&tail);
        self.lines.drain(sr + 1..er + 1);
        self.row = sr;
        self.col = sc;
        self.anchor = None;
        true
    }
    fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.checkpoint();
        self.remove_selection()
    }
    fn select_line(&mut self) {
        self.anchor = Some((self.row, 0));
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        } else if self.row > 0 {
            // Include the preceding newline when cutting the final line.
            self.anchor = Some((self.row - 1, self.lines[self.row - 1].len()));
            self.col = self.lines[self.row].len();
        } else {
            self.col = self.lines[self.row].len();
        }
    }
    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }
    fn insert(&mut self, value: &str) {
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let value: String = value
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        if value.is_empty() {
            return;
        }
        if self.byte_len - self.selected_len() + value.len() > text::MAX_TEXT_BYTES {
            self.message = "Text exceeds the editor's 2 MiB limit".into();
            return;
        }
        self.checkpoint();
        self.remove_selection();
        self.byte_len += value.len();
        self.anchor = None;
        let tail = self.lines[self.row].split_off(self.col);
        let parts: Vec<_> = value.split('\n').collect();
        self.lines[self.row].push_str(parts[0]);
        if parts.len() == 1 {
            self.col += parts[0].len();
            self.lines[self.row].push_str(&tail);
        } else {
            self.col = parts.last().unwrap().len();
            let mut added: Vec<String> = parts[1..].iter().map(|s| (*s).into()).collect();
            added.last_mut().unwrap().push_str(&tail);
            self.lines.splice(self.row + 1..self.row + 1, added);
            self.row += parts.len() - 1;
        }
    }
    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.anchor = None;
        if self.col > 0 {
            self.checkpoint();
            let start = self.lines[self.row][..self.col]
                .char_indices()
                .last()
                .unwrap()
                .0;
            self.lines[self.row].drain(start..self.col);
            self.byte_len -= self.col - start;
            self.col = start;
        } else if self.row > 0 {
            self.checkpoint_range(self.row - 1, self.row + 1);
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].push_str(&line);
            self.byte_len -= 1;
        }
    }
    fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.anchor = None;
        if self.col < self.lines[self.row].len() {
            self.checkpoint();
            let end = self.col
                + self.lines[self.row][self.col..]
                    .chars()
                    .next()
                    .unwrap()
                    .len_utf8();
            self.lines[self.row].drain(self.col..end);
            self.byte_len -= end - self.col;
        } else if self.row + 1 < self.lines.len() {
            self.checkpoint_range(self.row, self.row + 2);
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
            self.byte_len -= 1;
        }
    }
    fn up_down(&mut self, delta: isize) {
        let cells = display_width(&self.lines[self.row][..self.col]);
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.lines.len() - 1);
        self.col = byte_at_cell(&self.lines[self.row], cells);
    }
}

fn write_editor_clipboard(clipboard: &mut Option<arboard::Clipboard>, text: &str) -> bool {
    #[cfg(not(test))]
    {
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        let native = clipboard
            .as_mut()
            .is_some_and(|c| c.set_text(text.to_owned()).is_ok());
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)
        );
        native
    }
    #[cfg(test)]
    {
        let _ = (clipboard, text);
        false
    }
}
fn read_editor_clipboard(clipboard: &mut Option<arboard::Clipboard>) -> Option<String> {
    #[cfg(not(test))]
    {
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        clipboard.as_mut().and_then(|c| c.get_text().ok())
    }
    #[cfg(test)]
    {
        let _ = clipboard;
        None
    }
}

fn char_width(c: char) -> usize {
    if c == '\t' {
        4
    } else {
        c.width().unwrap_or(0)
    }
}
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}
fn byte_at_cell(s: &str, cell: usize) -> usize {
    let mut width = 0;
    for (idx, ch) in s.char_indices() {
        if width + char_width(ch) > cell {
            return idx;
        }
        width += char_width(ch);
    }
    s.len()
}

impl App {
    pub(super) fn open_text_editor(&mut self, path: PathBuf, line: usize) {
        let conflict = self.scm.git_status.as_ref().is_some_and(|status| {
            status.entries.iter().any(|(relative, kind)| {
                *kind == forge_git::GitStatus::Conflicted && status.repo_root.join(relative) == path
            })
        });
        self.load_text_editor(path, line, conflict);
    }

    pub(super) fn open_conflict_editor(&mut self, path: PathBuf) {
        self.load_text_editor(path, 1, true);
    }

    fn load_text_editor(&mut self, path: PathBuf, line: usize, conflict: bool) {
        if self.editing.active() || self.write_busy() {
            return;
        }
        self.close_tools();
        self.dismiss_menu();
        self.session.write_status = None;

        // Editing a worktree file from Git is navigation into Files, not an
        // editor modal layered over a Git Diff. Otherwise Esc closes the
        // editor and unexpectedly reveals the Git Activity underneath.
        // Preserve Git selection/history for an explicit F2 return.
        if (self.in_git_sidebar() || (conflict && !path.starts_with(&self.files.tree_root)))
            && path.is_file()
        {
            self.leave_git_sidebar();
            self.tools.return_focus = Some(PaneFocus::List);
            self.reveal_file(path.clone(), line);
        }
        self.editing.return_focus = Some(self.session.focus);
        let root = self.files.tree_root.clone();
        self.editing.terminal_config = false;
        self.load_editor_document(root, path, line, conflict);
    }

    // Configuration is a single explicitly selected file outside the workspace.
    // Use its parent as the editor's containment root; do not switch workspace
    // or suspend Forge, so every existing PTY remains alive.
    pub(super) fn open_terminal_config_editor(&mut self, path: PathBuf) {
        if self.editing.active() || self.write_busy() {
            return;
        }
        let Some(root) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        self.close_tools();
        self.dismiss_menu();
        self.editing.return_focus = Some(self.session.focus);
        self.editing.terminal_config = true;
        self.load_editor_document(root, path, 1, false);
    }

    fn load_editor_document(&mut self, root: PathBuf, path: PathBuf, line: usize, conflict: bool) {
        self.editing.pending_load = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("editor:load".into()),
            async move {
                tokio::task::spawn_blocking(move || {
                    let document = text::load(&root, &path)?;
                    // Grammar initialization and splitting large documents
                    // must not block input or the first loading frame.
                    let _ = forge_ui::TextLang::from_name(&path.to_string_lossy());
                    let mut editor = TextEditor::new(document, line);
                    if conflict {
                        editor.enable_conflicts(line == 1);
                    }
                    Ok(editor)
                })
                .await?
            },
        ));
    }
    fn close_text_editor(&mut self) {
        self.services.jobs.cancel_key("editor:load");
        self.services.jobs.cancel_key("editor:syntax");
        self.editing.pending_load = None;
        self.editing.view = None;
        self.editing.terminal_config = false;
        if let Some(focus) = self.editing.return_focus.take() {
            self.session.focus = focus;
        }
    }
    fn save_text_editor(&mut self, close: bool) {
        if self.editing.saving() {
            return;
        }
        let Some(view) = &mut self.editing.view else {
            return;
        };
        if !view.dirty() {
            view.confirm_close = false;
            view.message = "Saved".into();
            if close {
                self.close_text_editor();
            }
            return;
        }
        let document = view.document.clone();
        let value = view.value();
        if self.editing.terminal_config {
            if let Err(error) = Config::parse(&value) {
                view.message = format!("Invalid config: {error}");
                view.confirm_close = false;
                return;
            }
        }
        view.message = "Saving…".into();
        self.editing.close_after_save = close;
        self.editing.pending_save = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("editor:save".into()),
            async move { tokio::task::spawn_blocking(move || text::save(document, value)).await? },
        ));
    }
    pub(super) fn poll_editor(&mut self) {
        if let Some(view) = &mut self.editing.view {
            view.syntax_cache.poll();
        }
        if let Some(result) = self
            .editing
            .pending_load
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.editing.pending_load = None;
            match result {
                Ok(Ok(view)) => self.editing.view = Some(view),
                other => {
                    self.session.status = format!(
                        "Text editor error: {}",
                        match other {
                            Ok(Err(e)) => e.to_string(),
                            Err(e) => e.to_string(),
                            _ => unreachable!(),
                        }
                    );
                    self.close_text_editor();
                }
            }
        }
        if let Some(result) = self
            .editing
            .pending_save
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.editing.pending_save = None;
            match result {
                Ok(Ok(doc)) => {
                    let path = doc.path.clone();
                    if let Some(view) = &mut self.editing.view {
                        view.document = doc;
                        view.dirty_cache = OnceLock::from(false);
                        view.confirm_close = false;
                        view.message = "Saved".into();
                    }
                    self.session.status = format!("Saved {}", path.display());
                    if self.editing.terminal_config {
                        match Config::load_checked() {
                            Ok(config) => {
                                self.config.terminal = config.terminal;
                                self.shell_profiles = None;
                                self.shell_discovery = None;
                                self.session.status = "Shell profiles reloaded".into();
                            }
                            Err(error) => self.session.status = error,
                        }
                    } else {
                        self.request_status();
                        if self.in_git_sidebar() {
                            self.request_diff_for_scm_selection();
                        } else {
                            self.tools.reveal = Some((path, self.preview_source_line()));
                            self.tools.return_focus = Some(self.session.focus);
                            self.request_preview_for_selection();
                        }
                    }
                    if self.editing.close_after_save {
                        self.close_text_editor();
                    }
                }
                other => {
                    if let Some(view) = &mut self.editing.view {
                        view.message = match other {
                            Ok(Err(e)) => e.to_string(),
                            Err(e) => e.to_string(),
                            _ => unreachable!(),
                        };
                        view.confirm_close = false;
                    }
                }
            }
            self.editing.close_after_save = false;
        }
    }
    pub(super) fn handle_editor_paste(&mut self, value: &str) {
        if self.editing.saving() {
            return;
        }
        if let Some(view) = &mut self.editing.view {
            if !view.confirm_close {
                view.follow_cursor = true;
                view.insert(value);
            }
        }
    }
    pub(super) fn handle_editor_key(&mut self, mut key: KeyEvent) {
        if key.code == KeyCode::F(4) && key.modifiers.is_empty() {
            self.keys.open = true;
            return;
        }
        let Some(mapped) = self.keys.remap(key, true) else {
            return;
        };
        key = mapped;
        key = match (key.code, key.modifiers) {
            (KeyCode::Insert, KeyModifiers::CONTROL) => {
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            }
            (KeyCode::Insert, KeyModifiers::SHIFT) => {
                KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)
            }
            (KeyCode::Delete, KeyModifiers::SHIFT) => {
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
            }
            _ => key,
        };
        if self.editing.saving() {
            return;
        }
        let Some(view) = &mut self.editing.view else {
            if key.code == KeyCode::Esc {
                self.close_text_editor();
            }
            return;
        };
        // Keyboard input resumes caret following after wheel scrolling.
        view.follow_cursor = true;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('s') {
            let close = view.confirm_close;
            self.save_text_editor(close);
            return;
        }
        if view.confirm_close {
            match key.code {
                // Do not let an accidental Enter silently throw away a draft.
                KeyCode::Char('d' | 'D') => self.close_text_editor(),
                KeyCode::Enter | KeyCode::Esc => view.confirm_close = false,
                _ => {}
            }
            return;
        }
        if view.conflict_mode && key.modifiers == KeyModifiers::ALT {
            let action = match key.code {
                KeyCode::Up => Some(ConflictAction::Previous),
                KeyCode::Down => Some(ConflictAction::Next),
                KeyCode::Char('1') => Some(ConflictAction::Current),
                KeyCode::Char('2') => Some(ConflictAction::Incoming),
                KeyCode::Char('3') => Some(ConflictAction::Both),
                _ => None,
            };
            if let Some(action) = action {
                view.apply_conflict(action);
                return;
            }
        }
        if key.code == KeyCode::Esc && view.selection().is_some() {
            view.anchor = None;
            view.dragging = false;
            view.message = "Selection cleared".into();
            return;
        }
        if key.code == KeyCode::Esc || (ctrl && matches!(key.code, KeyCode::Char('q'))) {
            if view.dirty() {
                view.confirm_close = true;
            } else {
                self.close_text_editor();
            }
            return;
        }
        if ctrl {
            match key.code {
                KeyCode::Char('a') => {
                    view.select_all();
                    return;
                }
                KeyCode::Char('c' | 'x') => {
                    let whole_line = view.selection().is_none();
                    let line_text = whole_line.then(|| format!("{}\n", view.lines[view.row]));
                    if whole_line && key.code == KeyCode::Char('x') {
                        view.select_line();
                    }
                    let Some(text) = line_text.or_else(|| view.selected_text()) else {
                        view.message = "Select text first · Shift+arrows or mouse drag".into();
                        return;
                    };
                    self.editing.clipboard = text.clone();
                    self.editing.clipboard_line = whole_line;
                    let native = write_editor_clipboard(&mut self.editing.system_clipboard, &text);
                    if key.code == KeyCode::Char('x') {
                        view.delete_selection();
                    }
                    view.message = if native {
                        "Copied to clipboard"
                    } else {
                        "Copied within Forge · terminal clipboard requested · Ctrl-V Paste"
                    }
                    .into();
                    return;
                }
                KeyCode::Char('v') => {
                    let text = read_editor_clipboard(&mut self.editing.system_clipboard)
                        .unwrap_or_else(|| self.editing.clipboard.clone());
                    if text.is_empty() {
                        view.message =
                            "Clipboard empty · use terminal Paste for external text".into();
                    } else {
                        if self.editing.clipboard_line
                            && text == self.editing.clipboard
                            && view.selection().is_none()
                        {
                            view.col = 0;
                        }
                        view.insert(&text);
                    }
                    return;
                }
                _ => {}
            }
        }
        let movement = matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
        );
        if movement {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                view.anchor.get_or_insert((view.row, view.col));
            } else {
                if let Some((start, end)) = view.selection() {
                    if matches!(key.code, KeyCode::Left | KeyCode::Right) && !ctrl {
                        let (row, col) = if key.code == KeyCode::Left {
                            start
                        } else {
                            end
                        };
                        view.row = row;
                        view.col = col;
                        view.anchor = None;
                        return;
                    }
                }
                view.anchor = None;
            }
        }
        match key.code {
            KeyCode::Char('z') if ctrl => view.undo(false),
            KeyCode::Char('y') if ctrl => view.undo(true),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                view.insert(&c.to_string())
            }
            KeyCode::Enter => view.insert("\n"),
            KeyCode::Tab => view.insert("\t"),
            KeyCode::Backspace => view.backspace(),
            KeyCode::Delete => view.delete(),
            KeyCode::Left => {
                if view.col > 0 {
                    view.col = view.lines[view.row][..view.col]
                        .char_indices()
                        .last()
                        .unwrap()
                        .0;
                } else if view.row > 0 {
                    view.row -= 1;
                    view.col = view.lines[view.row].len();
                }
            }
            KeyCode::Right => {
                if view.col < view.lines[view.row].len() {
                    view.col += view.lines[view.row][view.col..]
                        .chars()
                        .next()
                        .unwrap()
                        .len_utf8();
                } else if view.row + 1 < view.lines.len() {
                    view.row += 1;
                    view.col = 0;
                }
            }
            KeyCode::Up => view.up_down(-1),
            KeyCode::Down => view.up_down(1),
            KeyCode::PageUp => view.up_down(-(view.viewport.height.max(1) as isize)),
            KeyCode::PageDown => view.up_down(view.viewport.height.max(1) as isize),
            KeyCode::Home => {
                if ctrl {
                    view.row = 0;
                }
                view.col = 0;
            }
            KeyCode::End => {
                if ctrl {
                    view.row = view.lines.len() - 1;
                }
                view.col = view.lines[view.row].len();
            }
            _ => {}
        }
    }
    pub(super) fn handle_editor_mouse(&mut self, mouse: MouseEvent) {
        if self.editing.saving() {
            return;
        }
        let Some(view) = &mut self.editing.view else {
            return;
        };
        if view.confirm_close {
            return;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some((_, action, target)) = view
                .conflict_hits
                .iter()
                .find(|(rect, _, _)| rect.contains((mouse.column, mouse.row).into()))
                .copied()
            {
                if let Some(start) = target.filter(|_| {
                    matches!(
                        action,
                        ConflictAction::Current | ConflictAction::Incoming | ConflictAction::Both
                    )
                }) {
                    if !view.conflicts().iter().any(|b| b.start == start) {
                        return;
                    }
                    view.row = start;
                    view.col = 0;
                }
                view.apply_conflict(action);
                return;
            }
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if view.viewport.contains((mouse.column, mouse.row).into()) =>
            {
                view.follow_cursor = true;
                if !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    view.anchor = None;
                } else {
                    view.anchor.get_or_insert((view.row, view.col));
                }
                view.row =
                    (view.top + (mouse.row - view.viewport.y) as usize).min(view.lines.len() - 1);
                view.col = byte_at_cell(
                    &view.lines[view.row],
                    view.left + (mouse.column - view.viewport.x) as usize,
                );
                view.anchor.get_or_insert((view.row, view.col));
                view.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left)
                if view.dragging && view.viewport.width > 0 && view.viewport.height > 0 =>
            {
                view.follow_cursor = true;
                if mouse.row < view.viewport.y {
                    view.top = view.top.saturating_sub(1);
                }
                if mouse.row >= view.viewport.bottom() {
                    view.top = (view.top + 1).min(view.lines.len() - 1);
                }
                let y = mouse.row.clamp(view.viewport.y, view.viewport.bottom() - 1);
                view.row = (view.top + (y - view.viewport.y) as usize).min(view.lines.len() - 1);
                let x = mouse
                    .column
                    .clamp(view.viewport.x, view.viewport.right() - 1);
                view.col = byte_at_cell(
                    &view.lines[view.row],
                    view.left + (x - view.viewport.x) as usize,
                );
            }
            MouseEventKind::Up(MouseButton::Left) => view.dragging = false,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                // Pan the viewport; never move the caret or selection anchor.
                let max_top = view
                    .lines
                    .len()
                    .saturating_sub(view.viewport.height.max(1) as usize);
                view.top = match mouse.kind {
                    MouseEventKind::ScrollUp => view.top.saturating_sub(3),
                    _ => view.top.saturating_add(3).min(max_top),
                };
                view.follow_cursor = false;
            }
            _ => {}
        }
    }
}

// Build borrowed style runs for the horizontal viewport. Work and allocations
// after locating its left edge depend on screen width, not source-line length.
fn editor_line<'a>(
    source: &'a str,
    colored: Option<&Line<'_>>,
    selection: Option<std::ops::Range<usize>>,
    left: usize,
    width: usize,
    highlight: Style,
) -> (Line<'a>, u16) {
    if width == 0 {
        return (Line::default(), 0);
    }
    let start = byte_at_cell(source, left);
    let start_cell = display_width(&source[..start]);
    let mut cell = start_cell;
    let mut syntax = colored.into_iter().flat_map(|line| line.spans.iter());
    let mut syntax_end = 0;
    let mut syntax_style = Style::default();
    let mut spans = Vec::new();
    let mut run_start = start;
    let mut run_end = start;
    let mut run_style = Style::default();
    for (offset, ch) in source[start..].char_indices() {
        let byte = start + offset;
        if cell >= left + width && char_width(ch) > 0 {
            break;
        }
        while byte >= syntax_end {
            let Some(span) = syntax.next() else { break };
            syntax_end += span.content.len();
            syntax_style = span.style;
        }
        let style = if selection
            .as_ref()
            .is_some_and(|range| range.contains(&byte))
        {
            highlight
        } else {
            syntax_style
        };
        if style != run_style || ch == '\t' {
            if run_start < byte {
                spans.push(Span::styled(&source[run_start..byte], run_style));
            }
            run_start = byte;
            run_style = style;
        }
        run_end = byte + ch.len_utf8();
        if ch == '\t' {
            spans.push(Span::styled("    ", style));
            run_start = run_end;
        }
        cell += char_width(ch);
    }
    if run_start < run_end {
        spans.push(Span::styled(&source[run_start..run_end], run_style));
    }
    if cell < left + width
        && selection
            .as_ref()
            .is_some_and(|range| range.contains(&source.len()))
    {
        spans.push(Span::styled(" ", highlight));
    }
    (
        Line::from(spans),
        (left - start_cell).min(u16::MAX as usize) as u16,
    )
}

pub(super) fn draw_editor(frame: &mut Frame<'_>, area: Rect, app: &mut App, p: Palette) {
    frame.render_widget(Clear, area);
    let saving = app.editing.saving();
    let binding_help = app.keys.editor_help();
    let close_help = format!(
        "Unsaved · Enter/Esc Keep editing · {} Save & close · D Discard & close",
        app.keys.action_key("save")
    );
    let Some(view) = &mut app.editing.view else {
        frame.render_widget(
            Paragraph::new("Loading text… · Esc cancel").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Text editor "),
            ),
            area,
        );
        return;
    };
    let title = format!(
        " Edit {}{} ",
        view.document
            .path
            .strip_prefix(&app.files.tree_root)
            .unwrap_or(&view.document.path)
            .display(),
        if view.dirty() { " *" } else { "" }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    view.conflict_hits.clear();
    let conflict_blocks = if view.conflict_mode {
        view.conflicts().to_vec()
    } else {
        Vec::new()
    };
    let active = if view.conflict_mode {
        view.active_conflict()
    } else {
        None
    };
    let mut inner = inner;
    if view.conflict_mode && inner.height >= 5 {
        let label = if conflict_blocks.is_empty() {
            "No complete conflicts · Review & save, then stage".to_owned()
        } else {
            format!(
                "Conflict {}/{} · Alt+↑/↓  Alt+1/2/3",
                active.map_or(0, |i| i + 1),
                conflict_blocks.len()
            )
        };
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(p.warning)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        if !conflict_blocks.is_empty() {
            let compact = inner.width < 60;
            let buttons = [
                (" Prev ", ConflictAction::Previous),
                (" Next ", ConflictAction::Next),
                (
                    if compact {
                        " Current "
                    } else {
                        " Accept Current "
                    },
                    ConflictAction::Current,
                ),
                (
                    if compact {
                        " Incoming "
                    } else {
                        " Accept Incoming "
                    },
                    ConflictAction::Incoming,
                ),
                (
                    if compact { " Both " } else { " Accept Both " },
                    ConflictAction::Both,
                ),
            ];
            let mut x = inner.x;
            for (label, action) in buttons {
                let width = label.len() as u16;
                if x.saturating_add(width) > inner.right() {
                    break;
                }
                let rect = Rect::new(x, inner.y + 1, width, 1);
                let enabled = !saving
                    && !view.confirm_close
                    && (active.is_some()
                        || matches!(action, ConflictAction::Previous | ConflictAction::Next));
                frame.render_widget(
                    Paragraph::new(label).style(
                        Style::default()
                            .fg(if enabled { p.accent } else { p.fg_dim })
                            .add_modifier(Modifier::BOLD),
                    ),
                    rect,
                );
                if enabled {
                    view.conflict_hits.push((
                        rect,
                        action,
                        active.map(|i| conflict_blocks[i].start),
                    ));
                }
                x += width;
            }
        }
        inner.y += 2;
        inner.height -= 2;
    }
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(if inner.height > 2 { 2 } else { 1 }),
    ])
    .split(inner);
    let gutter = (view.lines.len().to_string().len() + 2)
        .min(rows[0].width.saturating_sub(1) as usize) as u16;
    let text_area = Rect::new(
        rows[0].x + gutter,
        rows[0].y,
        rows[0].width.saturating_sub(gutter),
        rows[0].height,
    );
    view.viewport = text_area;
    if view.follow_cursor {
        if view.row < view.top {
            view.top = view.row;
        }
        if view.row >= view.top + text_area.height as usize {
            view.top = view
                .row
                .saturating_sub(text_area.height.saturating_sub(1) as usize);
        }
    } else {
        view.top = view.top.min(
            view.lines
                .len()
                .saturating_sub(text_area.height.max(1) as usize),
        );
    }
    let cursor = display_width(&view.lines[view.row][..view.col]);
    if view.follow_cursor {
        if cursor < view.left {
            view.left = cursor;
        }
        if cursor >= view.left + text_area.width as usize {
            view.left = cursor.saturating_sub(text_area.width.saturating_sub(1) as usize);
        }
    }
    // Only tokenize the visible editor window. Keep per-line styles separate
    // from selection, which must always take precedence over syntax colors.
    let syntax_theme = super::view::bridge_theme(p);
    let syntax_path = view.document.path.to_string_lossy();
    let visible_end = (view.top + text_area.height as usize).min(view.lines.len());
    let selection = view.selection();
    let colored = view.syntax_cache.visible(
        &app.services.jobs,
        &syntax_theme,
        &syntax_path,
        &view.lines,
        view.top,
        visible_end,
    );
    for (offset, line) in view
        .lines
        .iter()
        .enumerate()
        .skip(view.top)
        .take(text_area.height as usize)
    {
        let y = text_area.y + (offset - view.top) as u16;
        frame.render_widget(
            Paragraph::new(format!(
                "{:>width$} ",
                offset + 1,
                width = gutter.saturating_sub(1) as usize
            ))
            .style(
                Style::default().fg(conflict_blocks
                    .iter()
                    .find(|b| b.start <= offset && offset <= b.end)
                    .map_or(p.fg_dim, |b| {
                        if offset == b.start
                            || offset == b.end
                            || offset == b.separator
                            || b.base == Some(offset)
                        {
                            p.warning
                        } else if offset < b.base.unwrap_or(b.separator) {
                            p.success
                        } else if offset > b.separator {
                            p.info
                        } else {
                            p.fg_dim
                        }
                    })),
            ),
            Rect::new(rows[0].x, y, gutter, 1),
        );
        let highlight = Style::default().fg(p.on_accent).bg(p.selection);
        let selected = selection.and_then(|((sr, sc), (er, ec))| {
            (offset >= sr && offset <= er).then_some(
                (if offset == sr { sc } else { 0 })..(if offset == er {
                    ec
                } else {
                    line.len() + 1
                }),
            )
        });
        let (visible, scroll) = editor_line(
            line,
            colored.get(offset - view.top),
            selected.clone(),
            view.left,
            text_area.width as usize,
            highlight,
        );
        frame.render_widget(
            Paragraph::new(visible).scroll((0, scroll)),
            Rect::new(text_area.x, y, text_area.width, 1),
        );
        if !saving
            && !view.confirm_close
            && view.left == 0
            && conflict_blocks.iter().any(|b| b.start == offset)
            && display_width(line) + 30 <= text_area.width as usize
            && selected.is_none()
        {
            let mut x = text_area.right() - 29;
            for (label, action) in [
                ("[Current]", ConflictAction::Current),
                ("[Incoming]", ConflictAction::Incoming),
                ("[Both]", ConflictAction::Both),
            ] {
                let rect = Rect::new(x, y, label.len() as u16, 1);
                frame.render_widget(
                    Paragraph::new(label)
                        .style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
                    rect,
                );
                view.conflict_hits.push((rect, action, Some(offset)));
                x += label.len() as u16 + 1;
            }
        }
    }
    let help = if saving {
        "Saving…"
    } else if view.confirm_close {
        &close_help
    } else {
        &binding_help
    };
    let message = format!(
        "{}:{}  {}",
        view.row + 1,
        view.lines[view.row][..view.col].chars().count() + 1,
        view.message
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                help,
                Style::default()
                    .fg(if view.confirm_close {
                        p.warning
                    } else {
                        p.fg_dim
                    })
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(message),
        ]),
        rows[1],
    );
    if !view.confirm_close
        && !saving
        && text_area.width > 0
        && text_area.height > 0
        && view.row >= view.top
        && view.row < view.top + text_area.height as usize
        && cursor >= view.left
        && cursor < view.left + text_area.width as usize
    {
        frame.set_cursor_position((
            text_area.x + (cursor - view.left) as u16,
            text_area.y + (view.row - view.top) as u16,
        ));
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "manual editor latency measurements; run with --ignored --nocapture"]
    async fn profile_editor_latency() {
        for (name, content) in [
            ("short lines", "let item = 123;\n".repeat(60_000)),
            ("long line", "a".repeat(128 * 1024)),
            (
                "TypeScript",
                "const item = { value: 123 };\n".repeat(30_000),
            ),
        ] {
            let (_dir, mut app, path) = fixture(&content);
            app.editing.view = Some(TextEditor::new(
                text::load(&app.files.tree_root, &path).unwrap(),
                1,
            ));
            if name == "TypeScript" {
                app.editing
                    .view
                    .as_mut()
                    .unwrap()
                    .document
                    .path
                    .set_extension("ts");
            }
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
            // Warm up syntax initialization separately from steady-state redraws.
            let start = std::time::Instant::now();
            terminal
                .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
                .unwrap();
            println!(
                "{name}: first draw {:.3} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
            let start = std::time::Instant::now();
            for _ in 0..10 {
                terminal
                    .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
                    .unwrap();
            }
            println!(
                "{name}: redraw {:.3} ms/frame",
                start.elapsed().as_secs_f64() * 100.0
            );
            let view = app.editing.view.as_mut().unwrap();
            let start = std::time::Instant::now();
            for _ in 0..32 {
                view.insert("x");
                std::hint::black_box(view.dirty());
            }
            println!(
                "{name}: insert + dirty {:.3} ms/key",
                start.elapsed().as_secs_f64() * 1000.0 / 32.0
            );
            let start = std::time::Instant::now();
            for _ in 0..10 {
                let view = app.editing.view.as_mut().unwrap();
                view.top = view.top.saturating_add(3).min(view.lines.len() - 1);
                view.follow_cursor = false;
                terminal
                    .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
                    .unwrap();
            }
            println!(
                "{name}: scroll {:.3} ms/frame",
                start.elapsed().as_secs_f64() * 100.0
            );
        }
    }

    #[tokio::test]
    async fn syntax_cache_scrolls_within_lookbehind_and_skips_very_long_lines() {
        let mut cache = EditorSyntaxCache::default();
        let jobs = JobScheduler::new();
        let theme = forge_theme::Theme::modern_ide();
        let mut source = (0..100)
            .map(|i| format!("const item{i} = {i};"))
            .collect::<Vec<_>>();
        cache.visible(&jobs, &theme, "example.ts", &source, 0, 10);
        settle_syntax(&mut cache).await;
        let scrolled = cache.visible(&jobs, &theme, "example.ts", &source, 3, 10);
        assert_eq!(scrolled.len(), 7);
        assert_eq!(
            scrolled[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            source[3]
        );
        source[3] = "x".repeat(128 * 1024);
        cache.visible(&jobs, &theme, "example.rs", &source, 3, 10);
        assert!(
            cache.source[3].is_empty(),
            "long lines should bypass every syntax parser"
        );
    }

    #[test]
    fn editor_line_bounds_output_and_preserves_unicode_tabs_and_selection() {
        let highlight = Style::default().bg(ratatui::style::Color::Blue);
        let long = "x".repeat(128 * 1024);
        let (line, scroll) = editor_line(&long, None, None, 0, 80, highlight);
        assert_eq!(scroll, 0);
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content.len(), 80);
        assert!(matches!(
            line.spans[0].content,
            std::borrow::Cow::Borrowed(_)
        ));

        let text = "中\ta\u{301}bc";
        let syntax = Line::from(vec![
            Span::styled("中\t", Style::default().fg(ratatui::style::Color::Red)),
            Span::styled(
                "a\u{301}bc",
                Style::default().fg(ratatui::style::Color::Green),
            ),
        ]);
        let (line, scroll) = editor_line(text, Some(&syntax), Some(4..7), 3, 5, highlight);
        assert_eq!(
            scroll, 1,
            "scrolling into a tab retains its remaining cells"
        );
        assert_eq!(
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "    a\u{301}b"
        );
        assert_eq!(line.spans[0].style.fg, Some(ratatui::style::Color::Red));
        assert_eq!(line.spans[1].content, "a\u{301}");
        assert_eq!(line.spans[1].style, highlight);
        assert_eq!(line.spans[2].style.fg, Some(ratatui::style::Color::Green));

        let (line, _) = editor_line("a", None, Some(0..2), 0, 5, highlight);
        assert_eq!(
            line.spans.last().unwrap().content,
            " ",
            "selected newlines remain visible"
        );
        let (line, scroll) = editor_line(&long, None, None, 70_000, 5, highlight);
        assert_eq!(scroll, 0);
        assert_eq!(line.spans[0].content.len(), 5);
    }

    #[tokio::test]
    async fn undo_retains_only_changed_lines_and_tracks_bytes_and_saved_content() {
        let content = "中文\nsecond\nlast\n".repeat(1000);
        let (dir, _app, path) = fixture(&content);
        let mut view = TextEditor::new(text::load(dir.path(), &path).unwrap(), 1);
        assert!(!view.dirty());
        view.insert("x");
        assert!(view.dirty());
        assert_eq!(view.undo[0].lines, ["中文"]);
        view.backspace();
        assert!(
            !view.dirty(),
            "manually restoring saved text clears the dirty indicator"
        );
        view.row = 1;
        view.col = 0;
        view.backspace(); // Join with the preceding line.
        view.insert("\nnew\n");
        view.delete();
        view.anchor = Some((0, 3));
        view.row = 3;
        view.col = 2;
        assert_eq!(view.selected_len(), view.selected_text().unwrap().len());
        view.insert("replacement\n文");
        let edited = view.value();
        assert_eq!(view.byte_len, edited.len());
        let steps = view.undo.len();
        for _ in 0..steps {
            view.undo(false);
            assert_eq!(view.byte_len, view.value().len());
        }
        assert_eq!(view.value(), content);
        assert!(!view.dirty());
        for _ in 0..steps {
            view.undo(true);
            assert_eq!(view.byte_len, view.value().len());
        }
        assert_eq!(view.value(), edited);
        assert!(view.dirty());

        view.select_all();
        view.insert(&"a".repeat(text::MAX_TEXT_BYTES));
        assert_eq!(view.byte_len, text::MAX_TEXT_BYTES);
        let steps = view.undo.len();
        view.insert("中");
        assert_eq!(view.undo.len(), steps);
        assert_eq!(view.byte_len, text::MAX_TEXT_BYTES);
        view.select_all();
        view.insert("small");
        assert_eq!(view.value(), "small");
        assert_eq!(view.byte_len, 5);
    }

    async fn settle_syntax(cache: &mut EditorSyntaxCache) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while cache.pending.is_some() {
                cache.poll();
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn syntax_cache_tracks_edits_and_bounded_scroll_window() {
        let mut cache = EditorSyntaxCache::default();
        let jobs = JobScheduler::new();
        let theme = forge_theme::Theme::modern_ide();
        let mut source = (0..200)
            .map(|i| format!("const item{i} = {i};"))
            .collect::<Vec<_>>();
        cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        assert!(
            cache.pending.is_some(),
            "parsing must run outside the input frame"
        );
        settle_syntax(&mut cache).await;
        let first = cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        assert_eq!(first.len(), 20);
        let initial = first[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert_eq!(initial, source[100]);
        assert_eq!(cache.source.len(), 68); // 48 lookbehind + 20 visible
        source[100] = "const updated = 999;".to_owned();
        cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        let pending = cache.pending.as_ref().unwrap().id;
        for i in 0..10 {
            source[100] = format!("const updated = {i};");
            cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        }
        assert_eq!(
            cache.pending.as_ref().unwrap().id,
            pending,
            "rapid edits must share one in-flight parse instead of flooding completion wakeups"
        );
        settle_syntax(&mut cache).await;
        assert_eq!(
            cache.colored[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            initial,
            "an obsolete result must not replace current colors"
        );
        cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        settle_syntax(&mut cache).await;
        let edited = cache.visible(&jobs, &theme, "example.ts", &source, 100, 120);
        assert_eq!(
            edited[0]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            source[100]
        );
        assert_eq!(cache.source.len(), 68);
        assert_eq!(
            cache
                .visible(&jobs, &theme, "example.ts", &source, 101, 121)
                .len(),
            20
        );
    }

    use super::*;

    fn fixture(value: &str) -> (tempfile::TempDir, App, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        std::fs::write(&path, value).unwrap();
        let app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        (dir, app, path)
    }
    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers));
    }
    async fn finish(app: &mut App) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while app.editing.pending_load.is_some() || app.editing.pending_save.is_some() {
                app.poll_editor();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    const CONFLICTS: &str = "prefix\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> branch\nmiddle\n<<<<<<< HEAD\n=======\nincoming\n>>>>>>> branch\nsuffix\n";

    #[test]
    fn conflict_parser_rejects_malformed_blocks_and_understands_diff3() {
        let lines = |value: &str| value.split('\n').map(str::to_owned).collect::<Vec<_>>();
        let blocks = conflict_blocks(&lines(CONFLICTS));
        assert_eq!(
            blocks,
            vec![
                ConflictBlock {
                    start: 1,
                    base: Some(3),
                    separator: 5,
                    end: 7
                },
                ConflictBlock {
                    start: 9,
                    base: None,
                    separator: 10,
                    end: 12
                }
            ]
        );
        assert!(conflict_blocks(&lines("<<<<<<< HEAD\na\n=======\nb")).is_empty());
        assert!(conflict_blocks(&lines(
            "<<<<<<< HEAD\n<<<<<<< nested\n=======\n>>>>>>> branch"
        ))
        .is_empty());
        assert_eq!(
            conflict_blocks(&lines("<<<<<<<<< HEAD\na\n=========\nb\n>>>>>>>>> side")).len(),
            1
        );
    }

    #[tokio::test]
    async fn conflict_accept_both_and_empty_current_preserve_context_and_undo() {
        let (_dir, mut app, path) = fixture(CONFLICTS);
        app.open_conflict_editor(path.clone());
        finish(&mut app).await;
        let v = app.editing.view.as_ref().unwrap();
        assert_eq!(v.active_conflict(), Some(0));
        key(&mut app, KeyCode::Char('3'), KeyModifiers::ALT);
        let v = app.editing.view.as_ref().unwrap();
        assert_eq!(v.conflicts().len(), 1);
        assert!(v.value().starts_with("prefix\nours\ntheirs\nmiddle\n"));
        key(&mut app, KeyCode::Char('1'), KeyModifiers::ALT);
        let v = app.editing.view.as_ref().unwrap();
        assert_eq!(v.value(), "prefix\nours\ntheirs\nmiddle\nsuffix\n");
        assert!(v.conflicts().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFLICTS);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.view.as_ref().unwrap().conflicts().len(), 1);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), CONFLICTS);
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('2'), KeyModifiers::ALT);
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        finish(&mut app).await;
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "prefix\nours\ntheirs\nmiddle\nincoming\nsuffix\n"
        );
    }

    #[tokio::test]
    async fn inline_conflict_mouse_action_targets_its_own_block() {
        let (_dir, mut app, path) = fixture(CONFLICTS);
        app.open_conflict_editor(path);
        finish(&mut app).await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        let rect = app
            .editing
            .view
            .as_ref()
            .unwrap()
            .conflict_hits
            .iter()
            .find(|(_, action, target)| *action == ConflictAction::Incoming && *target == Some(9))
            .unwrap()
            .0;
        app.handle_editor_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        let v = app.editing.view.as_ref().unwrap();
        assert_eq!(v.conflicts().len(), 1);
        assert!(v.value().ends_with("middle\nincoming\nsuffix\n"));
        assert!(v.value().contains("||||||| base"));
    }

    #[tokio::test]
    async fn conflict_examples_in_regular_files_do_not_enable_resolution() {
        let (_dir, mut app, path) = fixture(CONFLICTS);
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        assert!(!app.editing.view.as_ref().unwrap().conflict_mode);
        key(&mut app, KeyCode::Char('3'), KeyModifiers::ALT);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), CONFLICTS);
    }

    #[tokio::test]
    async fn editor_opened_from_git_is_in_files_and_esc_never_reveals_git() {
        let (_dir, mut app, path) = fixture("original");
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.open_text_editor(path.clone(), 1);
        assert_eq!(
            app.session.activity,
            ActivityTab::Files,
            "Editor must not sit on top of the Git Diff Activity"
        );
        assert!(app.editing.active());
        finish(&mut app).await;
        assert!(app.editing.view.is_some());

        // Normal Esc closes the clean editor, without changing Activity.
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.editing.active());
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(app.session.focus, PaneFocus::List);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(app.session.focus, PaneFocus::List);

        // A dirty draft retains the normal confirmation behavior.
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.editing.exit_confirmation());
        assert_eq!(app.session.activity, ActivityTab::Files);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(
            app.editing.active(),
            "Esc on dirty confirmation keeps editing"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('D'), KeyModifiers::NONE);
        assert!(!app.editing.active());
        assert_eq!(app.session.activity, ActivityTab::Files);
    }

    #[tokio::test]
    async fn multiline_unicode_paste_save_and_editor_shortcuts_are_isolated() {
        let (_dir, mut app, path) = fixture("尾行");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        for c in "qgsp中文".chars() {
            key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.handle_editor_paste("粘贴\r\nsecond");
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "qgsp中文\n粘贴\nsecond尾行"
        );
        assert!(!app.session.should_quit);
        assert!(!app.in_git_sidebar());
        assert!(app.scm.pending_write.is_none());
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        finish(&mut app).await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "qgsp中文\n粘贴\nsecond尾行"
        );
        assert!(!app.editing.view.as_ref().unwrap().dirty());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.editing.active());
    }

    #[tokio::test]
    async fn dirty_close_can_resume_discard_or_save_and_close() {
        let (_dir, mut app, path) = fixture("original");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        app.handle_editor_paste("draft ");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.editing.view.as_ref().unwrap().confirm_close);
        app.handle_editor_paste("ignored");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.editing.view.as_ref().unwrap().confirm_close);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.editing.active(), "Enter should keep the draft");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(!app.editing.active());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        app.handle_editor_paste("saved ");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        finish(&mut app).await;
        assert!(!app.editing.active());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "saved original");
    }

    #[tokio::test]
    async fn esc_clears_editor_selection_before_exiting() {
        let (_dir, mut app, path) = fixture("hello");
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(app.editing.has_selection());

        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.editing.active());
        assert!(!app.editing.has_selection());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.editing.active());
    }

    #[tokio::test]
    async fn external_changes_keep_draft_and_disk_content() {
        let (_dir, mut app, path) = fixture("original");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        app.handle_editor_paste("draft ");
        std::fs::write(&path, "external").unwrap();
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        finish(&mut app).await;
        let view = app.editing.view.as_ref().unwrap();
        assert_eq!(view.value(), "draft original");
        assert!(view.dirty());
        assert!(view.message.contains("changed on disk"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external");
    }

    #[tokio::test]
    async fn unicode_cursor_delete_join_undo_redo_and_small_viewports() {
        let (_dir, mut app, path) = fixture("中a\nsecond");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "a\nsecond");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "中a\nsecond");
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::End, KeyModifiers::NONE);
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "asecond");
        app.handle_editor_paste("\n\t你好\nline\n");
        for (w, h) in [(1, 1), (4, 3), (12, 5), (40, 10), (100, 30)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|frame| draw_editor(frame, frame.area(), &mut app, Palette::default()))
                .unwrap();
        }
    }

    #[tokio::test]
    async fn selection_copy_cut_line_delete_paste_and_undo() {
        let (_dir, mut app, path) = fixture("中文\nsecond\nlast");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.editing.active());
        assert!(!app.session.should_quit);
        key(&mut app, KeyCode::Right, KeyModifiers::SHIFT);
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.clipboard, "中");
        key(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "文\nsecond\nlast"
        );
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "中文\nsecond\nlast"
        );
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.clipboard, "中文\n");
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "second\nlast");
        key(&mut app, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "中文\nsecond\nlast"
        );
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.handle_editor_paste("replacement");
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "replacement");
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "中文\nsecond\nlast"
        );
        for line in [2, 3] {
            app.editing.view = Some(TextEditor::new(
                text::load(_dir.path(), &path).unwrap(),
                line,
            ));
            key(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
            let expected = if line == 2 {
                "中文\nlast"
            } else {
                "中文\nsecond"
            };
            assert_eq!(app.editing.view.as_ref().unwrap().value(), expected);
            key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
            assert_eq!(
                app.editing.view.as_ref().unwrap().value(),
                "中文\nsecond\nlast"
            );
        }
    }

    #[tokio::test]
    async fn copy_without_selection_copies_line_without_changing_cursor_or_document() {
        let (_dir, mut app, path) = fixture("中文\nlast\n");
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        let view = app.editing.view.as_ref().unwrap();
        let before = view.snapshot(0, view.lines.len());
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.clipboard, "中文\n");
        assert!(app.editing.clipboard_line);
        let view = app.editing.view.as_ref().unwrap();
        assert_eq!(
            (view.row, view.col, view.anchor),
            (before.row, before.col, before.anchor)
        );
        assert_eq!(view.value(), before.lines.join("\n"));
        assert!(!view.dirty());
        assert!(view.undo.is_empty());
        key(&mut app, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(
            app.editing.view.as_ref().unwrap().value(),
            "中文\n中文\nlast\n"
        );
        key(&mut app, KeyCode::End, KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Insert, KeyModifiers::CONTROL);
        assert_eq!(app.editing.clipboard, "\n");
        assert!(!app.session.should_quit);
    }

    #[tokio::test]
    async fn mouse_wheel_scrolls_viewport_without_moving_cursor_or_selecting() {
        let content = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (_dir, mut app, path) = fixture(&content);
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 12)).unwrap();
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        let area = app.editing.view.as_ref().unwrap().viewport;
        let event = |kind| MouseEvent {
            kind,
            column: area.x + 2,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };

        // A click sets the anchor. Wheel events must not extend it.
        app.handle_editor_mouse(event(MouseEventKind::Down(MouseButton::Left)));
        app.handle_editor_mouse(event(MouseEventKind::Up(MouseButton::Left)));
        let before = {
            let v = app.editing.view.as_ref().unwrap();
            (v.row, v.col, v.anchor)
        };
        app.handle_editor_mouse(event(MouseEventKind::ScrollDown));
        {
            let v = app.editing.view.as_ref().unwrap();
            assert_eq!((v.row, v.col, v.anchor), before);
            assert!(v.selection().is_none());
            assert_eq!(v.top, 3);
        }
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        assert_eq!(
            app.editing.view.as_ref().unwrap().top,
            3,
            "render must not snap the wheel-scrolled viewport back to the caret"
        );
        app.handle_editor_mouse(event(MouseEventKind::ScrollUp));
        assert_eq!(app.editing.view.as_ref().unwrap().top, 0);
        app.handle_editor_mouse(event(MouseEventKind::ScrollDown));
        app.handle_editor_mouse(event(MouseEventKind::ScrollDown));
        assert_eq!(app.editing.view.as_ref().unwrap().top, 6);
        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        let v = app.editing.view.as_ref().unwrap();
        assert!(v.follow_cursor);
        assert!(v.row >= v.top && v.row < v.top + v.viewport.height as usize);
        assert!(v.selection().is_none());
    }

    #[tokio::test]
    async fn mouse_drag_highlights_unicode_selection_and_backspace_deletes_it() {
        let (_dir, mut app, path) = fixture("中abc\nnext");
        app.open_text_editor(path, 1);
        finish(&mut app).await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 12)).unwrap();
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        let area = app.editing.view.as_ref().unwrap().viewport;
        for (kind, x) in [
            (MouseEventKind::Down(MouseButton::Left), area.x),
            (MouseEventKind::Drag(MouseButton::Left), area.x + 3),
            (MouseEventKind::Up(MouseButton::Left), area.x + 3),
        ] {
            app.handle_editor_mouse(MouseEvent {
                kind,
                column: x,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(
            app.editing
                .view
                .as_ref()
                .unwrap()
                .selected_text()
                .as_deref(),
            Some("中a")
        );
        terminal
            .draw(|f| draw_editor(f, f.area(), &mut app, Palette::default()))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(area.x, area.y)].bg,
            Palette::default().selection
        );
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.editing.view.as_ref().unwrap().value(), "bc\nnext");
    }

    #[tokio::test]
    async fn binary_load_failure_and_cancelled_load_do_not_open_editor() {
        let (_dir, mut app, path) = fixture("\0binary");
        app.open_text_editor(path.clone(), 1);
        finish(&mut app).await;
        assert!(!app.editing.active());
        assert!(app.session.status.contains("Binary"));
        std::fs::write(&path, "text").unwrap();
        app.open_text_editor(path, 1);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.poll_editor();
        assert!(!app.editing.active());
    }
}

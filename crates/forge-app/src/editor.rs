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
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct EditingState {
    pub view: Option<TextEditor>,
    pending_load: Option<JobHandle<Result<(TextDocument, usize)>>>,
    pending_save: Option<JobHandle<Result<TextDocument>>>,
    return_focus: Option<PaneFocus>,
    close_after_save: bool,
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
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    row: usize,
    col: usize,
    anchor: Option<(usize, usize)>,
}

pub(super) struct TextEditor {
    document: TextDocument,
    lines: Vec<String>,
    row: usize,
    col: usize, // UTF-8 byte boundary
    top: usize,
    left: usize, // display cells
    viewport: Rect,
    message: String,
    confirm_close: bool,
    anchor: Option<(usize, usize)>,
    dragging: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl TextEditor {
    fn new(document: TextDocument, line: usize) -> Self {
        let lines: Vec<_> = document.text.split('\n').map(str::to_owned).collect();
        let row = line.saturating_sub(1).min(lines.len() - 1);
        Self {
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
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    fn value(&self) -> String {
        self.lines.join("\n")
    }
    fn dirty(&self) -> bool {
        self.value() != self.document.text
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.value(),
            row: self.row,
            col: self.col,
            anchor: self.anchor,
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.lines = snapshot.text.split('\n').map(str::to_owned).collect();
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.anchor = snapshot.anchor;
    }
    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.message.clear();
    }
    fn undo(&mut self, redo: bool) {
        let snapshot = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(snapshot) = snapshot {
            let current = self.snapshot();
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
    // Caller checkpoints once, so replacing a selection is one undo step.
    fn remove_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection() else {
            return false;
        };
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
        if self.value().len() - self.selected_text().map_or(0, |s| s.len()) + value.len()
            > text::MAX_TEXT_BYTES
        {
            self.message = "Text exceeds the editor's 2 MiB limit".into();
            return;
        }
        self.checkpoint();
        self.remove_selection();
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
            self.col = start;
        } else if self.row > 0 {
            self.checkpoint();
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].push_str(&line);
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
        } else if self.row + 1 < self.lines.len() {
            self.checkpoint();
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
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
        if self.editing.active() || self.write_busy() {
            return;
        }
        self.close_tools();
        self.dismiss_menu();
        self.session.write_status = None;
        self.editing.return_focus = Some(self.session.focus);
        let root = self.files.tree_root.clone();
        self.editing.pending_load = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("editor:load".into()),
            async move {
                let doc = tokio::task::spawn_blocking(move || text::load(&root, &path)).await??;
                Ok((doc, line))
            },
        ));
    }
    fn close_text_editor(&mut self) {
        self.services.jobs.cancel_key("editor:load");
        self.editing.pending_load = None;
        self.editing.view = None;
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
        view.message = "Saving…".into();
        self.editing.close_after_save = close;
        self.editing.pending_save = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("editor:save".into()),
            async move { tokio::task::spawn_blocking(move || text::save(document, value)).await? },
        ));
    }
    pub(super) fn poll_editor(&mut self) {
        if let Some(result) = self
            .editing
            .pending_load
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.editing.pending_load = None;
            match result {
                Ok(Ok((doc, line))) => self.editing.view = Some(TextEditor::new(doc, line)),
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
                        view.confirm_close = false;
                        view.message = "Saved".into();
                    }
                    self.session.status = format!("Saved {}", path.display());
                    self.request_status();
                    if self.in_git_sidebar() {
                        self.request_diff_for_scm_selection();
                    } else {
                        self.tools.reveal = Some((path, self.preview_source_line()));
                        self.tools.return_focus = Some(self.session.focus);
                        self.request_preview_for_selection();
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
                view.insert(value);
            }
        }
    }
    pub(super) fn handle_editor_key(&mut self, mut key: KeyEvent) {
        if key.code == KeyCode::F(4) {
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
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if view.viewport.contains((mouse.column, mouse.row).into()) =>
            {
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
            MouseEventKind::ScrollUp => view.up_down(-3),
            MouseEventKind::ScrollDown => view.up_down(3),
            _ => {}
        }
    }
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
    if view.row < view.top {
        view.top = view.row;
    }
    if view.row >= view.top + text_area.height as usize {
        view.top = view
            .row
            .saturating_sub(text_area.height.saturating_sub(1) as usize);
    }
    let cursor = display_width(&view.lines[view.row][..view.col]);
    if cursor < view.left {
        view.left = cursor;
    }
    if cursor >= view.left + text_area.width as usize {
        view.left = cursor.saturating_sub(text_area.width.saturating_sub(1) as usize);
    }
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
            .style(Style::default().fg(p.fg_dim)),
            Rect::new(rows[0].x, y, gutter, 1),
        );
        let selection = view.selection();
        let highlight = Style::default().fg(p.on_accent).bg(p.selection);
        let mut spans: Vec<Span> = line
            .char_indices()
            .map(|(byte, ch)| {
                let selected = selection
                    .is_some_and(|(start, end)| (offset, byte) >= start && (offset, byte) < end);
                let text = if ch == '\t' {
                    "    ".into()
                } else {
                    ch.to_string()
                };
                Span::styled(
                    text,
                    if selected {
                        highlight
                    } else {
                        Style::default()
                    },
                )
            })
            .collect();
        if selection
            .is_some_and(|(start, end)| (offset, line.len()) >= start && (offset, line.len()) < end)
        {
            spans.push(Span::styled(" ", highlight));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).scroll((0, view.left.min(u16::MAX as usize) as u16)),
            Rect::new(text_area.x, y, text_area.width, 1),
        );
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
    if !view.confirm_close && !saving && text_area.width > 0 && text_area.height > 0 {
        frame.set_cursor_position((
            text_area.x + (cursor - view.left) as u16,
            text_area.y + (view.row - view.top) as u16,
        ));
    }
}

#[cfg(test)]
mod tests {
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
        let before = app.editing.view.as_ref().unwrap().snapshot();
        key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.editing.clipboard, "中文\n");
        assert!(app.editing.clipboard_line);
        let view = app.editing.view.as_ref().unwrap();
        assert_eq!(
            (view.row, view.col, view.anchor),
            (before.row, before.col, before.anchor)
        );
        assert_eq!(view.value(), before.text);
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

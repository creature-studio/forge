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
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.lines = snapshot.text.split('\n').map(str::to_owned).collect();
        self.row = snapshot.row;
        self.col = snapshot.col;
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
    fn insert(&mut self, value: &str) {
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let value: String = value
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        if value.is_empty() {
            return;
        }
        if self.value().len() + value.len() > text::MAX_TEXT_BYTES {
            self.message = "Text exceeds the editor's 2 MiB limit".into();
            return;
        }
        self.checkpoint();
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
                        self.tools.reveal = Some((path, self.preview_state.preview_scroll + 1));
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
    pub(super) fn handle_editor_key(&mut self, key: KeyEvent) {
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
                KeyCode::Enter => self.close_text_editor(),
                KeyCode::Esc => view.confirm_close = false,
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Esc || (ctrl && matches!(key.code, KeyCode::Char('q' | 'c'))) {
            if view.dirty() {
                view.confirm_close = true;
            } else {
                self.close_text_editor();
            }
            return;
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
                view.row =
                    (view.top + (mouse.row - view.viewport.y) as usize).min(view.lines.len() - 1);
                view.col = byte_at_cell(
                    &view.lines[view.row],
                    view.left + (mouse.column - view.viewport.x) as usize,
                );
            }
            MouseEventKind::ScrollUp => view.up_down(-3),
            MouseEventKind::ScrollDown => view.up_down(3),
            _ => {}
        }
    }
}

pub(super) fn draw_editor(frame: &mut Frame<'_>, area: Rect, app: &mut App, p: Palette) {
    frame.render_widget(Clear, area);
    let saving = app.editing.saving();
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
        let expanded = line.replace('\t', "    ");
        frame.render_widget(
            Paragraph::new(expanded).scroll((0, view.left.min(u16::MAX as usize) as u16)),
            Rect::new(text_area.x, y, text_area.width, 1),
        );
    }
    let help = if saving {
        "Saving…"
    } else if view.confirm_close {
        "Unsaved changes · Ctrl-S Save & close · Enter Discard · Esc Keep editing"
    } else {
        "Ctrl-S Save · Esc Close · Ctrl-Z Undo · Ctrl-Y Redo · Paste / arrows / mouse"
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

//! Modal Git commit composer: safe two-step confirmation and multiline draft.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Subject,
    Body,
}

struct Draft {
    subject: tui_input::Input,
    body: Vec<tui_input::Input>,
    cursor_line: usize,
    field: Field,
    confirming: bool,
    error: String,
}
impl Default for Draft {
    fn default() -> Self {
        Self {
            subject: tui_input::Input::default(),
            body: vec![tui_input::Input::default()],
            cursor_line: 0,
            field: Field::Subject,
            confirming: false,
            error: String::new(),
        }
    }
}
impl Draft {
    fn message(&self) -> Option<String> {
        let subject = self.subject.value().trim();
        if subject.is_empty() || subject.contains(['\n', '\r']) {
            return None;
        }
        let body = self
            .body
            .iter()
            .map(|line| line.value())
            .collect::<Vec<_>>()
            .join("\n");
        let body = body.trim_matches('\n').trim_end();
        Some(if body.is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{body}")
        })
    }
    fn new_line(&mut self) {
        if self.body.len() >= 32 {
            self.error = "Body limited to 32 lines".into();
            return;
        }
        let current = &self.body[self.cursor_line];
        let index = current
            .value()
            .char_indices()
            .nth(current.cursor())
            .map(|(byte, _)| byte)
            .unwrap_or(current.value().len());
        let (left, right) = current.value().split_at(index);
        let left = left.to_owned();
        let right = right.to_owned();
        self.body[self.cursor_line] = tui_input::Input::new(left);
        self.cursor_line += 1;
        self.body.insert(
            self.cursor_line,
            tui_input::Input::new(right).with_cursor(0),
        );
    }
    fn backspace(&mut self) {
        if self.body[self.cursor_line].cursor() > 0 {
            self.body[self.cursor_line].handle_event(&Event::Key(KeyEvent::new(
                KeyCode::Backspace,
                KeyModifiers::NONE,
            )));
        } else if self.cursor_line > 0 {
            let trailing = self.body.remove(self.cursor_line).value().to_owned();
            self.cursor_line -= 1;
            let mut prev = self.body[self.cursor_line].value().to_owned();
            let col = prev.chars().count();
            prev.push_str(&trailing);
            self.body[self.cursor_line] = tui_input::Input::new(prev).with_cursor(col);
        }
    }
    fn paste(&mut self, raw: &str) {
        let text = raw.replace("\r\n", "\n").replace('\r', "\n");
        let parts = text.split('\n').collect::<Vec<_>>();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                if self.field == Field::Subject {
                    self.field = Field::Body;
                } else {
                    self.new_line();
                }
            }
            for ch in part.chars().filter(|ch| !ch.is_control()) {
                let key = Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                if self.field == Field::Subject {
                    self.subject.handle_event(&key);
                } else {
                    self.body[self.cursor_line].handle_event(&key);
                }
            }
        }
    }
    fn body_display(&self) -> String {
        let first = self.cursor_line.saturating_sub(4);
        self.body
            .iter()
            .enumerate()
            .skip(first)
            .take(5)
            .map(|(i, value)| {
                let focused = self.field == Field::Body && i == self.cursor_line;
                let mut text = value.value().to_owned();
                if focused {
                    let at = text
                        .char_indices()
                        .nth(value.cursor())
                        .map(|(pos, _)| pos)
                        .unwrap_or(text.len());
                    text.insert(at, '▏');
                }
                format!("{} {:>2}  {text}", if focused { '›' } else { ' ' }, i + 1)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[component]
pub(super) fn CommitEditor(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    let palette = hooks.use_palette();
    let draft = hooks.use_state(Draft::default);
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let mut d = draft.write();
                if d.confirming {
                    match key.code {
                        KeyCode::Esc => d.confirming = false,
                        KeyCode::Enter => {
                            if let Some(message) = d.message() {
                                let mut model = app.write();
                                if model
                                    .scm
                                    .git_status
                                    .as_ref()
                                    .is_some_and(|s| s.staged.is_empty())
                                {
                                    d.error = "No staged changes".into();
                                    d.confirming = false;
                                } else {
                                    model.request_commit(message);
                                    if model.scm.pending_write.is_some() {
                                        model.session.commit_open = false;
                                    } else {
                                        d.error = model.session.status.clone();
                                        d.confirming = false;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Esc => {
                            let mut model = app.write();
                            model.session.commit_open = false;
                            model.session.status = "commit cancelled".into();
                        }
                        KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if d.message().is_none() {
                                d.error = "Commit title cannot be empty".into();
                            } else if app
                                .read()
                                .scm
                                .git_status
                                .as_ref()
                                .is_some_and(|s| s.staged.is_empty())
                            {
                                d.error = "No staged changes".into();
                            } else {
                                d.confirming = true;
                                d.error.clear();
                            }
                        }
                        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if d.message().is_none() {
                                d.error = "Commit title cannot be empty".into();
                            } else if app
                                .read()
                                .scm
                                .git_status
                                .as_ref()
                                .is_some_and(|s| s.staged.is_empty())
                            {
                                d.error = "No staged changes".into();
                            } else {
                                d.confirming = true;
                                d.error.clear();
                            }
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            d.field = if d.field == Field::Subject {
                                Field::Body
                            } else {
                                Field::Subject
                            }
                        }
                        KeyCode::Enter if d.field == Field::Subject => d.field = Field::Body,
                        KeyCode::Enter => d.new_line(),
                        KeyCode::Up if d.field == Field::Body => {
                            d.cursor_line = d.cursor_line.saturating_sub(1)
                        }
                        KeyCode::Down if d.field == Field::Body => {
                            d.cursor_line = (d.cursor_line + 1).min(d.body.len() - 1)
                        }
                        KeyCode::Backspace if d.field == Field::Body => d.backspace(),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {}
                        _ => {
                            if d.field == Field::Subject {
                                d.subject.handle_event(&Event::Key(key));
                            } else {
                                let i = d.cursor_line;
                                d.body[i].handle_event(&Event::Key(key));
                            }
                        }
                    }
                }
            }
            Event::Paste(text) => {
                let mut d = draft.write();
                if !d.confirming {
                    d.paste(&text);
                }
            }
            _ => {}
        }
        EventResult::Consumed
    });
    let staged = app.read().scm.git_status.as_ref().map(|s| s.staged.clone());
    let count = staged.as_ref().map_or(0, Vec::len);
    let filenames = staged
        .as_ref()
        .map(|files| {
            files
                .iter()
                .take(4)
                .map(|entry| format!("  • {}", entry.path))
                .chain((files.len() > 4).then(|| format!("  … {} more files", files.len() - 4)))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| "  Git status loading · verify staged changes before confirming".into());
    let state = draft.read();
    let title = if state.confirming {
        " Confirm Commit "
    } else {
        " Commit Composer "
    };
    let subject_hint = format!(
        "SUBJECT  ({} characters; recommended ≤72)",
        state.subject.value().chars().count()
    );
    let body = if state.confirming {
        state.message().unwrap_or_default()
    } else {
        state.body_display()
    };
    let footer = if state.confirming {
        "Enter CONFIRM · Esc return to editing"
    } else {
        "Tab field · Enter newline · Ctrl+S / Ctrl+Enter review · Esc cancel"
    };
    let feedback = if state.error.is_empty() {
        app.read().session.status.clone()
    } else {
        state.error.clone()
    };
    element!(Border(top_title: Line::from(title), flex_direction: Direction::Vertical) {
        View(height: Constraint::Length(1)) { Text(text: format!("STAGED  ·  {} files  ·  commits ALL staged changes", count), style: Style::default().fg(palette.accent)) }
        View(height: Constraint::Length(5)) { Text(text: filenames, style: Style::default().fg(palette.fg_dim)) }
        View(height: Constraint::Length(1)) { Text(text: subject_hint, style: Style::default().fg(palette.fg)) }
        View(height: Constraint::Length(1)) { Input(input: state.subject.clone(), placeholder: "fix(ui): prevent preview overflow") }
        View(height: Constraint::Length(1)) { Text(text: "BODY  (optional; up to 32 lines)", style: Style::default().fg(palette.fg_dim)) }
        View(height: Constraint::Length(5)) { Text(text: body, style: Style::default().fg(palette.fg)) }
        View(height: Constraint::Length(1)) { Text(text: footer, style: Style::default().fg(palette.accent)) }
        View(height: Constraint::Length(1)) { Text(text: feedback, style: Style::default().fg(palette.warning)) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_message_subject_body_and_blank_lines() {
        let mut draft = Draft::default();
        assert_eq!(draft.message(), None);
        draft.paste("feat(editor): improve commit flow\nDetails of change\n\nMore notes");
        assert_eq!(draft.field, Field::Body);
        assert_eq!(
            draft.message().as_deref(),
            Some("feat(editor): improve commit flow\n\nDetails of change\n\nMore notes")
        );
        draft.new_line();
        draft.paste("Next note");
        assert!(draft.message().unwrap().ends_with("More notes\nNext note"));
    }

    #[test]
    fn body_backspace_joins_lines_at_cursor() {
        let mut draft = Draft::default();
        draft.field = Field::Body;
        draft.paste("one\ntwo");
        assert_eq!(draft.cursor_line, 1);
        draft.body[1] = tui_input::Input::new("two".into()).with_cursor(0);
        draft.backspace();
        assert_eq!(draft.body.len(), 1);
        assert_eq!(draft.body[0].value(), "onetwo");
        assert_eq!(draft.body[0].cursor(), 3);
    }

    #[test]
    fn body_display_marks_true_cursor_position() {
        let mut draft = Draft::default();
        draft.field = Field::Body;
        draft.body[0] = tui_input::Input::new("中文abc".into()).with_cursor(2);
        assert!(draft.body_display().contains("中文▏abc"));
    }
}

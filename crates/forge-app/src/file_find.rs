//! Incremental file-local find, independent of the preview's loaded pages.
//! The scanner reads bounded buffers and keeps match coordinates, not file text.
use super::*;
use std::io::{BufRead, BufReader};

const MAX_MATCHES: usize = 20_000;
const MAX_SCAN_BYTES: u64 = 128 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileMatch {
    pub line: usize,  // 1-based source line
    pub start: usize, // UTF-8 byte offsets within the line
    pub end: usize,
}

#[derive(Default)]
pub(super) struct FileFindState {
    pub open: bool,
    pub input_active: bool,
    pub path: PathBuf,
    pub query: String,
    pub cursor: usize, // UTF-8 byte index at a character boundary
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub matches: Vec<FileMatch>,
    pub active: Option<usize>,
    pub pending: Option<JobHandle<Result<FindScan, String>>>,
    pub capped: bool,
    pub error: Option<String>,
    pub jump_target: Option<usize>,
}

pub(super) struct FindScan {
    matches: Vec<FileMatch>,
    capped: bool,
}

fn compile_pattern(
    query: &str,
    match_case: bool,
    whole_word: bool,
    regex: bool,
) -> Result<regex::Regex, String> {
    let body = if regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let pattern = if whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!match_case)
        .build()
        .map_err(|e| format!("Invalid expression: {e}"))
}

fn scan_file(path: &Path, pattern: &regex::Regex) -> Result<FindScan, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(32 * 1024, file);
    let mut line = Vec::<u8>::new();
    let mut consumed = 0u64;
    let mut row = 0usize;
    let mut matches = Vec::new();
    let mut capped = false;
    loop {
        let mut seen_line = false;
        line.clear();
        loop {
            let buffer = reader.fill_buf().map_err(|e| e.to_string())?;
            if buffer.is_empty() {
                break;
            }
            seen_line = true;
            let count = buffer
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buffer.len(), |i| i + 1);
            let remaining = MAX_SCAN_BYTES.saturating_sub(consumed) as usize;
            let take = count.min(remaining);
            if line.len() < MAX_LINE_BYTES {
                let keep = take.min(MAX_LINE_BYTES - line.len());
                line.extend_from_slice(&buffer[..keep]);
            }
            let newline = buffer.get(count.saturating_sub(1)) == Some(&b'\n');
            reader.consume(take);
            consumed += take as u64;
            if take < count || consumed >= MAX_SCAN_BYTES {
                capped = true;
                break;
            }
            if newline {
                break;
            }
        }
        if !seen_line {
            break;
        }
        row += 1;
        if let Ok(text) = std::str::from_utf8(&line) {
            let text = text.trim_end_matches(['\r', '\n']);
            for found in pattern.find_iter(text) {
                if matches.len() >= MAX_MATCHES {
                    capped = true;
                    break;
                }
                matches.push(FileMatch {
                    line: row,
                    start: found.start(),
                    end: found.end(),
                });
            }
        }
        if capped {
            break;
        }
    }
    Ok(FindScan { matches, capped })
}

impl App {
    pub(super) fn open_file_find(&mut self) {
        if !matches!(self.preview_state.preview_mode, PreviewMode::File)
            || self.preview_state.preview_loading
        {
            self.session.status = "Open a text file first to find within it".into();
            return;
        }
        let Some(preview) = self.preview_state.preview.as_ref() else {
            return;
        };
        if !matches!(preview.content, PreviewContent::Text { .. }) {
            return;
        }
        let path = preview.path.clone();
        if self.preview_state.find.path != path {
            self.preview_state.find = FileFindState::default();
            self.preview_state.find.path = path;
        }
        if self.markdown_available() {
            self.preview_state.markdown_code = true;
        }
        self.preview_state.find.open = true;
        self.focus_find_input();
        self.preview_state.find.cursor = self.preview_state.find.query.len();
        if !self.preview_state.find.query.is_empty() && self.preview_state.find.matches.is_empty() {
            self.start_file_find();
        }
    }

    pub(super) fn close_file_find(&mut self) {
        self.preview_state.find.open = false;
        self.blur_find_input();
        self.preview_state.find.jump_target = None;
        self.preview_state.find.pending = None;
        self.services.jobs.cancel_key("fs:file-find");
    }

    pub(super) fn reset_file_find(&mut self) {
        self.close_file_find();
        self.preview_state.find = FileFindState::default();
    }

    pub(super) fn start_file_find(&mut self) {
        let find = &mut self.preview_state.find;
        find.pending = None;
        find.matches.clear();
        find.active = None;
        find.error = None;
        find.capped = false;
        find.jump_target = None;
        self.services.jobs.cancel_key("fs:file-find");
        if find.query.is_empty() || find.path.as_os_str().is_empty() {
            return;
        }
        let path = find.path.clone();
        let query = find.query.clone();
        let match_case = find.match_case;
        let whole_word = find.whole_word;
        let regex = find.regex;
        find.pending = Some(self.services.jobs.spawn(
            JobPriority::Normal,
            Some("fs:file-find".into()),
            async move {
                // The scheduler cancels superseded jobs while this debounce is
                // sleeping, avoiding one full file scan per typed character.
                tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                tokio::task::spawn_blocking(move || {
                    let pattern = compile_pattern(&query, match_case, whole_word, regex)?;
                    scan_file(&path, &pattern)
                })
                .await
                .map_err(|e| e.to_string())?
            },
        ));
    }

    pub(super) fn poll_file_find(&mut self) -> bool {
        let Some((_, result)) =
            pending_request::take_ready(&mut self.preview_state.find.pending, |job| job.try_join())
        else {
            return false;
        };
        if !self.preview_state.find.open {
            return true;
        }
        if self
            .preview_state
            .preview
            .as_ref()
            .is_none_or(|p| p.path != self.preview_state.find.path)
        {
            return true;
        }
        match result {
            Ok(Ok(scan)) => {
                let find = &mut self.preview_state.find;
                find.capped = scan.capped;
                find.matches = scan.matches;
                if !find.matches.is_empty() {
                    let here = self.preview_source_line();
                    let index = self
                        .preview_state
                        .find
                        .matches
                        .partition_point(|m| m.line < here);
                    self.preview_state.find.active =
                        Some(index.min(self.preview_state.find.matches.len() - 1));
                    self.navigate_file_find_to_active();
                }
            }
            Ok(Err(e)) => self.preview_state.find.error = Some(e),
            Err(JobError::Cancelled | JobError::Shutdown) => {}
        }
        true
    }

    pub(super) fn navigate_file_find(&mut self, next: bool) {
        let find = &mut self.preview_state.find;
        if find.matches.is_empty() {
            return;
        }
        let last = find.matches.len() - 1;
        find.active = Some(match find.active {
            None => {
                if next {
                    0
                } else {
                    last
                }
            }
            Some(i) if next => (i + 1) % find.matches.len(),
            Some(0) => last,
            Some(i) => i - 1,
        });
        self.navigate_file_find_to_active();
    }

    fn navigate_file_find_to_active(&mut self) {
        let find = &self.preview_state.find;
        let Some(index) = find.active else {
            return;
        };
        let Some(target) = find.matches.get(index) else {
            return;
        };
        let target_line = target.line;
        self.preview_state.find.jump_target = Some(target_line);
        self.advance_file_find_jump();
    }

    pub(super) fn advance_file_find_jump(&mut self) {
        if !self.preview_state.find.open {
            self.preview_state.find.jump_target = None;
            return;
        }
        let Some(line) = self.preview_state.find.jump_target else {
            return;
        };
        let Some(preview) = self.preview_state.preview.as_ref() else {
            return;
        };
        if preview.path != self.preview_state.find.path {
            self.preview_state.find.jump_target = None;
            return;
        }
        let PreviewContent::Text {
            lines, truncated, ..
        } = &preview.content
        else {
            return;
        };
        if lines.len() >= line {
            self.preview_state.find.jump_target = None;
            self.preview_state.navigation.pending_source_line = Some(line);
            self.preview_state.navigation.pending_find_byte = self
                .preview_state
                .find
                .active
                .and_then(|i| self.preview_state.find.matches.get(i))
                .map(|hit| hit.start);
            self.preview_state.presentation.preview_scroll = self
                .preview_state
                .presentation
                .text_rows
                .source_to_row(line);
        } else if *truncated {
            self.maybe_request_preview_page();
        } else {
            self.preview_state.find.jump_target = None;
        }
    }

    pub(super) fn handle_file_find_key(&mut self, key: KeyEvent) {
        if key
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('f' | 'F'))
        {
            self.close_file_find();
            self.open_tools(tools::PanelKind::Content);
            return;
        }
        if matches!(key.code, KeyCode::Esc) {
            self.close_file_find();
            return;
        }
        if matches!(key.code, KeyCode::Tab) {
            self.blur_find_input();
            return;
        }
        if matches!(key.code, KeyCode::F(3)) {
            self.navigate_file_find(!key.modifiers.contains(KeyModifiers::SHIFT));
            return;
        }
        if matches!(key.code, KeyCode::Enter) {
            self.navigate_file_find(!key.modifiers.contains(KeyModifiers::SHIFT));
            return;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('c' | 'C') => {
                    self.preview_state.find.match_case = !self.preview_state.find.match_case
                }
                KeyCode::Char('w' | 'W') => {
                    self.preview_state.find.whole_word = !self.preview_state.find.whole_word
                }
                KeyCode::Char('r' | 'R') => {
                    self.preview_state.find.regex = !self.preview_state.find.regex
                }
                _ => return,
            }
            self.start_file_find();
            return;
        }
        let find = &mut self.preview_state.find;
        let changed = match key.code {
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                find.query.clear();
                find.cursor = 0;
                true
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                if find.query.len() > 180 {
                    return;
                }
                find.query.insert(find.cursor, c);
                find.cursor += c.len_utf8();
                true
            }
            KeyCode::Backspace if find.cursor > 0 => {
                let prev = find.query[..find.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                find.query.drain(prev..find.cursor);
                find.cursor = prev;
                true
            }
            KeyCode::Delete if find.cursor < find.query.len() => {
                let next =
                    find.cursor + find.query[find.cursor..].chars().next().unwrap().len_utf8();
                find.query.drain(find.cursor..next);
                true
            }
            KeyCode::Left => {
                find.cursor = find.query[..find.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                false
            }
            KeyCode::Right => {
                if find.cursor < find.query.len() {
                    find.cursor += find.query[find.cursor..].chars().next().unwrap().len_utf8();
                }
                false
            }
            KeyCode::Home => {
                find.cursor = 0;
                false
            }
            KeyCode::End => {
                find.cursor = find.query.len();
                false
            }
            _ => false,
        };
        if changed {
            self.start_file_find();
        }
    }

    pub(super) fn handle_file_find_paste(&mut self, text: &str) {
        if !self.preview_state.find.open || !self.preview_state.find.input_active {
            return;
        }
        let find = &mut self.preview_state.find;
        let mut changed = false;
        for c in text.chars().filter(|c| !c.is_control()) {
            if find.query.len() > 180 {
                break;
            }
            find.query.insert(find.cursor, c);
            find.cursor += c.len_utf8();
            changed = true;
        }
        if changed {
            self.start_file_find();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn streaming_scan_reads_past_preview_limit_and_retains_utf8_byte_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let content = (1..=2800)
            .map(|line| {
                if line == 1753 {
                    format!("{line} 发生错误: ERROR code=42 error\n")
                } else {
                    format!("{line} request completed successfully\n")
                }
            })
            .collect::<String>();
        std::fs::write(&path, content).unwrap();

        let insensitive = compile_pattern("error", false, false, false).unwrap();
        let found = scan_file(&path, &insensitive).unwrap();
        assert_eq!(found.matches.len(), 2);
        assert!(found.matches.iter().all(|m| m.line == 1753));
        assert_eq!(found.matches[0].start, "1753 发生错误: ".len());
        assert_eq!(found.matches[0].end - found.matches[0].start, 5);

        let sensitive = compile_pattern("ERROR", true, false, false).unwrap();
        let found = scan_file(&path, &sensitive).unwrap();
        assert_eq!(found.matches.len(), 1);

        let words = compile_pattern("error", false, true, false).unwrap();
        let found = scan_file(&path, &words).unwrap();
        assert_eq!(found.matches.len(), 2);

        let regex = compile_pattern(r"code=\d{2}", true, false, true).unwrap();
        assert_eq!(scan_file(&path, &regex).unwrap().matches.len(), 1);
        assert!(compile_pattern("[", true, false, true).is_err());
    }

    async fn ready_file(app: &mut App, path: &Path) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                app.poll_pending_list();
                if app.files.pending_lists.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == path)
            .unwrap();
        app.request_preview_for_selection();
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                app.poll_pending_preview();
                if app.preview_state.pending_preview.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn find_jumps_into_not_yet_loaded_page_and_preserves_wrap_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("long.log");
        let content = (1..=2800)
            .map(|n| {
                if n == 1744 {
                    format!(
                        "{n} {}needle final target — 中文\n",
                        "long prefix ".repeat(12)
                    )
                } else {
                    format!("{n} some verbose test line\n")
                }
            })
            .collect::<String>();
        std::fs::write(&file, content).unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        ready_file(&mut app, &file).await;
        app.layout_state.geometry.preview_inner.height = 20;
        app.layout_state.geometry.preview_inner.width = 40;
        app.open_file_find();
        assert!(app.preview_state.find.open);
        app.preview_state.find.query = "needle".into();
        app.preview_state.find.cursor = 6;
        app.start_file_find();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                app.poll_file_find();
                app.poll_pending_page();
                if app.preview_state.find.pending.is_none()
                    && app.preview_state.find.jump_target.is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(app.preview_state.find.matches.len(), 1);
        assert_eq!(app.preview_state.find.active, Some(0));
        assert_eq!(app.preview_state.find.matches[0].line, 1744);
        assert_eq!(app.preview_state.navigation.pending_source_line, Some(1744));
        let byte = app.preview_state.find.matches[0].start;
        assert_eq!(app.preview_state.navigation.pending_find_byte, Some(byte));
        assert!(matches!(
            &app.preview_state.preview.as_ref().unwrap().content,
            PreviewContent::Text { lines, .. } if lines.len() >= 1744
        ));
        assert!(app.preview_state.find.jump_target.is_none());

        app.navigate_file_find(true);
        assert_eq!(app.preview_state.find.active, Some(0));
        app.handle_file_find_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preview_state.find.open);
        assert_eq!(app.preview_state.navigation.pending_source_line, Some(1744));
    }

    #[tokio::test]
    async fn closing_find_cancels_search_and_does_not_change_scroll_later() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("source.log");
        std::fs::write(
            &file,
            (0..1000)
                .map(|n| format!("{n} search text\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        ready_file(&mut app, &file).await;
        app.session.focus = PaneFocus::Preview;
        app.preview_state.presentation.preview_scroll = 10;
        app.open_file_find();
        app.preview_state.find.query = "search".into();
        app.start_file_find();
        assert!(app.preview_state.find.pending.is_some());
        app.handle_file_find_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preview_state.find.open);
        assert!(app.preview_state.find.pending.is_none());
        assert!(app.preview_state.find.jump_target.is_none());
        tokio::time::sleep(Duration::from_millis(200)).await;
        app.poll_file_find();
        assert_eq!(app.preview_state.presentation.preview_scroll, 10);
        assert!(app.preview_state.find.matches.is_empty());

        // Reopen keeps the previous query but starts a fresh scan.
        app.open_file_find();
        assert!(app.preview_state.find.open);
        assert!(app.preview_state.find.pending.is_some());
    }

    #[tokio::test]
    async fn find_shortcuts_leave_workspace_search_and_preview_scroll_accessible() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.log");
        std::fs::write(&file, "hello\nHELLO\n").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        ready_file(&mut app, &file).await;
        app.session.focus = PaneFocus::Preview;
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(app.preview_state.find.open && app.preview_state.find.input_active);
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(app.preview_state.find.query, "hi");
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.preview_state.find.query, "h");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(!app.preview_state.find.input_active);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preview_state.find.open);
        app.handle_key(KeyEvent::new(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert!(app
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, tools::PanelKind::Content)));
    }

    #[tokio::test]
    async fn file_find_paste_inserts_sanitized_query_and_runs_single_scan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("paste.log");
        std::fs::write(&file, "alpha\nbeta-target\ngamma\n").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        ready_file(&mut app, &file).await;
        app.session.focus = PaneFocus::Preview;
        app.open_file_find();
        assert!(app.preview_state.find.open && app.preview_state.find.input_active);

        app.handle_file_find_paste("beta-target\r\n");
        assert_eq!(app.preview_state.find.query, "beta-target");
        tokio::time::timeout(Duration::from_secs(3), async {
            while app.preview_state.find.pending.is_some() {
                app.poll_file_find();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(app.preview_state.find.matches.len(), 1);
        assert_eq!(app.preview_state.find.matches[0].line, 2);
    }
}

//! Lazy, single-session terminal. Only terminal output is parsed on the reader
//! thread; the UI never blocks on PTY reads and never stores unbounded output.
use std::{
    io::{Read, Write},
    num::NonZeroU16,
    path::Path,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex},
};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use ratatui::{layout::Rect, style::{Color, Modifier, Style}, Frame};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

static NEXT_TERMINAL_ID: AtomicU64 = AtomicU64::new(1);

pub struct EmbeddedTerminal {
    pub id: u64,
    pub visible: bool,
    pub focused: bool,
    pub maximized: bool,
    pub revision: Arc<AtomicU64>,
    pub drawn_revision: u64,
    pub ended: Arc<AtomicBool>,
    parser: Arc<Mutex<vt100::Parser>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    size: (u16, u16),
    pub area: Rect,
    selection: Option<((u16,u16),(u16,u16))>,
    /// Human readable tab title.
    pub name: String,
}

impl EmbeddedTerminal {
    pub fn start(cwd: &Path, width: u16, height: u16, notify: Arc<tokio::sync::Notify>) -> anyhow::Result<Self> {
        let cols = width.max(1);
        let rows = height.max(1);
        let pty = native_pty_system().openpty(PtySize {
            rows, cols, pixel_width: 0, pixel_height: 0,
        })?;
        // portable-pty chooses an OS-appropriate shell when SHELL is unset.
        let mut cmd = match std::env::var("SHELL") {
            Ok(shell) if !shell.is_empty() => CommandBuilder::new(shell),
            _ => CommandBuilder::new_default_prog(),
        };
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd)?;
        drop(pty.slave);
        let writer = pty.master.take_writer()?;
        let mut reader = pty.master.try_clone_reader()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(), 2000,
        )));
        let revision = Arc::new(AtomicU64::new(0));
        let read_parser = parser.clone();
        let read_revision = revision.clone();
        let read_notify = notify.clone();
        let ended = Arc::new(AtomicBool::new(false));
        let read_ended = ended.clone();
        std::thread::Builder::new().name("forge-pty-read".into()).spawn(move || {
            let mut bytes = [0u8; 8192];
            loop {
                let n = match reader.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                if let Ok(mut parser) = read_parser.lock() {
                    parser.process(&bytes[..n]);
                    read_revision.fetch_add(1, Ordering::Release);
                    // Wake the UI on output instead of waiting for a polling tick.
                    read_notify.notify_one();
                } else { break; }
            }
            read_ended.store(true, Ordering::Release);
            read_notify.notify_one();
        })?;
        Ok(Self {
            id: NEXT_TERMINAL_ID.fetch_add(1, Ordering::Relaxed),
            visible: true, focused: true, maximized: false, revision, drawn_revision: 0, ended,
            parser, master: Mutex::new(pty.master), writer: Mutex::new(writer), child, size: (cols, rows),
            area: Rect::default(), selection: None,
            name: cwd.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Shell".into()),
        })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_size = (cols.max(1), rows.max(1));
        if self.size == new_size { return; }
        if self.master.lock().expect("pty master").resize(PtySize {
            cols: new_size.0, rows: new_size.1,
            pixel_width: 0, pixel_height: 0
        }).is_ok() {
            if let Ok(mut parser) = self.parser.lock() {
                parser.set_size(NonZeroU16::new(new_size.1).unwrap(),
                                NonZeroU16::new(new_size.0).unwrap());
            }
            self.size = new_size;
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.ended.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe, "shell has exited",
            ));
        }
        let mut writer = self.writer.lock().map_err(|_| {
            std::io::Error::other("terminal input writer lock poisoned")
        })?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    pub fn paste(&mut self, text: &str) -> std::io::Result<()> {
        // Respect mode negotiated by the foreground program. In bracketed
        // paste, lines are delivered for editing rather than as Enter keys.
        let bracketed = self.parser.lock().map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?
            .screen().bracketed_paste();
        if bracketed {
            let mut bytes = Vec::with_capacity(text.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.write_input(&bytes)
        } else {
            self.write_input(text.as_bytes())
        }
    }

    pub fn send(&mut self, key: KeyEvent) -> std::io::Result<()> {
        let application = self.parser.lock().map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?
            .screen().application_cursor();
        let Some(data) = encode_key(key, application) else { return Ok(()); };
        self.scroll_to_bottom(); // any input returns to the live viewport
        self.selection = None;
        self.write_input(&data)
    }

    pub fn scroll(&mut self, amount: isize) {
        if let Ok(mut parser) = self.parser.lock() {
            let screen = parser.screen_mut();
            let current = screen.scrollback();
            let next = if amount < 0 { current.saturating_add(amount.unsigned_abs()) }
                       else { current.saturating_sub(amount as usize) };
            screen.set_scrollback(next);
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    pub fn scroll_to_bottom(&mut self) { self.scroll(isize::MAX); }

    pub fn scrollback_position(&self) -> usize {
        self.parser.lock().map(|parser| parser.screen().scrollback()).unwrap_or(0)
    }

    pub fn on_alternate_screen(&self) -> bool {
        self.parser.lock().map(|parser| parser.screen().alternate_screen()).unwrap_or(false)
    }

    /// Mouse events within the terminal viewport. Shift forces host selection.
    pub fn mouse(&mut self, event: MouseEvent) -> std::io::Result<Option<String>> {
        if !self.area.contains((event.column,event.row).into()) { return Ok(None); }
        let x = event.column.saturating_sub(self.area.x);
        let y = event.row.saturating_sub(self.area.y);
        let (mode, encoding, selecting) = {
            let parser = self.parser.lock().map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?;
            let screen = parser.screen();
            (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding(),
                event.modifiers.contains(KeyModifiers::SHIFT) || screen.mouse_protocol_mode() == MouseProtocolMode::None)
        };
        if selecting {
            match event.kind {
                MouseEventKind::ScrollUp => self.scroll(-3),
                MouseEventKind::ScrollDown => self.scroll(3),
                MouseEventKind::Down(MouseButton::Left) => {
                    self.selection = Some(((x,y),(x,y))); self.focused = true;
                },
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some((start,_)) = self.selection { self.selection = Some((start,(x,y))); }
                },
                MouseEventKind::Up(MouseButton::Left) => {
                    if let Some((start,_)) = self.selection {
                        self.selection = Some((start,(x,y)));
                        if start != (x,y) { return Ok(self.selected_text()); }
                    }
                },
                _ => {},
            }
            self.revision.fetch_add(1,Ordering::Release);
            return Ok(None);
        }
        if let Some(bytes) = encode_mouse(event, self.area, mode, encoding) {
            self.write_input(&bytes)?;
        }
        Ok(None)
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start,end) = self.selection?;
        let parser = self.parser.lock().ok()?;
        let screen = parser.screen();
        let width = self.area.width;
        let (a,b) = if (start.1,start.0) <= (end.1,end.0) {(start,end)} else {(end,start)};
        let mut lines = Vec::new();
        for y in a.1..=b.1 {
            let left = if y==a.1 {a.0} else {0};
            let right = if y==b.1 {b.0} else {width.saturating_sub(1)};
            let mut line = String::new();
            for x in left..=right {
                if let Some(cell) = screen.cell(y,x) { line.push_str(cell.contents()); }
            }
            lines.push(line.trim_end().to_string());
        }
        Some(lines.join("\n"))
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, bg: Color, fg: Color) {
        if area.width == 0 || area.height == 0 { return; }
        self.area = area;
        self.resize(area.width, area.height);
        // Clone only visible 32-byte cells while holding the parser mutex.
        // Ratatui writes and style calculations happen *after* unlocking, so
        // large frames cannot starve the PTY reader or delay input echo.
        let (cells, cursor, draw_cursor) = {
            let Ok(parser) = self.parser.lock() else { return; };
            let screen = parser.screen();
            let cells = (0..area.height).flat_map(|y|
                (0..area.width).map(move |x| screen.cell(y,x).cloned())
            ).collect::<Vec<_>>();
            // Full-screen TUIs (including coding agents) often hide the cursor
            // while animating status rows. Drawing our synthetic reversed cell
            // over their UI makes those rows visibly flicker.
            let draw_cursor = self.focused && !screen.hide_cursor() && screen.scrollback() == 0;
            (cells, screen.cursor_position(), draw_cursor)
        };
        let selection = self.selection.map(|(start,end)| {
            if (start.1,start.0) <= (end.1,end.0) {(start,end)} else {(end,start)}
        });
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = cells[(y as usize * area.width as usize) + x as usize].as_ref() else { continue; };
                let mut style = Style::default()
                    .fg(to_color(cell.fgcolor(), fg))
                    .bg(to_color(cell.bgcolor(), bg))
                    .add_modifier(if cell.bold() { Modifier::BOLD } else { Modifier::empty() });
                if draw_cursor && cursor == (y, x) {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if selection.is_some_and(|(start,end)|
                    (y,x) >= (start.1,start.0) && (y,x) <= (end.1,end.0))
                {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let symbol = cell.contents();
                let symbol = if symbol.is_empty() { " " } else { symbol };
                frame.buffer_mut()[(area.x + x, area.y + y)]
                    .set_symbol(symbol).set_style(style);
            }
        }
        self.drawn_revision = self.revision.load(Ordering::Acquire);
    }
}

/// Stable across processes: Rust's RandomState is deliberately not used.
fn modifier_param(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2*u8::from(mods.contains(KeyModifiers::ALT))
        + 4*u8::from(mods.contains(KeyModifiers::CONTROL))
}

fn encode_key(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let m = modifier_param(key.modifiers);
    let bytes = match key.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Tab if m == 1 => b"\t".to_vec(),
        KeyCode::BackTab | KeyCode::Tab if m == 2 => b"\x1b[Z".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up | KeyCode::Down | KeyCode::Right | KeyCode::Left => {
            let c = match key.code {
                KeyCode::Up => 'A', KeyCode::Down => 'B',
                KeyCode::Right => 'C', _ => 'D',
            };
            if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() }
            else if application_cursor { format!("\x1bO{c}").into_bytes() }
            else { format!("\x1b[{c}").into_bytes() }
        },
        KeyCode::Home | KeyCode::End => {
            let c = if key.code == KeyCode::Home { 'H' } else { 'F' };
            if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() }
            else if application_cursor { format!("\x1bO{c}").into_bytes() }
            else { format!("\x1b[{c}").into_bytes() }
        },
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown => {
            let n = match key.code {
                KeyCode::Insert => 2, KeyCode::Delete => 3,
                KeyCode::PageUp => 5, _ => 6,
            };
            if m > 1 { format!("\x1b[{n};{m}~").into_bytes() }
            else { format!("\x1b[{n}~").into_bytes() }
        },
        KeyCode::F(f @ 1..=4) => {
            let c = ['P','Q','R','S'][(f-1) as usize];
            if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() }
            else { format!("\x1bO{c}").into_bytes() }
        },
        KeyCode::F(f @ 5..=12) => {
            let n = [15,17,18,19,20,21,23,24][(f-5) as usize];
            if m > 1 { format!("\x1b[{n};{m}~").into_bytes() }
            else { format!("\x1b[{n}~").into_bytes() }
        },
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let code = match c.to_ascii_lowercase() {
                'a'..='z' => (c.to_ascii_lowercase() as u8) - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 27, '\\' | '4' => 28,
                ']' | '5' => 29, '^' | '6' => 30, '_' | '7' => 31,
                '?' | '8' => 127,
                _ => return None,
            };
            let mut data = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) { data.push(0x1b); }
            data.push(code); data
        },
        KeyCode::Char(c) => {
            let mut data = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) { data.push(0x1b); }
            data.extend(c.to_string().as_bytes()); data
        },
        _ => return None,
    };
    Some(bytes)
}

fn encode_mouse(event: MouseEvent, area: Rect, mode: MouseProtocolMode, encoding: MouseProtocolEncoding) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None { return None; }
    let release = matches!(event.kind, MouseEventKind::Up(_));
    let motion = matches!(event.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved);
    if mode == MouseProtocolMode::Press && (release || motion) { return None; }
    if mode == MouseProtocolMode::PressRelease && motion { return None; }
    if mode == MouseProtocolMode::ButtonMotion && matches!(event.kind, MouseEventKind::Moved) { return None; }
    let button = match event.kind {
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => 0u16,
        MouseEventKind::Down(MouseButton::Middle) | MouseEventKind::Drag(MouseButton::Middle) => 1,
        MouseEventKind::Down(MouseButton::Right) | MouseEventKind::Drag(MouseButton::Right) => 2,
        MouseEventKind::Up(_) | MouseEventKind::Moved => 3,
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
    };
    let mut b = button;
    if motion { b |= 32; }
    if event.modifiers.contains(KeyModifiers::SHIFT) { b |= 4; }
    if event.modifiers.contains(KeyModifiers::ALT) { b |= 8; }
    if event.modifiers.contains(KeyModifiers::CONTROL) { b |= 16; }
    let x = event.column.saturating_sub(area.x) as u32 + 1;
    let y = event.row.saturating_sub(area.y) as u32 + 1;
    if encoding == MouseProtocolEncoding::Sgr {
        return Some(format!("\x1b[<{b};{x};{y}{}", if release {'m'} else {'M'}).into_bytes());
    }
    // X10/UTF-8 coordinates encode each value with an offset of 32.
    if encoding == MouseProtocolEncoding::Default && (x > 223 || y > 223) { return None; }
    let mut out = b"\x1b[M".to_vec();
    for v in [u32::from(b)+32, x+32, y+32] {
        if encoding == MouseProtocolEncoding::Utf8 {
            let ch = char::from_u32(v)?;
            let mut buf = [0u8;4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        } else { out.push(u8::try_from(v).ok()?); }
    }
    Some(out)
}

fn to_color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r,g,b) => Color::Rgb(r,g,b),
        vt100::Color::Idx(index) => Color::Indexed(index),
    }
}

impl Drop for EmbeddedTerminal {
    fn drop(&mut self) {
        // Never leave an orphan shell when Forge exits.
        let _ = self.child.kill();
        // Reap the child even if it has already exited, avoiding zombies when
        // the user opens and closes several embedded terminals.
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn xterm_key_sequences_cover_application_mode_and_modifiers() {
        let key = |code, mods| KeyEvent::new(code, mods);
        assert_eq!(encode_key(key(KeyCode::Up, KeyModifiers::NONE),false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(key(KeyCode::Up, KeyModifiers::NONE),true).unwrap(), b"\x1bOA");
        assert_eq!(encode_key(key(KeyCode::Left, KeyModifiers::CONTROL),false).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode_key(key(KeyCode::Right, KeyModifiers::ALT),false).unwrap(), b"\x1b[1;3C");
        assert_eq!(encode_key(key(KeyCode::F(5), KeyModifiers::NONE),false).unwrap(), b"\x1b[15~");
        assert_eq!(encode_key(key(KeyCode::F(12), KeyModifiers::SHIFT),false).unwrap(), b"\x1b[24;2~");
        assert_eq!(encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL),false).unwrap(), b"\x03");
        assert_eq!(encode_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL),false).unwrap(), b"\x00");
    }

    #[test]
    fn negotiated_bracketed_paste_is_single_write_with_delimiters() {
        #[derive(Clone)]
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let cwd = std::env::current_dir().unwrap();
        let mut term = EmbeddedTerminal::start(&cwd,80,12,Arc::new(tokio::sync::Notify::new())).unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        term.writer = Mutex::new(Box::new(Capture(output.clone())));
        term.parser.lock().unwrap().process(b"\x1b[?2004h");
        term.paste("echo alpha\necho beta").unwrap();
        assert_eq!(&*output.lock().unwrap(), b"\x1b[200~echo alpha\necho beta\x1b[201~");
        output.lock().unwrap().clear();
        term.parser.lock().unwrap().process(b"\x1b[?2004l");
        term.paste("hello").unwrap();
        assert_eq!(&*output.lock().unwrap(), b"hello");
    }

    #[test]
    fn sgr_mouse_encoding_respects_child_mode() {
        let area = Rect::new(5,6,80,20);
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left), column: 8, row: 9,
            modifiers: KeyModifiers::NONE,
        };
        assert!(encode_mouse(event,area,MouseProtocolMode::None,MouseProtocolEncoding::Sgr).is_none());
        assert_eq!(encode_mouse(event,area,MouseProtocolMode::Press,MouseProtocolEncoding::Sgr).unwrap(),
            b"\x1b[<0;4;4M");
        let release = MouseEvent {kind:MouseEventKind::Up(MouseButton::Left),..event};
        assert_eq!(encode_mouse(release,area,MouseProtocolMode::PressRelease,MouseProtocolEncoding::Sgr).unwrap(),
            b"\x1b[<3;4;4m");
        assert!(encode_mouse(release,area,MouseProtocolMode::Press,MouseProtocolEncoding::Sgr).is_none());
    }

    #[test]
    fn scrollback_and_selection_keep_visible_text() {
        let cwd = std::env::current_dir().unwrap();
        let mut term = EmbeddedTerminal::start(&cwd,40,8,Arc::new(tokio::sync::Notify::new())).unwrap();
        term.area = Rect::new(1,1,40,8);
        term.parser.lock().unwrap().process(b"ONE\r\nTWO\r\nTHREE");
        term.selection = Some(((0,0),(2,1)));
        assert_eq!(term.selected_text().unwrap(), "ONE\nTWO");
        {
            let mut parser = term.parser.lock().unwrap();
            for i in 0..30 { parser.process(format!("{i:02}\r\n").as_bytes()); }
        }
        term.scroll(-4);
        assert!(term.parser.lock().unwrap().screen().scrollback() > 0);
        term.scroll_to_bottom();
        assert_eq!(term.parser.lock().unwrap().screen().scrollback(),0);
    }

    #[test]
    fn terminal_renderer_respects_child_cursor_visibility() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded = EmbeddedTerminal::start(
            &cwd, 40, 8, Arc::new(tokio::sync::Notify::new())
        ).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();

        // The foreground TUI owns its cursor visibility through DECSET 25.
        // Even when Forge's terminal pane has focus, a hidden child cursor
        // must not overwrite the status character with inverse video.
        {
            let mut parser = embedded.parser.lock().unwrap();
            parser.process(b"\x1b[?25l\x1b[1;1HSTATUS");
            assert!(parser.screen().hide_cursor());
        }
        embedded.focused = true;
        terminal.draw(|frame| {
            embedded.render(frame, Rect::new(0, 0, 40, 8), Color::Black, Color::White);
        }).unwrap();
        let first = &terminal.backend().buffer()[(0, 0)];
        assert_eq!(first.symbol(), "S");
        assert!(!first.modifier.contains(Modifier::REVERSED));

        // Once the child explicitly shows its cursor again, Forge may draw it.
        {
            let mut parser = embedded.parser.lock().unwrap();
            parser.process(b"\x1b[?25h\x1b[1;1H");
            assert!(!parser.screen().hide_cursor());
        }
        terminal.draw(|frame| {
            embedded.render(frame, Rect::new(0, 0, 40, 8), Color::Black, Color::White);
        }).unwrap();
        let first = &terminal.backend().buffer()[(0, 0)];
        assert!(first.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn terminal_renderer_waits_for_inflight_parser_instead_of_blank_frame() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded = EmbeddedTerminal::start(
            &cwd, 80, 12, Arc::new(tokio::sync::Notify::new())
        ).unwrap();
        {
            embedded.parser.lock().unwrap().process(b"VISIBLE_AFTER_LOCK");
        }
        let parser = embedded.parser.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let lock_holder = std::thread::spawn(move || {
            let _guard = parser.lock().unwrap();
            ready.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(60));
        });
        waiting.recv().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| {
            embedded.render(frame, Rect::new(0, 0, 80, 12), Color::Black, Color::White);
        }).unwrap();
        lock_holder.join().unwrap();
        let content = terminal.backend().buffer().content()
            .iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(content.contains("VISIBLE_AFTER_LOCK"), "{content}");
    }

    #[test]
    fn burst_output_is_bounded_and_shell_exits() {
        let cwd = std::env::current_dir().unwrap();
        let mut terminal = EmbeddedTerminal::start(
            &cwd, 100, 14, Arc::new(tokio::sync::Notify::new())
        ).unwrap();
        // Output exceeds vt100's 2,000-line history many times over.
        terminal.paste("python3 -c 'for i in range(20000): print(i)'\nexit\n").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !terminal.ended.load(Ordering::Acquire) &&
            std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        assert!(terminal.ended.load(Ordering::Acquire), "output flood prevented shell exit");
        let parser = terminal.parser.lock().unwrap();
        let screen = parser.screen();
        let visible = screen.rows_formatted(0, screen.size().1.get()).collect::<String>();
        assert!(visible.contains("19999") || visible.contains("exit"), "tail not rendered");
    }

    #[test]
    fn exited_shell_notifies_and_rejects_more_input() {
        let cwd = std::env::current_dir().unwrap();
        let notify = Arc::new(tokio::sync::Notify::new());
        let mut terminal = EmbeddedTerminal::start(&cwd, 80, 12, notify).unwrap();
        terminal.paste("exit\n").unwrap();
        let ended = (0..100).any(|_| {
            if terminal.ended.load(Ordering::Acquire) { return true; }
            std::thread::sleep(std::time::Duration::from_millis(20));
            terminal.ended.load(Ordering::Acquire)
        });
        assert!(ended, "PTY reader never signalled shell exit");
        assert_eq!(
            terminal.paste("should-not-write").unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe,
        );
    }

    #[test]
    fn shell_executes_commands_and_parser_tracks_output() {
        let cwd = std::env::current_dir().unwrap();
        let mut terminal = EmbeddedTerminal::start(&cwd, 80, 12, Arc::new(tokio::sync::Notify::new())).unwrap();
        for ch in "printf FORGE_PTY_SMOKE_TEST".chars() {
            terminal.send(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)).unwrap();
        }
        terminal.send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        let found = (0..80).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(25));
            terminal.parser.lock().unwrap().screen().contents().contains("FORGE_PTY_SMOKE_TEST")
        });
        assert!(found, "shell output never arrived at the VT parser");
        terminal.resize(60, 9);
        assert_eq!(terminal.size, (60, 9));
    }
}

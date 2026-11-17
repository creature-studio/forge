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
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct EmbeddedTerminal {
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
            visible: true, focused: true, maximized: false, revision, drawn_revision: 0, ended,
            parser, master: Mutex::new(pty.master), writer: Mutex::new(writer), child, size: (cols, rows),
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
        // Treat paste as literal text, never as Forge commands or keybindings.
        self.write_input(text.as_bytes())
    }

    pub fn send(&mut self, key: KeyEvent) -> std::io::Result<()> {
        let data = match key.code {
            KeyCode::Enter => b"\r".to_vec(),
            KeyCode::Backspace => b"\x7f".to_vec(),
            KeyCode::Tab => b"\t".to_vec(),
            KeyCode::BackTab => b"\x1b[Z".to_vec(),
            KeyCode::Esc => b"\x1b".to_vec(),
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Home => b"\x1b[H".to_vec(),
            KeyCode::End => b"\x1b[F".to_vec(),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic() =>
                vec![(c.to_ascii_lowercase() as u8) - b'a' + 1],
            KeyCode::Char(c) => {
                let mut data = Vec::new();
                if key.modifiers.contains(KeyModifiers::ALT) { data.push(0x1b); }
                data.extend(c.to_string().as_bytes());
                data
            },
            _ => return Ok(()),
        };
        self.write_input(&data)
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, bg: Color, fg: Color) {
        if area.width == 0 || area.height == 0 { return; }
        self.resize(area.width, area.height);
        // Never leave the pane blank because a PTY chunk arrived at the
        // same time as a frame. A try_lock miss used to skip *all* painting,
        // clearing previously rendered content in Ratatui's fresh buffer.
        // The reader holds this lock only while parsing one 8 KiB chunk.
        let Ok(parser) = self.parser.lock() else { return; };
        let screen = parser.screen();
        let cursor = screen.cursor_position();
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = screen.cell(y, x) else { continue; };
                let mut style = Style::default()
                    .fg(to_color(cell.fgcolor(), fg))
                    .bg(to_color(cell.bgcolor(), bg))
                    .add_modifier(if cell.bold() { Modifier::BOLD } else { Modifier::empty() });
                if self.focused && cursor == (y, x) {
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

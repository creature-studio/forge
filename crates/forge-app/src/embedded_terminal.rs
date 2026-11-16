//! Lazy, single-session terminal. Only terminal output is parsed on the reader
//! thread; the UI never blocks on PTY reads and never stores unbounded output.
use std::{
    io::{Read, Write},
    num::NonZeroU16,
    path::Path,
    sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex},
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
    parser: Arc<Mutex<vt100::Parser>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    size: (u16, u16),
}

impl EmbeddedTerminal {
    pub fn start(cwd: &Path, width: u16, height: u16) -> anyhow::Result<Self> {
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
        std::thread::Builder::new().name("forge-pty-read".into()).spawn(move || {
            let mut bytes = [0u8; 8192];
            while let Ok(n) = reader.read(&mut bytes) {
                if n == 0 { break; }
                if let Ok(mut parser) = read_parser.lock() {
                    parser.process(&bytes[..n]);
                    read_revision.fetch_add(1, Ordering::Release);
                } else { break; }
            }
        })?;
        Ok(Self {
            visible: true, focused: true, maximized: false, revision, drawn_revision: 0,
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

    pub fn paste(&mut self, text: &str) {
        // Treat paste as literal text, never as Forge commands or keybindings.
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(text.as_bytes());
            let _ = writer.flush();
        }
    }

    pub fn send(&mut self, key: KeyEvent) {
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
            _ => return,
        };
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(&data);
            let _ = writer.flush();
        }
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, bg: Color, fg: Color) {
        if area.width == 0 || area.height == 0 { return; }
        self.resize(area.width, area.height);
        let Ok(parser) = self.parser.try_lock() else { return; };
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
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn shell_executes_commands_and_parser_tracks_output() {
        let cwd = std::env::current_dir().unwrap();
        let mut terminal = EmbeddedTerminal::start(&cwd, 80, 12).unwrap();
        for ch in "printf FORGE_PTY_SMOKE_TEST".chars() {
            terminal.send(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        terminal.send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let found = (0..80).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(25));
            terminal.parser.lock().unwrap().screen().contents().contains("FORGE_PTY_SMOKE_TEST")
        });
        assert!(found, "shell output never arrived at the VT parser");
        terminal.resize(60, 9);
        assert_eq!(terminal.size, (60, 9));
    }
}

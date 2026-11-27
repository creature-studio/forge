//! Lazy, single-session terminal. Only terminal output is parsed on the reader
//! thread; the UI never blocks on PTY reads and never stores unbounded output.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};
use std::{
    io::Read,
    num::NonZeroU16,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

mod input;
mod protocol;
pub(crate) mod shell;
use input::InputWriter;
use protocol::InputProtocol;

static NEXT_TERMINAL_ID: AtomicU64 = AtomicU64::new(1);

// Windows cmd.exe executes registry AutoRun scripts before displaying its
// prompt. Those scripts may assume a normal console, block on initialization,
// or start another shell. A fresh embedded ConPTY must start independently.
#[cfg(windows)]
fn interactive_shell() -> CommandBuilder {
    let shell = std::env::var_os("ComSpec")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cmd.exe".into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(["/D", "/K"]);
    cmd
}

#[cfg(not(windows))]
fn interactive_shell() -> CommandBuilder {
    match std::env::var_os("SHELL").filter(|value| !value.is_empty()) {
        Some(shell) => CommandBuilder::new(shell),
        None => CommandBuilder::new_default_prog(),
    }
}

// Keep title state separate from the parser lock so drawing background tabs
// does not wait for their PTY screen processing.
#[derive(Default)]
struct TabTitle {
    value: Option<String>,
    dirty: bool,
}

#[derive(Clone)]
struct TerminalCallbacks {
    reply_writer: std::sync::Weak<InputWriter>,
    pending: Arc<Mutex<Option<Vec<u8>>>>,
    title: Arc<Mutex<TabTitle>>,
    discarded_rows: i64,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn on_scroll(&mut self, _: vt100::capture::RowContents<'_>, alternate_screen: bool) {
        if !alternate_screen {
            self.discarded_rows += 1;
        }
    }

    // ConPTY and other terminal applications may block until the host
    // answers Device Status Report (DSR). Reply through the same ordered,
    // bounded PTY input queue as keyboard input, never through the UI.
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if c != 'n' || i2.is_some() {
            return;
        }
        let response = match (i1, params) {
            (None, p) if p == [&[5u16][..]] => "\x1b[0n".to_owned(),
            (None | Some(b'?'), p) if p == [&[6u16][..]] => {
                let (row, col) = screen.cursor_position();
                let prefix = if i1.is_some() { "?" } else { "" };
                format!("\x1b[{prefix}{};{}R", row + 1, col + 1)
            }
            _ => return,
        };
        if let Some(writer) = self.reply_writer.upgrade() {
            let _ = writer.send(response.as_bytes());
        }
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        // OSC 0 and 2 arrive here via vt100; OSC 1 is icon-only.
        // Never let a child inject control characters into the tab strip.
        let title: String = String::from_utf8_lossy(title)
            .chars()
            .filter(|ch| !ch.is_control())
            .take(512)
            .collect();
        let title = title.trim();
        let value = (!title.is_empty()).then(|| title.to_owned());
        if let Ok(mut state) = self.title.lock() {
            if state.value != value {
                state.value = value;
                state.dirty = true;
            }
        }
    }

    // Forward the application's clipboard request, not the Ctrl+C keystroke.
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, selection: &[u8], data: &[u8]) {
        // Only expose the standard system clipboard (not the primary X11
        // selection); bound the payload to roughly 256 KiB of decoded text.
        const MAX_ENCODED_BYTES: usize = 350_000;
        if (selection.is_empty() || selection.contains(&b'c'))
            && !data.is_empty()
            && data.len() <= MAX_ENCODED_BYTES
        {
            if let Ok(mut pending) = self.pending.lock() {
                *pending = Some(data.to_vec());
            }
        }
    }
}

type TerminalParser = vt100::Parser<TerminalCallbacks>;

// DECSET 2026 (synchronized updates) makes multi-chunk TUI redraws atomic.
// vt100 0.18 tracks cells but does not track this display protocol. Parsing
// every chunk and immediately repainting exposes transient clear/repaint
// frames while Codex scrolls. The parser still consumes bytes in order; only
// the outer UI notification waits until the corresponding DECRST 2026.
#[derive(Debug, Default)]
struct FrameGate {
    filled: usize,
    active: bool,
    started: Option<Instant>,
}

impl FrameGate {
    const START: [u8; 8] = *b"\x1b[?2026h";
    const END: [u8; 8] = *b"\x1b[?2026l";
    const MAX_HOLD: Duration = Duration::from_millis(250);

    fn expire_if_due(&mut self) -> bool {
        if self.active && self.started.is_some_and(|t| t.elapsed() >= Self::MAX_HOLD) {
            self.active = false;
            self.started = None;
            true
        } else {
            false
        }
    }

    /// The parser must process the bytes regardless; returns whether the UI
    /// may publish its updated screen after processing this chunk.
    fn accept(&mut self, mut bytes: &[u8]) -> bool {
        let mut committed = self.expire_if_due();
        while !bytes.is_empty() {
            // Plain output cannot change the gate. Only examine candidate
            // escapes, retaining a partial prefix across PTY reads.
            if self.filled == 0 {
                let Some(start) = bytes.iter().position(|&byte| byte == b'\x1b') else {
                    break;
                };
                bytes = &bytes[start..];
            }
            let byte = bytes[0];
            bytes = &bytes[1..];
            if self.filled == Self::START.len() - 1 {
                if byte == Self::START[self.filled] {
                    self.active = true;
                    self.started = Some(Instant::now());
                } else if byte == Self::END[self.filled] {
                    self.active = false;
                    self.started = None;
                    committed = true;
                }
                self.filled = usize::from(byte == b'\x1b');
            } else if byte == Self::START[self.filled] {
                self.filled += 1;
            } else {
                self.filled = usize::from(byte == b'\x1b');
            }
        }
        committed || !self.active
    }
}

#[derive(Default)]
struct SyncMonitor {
    gate: FrameGate,
    closed: bool,
}

fn spawn_sync_watchdog(
    sync_state: Arc<(Mutex<SyncMonitor>, Condvar)>,
    sync_update: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
    notify: Arc<tokio::sync::Notify>,
) {
    let _ = std::thread::Builder::new()
        .name("forge-pty-sync".into())
        .spawn(move || {
            let (lock, cvar) = &*sync_state;
            let Ok(mut guard) = lock.lock() else {
                return;
            };
            loop {
                if guard.closed {
                    break;
                }
                if let (true, Some(started)) = (guard.gate.active, guard.gate.started) {
                    let elapsed = started.elapsed();
                    if elapsed >= FrameGate::MAX_HOLD {
                        guard.gate.active = false;
                        guard.gate.started = None;
                        sync_update.store(false, Ordering::Release);
                        revision.fetch_add(1, Ordering::Release);
                        drop(guard);
                        notify.notify_one();
                        let Ok(next) = lock.lock() else {
                            break;
                        };
                        guard = next;
                    } else {
                        let wait = FrameGate::MAX_HOLD - elapsed;
                        let Ok((next, _)) = cvar.wait_timeout(guard, wait) else {
                            break;
                        };
                        guard = next;
                    }
                } else {
                    let Ok(next) = cvar.wait(guard) else {
                        break;
                    };
                    guard = next;
                }
            }
        });
}

struct RenderSnapshot {
    revision: u64,
    parser_revision: u64,
    size: (u16, u16),
    cells: Vec<Option<vt100::Cell>>,
    cursor: (u16, u16),
    hide_cursor: bool,
    scrolled_back: bool,
    first_row: i64,
}

// A child TUI owns its negotiated mouse events by default. When no child
// mouse tracking is negotiated, Forge selects its displayed text instead.
// Shift always forces host selection, and SEL explicitly opts in to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MouseRouting {
    SelectText,
    Native,
}

// Pin ownership from the first press to release, including modifier changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftGesture {
    PendingClick(MouseEvent, (u16, i64)),
    HostSelection,
    ChildMouse,
}

#[cfg(unix)]
fn foreground_owner(master: &dyn MasterPty) -> Option<i32> {
    master.process_group_leader()
}

#[cfg(not(unix))]
fn foreground_owner(_master: &dyn MasterPty) -> Option<i32> {
    // portable-pty does not currently expose an equivalent Windows PTY
    // foreground job identifier.
    Some(0)
}

pub struct EmbeddedTerminal {
    pub id: u64,
    visible: bool,
    focused: bool,
    pub maximized: bool,
    pub revision: Arc<AtomicU64>,
    pub drawn_revision: u64,
    pending_clipboard: Arc<Mutex<Option<Vec<u8>>>>,
    /// A child has opened a synchronized output frame; keep the last complete
    /// screen visible until the closing sequence reaches the PTY reader.
    sync_update: Arc<AtomicBool>,
    sync_state: Arc<(Mutex<SyncMonitor>, Condvar)>,
    last_complete_frame: Option<RenderSnapshot>,
    pub ended: Arc<AtomicBool>,
    parser: Arc<Mutex<TerminalParser>>,
    parser_revision: Arc<AtomicU64>,
    protocol: Arc<Mutex<InputProtocol>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<InputWriter>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    size: (u16, u16),
    pub area: Rect,
    // Logical buffer rows, not viewport rows.
    selection: Option<((u16, i64), (u16, i64))>,
    selection_edge: Option<(u16, isize)>,
    // Display-only drag feedback when a child TUI owns mouse input. Never
    // used for copy or key routing: the child retains its native selection.
    native_highlight: Option<((u16, u16), (u16, u16))>,
    native_press: Option<(u16, u16)>,
    left_gesture: Option<LeftGesture>,
    pub(super) mouse_routing: MouseRouting,
    /// Fallback tab title (initial working directory).
    pub name: String,
    tab_title: Arc<Mutex<TabTitle>>,
}

impl EmbeddedTerminal {
    /// The workspace retains its last pane focus while the PTY owns input.
    /// Visibility and terminal focus are changed only through these methods.
    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(super) fn is_focused(&self) -> bool {
        self.focused
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        self.focused = focused && self.visible && !self.ended.load(Ordering::Acquire);
    }

    pub(super) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            self.cancel_pointer_capture();
            self.set_focused(false);
        }
    }

    pub(super) fn activate(&mut self) {
        self.set_visible(true);
        self.set_focused(true);
    }

    pub fn start(
        cwd: &Path,
        width: u16,
        height: u16,
        notify: Arc<tokio::sync::Notify>,
    ) -> anyhow::Result<Self> {
        Self::start_with_profile(cwd, width, height, notify, None)
    }

    pub(crate) fn start_with_profile(
        cwd: &Path,
        width: u16,
        height: u16,
        notify: Arc<tokio::sync::Notify>,
        profile: Option<&shell::ShellProfile>,
    ) -> anyhow::Result<Self> {
        let cols = width.max(1);
        let rows = height.max(1);
        let pty = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = if let Some(profile) = profile {
            profile.builder(cwd)
        } else {
            let mut cmd = interactive_shell();
            cmd.cwd(cwd);
            cmd
        };
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd)?;
        drop(pty.slave);
        let writer = Arc::new(InputWriter::new(pty.master.take_writer()?)?);
        let mut reader = pty.master.try_clone_reader()?;
        let pending_clipboard = Arc::new(Mutex::new(None));
        let tab_title = Arc::new(Mutex::new(TabTitle::default()));
        let parser = Arc::new(Mutex::new(TerminalParser::new_with_callbacks(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            2000,
            TerminalCallbacks {
                reply_writer: Arc::downgrade(&writer),
                pending: pending_clipboard.clone(),
                title: tab_title.clone(),
                discarded_rows: 0,
            },
        )));
        let revision = Arc::new(AtomicU64::new(0));
        let parser_revision = Arc::new(AtomicU64::new(0));
        let read_parser_revision = parser_revision.clone();
        let protocol = Arc::new(Mutex::new(InputProtocol::default()));
        let master = Arc::new(Mutex::new(pty.master));
        let read_master = master.clone();
        let read_protocol = protocol.clone();
        let read_parser = parser.clone();
        let read_revision = revision.clone();
        let read_notify = notify.clone();
        let read_clipboard = pending_clipboard.clone();
        let sync_update = Arc::new(AtomicBool::new(false));
        let read_sync_update = sync_update.clone();
        let sync_state = Arc::new((Mutex::new(SyncMonitor::default()), Condvar::new()));
        let read_sync_state = sync_state.clone();
        let ended = Arc::new(AtomicBool::new(false));
        let read_ended = ended.clone();
        std::thread::Builder::new()
            .name("forge-pty-read".into())
            .spawn(move || {
                let mut bytes = [0u8; 8192];
                let mut frame_owner = None;
                let mut watchdog_started = false;
                loop {
                    let n = match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    // The PTY reader owns output protocol negotiation. Bind
                    // requested input modes to the foreground job that emitted
                    // them, and end the previous lease when job ownership moves.
                    let foreground = read_master
                        .lock()
                        .ok()
                        .and_then(|master| foreground_owner(&**master));
                    // A new foreground program must not inherit a previous TUI's
                    // unfinished synchronized-update lease.
                    if let (Ok(mut protocol), Ok(mut parser)) =
                        (read_protocol.lock(), read_parser.lock())
                    {
                        if foreground != frame_owner {
                            if let Ok(mut sync) = read_sync_state.0.lock() {
                                sync.gate = FrameGate::default();
                                read_sync_update.store(false, Ordering::Release);
                            }
                            read_sync_state.1.notify_one();
                            frame_owner = foreground;
                        }
                        // Decide the screen boundary before parsing so that UI
                        // renders triggered by other events also see the gate.
                        let (commit, active) = if let Ok(mut sync) = read_sync_state.0.lock() {
                            let commit = sync.gate.accept(&bytes[..n]);
                            let active = sync.gate.active;
                            read_sync_update.store(active, Ordering::Release);
                            (commit, active)
                        } else {
                            break;
                        };
                        protocol.output(&mut parser, foreground, &bytes[..n]);
                        read_parser_revision.fetch_add(1, Ordering::Release);
                        if active && !watchdog_started {
                            watchdog_started = true;
                            spawn_sync_watchdog(
                                read_sync_state.clone(),
                                read_sync_update.clone(),
                                read_revision.clone(),
                                read_notify.clone(),
                            );
                        }
                        read_sync_state.1.notify_one();
                        // Clipboard callbacks may happen while a synchronized
                        // frame is open. Wake the UI for OSC 52 even then.
                        if commit || read_clipboard.lock().is_ok_and(|slot| slot.is_some()) {
                            read_revision.fetch_add(1, Ordering::Release);
                            read_notify.notify_one();
                        }
                    } else {
                        break;
                    }
                }
                if let Ok(mut sync) = read_sync_state.0.lock() {
                    sync.closed = true;
                    sync.gate = FrameGate::default();
                }
                read_sync_state.1.notify_all();
                read_sync_update.store(false, Ordering::Release);
                read_revision.fetch_add(1, Ordering::Release);
                read_ended.store(true, Ordering::Release);
                read_notify.notify_one();
            })?;
        Ok(Self {
            id: NEXT_TERMINAL_ID.fetch_add(1, Ordering::Relaxed),
            visible: true,
            focused: true,
            maximized: false,
            revision,
            drawn_revision: 0,
            ended,
            sync_update,
            sync_state,
            last_complete_frame: None,
            pending_clipboard,
            parser,
            parser_revision,
            protocol,
            master,
            writer,
            child,
            size: (cols, rows),
            area: Rect::default(),
            selection: None,
            selection_edge: None,
            native_highlight: None,
            native_press: None,
            left_gesture: None,
            mouse_routing: MouseRouting::Native,
            name: profile.map(|p| p.name.clone()).unwrap_or_else(|| {
                cwd.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Shell".into())
            }),
            tab_title,
        })
    }

    pub fn display_title(&self) -> String {
        self.tab_title
            .lock()
            .ok()
            .and_then(|state| state.value.clone())
            .unwrap_or_else(|| self.name.clone())
    }

    /// Read the title and acknowledge it atomically when drawing the tab bar.
    pub fn draw_tab_title(&self) -> String {
        self.tab_title
            .lock()
            .ok()
            .and_then(|mut state| {
                state.dirty = false;
                state.value.clone()
            })
            .unwrap_or_else(|| self.name.clone())
    }

    pub fn tab_title_changed(&self) -> bool {
        self.tab_title.lock().is_ok_and(|state| state.dirty)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_size = (cols.max(1), rows.max(1));
        if self.size == new_size {
            return;
        }
        self.selection = None;
        self.selection_edge = None;
        if matches!(
            self.left_gesture,
            Some(LeftGesture::HostSelection | LeftGesture::PendingClick(..))
        ) {
            self.left_gesture = None;
        }
        self.native_highlight = None;
        self.native_press = None;
        if self
            .master
            .lock()
            .expect("pty master")
            .resize(PtySize {
                cols: new_size.0,
                rows: new_size.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .is_ok()
        {
            if let Ok(mut parser) = self.parser.lock() {
                parser.set_size(
                    NonZeroU16::new(new_size.1).unwrap(),
                    NonZeroU16::new(new_size.0).unwrap(),
                );
                self.parser_revision.fetch_add(1, Ordering::Release);
            }
            self.size = new_size;
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.ended.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "shell has exited",
            ));
        }
        self.writer.send(bytes)
    }

    pub fn paste(&mut self, text: &str) -> std::io::Result<()> {
        // Respect mode negotiated by the foreground program. In bracketed
        // paste, lines are delivered for editing rather than as Enter keys.
        let bracketed = self.input_screen(|screen| screen.bracketed_paste())?;
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
        let application = self.input_screen(|screen| screen.application_cursor())?;
        let Some(data) = encode_key(key, application) else {
            return Ok(());
        };
        self.scroll_to_bottom(); // any input returns to the live viewport
        self.selection = None;
        self.selection_edge = None;
        self.native_highlight = None;
        self.native_press = None;
        self.write_input(&data)
    }

    pub fn scroll(&mut self, amount: isize) {
        if amount != 0 {
            self.native_highlight = None;
            self.native_press = None;
        }
        if let Ok(mut parser) = self.parser.lock() {
            let screen = parser.screen_mut();
            let current = screen.scrollback();
            let next = if amount < 0 {
                current.saturating_add(amount.unsigned_abs())
            } else {
                current.saturating_sub(amount as usize)
            };
            screen.set_scrollback(next);
            self.parser_revision.fetch_add(1, Ordering::Release);
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    fn first_buffer_row(parser: &TerminalParser) -> i64 {
        let screen = parser.screen();
        if screen.alternate_screen() {
            0
        } else {
            parser.callbacks().discarded_rows + screen.scrollback_len() as i64
                - screen.scrollback() as i64
        }
    }

    fn buffer_point(&self, x: u16, y: u16) -> (u16, i64) {
        let first = self
            .parser
            .lock()
            .map(|p| Self::first_buffer_row(&p))
            .unwrap_or(0);
        (x, first + i64::from(y))
    }

    fn extend_selection(&mut self, x: u16, y: u16) {
        if let Some((start, _)) = self.selection {
            self.selection = Some((start, self.buffer_point(x, y)));
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    fn track_selection_edge(&mut self, x: u16, y: u16) {
        self.selection_edge = if y == 0 {
            Some((x, -1))
        } else if y == self.area.height.saturating_sub(1) {
            Some((x, 1))
        } else {
            None
        };
    }

    pub(super) fn captures_host_selection(&self) -> bool {
        self.left_gesture == Some(LeftGesture::HostSelection)
    }

    pub(super) fn selection_autoscroll_active(&self) -> bool {
        self.visible
            && self.left_gesture == Some(LeftGesture::HostSelection)
            && self.selection_edge.is_some()
            && self.parser.lock().is_ok_and(|p| {
                let screen = p.screen();
                match self.selection_edge {
                    Some((_, -1)) => screen.scrollback() < screen.scrollback_len(),
                    Some(_) => screen.scrollback() > 0,
                    None => false,
                }
            })
    }

    pub(super) fn tick_selection_autoscroll(&mut self) {
        if !self.selection_autoscroll_active() {
            return;
        }
        if let Some((x, direction)) = self.selection_edge {
            self.scroll(direction);
            let y = if direction < 0 {
                0
            } else {
                self.area.height.saturating_sub(1)
            };
            self.extend_selection(x, y);
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll(isize::MAX);
    }

    pub fn scrollback_position(&self) -> usize {
        self.parser
            .lock()
            .map(|parser| parser.screen().scrollback())
            .unwrap_or(0)
    }

    pub fn on_alternate_screen(&self) -> bool {
        self.parser
            .lock()
            .map(|parser| parser.screen().alternate_screen())
            .unwrap_or(false)
    }

    fn input_screen<T>(&self, read: impl FnOnce(&vt100::Screen) -> T) -> std::io::Result<T> {
        let foreground = self
            .master
            .lock()
            .map_err(|_| std::io::Error::other("terminal master lock poisoned"))
            .map(|master| foreground_owner(&**master))?;
        let mut protocol = self
            .protocol
            .lock()
            .map_err(|_| std::io::Error::other("terminal protocol lock poisoned"))?;
        let mut parser = self
            .parser
            .lock()
            .map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?;
        if protocol.sync(&mut parser, foreground) {
            self.parser_revision.fetch_add(1, Ordering::Release);
            self.revision.fetch_add(1, Ordering::Release);
        }
        Ok(read(parser.screen()))
    }

    /// Called by the UI after a foreground app requested OSC 52 copying.
    /// The app chooses the content; Forge only transports the protocol to the
    /// outer terminal, so Ctrl+C remains owned by the app itself.
    pub(super) fn take_clipboard_request(&self) -> Option<Vec<u8>> {
        self.pending_clipboard.lock().ok()?.take()
    }

    /// True while this terminal owns a left-button selection gesture, even
    /// if the pointer has left the viewport (release must reach its owner).
    pub fn captures_left_drag(&self) -> bool {
        self.left_gesture.is_some()
    }

    /// End a previous gesture before a new press or a modal handoff.
    /// A native child receives its release; a pending click is discarded.
    pub(super) fn cancel_pointer_capture(&mut self) {
        if self.left_gesture == Some(LeftGesture::ChildMouse) {
            let (x, y) = self
                .native_highlight
                .map(|(_, end)| end)
                .or(self.native_press)
                .unwrap_or_default();
            let _ = self.mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: self.area.x.saturating_add(x),
                row: self.area.y.saturating_add(y),
                modifiers: KeyModifiers::NONE,
            });
        }
        self.left_gesture = None;
        self.selection_edge = None;
        self.native_press = None;
    }

    /// Toggle this tab between selection-first and protocol-first routing.
    /// A gesture already in progress keeps its existing owner until release.
    pub(super) fn toggle_mouse_routing(&mut self) -> MouseRouting {
        self.native_highlight = None;
        self.native_press = None;
        self.mouse_routing = match self.mouse_routing {
            MouseRouting::SelectText => MouseRouting::Native,
            MouseRouting::Native => MouseRouting::SelectText,
        };
        self.revision.fetch_add(1, Ordering::Release);
        self.mouse_routing
    }

    /// Route left drag by explicit per-tab preference, while retaining the
    /// negotiated mouse protocol for clicks, hover, wheel, and native mode.
    /// Shift+left-drag explicitly overrides that mode for host text selection.
    /// A gesture retains its initial owner until release, including when the
    /// pointer leaves the viewport or the modifier keys change mid-drag.
    pub fn mouse(&mut self, event: MouseEvent) -> std::io::Result<Option<String>> {
        if self.area.width == 0 || self.area.height == 0 {
            return Ok(None);
        }
        let inside = self.area.contains((event.column, event.row).into());
        if !inside && !self.captures_left_drag() {
            return Ok(None);
        }
        // Keep a captured drag alive when it crosses the terminal border.
        let event = MouseEvent {
            column: event
                .column
                .clamp(self.area.x, self.area.right().saturating_sub(1)),
            row: event
                .row
                .clamp(self.area.y, self.area.bottom().saturating_sub(1)),
            ..event
        };
        let x = event.column.saturating_sub(self.area.x);
        let y = event.row.saturating_sub(self.area.y);

        if let Some(LeftGesture::PendingClick(down, start)) = self.left_gesture {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.left_gesture = Some(LeftGesture::HostSelection);
                    self.selection = Some((start, self.buffer_point(x, y)));
                    self.track_selection_edge(x, y);
                    self.revision.fetch_add(1, Ordering::Release);
                    return Ok(None);
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.left_gesture = None;
                    // Some hosts omit an intermediate Drag at low pointer
                    // sampling rates. Different release coordinates are still
                    // a text selection, not a click.
                    let end = self.buffer_point(x, y);
                    if start != end {
                        self.selection = Some((start, end));
                        self.revision.fetch_add(1, Ordering::Release);
                        return Ok(self.selected_text());
                    }
                    // No movement: deliver a full click pair to the child.
                    let foreground = self
                        .master
                        .lock()
                        .map_err(|_| std::io::Error::other("terminal master lock poisoned"))
                        .map(|master| foreground_owner(&**master))?;
                    let tracking = {
                        let mut protocol = self.protocol.lock().map_err(|_| {
                            std::io::Error::other("terminal protocol lock poisoned")
                        })?;
                        let mut parser = self
                            .parser
                            .lock()
                            .map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?;
                        if protocol.sync(&mut parser, foreground) {
                            self.parser_revision.fetch_add(1, Ordering::Release);
                            self.revision.fetch_add(1, Ordering::Release);
                        }
                        protocol.mouse(&mut parser, foreground)
                    };
                    if let Some((mode, encoding)) = tracking {
                        for click in [down, event] {
                            if let Some(bytes) = encode_mouse(click, self.area, mode, encoding) {
                                self.write_input(&bytes)?;
                            }
                        }
                    }
                    return Ok(None);
                }
                _ => return Ok(None),
            }
        }

        if self.left_gesture == Some(LeftGesture::HostSelection) {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.track_selection_edge(x, y);
                    self.extend_selection(x, y);
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.left_gesture = None;
                    self.selection_edge = None;
                    self.extend_selection(x, y);
                    if self.selection.is_some_and(|(start, end)| start != end) {
                        return Ok(self.selected_text());
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    self.scroll(if event.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    });
                    self.extend_selection(x, y);
                }
                _ => {}
            }
            return Ok(None);
        }

        let foreground = self
            .master
            .lock()
            .map_err(|_| std::io::Error::other("terminal master lock poisoned"))
            .map(|master| foreground_owner(&**master))?;
        let negotiated = {
            let mut protocol = self
                .protocol
                .lock()
                .map_err(|_| std::io::Error::other("terminal protocol lock poisoned"))?;
            let mut parser = self
                .parser
                .lock()
                .map_err(|_| std::io::Error::other("terminal parser lock poisoned"))?;
            if protocol.sync(&mut parser, foreground) {
                self.parser_revision.fetch_add(1, Ordering::Release);
                self.revision.fetch_add(1, Ordering::Release);
            }
            protocol.mouse(&mut parser, foreground)
        };
        // Shift can override mouse tracking only when starting a new gesture.
        // Releasing Shift mid-drag must not steal it from the child or host.
        let child_mouse = if self.left_gesture != Some(LeftGesture::ChildMouse)
            && event.modifiers.contains(KeyModifiers::SHIFT)
        {
            None
        } else {
            negotiated
        };

        if self.left_gesture == Some(LeftGesture::ChildMouse)
            && matches!(
                event.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            // The TUI owns the gesture, including its eventual copy action.
            // Mirroring the dragged cells is purely a terminal visual overlay,
            // matching a host emulator's selection feedback without hijacking
            // any mouse event or manufacturing a Forge clipboard selection.
            if let Some(start) = self.native_press {
                if (x, y) != start {
                    self.native_highlight = Some((start, (x, y)));
                    self.revision.fetch_add(1, Ordering::Release);
                }
            }
            if let Some((mode, encoding)) = child_mouse {
                if let Some(bytes) = encode_mouse(event, self.area, mode, encoding) {
                    self.write_input(&bytes)?;
                }
            }
            if matches!(event.kind, MouseEventKind::Up(MouseButton::Left)) {
                self.left_gesture = None;
                self.native_press = None;
            }
            return Ok(None);
        }

        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.set_focused(true);
            self.selection = None;
            self.selection_edge = None;
            self.native_highlight = None;
            self.native_press = None;
            if child_mouse.is_some()
                && self.mouse_routing == MouseRouting::SelectText
                && !event.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.left_gesture = Some(LeftGesture::PendingClick(event, self.buffer_point(x, y)));
                self.revision.fetch_add(1, Ordering::Release);
                return Ok(None);
            }
            if child_mouse.is_some() {
                self.left_gesture = Some(LeftGesture::ChildMouse);
                self.native_press = Some((x, y));
            } else {
                self.left_gesture = Some(LeftGesture::HostSelection);
                let point = self.buffer_point(x, y);
                self.selection = Some((point, point));
                self.revision.fetch_add(1, Ordering::Release);
                return Ok(None);
            }
            self.revision.fetch_add(1, Ordering::Release);
        }
        if matches!(
            event.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            self.native_highlight = None;
            self.native_press = None;
            self.revision.fetch_add(1, Ordering::Release);
        }
        if let Some((mode, encoding)) = child_mouse {
            if let Some(bytes) = encode_mouse(event, self.area, mode, encoding) {
                self.write_input(&bytes)?;
            }
            return Ok(None);
        }
        if matches!(event.kind, MouseEventKind::Moved) {
            return Ok(None);
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll(-3),
            MouseEventKind::ScrollDown => self.scroll(3),
            _ => {}
        }
        self.revision.fetch_add(1, Ordering::Release);
        Ok(None)
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?;
        let parser = self.parser.lock().ok()?;
        // Copy via a private viewport without changing the displayed scrollback.
        let mut screen = parser.screen().clone();
        let base = if screen.alternate_screen() {
            0
        } else {
            parser.callbacks().discarded_rows
        };
        let live = base + screen.scrollback_len() as i64;
        let height = i64::from(screen.size().0.get());
        let width = self.area.width;
        let (a, b) = if (start.1, start.0) <= (end.1, end.0) {
            (start, end)
        } else {
            (end, start)
        };
        let mut lines = Vec::new();
        // Evicted rows must not be replaced with unrelated content.
        if a.1 < base || b.1 >= live + height {
            return None;
        }
        for y in a.1..=b.1 {
            screen.set_scrollback((live - y).max(0) as usize);
            let row = (y - live).max(0) as u16;
            let left = if y == a.1 { a.0 } else { 0 };
            let right = if y == b.1 {
                b.0
            } else {
                width.saturating_sub(1)
            };
            let mut line = String::new();
            for x in left..=right {
                if let Some(cell) = screen.cell(row, x) {
                    line.push_str(cell.contents());
                }
            }
            lines.push(line.trim_end().to_string());
        }
        Some(lines.join("\n"))
    }

    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        bg: Color,
        fg: Color,
        owns_keyboard: bool,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.area = area;
        self.resize(area.width, area.height);
        // While the foreground TUI is in DECSET 2026, other Forge events
        // (such as hover) can trigger renders independently of PTY output.
        // Keep the previously committed screen visible until the matching
        // DECRST 2026 rather than exposing a halfway-cleared Codex frame.
        if self.sync_update.load(Ordering::Acquire) {
            if let Ok(mut sync) = self.sync_state.0.lock() {
                if sync.gate.expire_if_due() {
                    self.sync_update.store(false, Ordering::Release);
                    self.revision.fetch_add(1, Ordering::Release);
                }
            }
        }
        {
            let Ok(parser) = self.parser.lock() else {
                return;
            };
            // The reader publishes the gate and screen while holding this lock.
            // Recheck here so an update begun while we waited stays hidden.
            let waiting_for_frame = self.sync_update.load(Ordering::Acquire);
            let parser_revision = self.parser_revision.load(Ordering::Acquire);
            let size = (area.width, area.height);
            let need_snapshot = self.last_complete_frame.as_ref().is_none_or(|snapshot| {
                snapshot.size != size || snapshot.parser_revision != parser_revision
            });
            if !waiting_for_frame && need_snapshot {
                let screen = parser.screen();
                self.last_complete_frame = Some(RenderSnapshot {
                    revision: self.revision.load(Ordering::Acquire),
                    parser_revision,
                    size,
                    cells: (0..area.height)
                        .flat_map(|y| (0..area.width).map(move |x| screen.cell(y, x).cloned()))
                        .collect(),
                    cursor: screen.cursor_position(),
                    hide_cursor: screen.hide_cursor(),
                    scrolled_back: screen.scrollback() != 0,
                    first_row: Self::first_buffer_row(&parser),
                });
            } else if !waiting_for_frame {
                self.last_complete_frame.as_mut().unwrap().revision =
                    self.revision.load(Ordering::Acquire);
            }
        }
        // Opening or resizing during a synchronized update must not publish
        // the partial parser screen. Pad/crop the last committed frame instead.
        if self
            .last_complete_frame
            .as_ref()
            .is_none_or(|s| s.size != (area.width, area.height))
        {
            let previous = self.last_complete_frame.take();
            let mut cells = vec![None; area.width as usize * area.height as usize];
            if let Some(old) = &previous {
                for y in 0..area.height.min(old.size.1) {
                    for x in 0..area.width.min(old.size.0) {
                        cells[y as usize * area.width as usize + x as usize] =
                            old.cells[y as usize * old.size.0 as usize + x as usize].clone();
                    }
                }
            }
            self.last_complete_frame = Some(RenderSnapshot {
                revision: previous.as_ref().map_or(0, |s| s.revision),
                parser_revision: u64::MAX,
                size: (area.width, area.height),
                cells,
                cursor: (0, 0),
                hide_cursor: true,
                scrolled_back: false,
                first_row: previous.as_ref().map_or(0, |s| s.first_row),
            });
        }
        let snapshot = self
            .last_complete_frame
            .as_ref()
            .expect("a snapshot was just built");
        let cells = &snapshot.cells;
        let cursor = snapshot.cursor;
        // IMEs (including browser terminal IMEs) anchor composition to the
        // real outer cursor, not to an inverse-video cell in the PTY grid.
        // Use the same committed snapshot as the displayed cells, and let
        // the outer terminal draw its own cursor rather than double-inverting.
        if owns_keyboard
            && self.visible
            && self.focused
            && !self.ended.load(Ordering::Acquire)
            && !snapshot.hide_cursor
            && !snapshot.scrolled_back
            && cursor.0 < area.height
        {
            // vt100 may report the pending-wrap column just beyond the last cell.
            frame.set_cursor_position((area.x + cursor.1.min(area.width - 1), area.y + cursor.0));
        }
        // APP mode's native drag feedback is visual only. An explicit Forge
        // text selection (Shift+drag or SEL mode) has priority if present.
        let selection = self
            .selection
            .or_else(|| {
                self.native_highlight.map(|(a, b)| {
                    (
                        (a.0, i64::from(a.1) + snapshot.first_row),
                        (b.0, i64::from(b.1) + snapshot.first_row),
                    )
                })
            })
            .map(|(start, end)| {
                if (start.1, start.0) <= (end.1, end.0) {
                    (start, end)
                } else {
                    (end, start)
                }
            });
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = cells[(y as usize * area.width as usize) + x as usize].as_ref()
                else {
                    continue;
                };
                // Render all VT SGR attributes. Missing inverse/dim/italic/
                // underline made Codex markdown differ from a real terminal.
                let mut attrs = Modifier::empty();
                if cell.bold() {
                    attrs |= Modifier::BOLD;
                }
                if cell.dim() {
                    attrs |= Modifier::DIM;
                }
                if cell.italic() {
                    attrs |= Modifier::ITALIC;
                }
                if cell.underline() {
                    attrs |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    attrs |= Modifier::REVERSED;
                }
                let mut style = Style::default()
                    .fg(to_color(cell.fgcolor(), fg))
                    .bg(to_color(cell.bgcolor(), bg))
                    .add_modifier(attrs);
                if selection.is_some_and(|(start, end)| {
                    let row = snapshot.first_row + i64::from(y);
                    (row, x) >= (start.1, start.0) && (row, x) <= (end.1, end.0)
                }) {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let symbol = cell.contents();
                let symbol = if symbol.is_empty() { " " } else { symbol };
                // Ratatui's set_style merges modifiers; reset the cell first
                // so a prior frame's highlight/inverse cannot leak through.
                let cell_out = &mut frame.buffer_mut()[(area.x + x, area.y + y)];
                cell_out.reset();
                cell_out.set_symbol(symbol).set_style(style);
            }
        }
        self.drawn_revision = snapshot.revision;
    }
}

/// Stable across processes: Rust's RandomState is deliberately not used.
fn modifier_param(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(mods.contains(KeyModifiers::ALT))
        + 4 * u8::from(mods.contains(KeyModifiers::CONTROL))
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
                KeyCode::Up => 'A',
                KeyCode::Down => 'B',
                KeyCode::Right => 'C',
                _ => 'D',
            };
            if m > 1 {
                format!("\x1b[1;{m}{c}").into_bytes()
            } else if application_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            }
        }
        KeyCode::Home | KeyCode::End => {
            let c = if key.code == KeyCode::Home { 'H' } else { 'F' };
            if m > 1 {
                format!("\x1b[1;{m}{c}").into_bytes()
            } else if application_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            }
        }
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown => {
            let n = match key.code {
                KeyCode::Insert => 2,
                KeyCode::Delete => 3,
                KeyCode::PageUp => 5,
                _ => 6,
            };
            if m > 1 {
                format!("\x1b[{n};{m}~").into_bytes()
            } else {
                format!("\x1b[{n}~").into_bytes()
            }
        }
        KeyCode::F(f @ 1..=4) => {
            let c = ['P', 'Q', 'R', 'S'][(f - 1) as usize];
            if m > 1 {
                format!("\x1b[1;{m}{c}").into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        KeyCode::F(f @ 5..=12) => {
            let n = [15, 17, 18, 19, 20, 21, 23, 24][(f - 5) as usize];
            if m > 1 {
                format!("\x1b[{n};{m}~").into_bytes()
            } else {
                format!("\x1b[{n}~").into_bytes()
            }
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let code = match c.to_ascii_lowercase() {
                'a'..='z' => (c.to_ascii_lowercase() as u8) - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 27,
                '\\' | '4' => 28,
                ']' | '5' => 29,
                '^' | '6' => 30,
                '_' | '7' => 31,
                '?' | '8' => 127,
                _ => return None,
            };
            let mut data = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                data.push(0x1b);
            }
            data.push(code);
            data
        }
        KeyCode::Char(c) => {
            let mut data = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                data.push(0x1b);
            }
            data.extend(c.to_string().as_bytes());
            data
        }
        _ => return None,
    };
    Some(bytes)
}

fn encode_mouse(
    event: MouseEvent,
    area: Rect,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }
    let release = matches!(event.kind, MouseEventKind::Up(_));
    let motion = matches!(event.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved);
    if mode == MouseProtocolMode::Press && (release || motion) {
        return None;
    }
    if mode == MouseProtocolMode::PressRelease && motion {
        return None;
    }
    if mode == MouseProtocolMode::ButtonMotion && matches!(event.kind, MouseEventKind::Moved) {
        return None;
    }
    let button = match event.kind {
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => 0u16,
        MouseEventKind::Down(MouseButton::Middle) | MouseEventKind::Drag(MouseButton::Middle) => 1,
        MouseEventKind::Down(MouseButton::Right) | MouseEventKind::Drag(MouseButton::Right) => 2,
        // In SGR 1006, the trailing 'm' marks release; preserve which
        // button was released. Legacy X10/UTF-8 instead use button code 3.
        MouseEventKind::Up(MouseButton::Left) if encoding == MouseProtocolEncoding::Sgr => 0,
        MouseEventKind::Up(MouseButton::Middle) if encoding == MouseProtocolEncoding::Sgr => 1,
        MouseEventKind::Up(MouseButton::Right) if encoding == MouseProtocolEncoding::Sgr => 2,
        MouseEventKind::Up(_) | MouseEventKind::Moved => 3,
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
    };
    let mut b = button;
    if motion {
        b |= 32;
    }
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        b |= 4;
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        b |= 8;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        b |= 16;
    }
    let x = event.column.saturating_sub(area.x) as u32 + 1;
    let y = event.row.saturating_sub(area.y) as u32 + 1;
    if encoding == MouseProtocolEncoding::Sgr {
        return Some(format!("\x1b[<{b};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes());
    }
    // X10/UTF-8 coordinates encode each value with an offset of 32.
    if encoding == MouseProtocolEncoding::Default && (x > 223 || y > 223) {
        return None;
    }
    let mut out = b"\x1b[M".to_vec();
    for v in [u32::from(b) + 32, x + 32, y + 32] {
        if encoding == MouseProtocolEncoding::Utf8 {
            let ch = char::from_u32(v)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        } else {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

fn to_color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        vt100::Color::Idx(index) => Color::Indexed(index),
    }
}

impl Drop for EmbeddedTerminal {
    fn drop(&mut self) {
        if let Ok(mut sync) = self.sync_state.0.lock() {
            sync.closed = true;
            sync.gate = FrameGate::default();
        }
        self.sync_state.1.notify_all();
        // Never leave an orphan shell when Forge exits.
        let _ = self.child.kill();
        // Reap the child even if it has already exited, avoiding zombies when
        // the user opens and closes several embedded terminals.
        let _ = self.child.wait();
    }
}

// Unlike the Unix-specific protocol suite, this exercises the actual native
// PTY on both platforms, including startup, input, output and rendering.
#[cfg(all(test, any(unix, windows)))]
mod native_pty_tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn osc_tab_titles_are_streamed_isolated_and_resettable() {
        let make_parser = || {
            let title = Arc::new(Mutex::new(TabTitle::default()));
            let parser = TerminalParser::new_with_callbacks(
                NonZeroU16::new(12).unwrap(),
                NonZeroU16::new(60).unwrap(),
                200,
                TerminalCallbacks {
                    reply_writer: std::sync::Weak::new(),
                    pending: Arc::new(Mutex::new(None)),
                    title: title.clone(),
                    discarded_rows: 0,
                },
            );
            (parser, title)
        };
        let (mut first, first_title) = make_parser();
        let (mut second, second_title) = make_parser();
        first.process(b"\x1b]0;Antigravity");
        assert!(first_title.lock().unwrap().value.is_none());
        first.process(b"\x07");
        assert_eq!(
            first_title.lock().unwrap().value.as_deref(),
            Some("Antigravity")
        );
        assert!(first_title.lock().unwrap().dirty);
        assert!(second_title.lock().unwrap().value.is_none());

        second.process(b"\x1b]2;Codex\x1b");
        second.process(b"\\");
        assert_eq!(second_title.lock().unwrap().value.as_deref(), Some("Codex"));
        first_title.lock().unwrap().dirty = false;
        first.process(b"\x1b]2;Antigravity\x07");
        assert!(!first_title.lock().unwrap().dirty);
        first.process(b"\x1b]1;icon only\x07");
        assert_eq!(
            first_title.lock().unwrap().value.as_deref(),
            Some("Antigravity")
        );
        first.process("\x1b]2;中文标题\x07".as_bytes());
        assert_eq!(
            first_title.lock().unwrap().value.as_deref(),
            Some("中文标题")
        );
        first.process(b"\x1b]2;\x07");
        assert!(first_title.lock().unwrap().value.is_none());
        assert!(first_title.lock().unwrap().dirty);
        assert_eq!(second_title.lock().unwrap().value.as_deref(), Some("Codex"));
    }

    #[cfg(unix)]
    #[test]
    fn native_pty_osc_title_updates_tab_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(dir.path(), 80, 12, Arc::new(tokio::sync::Notify::new()))
                .expect("start native interactive shell");
        embedded
            .paste(r"printf '\033]2;Forge-%s\007' OSC; sleep 2")
            .unwrap();
        embedded
            .send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while embedded.display_title() != "Forge-OSC" {
            assert!(
                Instant::now() < deadline,
                "PTY title: {:?}",
                embedded.display_title()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(embedded.tab_title_changed());
        assert_eq!(embedded.draw_tab_title(), "Forge-OSC");
        assert!(!embedded.tab_title_changed());
    }

    #[test]
    fn terminal_answers_device_status_queries_at_actual_cursor() {
        use std::io::{self, Write};
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(InputWriter::capture(Box::new(Capture(bytes.clone()))));
        let mut parser = TerminalParser::new_with_callbacks(
            NonZeroU16::new(12).unwrap(),
            NonZeroU16::new(80).unwrap(),
            0,
            TerminalCallbacks {
                reply_writer: Arc::downgrade(&writer),
                pending: Arc::new(Mutex::new(None)),
                title: Arc::new(Mutex::new(TabTitle::default())),
                discarded_rows: 0,
            },
        );
        parser.process(b"\x1b[3;7H\x1b[");
        assert!(bytes.lock().unwrap().is_empty());
        parser.process(b"6n\x1b[5n\x1b[?6n\x1b[9n");
        assert_eq!(&*bytes.lock().unwrap(), b"\x1b[3;7R\x1b[0n\x1b[?3;7R");
    }

    #[test]
    fn native_shell_starts_and_renders_command_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(dir.path(), 80, 12, Arc::new(tokio::sync::Notify::new()))
                .expect("start native interactive shell");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let contents = embedded.parser.lock().unwrap().screen().contents();
            if !contents.trim().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline && !embedded.ended.load(Ordering::Acquire),
                "native shell produced no startup output"
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        // The expected string cannot occur in the echoed command itself.
        #[cfg(windows)]
        let (command, expected) = ("echo FORGE_PTY_%OS%_READY", "FORGE_PTY_Windows_NT_READY");
        #[cfg(unix)]
        let (command, expected) = ("printf '%s%s\\n' FORGE_PTY_ READY", "FORGE_PTY_READY");

        embedded.paste(command).unwrap();
        embedded
            .send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut ui = Terminal::new(TestBackend::new(80, 12)).unwrap();
        loop {
            ui.draw(|frame| {
                embedded.render(
                    frame,
                    Rect::new(0, 0, 80, 12),
                    Color::Black,
                    Color::White,
                    true,
                );
            })
            .unwrap();
            let rendered = (0..12)
                .map(|y| {
                    (0..80)
                        .map(|x| ui.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if rendered.contains(expected) {
                break;
            }
            assert!(
                Instant::now() < deadline && !embedded.ended.load(Ordering::Acquire),
                "native shell command output not rendered: {rendered:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        embedded.resize(60, 9);
        assert_eq!(embedded.size, (60, 9));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn xterm_key_sequences_cover_application_mode_and_modifiers() {
        let key = |code, mods| KeyEvent::new(code, mods);
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(key(KeyCode::Left, KeyModifiers::CONTROL), false).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(key(KeyCode::Right, KeyModifiers::ALT), false).unwrap(),
            b"\x1b[1;3C"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(5), KeyModifiers::NONE), false).unwrap(),
            b"\x1b[15~"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(12), KeyModifiers::SHIFT), false).unwrap(),
            b"\x1b[24;2~"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false).unwrap(),
            b"\x03"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL), false).unwrap(),
            b"\x00"
        );
    }

    /// Emulate a decoder receiving bytes while a specific job owns the PTY.
    /// The mouse protocol lease must be issued through the same state machine
    /// used by the real reader, never by mutating the VT parser in isolation.
    fn child_output(term: &EmbeddedTerminal, owner: Option<i32>, data: &[u8]) {
        let mut protocol = term.protocol.lock().unwrap();
        let mut parser = term.parser.lock().unwrap();
        protocol.output(&mut parser, owner, data);
        term.parser_revision.fetch_add(1, Ordering::Release);
    }

    #[test]
    fn negotiated_bracketed_paste_is_single_write_with_delimiters() {
        let cwd = std::env::current_dir().unwrap();
        let mut term =
            EmbeddedTerminal::start(&cwd, 80, 12, Arc::new(tokio::sync::Notify::new())).unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        term.writer = Arc::new(InputWriter::capture(Box::new(Capture(output.clone()))));
        let fg = term.master.lock().unwrap().process_group_leader();
        child_output(&term, fg, b"\x1b[?2004h");
        term.paste("echo alpha\necho beta").unwrap();
        assert_eq!(
            &*output.lock().unwrap(),
            b"\x1b[200~echo alpha\necho beta\x1b[201~"
        );
        output.lock().unwrap().clear();
        child_output(&term, fg, b"\x1b[?2004l");
        term.paste("hello").unwrap();
        assert_eq!(&*output.lock().unwrap(), b"hello");
    }

    #[test]
    fn stale_sgr_mouse_mode_does_not_inject_events_into_shell() {
        let cwd = std::env::current_dir().unwrap();
        let mut term =
            EmbeddedTerminal::start(&cwd, 80, 20, Arc::new(tokio::sync::Notify::new())).unwrap();
        term.area = Rect::new(0, 0, 80, 20);
        let shell_pid = term.child.process_id().unwrap() as i32;
        for _ in 0..60 {
            if term.master.lock().unwrap().process_group_leader() == Some(shell_pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            term.master.lock().unwrap().process_group_leader(),
            Some(shell_pid),
            "the interactive child shell should own the foreground PTY"
        );

        let recorded = Arc::new(Mutex::new(Vec::new()));
        term.writer = Arc::new(InputWriter::capture(Box::new(Capture(recorded.clone()))));
        let movement = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 12,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        // A TUI's mouse lease belongs to its foreground job, even if it
        // crashes leaving 1003 and the alternate-screen enabled.
        child_output(
            &term,
            Some(shell_pid + 1000),
            b"\x1b[?1003h\x1b[?1006h\x1b[?1049h",
        );
        assert_eq!(
            term.parser.lock().unwrap().screen().mouse_protocol_mode(),
            MouseProtocolMode::AnyMotion
        );
        term.mouse(movement).unwrap();
        assert!(
            recorded.lock().unwrap().is_empty(),
            "a dead child must not keep sending mouse reports"
        );
        assert!(!term.parser.lock().unwrap().screen().alternate_screen());
        assert_eq!(
            term.parser.lock().unwrap().screen().mouse_protocol_mode(),
            MouseProtocolMode::None
        );
        term.mouse(movement).unwrap();
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            ..movement
        };
        term.mouse(click).unwrap();
        assert!(
            recorded.lock().unwrap().is_empty(),
            "mouse escapes leaked into the idle shell"
        );
        assert!(
            term.selection.is_some(),
            "a shell prompt should use host-side selection"
        );
        // The ordinary shell still gets local drag selection without mouse
        // tracking, including a release beyond the viewport.
        term.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 17,
            ..click
        })
        .unwrap();
        assert_eq!(term.selection, Some(((12, 5), (17, 5))));
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 99,
            row: 99,
            ..click
        })
        .unwrap();
        assert_eq!(term.selection, Some(((12, 5), (79, 19))));
        assert!(!term.captures_left_drag());
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[test]
    fn foreground_child_program_still_receives_mouse_reports() {
        let cwd = std::env::current_dir().unwrap();
        let mut term =
            EmbeddedTerminal::start(&cwd, 80, 20, Arc::new(tokio::sync::Notify::new())).unwrap();
        term.area = Rect::new(0, 0, 80, 20);
        let shell = term.child.process_id().unwrap() as i32;
        // A child launched by the interactive shell owns a different
        // foreground process group (the same check mouse() uses).
        term.write_input(b"cat\n").unwrap();
        let mut foreground = None;
        for _ in 0..200 {
            foreground = term.master.lock().unwrap().process_group_leader();
            if foreground.is_some_and(|group| group != shell) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            foreground.is_some_and(|group| group != shell),
            "shell did not hand the PTY to its foreground child"
        );

        // Capture forwarded bytes instead of writing synthetic mouse traffic
        // into cat; the real child is only used to verify PTY ownership.
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let original_writer = std::mem::replace(
            &mut term.writer,
            Arc::new(InputWriter::capture(Box::new(Capture(recorded.clone())))),
        );
        child_output(&term, foreground, b"\x1b[?1003h\x1b[?1006h");
        term.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 7,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
        .unwrap();
        let encoded = recorded.lock().unwrap().clone();
        assert_eq!(
            encoded, b"\x1b[<35;8;5M",
            "mouse-aware foreground TUIs must keep receiving SGR motion"
        );
        recorded.lock().unwrap().clear();

        // Application-native routing is the default: a mouse-aware child
        // owns drag and its own copy semantics. Exercise the optional host
        // selection mode separately, without special casing Codex.
        assert_eq!(term.mouse_routing, MouseRouting::Native);
        assert_eq!(term.toggle_mouse_routing(), MouseRouting::SelectText);
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 7,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        term.mouse(down).unwrap();
        assert!(term.captures_left_drag());
        assert!(
            recorded.lock().unwrap().is_empty(),
            "select-first mode delays click until release"
        );
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..down
        })
        .unwrap();
        assert!(!term.captures_left_drag());
        assert_eq!(&*recorded.lock().unwrap(), b"\x1b[<0;8;5M\x1b[<0;8;5m");
        recorded.lock().unwrap().clear();

        // A real text drag selects several characters without emitting child
        // mouse reports. It also remains captured outside the viewport.
        child_output(&term, foreground, b"\x1b[5;8HABCDE");
        term.mouse(down).unwrap();
        term.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 9,
            row: 4,
            ..down
        })
        .unwrap();
        let text = term
            .mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 9,
                row: 4,
                ..down
            })
            .unwrap();
        assert_eq!(text.as_deref(), Some("ABC"));
        assert_eq!(term.selection, Some(((7, 4), (9, 4))));
        // Selection must be visible in the rendered screen, not just in the
        // clipboard result. The hardware cursor is independent of the selection.
        {
            use ratatui::{backend::TestBackend, Terminal};
            let mut ui = Terminal::new(TestBackend::new(80, 20)).unwrap();
            ui.draw(|frame| {
                term.render(
                    frame,
                    Rect::new(0, 0, 80, 20),
                    Color::Black,
                    Color::White,
                    true,
                )
            })
            .unwrap();
            for x in 7..=9 {
                assert!(
                    ui.backend().buffer()[(x, 4)]
                        .modifier
                        .contains(Modifier::REVERSED),
                    "selected column {x} was not highlighted"
                );
            }
        }
        assert!(!term.captures_left_drag());
        assert!(recorded.lock().unwrap().is_empty());
        term.mouse(down).unwrap();
        // Handle terminals that report the release but omit a drag event.
        let text = term
            .mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 9,
                row: 4,
                ..down
            })
            .unwrap();
        assert_eq!(text.as_deref(), Some("ABC"));
        assert!(recorded.lock().unwrap().is_empty());

        // The mode switch restores protocol-first mouse gestures for TUIs
        // that need native drag widgets.
        assert_eq!(term.toggle_mouse_routing(), MouseRouting::Native);
        assert_eq!(term.mouse_routing, MouseRouting::Native);

        // Normal drag is fully owned by the foreground TUI. Release remains
        // forwarded even after crossing the terminal viewport.
        term.mouse(down).unwrap();
        term.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 12,
            row: 4,
            ..down
        })
        .unwrap();
        assert!(term.captures_left_drag());
        // The child owns the actual selection and clipboard; Forge only
        // highlights the dragged cells in the embedded terminal display.
        assert_eq!(term.native_highlight, Some(((7, 4), (12, 4))));
        assert!(term.selection.is_none());
        {
            use ratatui::{backend::TestBackend, Terminal};
            let mut ui = Terminal::new(TestBackend::new(80, 20)).unwrap();
            ui.draw(|frame| {
                term.render(
                    frame,
                    Rect::new(0, 0, 80, 20),
                    Color::Reset,
                    Color::Reset,
                    true,
                )
            })
            .unwrap();
            assert!(!ui.backend().buffer()[(6, 4)]
                .modifier
                .contains(Modifier::REVERSED));
            for x in 7..=12 {
                assert!(
                    ui.backend().buffer()[(x, 4)]
                        .modifier
                        .contains(Modifier::REVERSED),
                    "native TUI drag column {x} should have visible feedback"
                );
            }
            assert!(!ui.backend().buffer()[(13, 4)]
                .modifier
                .contains(Modifier::REVERSED));
        }
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 99,
            row: 21,
            ..down
        })
        .unwrap();
        assert!(!term.captures_left_drag());
        assert!(term.selection.is_none());
        assert_eq!(term.native_highlight, Some(((7, 4), (79, 19))));
        assert_eq!(
            &*recorded.lock().unwrap(),
            b"\x1b[<0;8;5M\x1b[<32;13;5M\x1b[<0;80;20m"
        );
        recorded.lock().unwrap().clear();
        // A new click dismisses the visual overlay, without leaking a host
        // copy selection or changing any child mouse protocol bytes.
        term.mouse(down).unwrap();
        assert!(term.native_highlight.is_none());
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..down
        })
        .unwrap();
        assert!(term.native_highlight.is_none());
        recorded.lock().unwrap().clear();

        // Shift+drag explicitly belongs to Forge's text selection. The
        // modifier can be released during the gesture without leaking to TUI.
        let select_down = MouseEvent {
            modifiers: KeyModifiers::SHIFT,
            ..down
        };
        term.mouse(select_down).unwrap();
        term.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 12,
            row: 4,
            ..down
        })
        .unwrap();
        assert!(term.captures_left_drag());
        assert_eq!(term.selection, Some(((7, 4), (12, 4))));
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 99,
            row: 21,
            ..down
        })
        .unwrap();
        assert_eq!(term.selection, Some(((7, 4), (79, 19))));
        assert!(!term.captures_left_drag());
        assert!(
            recorded.lock().unwrap().is_empty(),
            "Shift selection must not inject mouse events into the TUI"
        );

        // Ctrl remains a native mouse modifier, not a mandatory escape hatch.
        let native_down = MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..down
        };
        term.mouse(native_down).unwrap();
        term.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 8,
            ..native_down
        })
        .unwrap();
        term.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 8,
            ..native_down
        })
        .unwrap();
        assert_eq!(
            &*recorded.lock().unwrap(),
            b"\x1b[<16;8;5M\x1b[<48;9;5M\x1b[<16;9;5m"
        );
        term.writer = original_writer;
        term.write_input(b"\x03").unwrap(); // restore shell foreground
    }

    #[tokio::test]
    async fn app_routes_full_terminal_drag_and_release_across_other_panes() {
        // Covers the real App input router: terminal cell selection must keep
        // receiving events even if the pointer crosses the tab bar/sidebar.
        let dir = tempfile::tempdir().unwrap();
        let mut app =
            crate::create_workspace_at(forge_theme::ColorMode::Ansi16, dir.path().to_path_buf());
        let mut terminal =
            EmbeddedTerminal::start(dir.path(), 40, 10, Arc::new(tokio::sync::Notify::new()))
                .unwrap();
        terminal.area = Rect::new(20, 5, 40, 10);
        terminal
            .parser
            .lock()
            .unwrap()
            .process(b"\x1b[3;3HTERMINAL SELECTION");
        app.terminals.active = Some(terminal);
        let send = |app: &mut crate::App, kind: MouseEventKind, column, row| {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        };
        send(&mut app, MouseEventKind::Down(MouseButton::Left), 22, 7);
        send(&mut app, MouseEventKind::Drag(MouseButton::Left), 26, 7);
        assert!(app.terminals.active.as_ref().unwrap().captures_left_drag());
        assert_eq!(
            app.terminals.active.as_ref().unwrap().selection,
            Some(((2, 2), (6, 2)))
        );
        send(&mut app, MouseEventKind::Up(MouseButton::Left), 26, 7);
        assert_eq!(
            app.terminals
                .active
                .as_ref()
                .unwrap()
                .selected_text()
                .as_deref(),
            Some("TERMI")
        );
        assert_eq!(app.session.status, "Terminal selected 5 characters");
        assert!(!app.terminals.active.as_ref().unwrap().captures_left_drag());

        send(&mut app, MouseEventKind::Down(MouseButton::Left), 22, 7);
        send(&mut app, MouseEventKind::Drag(MouseButton::Left), 26, 7);
        // Cross from the terminal into the outer workspace. The original
        // owner keeps the release rather than switching panels or leaking a
        // pressed state into the next interaction.
        send(&mut app, MouseEventKind::Up(MouseButton::Left), 1, 4);
        assert_eq!(
            app.terminals.active.as_ref().unwrap().selection,
            Some(((2, 2), (0, 0)))
        );
        assert!(!app.terminals.active.as_ref().unwrap().captures_left_drag());
        assert!(app.session.status.contains("Terminal selected"));
    }

    #[test]
    fn tui_osc52_clipboard_request_is_preserved_across_pty_chunks() {
        let pending = Arc::new(Mutex::new(None));
        let mut parser = TerminalParser::new_with_callbacks(
            NonZeroU16::new(12).unwrap(),
            NonZeroU16::new(60).unwrap(),
            200,
            TerminalCallbacks {
                reply_writer: std::sync::Weak::new(),
                pending: pending.clone(),
                title: Arc::new(Mutex::new(TabTitle::default())),
                discarded_rows: 0,
            },
        );
        parser.process(b"\x1b]52;c;Rk9SR0VfTkFUSVZFX0NPUFk=");
        assert!(pending.lock().unwrap().is_none());
        parser.process(b"\x07");
        assert_eq!(
            pending.lock().unwrap().take().as_deref(),
            Some(b"Rk9SR0VfTkFUSVZFX0NPUFk=".as_slice())
        );
        // Clipboard requests and terminal-screen parsing are independent.
        parser.process(b"VISIBLE\x1b]52;c;?\x07");
        assert!(parser.screen().contents().contains("VISIBLE"));
        assert!(
            pending.lock().unwrap().is_none(),
            "a clipboard-read request must not overwrite the clipboard"
        );
    }

    #[tokio::test]
    async fn ctrl_c_is_forwarded_to_focused_tui_even_with_host_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app =
            crate::create_workspace_at(forge_theme::ColorMode::Ansi16, dir.path().to_path_buf());
        let mut term =
            EmbeddedTerminal::start(dir.path(), 60, 12, Arc::new(tokio::sync::Notify::new()))
                .unwrap();
        term.area = Rect::new(0, 0, 60, 12);
        term.focused = true;
        term.parser
            .lock()
            .unwrap()
            .process(b"\x1b[1;1HSELECTED CODEX TEXT");
        term.selection = Some(((0, 0), (12, 0)));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let original = std::mem::replace(
            &mut term.writer,
            Arc::new(InputWriter::capture(Box::new(Capture(recorded.clone())))),
        );
        app.terminals.active = Some(term);

        // Forge cannot infer the child's own selection state. Let Codex (or
        // any mouse-aware TUI) handle Ctrl+C without stealing the shortcut.
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(&*recorded.lock().unwrap(), b"\x03");
        assert!(!app.session.should_quit);
        assert!(app.terminals.active.as_ref().unwrap().selection.is_none());
        assert!(app.session.terminal_copy_notice.is_none());

        // Forge's explicit host shortcut copies a host selection without
        // sending anything new to the foreground process.
        app.terminals.active.as_mut().unwrap().selection = Some(((0, 0), (12, 0)));
        app.handle_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(&*recorded.lock().unwrap(), b"\x03");
        assert_eq!(app.session.status, "Terminal selected 13 characters");
        assert!(!app.session.should_quit);
        assert_eq!(
            app.session.terminal_copy_notice.as_ref().unwrap().message,
            "Copied to clipboard"
        );

        let term = app.terminals.active.as_mut().unwrap();
        term.writer = original;
    }

    #[test]
    fn sgr_mouse_encoding_respects_child_mode() {
        let area = Rect::new(5, 6, 80, 20);
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 9,
            modifiers: KeyModifiers::NONE,
        };
        assert!(encode_mouse(
            event,
            area,
            MouseProtocolMode::None,
            MouseProtocolEncoding::Sgr
        )
        .is_none());
        assert_eq!(
            encode_mouse(
                event,
                area,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Sgr
            )
            .unwrap(),
            b"\x1b[<0;4;4M"
        );
        let release = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..event
        };
        assert_eq!(
            encode_mouse(
                release,
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            )
            .unwrap(),
            b"\x1b[<0;4;4m"
        );
        assert!(encode_mouse(
            release,
            area,
            MouseProtocolMode::Press,
            MouseProtocolEncoding::Sgr
        )
        .is_none());
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            ..event
        };
        assert!(encode_mouse(
            drag,
            area,
            MouseProtocolMode::PressRelease,
            MouseProtocolEncoding::Sgr
        )
        .is_none());
        assert_eq!(
            encode_mouse(
                drag,
                area,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr
            )
            .unwrap(),
            b"\x1b[<32;4;4M"
        );
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            ..event
        };
        assert!(encode_mouse(
            moved,
            area,
            MouseProtocolMode::ButtonMotion,
            MouseProtocolEncoding::Sgr
        )
        .is_none());
        assert_eq!(
            encode_mouse(
                moved,
                area,
                MouseProtocolMode::AnyMotion,
                MouseProtocolEncoding::Sgr
            )
            .unwrap(),
            b"\x1b[<35;4;4M"
        );
    }

    #[test]
    fn sgr_button_releases_preserve_button_and_legacy_release_code() {
        let area = Rect::new(5, 6, 80, 20);
        for (button, sgr_code) in [
            (MouseButton::Left, 0u8),
            (MouseButton::Middle, 1),
            (MouseButton::Right, 2),
        ] {
            let release = MouseEvent {
                kind: MouseEventKind::Up(button),
                column: 8,
                row: 9,
                modifiers: KeyModifiers::NONE,
            };
            assert_eq!(
                encode_mouse(
                    release,
                    area,
                    MouseProtocolMode::PressRelease,
                    MouseProtocolEncoding::Sgr
                )
                .unwrap(),
                format!("\x1b[<{sgr_code};4;4m").into_bytes(),
                "SGR release must preserve the released button"
            );
            for encoding in [MouseProtocolEncoding::Default, MouseProtocolEncoding::Utf8] {
                assert_eq!(
                    encode_mouse(release, area, MouseProtocolMode::PressRelease, encoding).unwrap(),
                    b"\x1b[M#$$",
                    "legacy mouse encoding must keep button 3 on release"
                );
            }
        }
        let modified_release = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Right),
            column: 8,
            row: 9,
            modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        };
        assert_eq!(
            encode_mouse(
                modified_release,
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
            )
            .unwrap(),
            b"\x1b[<22;4;4m",
            "release must preserve button identity and modifier bits"
        );
    }

    #[test]
    fn scrollback_selection_stays_on_text_and_extends_across_pages() {
        let cwd = std::env::current_dir().unwrap();
        let mut term =
            EmbeddedTerminal::start(&cwd, 40, 8, Arc::new(tokio::sync::Notify::new())).unwrap();
        term.area = Rect::new(1, 1, 40, 8);
        // A dedicated parser makes the history deterministic independently of
        // asynchronous shell startup output.
        term.parser = Arc::new(Mutex::new(TerminalParser::new_with_callbacks(
            NonZeroU16::new(8).unwrap(),
            NonZeroU16::new(40).unwrap(),
            200,
            TerminalCallbacks {
                reply_writer: std::sync::Weak::new(),
                pending: term.pending_clipboard.clone(),
                title: term.tab_title.clone(),
                discarded_rows: 0,
            },
        )));
        {
            let mut parser = term.parser.lock().unwrap();
            for i in 0..30 {
                parser.process(format!("{i:02}\r\n").as_bytes());
            }
        }
        let event = |kind, row| MouseEvent {
            kind,
            column: 1,
            row,
            modifiers: KeyModifiers::SHIFT,
        };
        term.mouse(event(MouseEventKind::Down(MouseButton::Left), 3))
            .unwrap();
        term.mouse(event(MouseEventKind::Up(MouseButton::Left), 4))
            .unwrap();
        let initial = term.selected_text().unwrap();
        let selection = term.selection;
        term.scroll(-5);
        assert_eq!(term.selection, selection);
        assert_eq!(term.selected_text().as_deref(), Some(initial.as_str()));
        term.scroll_to_bottom();
        assert_eq!(term.selected_text().as_deref(), Some(initial.as_str()));

        // Hold the pointer at the top edge across more than a full viewport.
        term.mouse(event(MouseEventKind::Down(MouseButton::Left), 5))
            .unwrap();
        let anchor = term.selection.unwrap().0;
        term.mouse(event(MouseEventKind::Drag(MouseButton::Left), 1))
            .unwrap();
        for _ in 0..12 {
            term.tick_selection_autoscroll();
        }
        assert_eq!(term.scrollback_position(), 12);
        assert_eq!(term.selection.unwrap().0, anchor);
        assert!(term.selected_text().unwrap().lines().count() > 8);
        term.mouse(event(MouseEventKind::ScrollUp, 1)).unwrap();
        assert_eq!(term.scrollback_position(), 15);
        assert_eq!(term.selection.unwrap().0, anchor);
        let selected = term
            .mouse(event(MouseEventKind::Up(MouseButton::Left), 1))
            .unwrap()
            .unwrap();
        assert!(selected.starts_with("08"), "{selected:?}");
        assert!(!term.selection_autoscroll_active());
        term.scroll_to_bottom();
        assert_eq!(term.selected_text().as_deref(), Some(selected.as_str()));

        // Reverse direction: drag toward the live bottom from history.
        term.scroll(-15);
        term.mouse(event(MouseEventKind::Down(MouseButton::Left), 3))
            .unwrap();
        let anchor = term.selection.unwrap().0;
        term.mouse(event(MouseEventKind::Drag(MouseButton::Left), 8))
            .unwrap();
        for _ in 0..10 {
            term.tick_selection_autoscroll();
        }
        assert_eq!(term.scrollback_position(), 5);
        assert_eq!(term.selection.unwrap().0, anchor);
        assert!(term.selected_text().unwrap().lines().count() > 8);
        term.mouse(event(MouseEventKind::Up(MouseButton::Left), 8))
            .unwrap();
        assert!(!term.selection_autoscroll_active());
    }

    #[test]
    fn synchronized_sequences_survive_every_chunk_boundary_and_false_prefixes() {
        for split in 0..=FrameGate::START.len() {
            let mut gate = FrameGate::default();
            gate.accept(b"plain\x1b[31mred\x1b[?20\x1b");
            gate.accept(&FrameGate::START[..split]);
            gate.accept(&FrameGate::START[split..]);
            assert!(gate.active, "start split at {split}");
            gate.accept(b"large plain output without escapes");
            assert!(gate.active);
            gate.accept(&FrameGate::END[..split]);
            gate.accept(&FrameGate::END[split..]);
            assert!(!gate.active, "end split at {split}");
        }
    }

    #[test]
    fn synchronized_output_hides_incomplete_updates_and_recovers() {
        let mut gate = FrameGate::default();
        assert!(gate.accept(b"ordinary text"));
        assert!(!gate.active);
        assert!(!gate.accept(b"\x1b[?2026h\x1b[2Jpartial"));
        assert!(gate.active);
        assert!(!gate.accept(b" text\x1b[1;1Hstill not done"));
        assert!(gate.active);
        // End sequence deliberately arrives across PTY reads.
        assert!(!gate.accept(b"\x1b[?202"));
        assert!(gate.accept(b"6l"));
        assert!(!gate.active);
        assert!(gate.accept(b"stable"));
        assert!(!gate.accept(b"\x1b[?2026hno end"));
        gate.started = Some(Instant::now() - FrameGate::MAX_HOLD - Duration::from_millis(1));
        assert!(gate.accept(b"new output after broken frame"));
        assert!(
            !gate.active,
            "subsequent output must recover an incomplete frame"
        );
    }

    #[test]
    fn terminal_snapshot_reuses_cells_for_selection_and_resizes_during_sync() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut embedded = EmbeddedTerminal::start(
            &std::env::current_dir().unwrap(),
            30,
            5,
            Arc::new(tokio::sync::Notify::new()),
        )
        .unwrap();
        embedded.paste("exit\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !embedded.ended.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(embedded.ended.load(Ordering::Acquire));
        // Discard shell startup output and its scrollback before testing resize.
        *embedded.parser.lock().unwrap() = TerminalParser::new_with_callbacks(
            NonZeroU16::new(5).unwrap(),
            NonZeroU16::new(30).unwrap(),
            2000,
            TerminalCallbacks {
                reply_writer: std::sync::Weak::new(),
                pending: embedded.pending_clipboard.clone(),
                title: embedded.tab_title.clone(),
                discarded_rows: 0,
            },
        );
        embedded.parser.lock().unwrap().process(b"\x1b[2J\x1b[HOLD");
        embedded.parser_revision.fetch_add(1, Ordering::Release);
        embedded.focused = false;
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let draw = |embedded: &mut EmbeddedTerminal, terminal: &mut Terminal<TestBackend>, area| {
            terminal
                .draw(|frame| embedded.render(frame, area, Color::Reset, Color::Reset, true))
                .unwrap();
        };
        draw(&mut embedded, &mut terminal, Rect::new(0, 0, 30, 5));
        let cells = embedded
            .last_complete_frame
            .as_ref()
            .unwrap()
            .cells
            .as_ptr();
        embedded.selection = Some(((0, 0), (2, 0)));
        embedded.revision.fetch_add(1, Ordering::Release);
        draw(&mut embedded, &mut terminal, Rect::new(0, 0, 30, 5));
        assert_eq!(
            cells,
            embedded
                .last_complete_frame
                .as_ref()
                .unwrap()
                .cells
                .as_ptr()
        );
        assert!(terminal.backend().buffer()[(0, 0)]
            .modifier
            .contains(Modifier::REVERSED));
        assert_eq!(
            embedded.drawn_revision,
            embedded.revision.load(Ordering::Acquire)
        );

        let parser = embedded.parser.clone();
        let sync_update = embedded.sync_update.clone();
        let parser_revision = embedded.parser_revision.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut parser = parser.lock().unwrap();
            ready.send(()).unwrap();
            // Let render begin while output processing holds the parser lock.
            std::thread::sleep(Duration::from_millis(60));
            sync_update.store(true, Ordering::Release);
            parser.process(b"\x1b[2J\x1b[HPARTIAL");
            parser_revision.fetch_add(1, Ordering::Release);
        });
        waiting.recv().unwrap();
        draw(&mut embedded, &mut terminal, Rect::new(0, 0, 30, 5));
        reader.join().unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "O");

        embedded.sync_update.store(true, Ordering::Release);
        embedded
            .parser
            .lock()
            .unwrap()
            .process(b"\x1b[2J\x1b[HPARTIAL");
        embedded.parser_revision.fetch_add(1, Ordering::Release);
        draw(&mut embedded, &mut terminal, Rect::new(0, 0, 40, 8));
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "O");
        embedded.sync_update.store(false, Ordering::Release);
        embedded.revision.fetch_add(1, Ordering::Release);
        draw(&mut embedded, &mut terminal, Rect::new(0, 0, 40, 8));
        assert_eq!(
            terminal.backend().buffer()[(0, 0)].symbol(),
            "P",
            "{}",
            embedded.parser.lock().unwrap().screen().contents()
        );
    }

    #[test]
    fn terminal_sync_frame_snapshot_prevents_partial_scroll_flash() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(&cwd, 30, 5, Arc::new(tokio::sync::Notify::new())).unwrap();
        embedded.focused = false;
        embedded
            .parser
            .lock()
            .unwrap()
            .process(b"\x1b[1;1HOLD FRAME");
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        let draw = |embedded: &mut EmbeddedTerminal, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| {
                    embedded.render(
                        frame,
                        Rect::new(0, 0, 30, 5),
                        Color::Reset,
                        Color::Reset,
                        true,
                    )
                })
                .unwrap();
            (0..30)
                .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
                .collect::<String>()
        };
        assert!(draw(&mut embedded, &mut terminal).starts_with("OLD FRAME"));
        embedded.sync_update.store(true, Ordering::Release);
        embedded
            .parser
            .lock()
            .unwrap()
            .process(b"\x1b[2J\x1b[1;1HNEW FRAME");
        embedded.parser_revision.fetch_add(1, Ordering::Release);
        let partial = draw(&mut embedded, &mut terminal);
        assert!(
            partial.starts_with("OLD FRAME"),
            "mid-update flashed {partial:?}"
        );
        embedded.sync_update.store(false, Ordering::Release);
        embedded.revision.fetch_add(1, Ordering::Release);
        let committed = draw(&mut embedded, &mut terminal);
        assert!(
            committed.starts_with("NEW FRAME"),
            "committed {committed:?}"
        );
    }

    #[test]
    fn terminal_preserves_default_colors_and_markdown_sgr_attributes() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(&cwd, 30, 5, Arc::new(tokio::sync::Notify::new())).unwrap();
        embedded.focused = false;
        // ANSI bold, dim, italic, underline and inverse are common in Codex
        // Markdown headings, emphasis, links and fenced code content.
        embedded.parser.lock().unwrap().process(
            b"\x1b[1;3;4;7mM\x1b[0m\x1b[2mD\x1b[0m \x1b[38;2;80;160;240;48;2;11;22;33mR\x1b[0mN",
        );
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal
            .draw(|frame| {
                embedded.render(
                    frame,
                    Rect::new(0, 0, 30, 5),
                    Color::Reset,
                    Color::Reset,
                    true,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let md = &buf[(0, 0)];
        for modifier in [
            Modifier::BOLD,
            Modifier::ITALIC,
            Modifier::UNDERLINED,
            Modifier::REVERSED,
        ] {
            assert!(
                md.modifier.contains(modifier),
                "missing attribute {modifier:?}"
            );
        }
        assert_eq!(md.fg, Color::Reset);
        assert_eq!(md.bg, Color::Reset);
        let faint = &buf[(1, 0)];
        assert!(faint.modifier.contains(Modifier::DIM));
        assert!(!faint.modifier.contains(Modifier::BOLD));
        let rgb = &buf[(3, 0)];
        assert_eq!(rgb.fg, Color::Rgb(80, 160, 240));
        assert_eq!(rgb.bg, Color::Rgb(11, 22, 33));
        let ordinary = &buf[(4, 0)];
        assert_eq!(ordinary.fg, Color::Reset);
        assert_eq!(ordinary.bg, Color::Reset);
        assert!(!ordinary.modifier.contains(Modifier::REVERSED));
        assert!(!ordinary.modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn terminal_renderer_maps_real_cursor_and_respects_keyboard_ownership() {
        use ratatui::{backend::TestBackend, layout::Position, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(&cwd, 40, 8, Arc::new(tokio::sync::Notify::new())).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let area = Rect::new(10, 6, 40, 8);
        // Isolate shell startup output from the deterministic screen fixture.
        embedded.parser = Arc::new(Mutex::new(TerminalParser::new_with_callbacks(
            NonZeroU16::new(8).unwrap(),
            NonZeroU16::new(40).unwrap(),
            200,
            TerminalCallbacks {
                reply_writer: std::sync::Weak::new(),
                pending: embedded.pending_clipboard.clone(),
                title: embedded.tab_title.clone(),
                discarded_rows: 0,
            },
        )));
        embedded.visible = true;
        embedded.focused = true;
        let draw = |term: &mut EmbeddedTerminal, ui: &mut Terminal<TestBackend>, owns_keyboard| {
            ui.draw(|frame| term.render(frame, area, Color::Black, Color::White, owns_keyboard))
                .unwrap();
        };
        let process = |term: &EmbeddedTerminal, bytes: &[u8]| {
            term.parser.lock().unwrap().process(bytes);
            term.parser_revision.fetch_add(1, Ordering::Release);
        };
        process(&embedded, b"\x1b[?25l\x1b[1;1HSTATUS");
        draw(&mut embedded, &mut terminal, true);
        assert!(!terminal.backend().cursor_visible());
        assert_eq!(terminal.backend().buffer()[(10, 6)].symbol(), "S");
        assert!(!terminal.backend().buffer()[(10, 6)]
            .modifier
            .contains(Modifier::REVERSED));

        // The child cursor is relative to the PTY, including the pane's offset.
        process(&embedded, b"\x1b[?25h\x1b[5;7H");
        draw(&mut embedded, &mut terminal, true);
        assert!(terminal.backend().cursor_visible());
        assert_eq!(terminal.backend().cursor_position(), Position::new(16, 10));
        assert!(!terminal.backend().buffer()[(16, 10)]
            .modifier
            .contains(Modifier::REVERSED));
        draw(&mut embedded, &mut terminal, false);
        assert!(!terminal.backend().cursor_visible());

        embedded.focused = false;
        draw(&mut embedded, &mut terminal, true);
        assert!(!terminal.backend().cursor_visible());
        embedded.focused = true;
        process(&embedded, b"\x1b[8;1H\r\n\r\n\r\n");
        embedded.scroll(-2);
        draw(&mut embedded, &mut terminal, true);
        assert!(!terminal.backend().cursor_visible());
        embedded.scroll_to_bottom();
        draw(&mut embedded, &mut terminal, true);
        assert!(terminal.backend().cursor_visible());
        assert_eq!(terminal.backend().cursor_position(), Position::new(10, 13));

        // Resize/reposition the terminal and translate from the new origin.
        terminal
            .draw(|frame| {
                embedded.render(
                    frame,
                    Rect::new(2, 3, 30, 6),
                    Color::Black,
                    Color::White,
                    true,
                );
            })
            .unwrap();
        process(&embedded, b"\x1b[3;5H");
        terminal
            .draw(|frame| {
                embedded.render(
                    frame,
                    Rect::new(2, 3, 30, 6),
                    Color::Black,
                    Color::White,
                    true,
                );
            })
            .unwrap();
        assert_eq!(terminal.backend().cursor_position(), Position::new(6, 5));
    }

    #[test]
    fn terminal_renderer_waits_for_inflight_parser_instead_of_blank_frame() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let mut embedded =
            EmbeddedTerminal::start(&cwd, 80, 12, Arc::new(tokio::sync::Notify::new())).unwrap();
        {
            embedded
                .parser
                .lock()
                .unwrap()
                .process(b"VISIBLE_AFTER_LOCK");
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
        terminal
            .draw(|frame| {
                embedded.render(
                    frame,
                    Rect::new(0, 0, 80, 12),
                    Color::Black,
                    Color::White,
                    true,
                );
            })
            .unwrap();
        lock_holder.join().unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("VISIBLE_AFTER_LOCK"), "{content}");
    }

    #[test]
    fn burst_output_is_bounded_and_shell_exits() {
        let cwd = std::env::current_dir().unwrap();
        let mut terminal =
            EmbeddedTerminal::start(&cwd, 100, 14, Arc::new(tokio::sync::Notify::new())).unwrap();
        // Output exceeds vt100's 2,000-line history many times over.
        terminal
            .paste("python3 -c 'for i in range(20000): print(i)'\nexit\n")
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !terminal.ended.load(Ordering::Acquire) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        assert!(
            terminal.ended.load(Ordering::Acquire),
            "output flood prevented shell exit"
        );
        let parser = terminal.parser.lock().unwrap();
        let screen = parser.screen();
        let visible = screen
            .rows_formatted(0, screen.size().1.get())
            .collect::<String>();
        assert!(
            visible.contains("19999") || visible.contains("exit"),
            "tail not rendered"
        );
    }

    #[test]
    fn exited_shell_notifies_and_rejects_more_input() {
        let cwd = std::env::current_dir().unwrap();
        let notify = Arc::new(tokio::sync::Notify::new());
        let mut terminal = EmbeddedTerminal::start(&cwd, 80, 12, notify).unwrap();
        terminal.paste("exit\n").unwrap();
        let ended = (0..100).any(|_| {
            if terminal.ended.load(Ordering::Acquire) {
                return true;
            }
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
        let mut terminal =
            EmbeddedTerminal::start(&cwd, 80, 12, Arc::new(tokio::sync::Notify::new())).unwrap();
        for ch in "printf FORGE_PTY_SMOKE_TEST".chars() {
            terminal
                .send(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
                .unwrap();
        }
        terminal
            .send(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let found = (0..80).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(25));
            terminal
                .parser
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("FORGE_PTY_SMOKE_TEST")
        });
        assert!(found, "shell output never arrived at the VT parser");
        terminal.resize(60, 9);
        assert_eq!(terminal.size, (60, 9));
    }

    #[tokio::test]
    async fn decset_2026_unfinished_frame_expires_when_pty_is_idle() {
        use ratatui::{backend::TestBackend, Terminal};
        let cwd = std::env::current_dir().unwrap();
        let notify = Arc::new(tokio::sync::Notify::new());
        let mut embedded = EmbeddedTerminal::start(&cwd, 50, 6, notify.clone()).unwrap();
        embedded.focused = false;
        let mut ui = Terminal::new(TestBackend::new(50, 6)).unwrap();

        // Emit DECSET 2026 + text, then block without sending DECRST 2026 or any
        // subsequent PTY output.
        embedded
            .write_input(b"python3 -c 'import sys,time; sys.stdout.write(\"\\x1b[?2026hIDLE_SYNC_VISIBLE\"); sys.stdout.flush(); time.sleep(2)'\r")
            .unwrap();

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                notify.notified().await;
                ui.draw(|frame| {
                    embedded.render(
                        frame,
                        Rect::new(0, 0, 50, 6),
                        Color::Reset,
                        Color::Reset,
                        true,
                    )
                })
                .unwrap();
                let screen = (0..6)
                    .map(|y| {
                        (0..50)
                            .map(|x| ui.backend().buffer()[(x, y)].symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if screen.contains("IDLE_SYNC_VISIBLE")
                    && !embedded.sync_update.load(Ordering::Acquire)
                {
                    break;
                }
            }
        })
        .await
        .expect("idle DECSET 2026 frame must auto-expire and notify UI within timeout");
    }
}

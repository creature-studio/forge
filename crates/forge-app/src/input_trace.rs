//! Opt-in diagnostic of navigation and pointer events.
//! FORGE_INPUT_TRACE=1 logs to the OS temporary directory.
//! No ordinary typing, prompt text, file contents or paths are recorded.
use super::{ActivityAction, ActivityTab, App, PaneFocus};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::io::Write;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NavState {
    activity: ActivityTab,
    focus: PaneFocus,
    sidebar: bool,
    pressed: Option<ActivityAction>,
}

pub(super) fn nav_state(app: &App) -> NavState {
    NavState {
        activity: app.session.activity,
        focus: app.session.focus,
        sidebar: app.session.sidebar_visible,
        pressed: app.session.activity_pressed,
    }
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FORGE_INPUT_TRACE").ok().as_deref() == Some("1"))
}

pub(super) fn log(message: &str) {
    if !enabled() {
        return;
    }
    let path = std::env::temp_dir().join(format!("forge-input-trace-{}.log", std::process::id()));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512_000) {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(path) {
        let _ = writeln!(
            file,
            "{} {message}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
    }
}

pub(super) fn event(event: &Event, before: NavState, after: NavState) {
    if !enabled() {
        return;
    }
    let kind = match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('c' | 'C')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT)
                {
                    "key:Ctrl+Shift+C"
                } else {
                    "key:Ctrl+C"
                }
            }
            KeyCode::Esc => "key:Esc",
            KeyCode::F(1) => "key:F1",
            KeyCode::F(2) => "key:F2",
            KeyCode::Tab | KeyCode::BackTab => "key:Tab",
            KeyCode::Char('g') => "key:unbound-g",
            KeyCode::Char('G') => "key:unbound-G",
            KeyCode::Char('[') => "key:unbound-[",
            KeyCode::Char(']') => "key:unbound-]",
            _ => "key:other",
        }
        .to_owned(),
        Event::Mouse(m) => format!("mouse:{:?}@{},{}", m.kind, m.column, m.row),
        Event::Paste(_) => "paste".to_owned(),
        _ => "other-event".to_owned(),
    };
    if kind == "key:other" && before == after {
        return;
    }
    log(&format!("input {kind} {before:?} -> {after:?}"));
}

pub(super) fn render(app: &App) {
    if enabled() {
        log(&format!("render {:?}", nav_state(app)));
    }
}

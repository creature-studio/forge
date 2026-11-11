//! Named bindings shared by dispatch, shortcut settings and key labels.
use super::*;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::collections::BTreeMap;

struct Binding {
    id: &'static str,
    name: &'static str,
    editor: bool,
    default: &'static str,
    canonical: &'static str,
}
macro_rules! binding {
    ($id:literal,$name:literal,$editor:literal,$default:literal,$canonical:literal) => {
        Binding {
            id: $id,
            name: $name,
            editor: $editor,
            default: $default,
            canonical: $canonical,
        }
    };
}
static BINDINGS: &[Binding] = &[
    binding!("edit", "Edit file", false, "Ctrl+e", "e"),
    binding!("open_file", "Open file in Files", false, "o", "o"),
    binding!("settings", "Settings", false, ",", ","),
    binding!("find_file", "Find file", false, "Ctrl+p", "Ctrl+p"),
    binding!("find_content", "Search contents", false, "Ctrl+f", "Ctrl+f"),
    binding!("commands", "Command palette", false, ":", ":"),
    binding!(
        "commands_alt",
        "Command palette (alternative)",
        false,
        "Ctrl+k",
        "Ctrl+k"
    ),
    binding!("refresh", "Refresh workspace", false, "r", "r"),
    binding!("files_tab", "Files tab", false, "F1", "F1"),
    binding!("git_tab", "Git tab", false, "F2", "F2"),
    binding!("focus_pane", "Switch pane", false, "Tab", "Tab"),
    binding!("shortcuts", "Shortcut settings", false, "F4", "F4"),
    binding!("open_item", "Open / expand", false, "Enter", "Enter"),
    binding!("back", "Back / cancel", false, "Esc", "Esc"),
    binding!("move_up", "Move up", false, "k", "k"),
    binding!("move_down", "Move down", false, "j", "j"),
    binding!("move_parent", "Go to parent", false, "h", "h"),
    binding!("move_child", "Open child", false, "l", "l"),
    binding!("preview_mode", "Markdown Preview / Code", false, "F3", "F3"),
    binding!("unstaged_group", "Jump to changes", false, "[", "["),
    binding!("staged_group", "Jump to staged", false, "]", "]"),
    binding!("toggle_git", "Toggle Files / Git", false, "g", "g"),
    binding!("help", "Help", false, "?", "?"),
    binding!("quit", "Quit workspace", false, "q", "q"),
    binding!(
        "ctrl_c_guard",
        "Ctrl-C never exits",
        false,
        "Ctrl+c",
        "Ctrl+c"
    ),
    binding!("selection", "Toggle selection", false, "v", "v"),
    binding!("selection_all", "Select visible files", false, "V", "V"),
    binding!("commit", "Commit staged changes", false, "c", "c"),
    binding!("stage", "Stage selection / hunk", false, "s", "s"),
    binding!("unstage", "Unstage selection / hunk", false, "u", "u"),
    binding!("discard", "Discard changes", false, "x", "x"),
    binding!("diff", "View selected diff", false, "d", "d"),
    binding!("pull", "Pull repository", false, "p", "p"),
    binding!("push", "Push repository", false, "P", "P"),
    binding!("history", "Show commit history", false, "G", "G"),
    binding!(
        "line_commit",
        "Line blame / commit details",
        false,
        "F6",
        "F6"
    ),
    binding!("file_history", "File history", false, "H", "H"),
    binding!("inline_peek", "Source change peek", false, "F7", "F7"),
    binding!("commit_history", "Commit history dialog", false, "L", "L"),
    binding!("external_editor", "External editor", false, "E", "E"),
    binding!("filters", "File filters", false, "F", "F"),
    binding!("branches", "Branches", false, "B", "B"),
    binding!("stashes", "Stashes", false, "Z", "Z"),
    binding!("sync", "Remote synchronization", false, "S", "S"),
    binding!("worktrees", "Worktrees", false, "W", "W"),
    binding!("lines", "Diff line actions", false, "I", "I"),
    binding!("save", "Save", true, "Ctrl+s", "Ctrl+s"),
    binding!("copy", "Copy selection / line", true, "Ctrl+c", "Ctrl+c"),
    binding!("cut", "Cut selection / line", true, "Ctrl+x", "Ctrl+x"),
    binding!("paste", "Paste", true, "Ctrl+v", "Ctrl+v"),
    binding!("select_all", "Select all", true, "Ctrl+a", "Ctrl+a"),
    binding!("undo", "Undo", true, "Ctrl+z", "Ctrl+z"),
    binding!("redo", "Redo", true, "Ctrl+y", "Ctrl+y"),
];

const GROUPS: [&str; 5] = ["All", "Files", "Git", "Editor", "Navigation"];

fn category(b: &Binding) -> &'static str {
    if b.editor {
        return "Editor";
    }
    match b.id {
        "edit" | "find_file" | "find_content" | "external_editor" | "filters" | "selection"
        | "selection_all" | "preview_mode" | "open_file" | "inline_peek" => "Files",
        "history" | "line_commit" | "file_history" | "commit_history" | "branches" | "stashes"
        | "sync" | "worktrees" | "lines" | "commit" | "stage" | "unstage" | "discard" | "diff"
        | "pull" | "push" | "unstaged_group" | "staged_group" => "Git",
        "files_tab" | "git_tab" | "focus_pane" | "toggle_git" | "help" | "quit" | "shortcuts"
        | "open_item" | "back" | "move_up" | "move_down" | "move_parent" | "move_child" => {
            "Navigation"
        }
        _ => "Navigation",
    }
}

// The keys required to escape a modal, navigate panes and exit deliberately are
// fixed; exposing them in F4 is useful, but rebinding could strand the user.
fn fixed(b: &Binding) -> bool {
    matches!(
        b.id,
        "files_tab"
            | "git_tab"
            | "focus_pane"
            | "help"
            | "quit"
            | "shortcuts"
            | "open_item"
            | "back"
            | "move_up"
            | "move_down"
            | "move_parent"
            | "move_child"
            | "preview_mode"
            | "unstaged_group"
            | "staged_group"
            | "ctrl_c_guard"
            | "settings"
    )
}

pub(super) struct Keybindings {
    pub open: bool,
    overrides: BTreeMap<String, String>,
    query: String,
    selected: usize,
    group: usize,
    recording: bool,
    pub message: String,
    path: Option<PathBuf>,
}
impl Default for Keybindings {
    fn default() -> Self {
        #[allow(unused_mut)]
        let mut state = Self {
            open: false,
            overrides: BTreeMap::new(),
            query: String::new(),
            selected: 0,
            group: 0,
            recording: false,
            message: String::new(),
            path: None,
        };
        #[cfg(not(test))]
        {
            state.path = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
                .map(|p| p.join("forge/keybindings.conf"));
            if let Some(path) = &state.path {
                match std::fs::read_to_string(path) {
                    Ok(text) => {
                        let mut loaded = BTreeMap::new();
                        let result = (|| -> Result<(), String> {
                            for line in text
                                .lines()
                                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                            {
                                let (id, chord) =
                                    line.split_once('=').ok_or("Invalid keybinding line")?;
                                if !BINDINGS.iter().any(|b| b.id == id.trim()) {
                                    return Err(format!("Unknown action: {id}"));
                                }
                                parse(chord.trim())
                                    .ok_or_else(|| format!("Invalid shortcut: {chord}"))?;
                                loaded.insert(id.trim().into(), chord.trim().into());
                            }
                            validate(&loaded)?;
                            Ok(())
                        })();
                        match result {
                            Ok(()) => state.overrides = loaded,
                            Err(e) => state.message = format!("Bindings not loaded: {e}"),
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => state.message = format!("Bindings not loaded: {e}"),
                }
            }
        }
        state
    }
}
fn parse(chord: &str) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let mut tail = chord;
    loop {
        if let Some(rest) = tail.strip_prefix("Ctrl+") {
            mods |= KeyModifiers::CONTROL;
            tail = rest;
        } else if let Some(rest) = tail.strip_prefix("Alt+") {
            mods |= KeyModifiers::ALT;
            tail = rest;
        } else if let Some(rest) = tail.strip_prefix("Shift+") {
            mods |= KeyModifiers::SHIFT;
            tail = rest;
        } else {
            break;
        }
    }
    let code = if tail.starts_with('F') && tail.len() > 1 {
        KeyCode::F(tail[1..].parse().ok()?)
    } else if tail == "Tab" {
        KeyCode::Tab
    } else if tail == "Enter" {
        KeyCode::Enter
    } else if tail == "Esc" {
        KeyCode::Esc
    } else {
        let mut chars = tail.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        KeyCode::Char(c)
    };
    Some(KeyEvent::new(code, mods))
}
fn same(a: KeyEvent, b: KeyEvent) -> bool {
    let normalized = |mut k: KeyEvent| {
        if let KeyCode::Char(c) = k.code {
            if c.is_ascii_uppercase() || c.is_ascii_punctuation() {
                k.modifiers.remove(KeyModifiers::SHIFT);
            }
        }
        (k.code, k.modifiers)
    };
    normalized(a) == normalized(b)
}
fn chord(k: KeyEvent) -> Option<String> {
    let key = match k.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => return None,
    };
    let mut out = String::new();
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("Ctrl+");
    }
    if k.modifiers.contains(KeyModifiers::ALT) {
        out.push_str("Alt+");
    }
    if k.modifiers.contains(KeyModifiers::SHIFT)
        && !matches!(k.code,KeyCode::Char(c) if c.is_ascii_uppercase() || c.is_ascii_punctuation())
    {
        out.push_str("Shift+");
    }
    out.push_str(&key);
    Some(out)
}
fn effective<'a>(b: &Binding, map: &'a BTreeMap<String, String>) -> &'a str {
    map.get(b.id).map(String::as_str).unwrap_or(b.default)
}
fn allowed(k: KeyEvent) -> bool {
    match k.code {
        KeyCode::F(n) => (5..=12).contains(&n),
        KeyCode::Char(c) => {
            (k.modifiers.contains(KeyModifiers::CONTROL) || k.modifiers.contains(KeyModifiers::ALT))
                && !matches!(c.to_ascii_lowercase(), 'q' | 'i' | 'm' | 'h' | 'j' | '[')
                && c != ' '
        }
        _ => false,
    }
}
// New defaults preserve older explicit assignments to those chords.
fn shadowed_alias(b: &Binding, map: &BTreeMap<String, String>) -> bool {
    matches!(b.id, "commands_alt" | "line_commit" | "inline_peek")
        && !map.contains_key(b.id)
        && BINDINGS.iter().any(|other| {
            other.id != b.id
                && !other.editor
                && map.contains_key(other.id)
                && same(
                    parse(effective(other, map)).expect("validated chord"),
                    parse(b.default).unwrap(),
                )
        })
}

fn validate(map: &BTreeMap<String, String>) -> Result<(), String> {
    for (i, b) in BINDINGS.iter().enumerate() {
        let key = parse(effective(b, map)).ok_or("Invalid shortcut")?;
        if map.contains_key(b.id) && effective(b, map) != b.default {
            if fixed(b) {
                return Err(format!("{} is a fixed navigation shortcut", b.name));
            }
            if !allowed(key) || same(key, parse("Ctrl+c").unwrap()) {
                return Err(
                    "Use Ctrl/Alt + letter or F5–F12; Ctrl-C, navigation, Esc and F4 are reserved"
                        .into(),
                );
            }
        }
        for other in &BINDINGS[..i] {
            if b.editor == other.editor && same(key, parse(effective(other, map)).unwrap()) {
                if shadowed_alias(b, map) || shadowed_alias(other, map) {
                    continue;
                }
                return Err(format!("Already bound to {}", other.name));
            }
        }
    }
    Ok(())
}
impl Keybindings {
    pub fn remap(&self, key: KeyEvent, editor: bool) -> Option<KeyEvent> {
        let configured = BINDINGS.iter().find(|b| {
            b.editor == editor
                && self.overrides.contains_key(b.id)
                && same(key, parse(effective(b, &self.overrides)).unwrap())
        });
        let active = configured.or_else(|| {
            BINDINGS.iter().find(|b| {
                b.editor == editor
                    && !shadowed_alias(b, &self.overrides)
                    && same(key, parse(effective(b, &self.overrides)).unwrap())
            })
        });
        if let Some(b) = active {
            return parse(b.canonical);
        }
        if BINDINGS.iter().any(|b| {
            b.editor == editor
                && (same(key, parse(b.canonical).unwrap()) || same(key, parse(b.default).unwrap()))
        }) {
            return None;
        }
        Some(key)
    }
    pub fn label(&self, default: &str) -> String {
        let normalized = default.replace("Ctrl-", "Ctrl+");
        BINDINGS
            .iter()
            .find(|b| !b.editor && (b.canonical == normalized || b.default == normalized))
            .map(|b| {
                let raw = effective(b, &self.overrides);
                if let Some((prefix, tail)) = raw.rsplit_once('+') {
                    format!("{}-{}", prefix.replace('+', "-"), tail.to_uppercase())
                } else {
                    raw.to_owned()
                }
            })
            .unwrap_or_else(|| default.into())
    }
    pub fn action_key(&self, id: &str) -> &str {
        let binding = BINDINGS.iter().find(|b| b.id == id).expect("known action");
        effective(binding, &self.overrides)
    }
    pub fn command_label(&self, label: &str) -> String {
        if let Some((text, key)) = label.rsplit_once("  ") {
            return format!("{}  {}", text, self.label(key.trim()));
        }
        label.into()
    }
    pub fn editor_help(&self) -> String {
        ["save", "copy", "cut", "paste", "undo", "redo"]
            .iter()
            .map(|id| {
                let b = BINDINGS.iter().find(|b| b.id == *id).unwrap();
                format!("{} {}", effective(b, &self.overrides), b.name)
            })
            .collect::<Vec<_>>()
            .join(" · ")
            + " · Esc Close · F4 Shortcuts"
    }
    fn rows(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        BINDINGS
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                let group = GROUPS[self.group];
                (group == "All" || category(b) == group)
                    && format!(
                        "{} {} {}",
                        category(b),
                        b.name,
                        effective(b, &self.overrides)
                    )
                    .to_lowercase()
                    .contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn update(&mut self, id: &str, value: Option<String>) -> Result<(), String> {
        let mut next = self.overrides.clone();
        if let Some(v) = value {
            next.insert(id.into(), v);
        } else {
            next.remove(id);
        }
        validate(&next)?;
        let path = self
            .path
            .as_ref()
            .ok_or("No configuration directory available")?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let text = format!(
            "# Forge keyboard shortcuts. F4 opens settings.\n{}",
            next.iter()
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect::<String>()
        );
        let temp = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
        self.overrides = next;
        Ok(())
    }
    pub fn handle(&mut self, key: KeyEvent) {
        if self.recording {
            if key.code == KeyCode::Esc {
                self.recording = false;
                self.message = "Binding cancelled".into();
                return;
            }
            let rows = self.rows();
            let Some(&idx) = rows.get(self.selected) else {
                self.recording = false;
                return;
            };
            let Some(value) = chord(key) else {
                self.message = "Use Ctrl/Alt + letter or F5–F12".into();
                return;
            };
            let id = BINDINGS[idx].id;
            if fixed(&BINDINGS[idx]) {
                self.recording = false;
                self.message = "This navigation shortcut is fixed".into();
                return;
            }
            match self.update(id, Some(value)) {
                Ok(()) => {
                    self.recording = false;
                    self.message = "Saved · active now and after restart".into();
                }
                Err(e) => self.message = e,
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::F(4) => self.open = false,
            KeyCode::Tab | KeyCode::BackTab => {
                self.group = if key.code == KeyCode::BackTab {
                    (self.group + GROUPS.len() - 1) % GROUPS.len()
                } else {
                    (self.group + 1) % GROUPS.len()
                };
                self.selected = 0;
                self.message.clear();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.rows().len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(&i) = self.rows().get(self.selected) {
                    if fixed(&BINDINGS[i]) {
                        self.message = "Fixed navigation shortcut · cannot rebind".into();
                    } else {
                        self.recording = true;
                        self.message = "Press Ctrl/Alt + letter or F5–F12 · Esc cancel".into();
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(&i) = self.rows().get(self.selected) {
                    self.message = if fixed(&BINDINGS[i]) {
                        "Fixed navigation shortcut".into()
                    } else {
                        match self.update(BINDINGS[i].id, None) {
                            Ok(()) => "Default restored".into(),
                            Err(e) => e,
                        }
                    };
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
    }
}
pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, keys: &Keybindings, p: Palette) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Keyboard shortcuts · F4 ")
        .title_bottom(" Tab Group · ↑/↓ Select · Enter Rebind · Del Reset · Esc Close ")
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = inner.height.saturating_sub(5) as usize;
    let mut groups = Vec::new();
    for (i, group) in GROUPS.iter().enumerate() {
        groups.push(Span::styled(
            format!(" {} ", group),
            Style::default()
                .fg(if keys.group == i {
                    p.on_accent
                } else {
                    p.fg_dim
                })
                .bg(if keys.group == i { p.selection } else { p.bg })
                .add_modifier(if keys.group == i {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        groups.push(Span::raw(" "));
    }
    let mut lines = vec![
        Line::from(groups),
        Line::from(format!("Search: {}   [Tab: filter group]", keys.query)),
        Line::from("  CONTEXT       ACTION                              SHORTCUT"),
    ];
    let rows = keys.rows();
    let start = keys.selected.saturating_sub(height.saturating_sub(1));
    for (index, &id) in rows.iter().enumerate().skip(start).take(height) {
        let b = &BINDINGS[id];
        let selected = index == keys.selected;
        let base = if selected {
            Style::default().bg(p.selection).fg(p.on_accent)
        } else {
            Style::default().fg(p.fg)
        };
        let key = effective(b, &keys.overrides);
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{} {:12} {:35} ",
                    if selected { "›" } else { " " },
                    category(b),
                    b.name
                ),
                base,
            ),
            Span::styled(
                format!(" {key} "),
                base.fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if fixed(b) {
                    "  fixed"
                } else if shadowed_alias(b, &keys.overrides) {
                    "  assigned elsewhere"
                } else if keys.overrides.contains_key(b.id) {
                    "  modified"
                } else {
                    ""
                },
                base,
            ),
        ]));
    }
    if rows.is_empty() {
        lines.push(Line::from("No matching shortcuts"));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height > 1 {
        frame.render_widget(
            Paragraph::new(&*keys.message).style(Style::default().fg(p.warning)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebind_conflicts_persistence_reset_and_old_key_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let mut keys = Keybindings::default();
        keys.path = Some(dir.path().join("keys.conf"));
        assert!(same(
            keys.remap(parse("Ctrl+e").unwrap(), false).unwrap(),
            parse("e").unwrap()
        ));
        assert!(keys.remap(parse("e").unwrap(), false).is_none());
        assert!(keys.update("edit", Some("Ctrl+p".into())).is_err());
        assert!(!keys.path.as_ref().unwrap().exists());
        keys.update("edit", Some("F6".into())).unwrap();
        assert!(std::fs::read_to_string(keys.path.as_ref().unwrap())
            .unwrap()
            .contains("edit=F6"));
        assert!(keys.remap(parse("Ctrl+e").unwrap(), false).is_none());
        assert!(same(
            keys.remap(parse("F6").unwrap(), false).unwrap(),
            parse("e").unwrap()
        ));
        // Identical chords in independent editor and workspace contexts are valid.
        keys.update("save", Some("F6".into())).unwrap();
        assert!(same(
            keys.remap(parse("F6").unwrap(), true).unwrap(),
            parse("Ctrl+s").unwrap()
        ));
        assert!(keys.update("edit", Some("F4".into())).is_err());
        assert!(keys.update("edit", Some("Ctrl+q".into())).is_err());
        keys.update("edit", None).unwrap();
        assert!(keys.remap(parse("Ctrl+e").unwrap(), false).is_some());
    }
    #[test]
    fn expanded_action_registry_preserves_alias_and_rebinding_contract() {
        let dir = tempfile::tempdir().unwrap();
        let mut keys = Keybindings::default();
        keys.path = Some(dir.path().join("forge.conf"));
        assert!(same(
            keys.remap(parse("Ctrl+k").unwrap(), false).unwrap(),
            parse("Ctrl+k").unwrap()
        ));
        keys.update("stage", Some("F6".into())).unwrap();
        assert!(keys.remap(parse("s").unwrap(), false).is_none());
        assert!(same(
            keys.remap(parse("F6").unwrap(), false).unwrap(),
            parse("s").unwrap()
        ));
        assert!(keys.update("unstage", Some("F6".into())).is_err());
        assert!(keys.update("discard", Some("Ctrl+c".into())).is_err());
        assert!(keys.update("files_tab", Some("F8".into())).is_err());
        // Existing custom Ctrl-K bindings should not invalidate the whole
        // config when the optional new palette alias is introduced.
        keys.update("stage", Some("Ctrl+k".into())).unwrap();
        assert!(same(
            keys.remap(parse("Ctrl+k").unwrap(), false).unwrap(),
            parse("s").unwrap()
        ));
        keys.update("stage", Some("F6".into())).unwrap();
        keys.update("commands_alt", Some("F7".into())).unwrap();
        assert!(keys.remap(parse("Ctrl+k").unwrap(), false).is_none());
        assert!(same(
            keys.remap(parse("F7").unwrap(), false).unwrap(),
            parse("Ctrl+k").unwrap()
        ));
        let config = std::fs::read_to_string(keys.path.as_ref().unwrap()).unwrap();
        assert!(config.contains("stage=F6") && config.contains("commands_alt=F7"));
    }

    #[test]
    fn grouped_shortcuts_show_fixed_navigation_without_capturing_them() {
        let mut keys = Keybindings::default();
        keys.open = true;
        keys.handle(parse("Tab").unwrap());
        assert_eq!(keys.group, 1);
        assert!(keys
            .rows()
            .iter()
            .all(|&i| category(&BINDINGS[i]) == "Files"));
        keys.handle(parse("Tab").unwrap());
        assert_eq!(keys.group, 2);
        assert!(keys.rows().iter().all(|&i| category(&BINDINGS[i]) == "Git"));
        keys.handle(parse("Tab").unwrap());
        assert_eq!(keys.group, 3);
        assert!(keys
            .rows()
            .iter()
            .all(|&i| category(&BINDINGS[i]) == "Editor"));
        keys.handle(parse("Tab").unwrap());
        assert_eq!(keys.group, 4);
        let fixed_i = keys
            .rows()
            .iter()
            .position(|&i| BINDINGS[i].id == "focus_pane")
            .unwrap();
        keys.selected = fixed_i;
        keys.handle(parse("Enter").unwrap());
        assert!(!keys.recording);
        assert!(keys.message.contains("Fixed"));
    }

    #[tokio::test]
    async fn ctrl_c_in_context_menu_is_safe_and_ctrl_k_opens_commands() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one.txt"), "one").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.files.children.insert(
            dir.path().to_owned(),
            vec![Entry {
                id: forge_model::PathId(42),
                name: "one.txt".into(),
                path: dir.path().join("one.txt"),
                kind: EntryKind::File,
            }],
        );
        app.rebuild_visible();
        app.open_file_context_menu(0, 5, 5);
        assert!(app.session.context_menu.is_some());
        app.handle_key(parse("Ctrl+c").unwrap());
        assert!(!app.session.should_quit);
        assert!(app.session.context_menu.is_none());
        app.handle_key(parse("Ctrl+k").unwrap());
        assert!(app
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, tools::PanelKind::Commands)));
    }

    #[test]
    fn search_capture_cancel_and_highlight_render() {
        let mut keys = Keybindings::default();
        keys.open = true;
        for c in "Edit file".chars() {
            keys.handle(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(keys.rows(), vec![0]);
        keys.handle(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(keys.recording);
        keys.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!keys.recording);
        assert!(keys.open);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 15)).unwrap();
        term.draw(|f| draw(f, f.area(), &keys, Palette::default()))
            .unwrap();
        assert!(term
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.fg == Palette::default().accent && c.modifier.contains(Modifier::BOLD)));
        // Tiny terminals must remain safe.
        let mut tiny = ratatui::Terminal::new(ratatui::backend::TestBackend::new(2, 2)).unwrap();
        tiny.draw(|f| draw(f, f.area(), &keys, Palette::default()))
            .unwrap();
    }
    #[tokio::test]
    async fn ctrl_e_opens_editor_and_shortcuts_do_not_edit_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "original").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        tokio::time::timeout(Duration::from_secs(3), async {
            while !app.files.pending_lists.is_empty() {
                app.poll_pending_list();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        app.handle_key(parse("Ctrl+e").unwrap());
        assert!(app.editing.active());
        app.handle_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
        assert!(app.keys.open);
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.keys.query, "s");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    }
}

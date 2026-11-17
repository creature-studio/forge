//! Kit components. Workspace state is scoped to this tree; only the palette is
//! process-wide. Draft text lives exclusively in the mounted commit editor.
use super::*;
use crossterm::event::{Event, KeyEventKind};
use forge_ui::{
    draw_file_list, draw_preview, draw_scm_sidebar, FileListState, PreviewView, StagingState,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use ratatui_kit::{Component, ComponentDrawer, ComponentUpdater, Context};
use tui_input::backend::crossterm::EventHandler;
mod terminal_tabs;

// Keep the terminal's own foreground and background. Accent belongs to small
// focus marks; selection is a quiet neutral surface, like NSH's TUI controls.
fn workspace_palette() -> Palette {
    let mut palette = Palette::default();
    // Never inherit the terminal profile background. Ubuntu's default purple
    // otherwise leaks through empty header/footer/sidebar cells, while code
    // panes paint their own dark background.
    palette.bg = Color::Rgb(16, 18, 24);
    palette.selection = Color::DarkGray;
    palette.on_accent = Color::White;
    palette
}

/// Theme presets reuse the existing Ratatui Kit palette and persist via Config.theme.
/// Keep the original palette as the backwards-compatible default.
fn theme_palette(name: Option<&str>) -> Palette {
    let mut p = workspace_palette();
    let (bg, _surface, fg, dim, accent, border, selection) = match name.unwrap_or("current") {
        "vscode-dark" => (
            0x1e1e1e, 0x252526, 0xd4d4d4, 0x858585, 0x569cd6, 0x3c3c3c, 0x264f78,
        ),
        "vscode-black" => (
            0x080808, 0x151515, 0xd4d4d4, 0x858585, 0x569cd6, 0x333333, 0x223852,
        ),
        "github-dark" => (
            0x0d1117, 0x161b22, 0xe6edf3, 0x8b949e, 0x58a6ff, 0x30363d, 0x264f78,
        ),
        "nord" => (
            0x2e3440, 0x3b4252, 0xeceff4, 0x9aa6b2, 0x88c0d0, 0x4c566a, 0x434c5e,
        ),
        _ => return p,
    };
    let color = |hex: u32| Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    p.bg = color(bg);
    p.fg = color(fg);
    p.fg_dim = color(dim);
    p.accent = color(accent);
    p.border = color(border);
    p.border_active = color(accent);
    p.selection = color(selection);
    p.on_accent = color(fg);
    p.success = color(0x4ec9b0);
    p.warning = color(0xdcdcaa);
    p.error = color(0xf48771);
    p.info = color(accent);
    // Surface is used by the sidebar/header; the source preview retains `bg`.
    p
}

#[derive(Default, Props)]
pub struct WorkspaceProps {
    pub color_mode: ColorMode,
    pub(super) initial: Option<App>,
    pub(super) editor_handoff:
        Option<std::sync::Arc<std::sync::Mutex<Option<tools::EditorResume>>>>,
}

#[component]
pub fn Workspace(props: &mut WorkspaceProps, mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    // Route Ctrl-C through App so an editor copy cannot bypass dirty-close handling.
    hooks
        .use_context_mut::<ratatui_kit::SystemContext>()
        .set_auto_quit_on_ctrl_c(false);
    let app = hooks.use_state(|| {
        props
            .initial
            .take()
            .unwrap_or_else(|| create_workspace(props.color_mode))
    });
    let palette = theme_palette(app.read().config.theme.as_deref());
    let completed = app.read().services.jobs.completion_notifier();
    hooks.use_future(async move {
        loop {
            completed.notified().await;
            let mut model = app.write();
            model.poll_pending_list();
            model.poll_pending_status();
            model.poll_pending_preview();
            model.poll_pending_page();
            model.poll_file_find();
            model.poll_pending_diff();
            model.poll_pending_write();
            model.poll_tools();
            model.poll_editor();
            model.poll_history();
            model.poll_blame();
            model.tick_blame();
            model.poll_inline_diff();
            model.tick_inline_diff();
        }
    });
    // Expire short success/error notifications without repainting an idle
    // terminal on every tick. The separate write-status channel is a toast,
    // while sync state, branch and selected-file context remain persistent.
    let notice_app = app;
    hooks.use_future(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if notice_app.read().session.write_status.is_some() {
                notice_app.write().expire_write_notice();
            }
        }
    });
    // A PTY output event schedules the first frame immediately. Coalesce
    // subsequent output to 30 FPS, with no idle timer or hidden-panel redraw.
    let terminal_app = app;
    let terminal_notify = terminal_app.read().terminal_notify.clone();
    hooks.use_future(async move {
        loop {
            terminal_notify.notified().await;
            let changed = {
                let model = terminal_app.read();
                model.terminal.as_ref().is_some_and(|t| t.visible
                    && (t.revision.load(std::sync::atomic::Ordering::Acquire) != t.drawn_revision
                        || (t.ended.load(std::sync::atomic::Ordering::Acquire) && t.focused)))
            };
            if changed {
                // A mutable state access is required to wake ratatui-kit;
                // merely acquiring a write guard does not trigger a frame.
                let mut model = terminal_app.write();
                if let Some(term) = &mut model.terminal {
                    if term.ended.load(std::sync::atomic::Ordering::Acquire) && term.focused {
                        term.focused = false;
                        model.session.status = "Terminal session exited; press F8 to hide".into();
                    }
                }
                model.terminal_frame_epoch = model.terminal_frame_epoch.wrapping_add(1);
                drop(model);
                // Output floods must not starve input, Git, or the file browser.
                tokio::time::sleep(Duration::from_millis(33)).await;
            }
        }
    });
    // Watch only the root and the directories actually opened in the tree.
    // Registering Recursive on an entire Java/Node monorepo is synchronous in
    // notify/inotify and can block the Tokio UI thread for seconds/minutes,
    // leaving the Files panel stuck at "listing ...". Git metadata gets its
    // own shallow watch so stage/branch changes still refresh.
    let observed_app = app;
    hooks.use_future(async move {
        use notify::{RecursiveMode, Watcher};
        use std::collections::HashSet;
        let (mailbox, mut receiver) = super::auto_refresh::WatchMailbox::new(256);
        let callback_mailbox = mailbox.clone();
        let mut watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event) => callback_mailbox.offer(event),
                    Err(_) => callback_mailbox.mark_overflow(),
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    observed_app.write().session.status =
                        format!("File watcher unavailable: {error}");
                    return;
                }
            };
        let mut root = PathBuf::new();
        let mut watched = HashSet::<PathBuf>::new();
        loop {
            let (next_root, desired, git_watch) = {
                let model = observed_app.read();
                let next_root = model.files.tree_root.clone();
                let mut desired = HashSet::from([next_root.clone()]);
                // Listings exist only for expanded directories. No recursive
                // project walk is needed, even when node_modules contains
                // hundreds of thousands of nested subdirectories.
                for folder in &model.files.expanded {
                    if folder.starts_with(&next_root) && model.files.children.contains_key(folder) {
                        desired.insert(folder.clone());
                    }
                }
                let git_root = model
                    .scm
                    .git_status
                    .as_ref()
                    .map(|s| s.repo_root.clone())
                    .filter(|p| next_root.starts_with(p))
                    .unwrap_or_else(|| next_root.clone());
                let git_watch = super::auto_refresh::GitWatchPaths::resolve(&git_root);
                if let Some(paths) = &git_watch {
                    desired.extend(paths.watch_directories());
                }
                (next_root, desired, git_watch)
            };
            if root != next_root {
                for old in watched.drain() {
                    let _ = watcher.unwatch(&old);
                }
                root = next_root;
                // Discard queued events from the old workspace.
                while receiver.try_recv().is_ok() {}
                mailbox.take_overflow();
            }
            for old in watched.difference(&desired).cloned().collect::<Vec<_>>() {
                let _ = watcher.unwatch(&old);
                watched.remove(&old);
            }
            for new in desired.difference(&watched).cloned().collect::<Vec<_>>() {
                let mode = if git_watch
                    .as_ref()
                    .is_some_and(|paths| paths.recursive_refs_root(&new))
                {
                    RecursiveMode::Recursive // refs only, not .git/objects
                } else {
                    RecursiveMode::NonRecursive
                };
                if let Err(error) = watcher.watch(&new, mode) {
                    // A directory can disappear between the snapshot and
                    // registration. The next refresh will reconcile watches.
                    if new == root {
                        observed_app.write().session.status = format!("File watcher: {error}");
                    }
                } else {
                    watched.insert(new);
                }
            }
            tokio::select! {
                Some(first) = receiver.recv() => {
                    let mut changes = vec![first];
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    // A noisy build can continuously generate events. Bound the
                    // per-batch work, and fall back to a full reconciliation.
                    while changes.len() < 512 {
                        match receiver.try_recv() {
                            Ok(event) => changes.push(event),
                            Err(_) => break,
                        }
                    }
                    if changes.len() == 512 {
                        mailbox.mark_overflow();
                    }
                    let overflowed = mailbox.take_overflow();
                    let git_metadata_changed = overflowed || git_watch.as_ref().is_some_and(|paths|
                        changes.iter().any(|event| paths.matches_event(event))
                    );
                    if !git_metadata_changed
                        && !changes.iter().any(|event| super::auto_refresh::relevant(event, &root))
                    {
                        continue;
                    }
                    let changed_paths: Vec<PathBuf> = changes
                        .iter()
                        .filter(|event| super::auto_refresh::relevant(event, &root))
                        .flat_map(|event| event.paths.iter().cloned())
                        .collect();
                    let mut model = observed_app.write();
                    if model.files.tree_root == root {
                        let selected = if overflowed {
                            model.selected_entry().map(|entry| entry.path.clone())
                        } else {
                            None
                        };
                        model.refresh_external_changes(git_metadata_changed);
                        model.refresh_preview_if_affected(&changed_paths);
                        if let Some(path) = selected {
                            model.refresh_preview_if_affected(&[path]);
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    observed_app.write().tick_blame();
                    observed_app.write().tick_inline_diff();
                }
            }
        }
    });
    let (scheduler, pending_ids) = {
        let model = app.read();
        let mut ids: Vec<_> = model.files.pending_lists.iter().map(|p| p.id).collect();
        if let Some(p) = &model.scm.pending_status {
            ids.push(p.id);
        }
        (model.services.jobs.clone(), ids)
    };
    hooks.use_on_drop(move || {
        for id in &pending_ids {
            scheduler.cancel(*id);
        }
        for key in [
            "fs:preview",
            "git:diff",
            "git:write",
            "tools:read",
            "editor:load",
            "git:blame",
            "git:gutter",
            "git:overview",
            "git:blame-popup",
        ] {
            scheduler.cancel_key(key);
        }
    });
    let mut exit = hooks.use_exit();
    let editor_handoff = props.editor_handoff.clone();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            Event::Paste(text) if model.terminal.as_ref().is_some_and(|t| t.visible && t.focused) => {
                if let Some(term) = &mut model.terminal {
                    if let Err(error) = term.paste(&text) {
                        term.focused = false;
                        model.session.status = format!("Terminal paste failed: {error}");
                    }
                }
            },
            _ => return EventResult::Ignored,
        }
        if let Some(handoff) = &editor_handoff {
            if let Some(resume) = tools::EditorResume::capture(&mut model) {
                *handoff.lock().expect("editor handoff") = Some(resume);
                exit();
            }
        }
        if model.session.should_quit {
            exit();
        }
        EventResult::Consumed
    });
    let mut editor_exit = hooks.use_exit();
    if let Some(handoff) = &props.editor_handoff {
        if let Some(resume) = tools::EditorResume::capture(&mut app.write_no_update()) {
            *handoff.lock().expect("editor handoff") = Some(resume);
            editor_exit();
        }
    }
    let model = app.read();
    let commit_open = model.session.commit_open;
    let menu_open = model.session.context_menu.is_some();
    let help_open = model.session.help_open;
    let tools_open = model.tools.panel.is_some();
    let editing_open = model.editing.active();
    let keys_open = model.keys.open;
    let blame_open = model.blame.popup.is_some();
    let focus = model.session.focus;
    let terminal_size = hooks.use_terminal_size();
    let previous_area = hooks.use_previous_size();
    let width = if previous_area.width > 0 {
        previous_area.width
    } else {
        terminal_size.0
    };
    let height = if previous_area.height > 0 {
        previous_area.height
    } else {
        terminal_size.1
    };
    let terminal_open = model.terminal.as_ref().is_some_and(|t| t.visible);
    let terminal_maximized = model.terminal.as_ref().is_some_and(|t| t.visible && t.maximized);
    let terminal_height = if terminal_open && !terminal_maximized && height >= 14 {
        (height / 3).clamp(5, 16)
    } else { 0 };
    let dual = width >= 80;
    let show_list = dual || focus != PaneFocus::Preview;
    let show_preview = dual || focus == PaneFocus::Preview;
    // A command search with just a few matches should not consume the screen.
    let tools_modal_height = if let Some(panel) = model.tools.panel.as_ref() {
        if matches!(panel.kind, tools::PanelKind::Commands) {
            let rows = panel.rows.len().min(12) as u16;
            Constraint::Length((rows + 7).max(8).min(height.saturating_sub(2).max(4)))
        } else if matches!(panel.kind, tools::PanelKind::Settings) {
            Constraint::Length(height.saturating_sub(2).min(21).max(8))
        } else {
            Constraint::Percentage(90)
        }
    } else {
        Constraint::Percentage(90)
    };
    drop(model);
    {
        let mut model = app.write_no_update();
        model.session.layout = PaneLayout::default();
    }
    element!(PaletteProvider(palette: palette) {
        ContextProvider(value: Context::owned(app)) {
            View(flex_direction: Direction::Vertical) {
                if terminal_maximized {
                    View(height: Constraint::Min(0)) {
                        NativePane(kind: PaneKind::Terminal)
                    }
                } else {
                View(height: Constraint::Length(if height >= 12 { 2 } else { 1 })) {
                    NativePane(kind: PaneKind::Header)
                }
                View(flex_direction: Direction::Horizontal, height: Constraint::Min(0)) {
                    if show_list {
                        View(width: if dual { Constraint::Length((width / 4).clamp(24, 40)) } else { Constraint::Percentage(100) }) {
                            NativePane(kind: PaneKind::Sidebar)
                        }
                    }
                    if dual {
                        View(width: Constraint::Length(1)) { NativePane(kind: PaneKind::Divider) }
                    }
                    if show_preview {
                        View(width: Constraint::Min(0)) {
                            NativePane(kind: PaneKind::Preview)
                        }
                    }
                }
                if terminal_height > 0 {
                    View(height: Constraint::Length(terminal_height)) {
                        NativePane(kind: PaneKind::Terminal)
                    }
                }
                View(height: Constraint::Length(if height >= 12 { 3 } else if height >= 5 { 2 } else { 1 })) {
                    NativePane(kind: PaneKind::Footer)
                }
                }
            }
            if menu_open {
                Modal(open: true, width: Constraint::Length(0), height: Constraint::Length(0)) {
                    Menu()
                }
            }
            if commit_open {
                Modal(open: true, width: Constraint::Percentage(80), height: Constraint::Length(height.saturating_sub(2).min(20).max(7))) {
                    CommitEditor()
                }
            }
            if help_open && !keys_open {
                Modal(open: true, width: Constraint::Percentage(90), height: Constraint::Percentage(85)) {
                    Help()
                }
            }
            if tools_open {
                Modal(open: true, width: if app.read().tools.panel.as_ref().is_some_and(|p| matches!(p.kind, tools::PanelKind::Settings)) { Constraint::Percentage(74) } else { Constraint::Percentage(92) }, height: tools_modal_height) {
                    Tools()
                }
            }
            if editing_open && !keys_open {
                Modal(open: true, width: Constraint::Percentage(96), height: Constraint::Percentage(94)) {
                    TextEditor()
                }
            }
            if keys_open {
                Modal(open: true, width: Constraint::Percentage(92), height: Constraint::Percentage(90)) {
                    Help()
                }
            }
            if blame_open {
                Modal(open: true, width: Constraint::Percentage(88), height: Constraint::Percentage(84)) {
                    BlamePopup()
                }
            }
        }
    })
}

#[component]
fn TextEditor(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_editor_mouse(mouse),
            Event::Paste(text) => model.handle_editor_paste(&text),
            _ => {}
        }
        EventResult::Consumed
    });
    element!(NativePane(kind: PaneKind::Editor))
}

#[component]
fn Tools(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_tools_key(key),
            Event::Mouse(mouse) => model.handle_tools_mouse(mouse),
            Event::Paste(text) => {
                if model.tools.panel.as_ref().is_some_and(|p| p.is_input()) {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        model
                            .handle_tools_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                    }
                }
            }
            _ => {}
        }
        EventResult::Consumed
    });
    element!(NativePane(kind: PaneKind::Tools))
}

#[path = "commit_composer.rs"]
mod commit_composer;
use commit_composer::CommitEditor;

#[component]
fn BlamePopup(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            _ => {}
        }
        EventResult::Consumed
    });
    element!(NativePane(kind: PaneKind::Blame))
}

#[component]
fn Menu(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            _ => {}
        }
        EventResult::Consumed
    });
    element!(NativePane(kind: PaneKind::Menu))
}

#[component]
fn Help(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    let mut exit = hooks.use_exit();
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        let mut model = app.write();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            _ => {}
        }
        if model.session.should_quit {
            exit();
        }
        EventResult::Consumed
    });
    element!(NativePane(kind: PaneKind::Help))
}

#[derive(Default, Clone, Copy)]
enum PaneKind {
    Header,
    #[default]
    Sidebar,
    Preview,
    Terminal,
    Divider,
    Footer,
    Menu,
    Help,
    Tools,
    Editor,
    Blame,
}

#[derive(Default, Props)]
struct NativePaneProps {
    kind: PaneKind,
}

/// Native bridges own no event loop or application lifetime. Kit computes their
/// area; drawing only updates viewport caches without scheduling another frame.
/// One-row horizontal position indicator; only shown for actual overflow.
fn draw_horizontal_position(frame: &mut Frame<'_>, area: Rect, offset: usize, full: usize, p: Palette) {
    if area.height == 0 || area.width < 6 { return; }
    let width = area.width as usize;
    let max_offset = full.saturating_sub(width);
    if max_offset == 0 { return; }
    let thumb = ((width * width + full / 2) / full).clamp(1, width);
    let left = (offset.min(max_offset) * (width - thumb) + max_offset / 2) / max_offset;
    for x in 0..width {
        let selected = x >= left && x < left + thumb;
        frame.buffer_mut()[(area.x + x as u16, area.y)]
            .set_symbol(if selected { "━" } else { "─" })
            .set_style(Style::default().bg(p.bg)
                .fg(if selected { p.accent } else { p.border }));
    }
}

/// A compact 2-column file overview: markers next to the existing position thumb.
/// Only project known rows; no I/O or scanning occurs during drawing.
fn overview_marker_y(row: usize, total: usize, height: usize) -> usize {
    if height <= 1 || total <= 1 { return 0; }
    row.min(total - 1).saturating_mul(height - 1) / (total - 1)
}

fn draw_file_overview(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) {
    if area.width < 3 || area.height < 3
        || !matches!(model.preview_state.preview_mode, PreviewMode::File)
        || (model.markdown_available() && !model.preview_state.markdown_code)
    { return; }
    let total = model.overview_git_data().map_or_else(|| model.preview_total_lines(), |(n, _)| n);
    let height = area.height as usize;
    if total <= height { return; }
    let x = area.right() - 2;
    let mut marks = vec![None; height];
    // Git and search use *source-line* coordinates across the entire file.
    // The preview's rendered rows can cover only the first 400 lines.
    let known_lines = model.overview_git_data().map(|(total, _)| total);
    if let Some((source_lines, git_marks)) = model.overview_git_data() {
        for &(line, color) in git_marks {
            if line > 0 {
                let y = overview_marker_y(line - 1, source_lines.max(1), height);
                marks[y] = Some(("▪", color));
            }
        }
    }
    if model.preview_state.find.open
        && !model.preview_state.find.query.is_empty()
        && model.preview_state.preview.as_ref().is_some_and(|preview| {
            preview.path == model.preview_state.find.path
        })
    {
        for hit in &model.preview_state.find.matches {
            let row = if let Some(source_lines) = known_lines {
                Some(overview_marker_y(hit.line.saturating_sub(1), source_lines.max(1), height))
            } else {
                model.preview_state.text_rows.source_row_range(hit.line)
                    .map(|range| overview_marker_y(range.start, total, height))
            };
            if let Some(y) = row { marks[y] = Some(("•", Color::Yellow)); }
        }
    }
    for (y, marker) in marks.into_iter().enumerate() {
        if let Some((symbol, color)) = marker {
            frame.buffer_mut()[(x, area.y + y as u16)]
                .set_symbol(symbol)
                .set_style(Style::default().fg(color).bg(p.bg));
        }
    }
}

struct NativePane {
    app: Option<State<App>>,
    kind: PaneKind,
    palette: Palette,
}

impl Component for NativePane {
    type Props<'a> = NativePaneProps;
    fn new(props: &Self::Props<'_>) -> Self {
        Self {
            app: None,
            kind: props.kind,
            palette: Palette::default(),
        }
    }
    fn update(
        &mut self,
        props: &mut Self::Props<'_>,
        mut hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        let hooks = hooks.with_context_stack(updater.component_context_stack());
        self.app = Some(*hooks.use_context::<State<App>>());
        self.kind = props.kind;
        self.palette = hooks.use_palette();
    }
    fn draw(&mut self, drawer: &mut ComponentDrawer<'_, '_>) {
        let Some(app) = self.app else {
            return;
        };
        let mut model = app.write_no_update();
        let mut area = drawer.area;
        // Ratatui Reset is transparent to terminal background. Paint every
        // cell owned by a pane before drawing text, including blank rows.
        // This keeps all panels consistent across terminal color profiles.
        drawer.frame.buffer_mut().set_style(
            area,
            Style::default().bg(self.palette.bg).fg(self.palette.fg),
        );
        let theme = bridge_theme(self.palette);
        match self.kind {
            PaneKind::Header => {
                draw_workspace_header(drawer.frame, area, &mut model, self.palette);
                model.session.layout.header = area;
            }
            PaneKind::Sidebar => {
                model.session.layout.list = area;
                if model.in_git_sidebar() {
                    let mut scroll = model.scm.scroll_scm;
                    let mut sticky_headers = std::mem::take(&mut model.scm.sticky_headers);
                    let sticky_scroll = model.config.tree_sticky_scroll;
                    let (unstaged, staged) = model.scm_slices();
                    let focused = model.sidebar_focused();
                    let history = model.history_view();
                    let layout = draw_scm_sidebar(
                        drawer.frame,
                        area,
                        &theme,
                        &mut StagingState {
                            unstaged,
                            staged,
                            expanded_unstaged: &model.scm.scm_expanded_unstaged,
                            expanded_staged: &model.scm.scm_expanded_staged,
                            history: &history,
                            selected: model.scm.scm_selected,
                            scroll: &mut scroll,
                            focused,
                            sticky_scroll,
                            compact_folders: model.config.compact_folders,
                            follow_selection: model.scm.follow_selection,
                            sticky_headers: &mut sticky_headers,
                        },
                    );
                    model.scm.sticky_headers = sticky_headers;
                    model.session.layout.staging = layout;
                    model.session.layout.list_inner = layout.inner;
                    model.scm.scroll_scm = scroll;
                } else {
                    let mut scroll = model.files.scroll;
                    let mut sticky_headers = std::mem::take(&mut model.files.sticky_headers);
                    let sticky_scroll = model.config.tree_sticky_scroll;
                    let inner = draw_file_list(
                        drawer.frame,
                        area,
                        &theme,
                        &mut FileListState {
                            rows: &model.files.visible,
                            selected: model.files.selected,
                            scroll: &mut scroll,
                            git: model.scm.git_status.as_ref(),
                            focused: model.sidebar_focused(),
                            sticky_scroll,
                            follow_selection: model.files.follow_selection,
                            sticky_headers: &mut sticky_headers,
                        },
                    );
                    model.files.sticky_headers = sticky_headers;
                    model.session.layout.list_inner = inner;
                    model.files.scroll = scroll;
                }
                draw_marks(drawer.frame, &model, self.palette);
                let (first, total) = if model.in_git_sidebar() {
                    (model.scm.scroll_scm, model.scm_rows().len())
                } else { (model.files.scroll, model.files.visible.len()) };
                let lane = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
                let bar = VerticalScrollbar::new(lane, first, total);
                model.session.layout.sidebar_scrollbar =
                    if bar.geometry().is_some() { lane } else { Rect::default() };
                bar.draw(drawer.frame, self.palette.bg, self.palette.fg_dim, self.palette.accent);
            }
            PaneKind::Preview => {
                model.session.layout.preview = area;
                area = draw_preview_heading(drawer.frame, area, &mut model, self.palette);
                // A 3-column focus ruler, not a code minimap: preserve source width.
                let ruler_area = if model.config.show_overview_ruler
                    && area.width >= 36 && area.height >= 5
                    && matches!(model.preview_state.preview_mode, PreviewMode::File)
                    && (!model.markdown_available() || model.preview_state.markdown_code)
                    && matches!(model.preview_state.preview.as_ref().map(|p| &p.content),
                        Some(PreviewContent::Text { .. }))
                {
                    let sidebar = Rect::new(area.right() - 3, area.y, 3, area.height);
                    area.width -= 3;
                    Some(sidebar)
                } else { None };
                // Without the overview ruler, reserve a separate one-cell lane.
                // Drawing a scrollbar over the preview truncates CJK/diff text
                // and can leave half of a wide glyph behind the scrollbar.
                let scrollbar_lane = if ruler_area.is_none() && area.width > 3 {
                    let lane = Rect::new(area.right() - 1, area.y, 1, area.height);
                    area.width -= 1;
                    Some(lane)
                } else {
                    None
                };
                // The navigation ruler owns three terminal columns. Source
                // wrapping, Blame cursor validation and code rendering MUST
                // all use the same effective width. Comparing against the
                // original full pane erased clicked Blame on every redraw.
                let previous_width = model.preview_state.text_rows.width();
                let clicked_source = if previous_width != area.width {
                    model.blame.cursor.as_ref().and_then(|(path, row, scroll)| {
                        (Some(path) == model.preview_state.preview.as_ref().map(|p| &p.path)
                            && *scroll == model.preview_state.preview_scroll)
                            .then(|| model.preview_state.text_rows.source_line_at(*row))
                            .flatten()
                            .map(|line| (path.clone(), line, *scroll))
                    })
                } else { None };
                // Rebuild source-row mapping before asking whether inline
                // Blame has a line to show. A same-file external refresh
                // clears this cache; querying attribution first would hide
                // its status row for one frame, expand the text viewport,
                // and permanently clamp a reader positioned at EOF.
                if model.config.git.blame.show_inline
                    && !model.preview_state.preview_loading
                    && matches!(model.preview_state.preview_mode, PreviewMode::File)
                {
                    let mut rows = std::mem::take(&mut model.preview_state.text_rows);
                    if let Some(Preview {
                        path,
                        content: PreviewContent::Text { lines, truncated, bytes_read },
                    }) = &model.preview_state.preview {
                        rows.ensure_wrapping(
                            &theme, lines, *truncated, *bytes_read,
                            TextLang::from_name(&path.to_string_lossy()),
                            area.width, model.config.wrap_code,
                        );
                    }
                    if let Some((path, source, scroll)) = clicked_source {
                        // Preserve attribution to the SOURCE line when wrapping
                        // changes (resize / toggling Focus Overview).
                        model.blame.cursor = Some((path, rows.source_to_row(source), scroll));
                    }
                    model.preview_state.text_rows = rows;
                }
                let blame_bar = if model.config.git.blame.show_inline
                    && model.current_blame().is_some()
                    && area.height > 1
                {
                    let bar = Rect::new(area.x, area.bottom() - 1, area.width, 1);
                    area.height -= 1;
                    Some(bar)
                } else {
                    None
                };
                if model.markdown_available() {
                    let mut markdown = std::mem::take(&mut model.preview_state.markdown);
                    if let Some(Preview {
                        content:
                            PreviewContent::Text {
                                lines,
                                truncated,
                                bytes_read,
                            },
                        ..
                    }) = &model.preview_state.preview
                    {
                        markdown.ensure(lines, *truncated, *bytes_read, area.width, &theme);
                    }
                    model.preview_state.markdown = markdown;
                }
                let mut scroll = model.preview_state.preview_scroll;
                // Detach caches before borrowing the preview. Code view uses
                // the same safe, pre-wrapped row boundaries as Markdown, so
                // a two-cell CJK glyph can never spill into the Files pane.
                let mut cache = std::mem::take(&mut model.preview_state.split_cache);
                let mut text_cache = std::mem::take(&mut model.preview_state.text_rows);
                let pending_line = model.preview_state.pending_source_line.take();
                let pending_find_byte = model.preview_state.pending_find_byte.take();
                let (_, content) = preview_content(&model);
                let numbered = matches!(content, PreviewView::Text { .. });
                let mut horizontal_x = model.preview_state.preview_scroll_x;
                let mut horizontal_bar = None;
                let inner = match content {
                    PreviewView::Text {
                        lines,
                        truncated,
                        bytes_read,
                        language,
                    } => {
                        let wrap = model.config.wrap_code;
                        text_cache.ensure_wrapping(&theme, lines, truncated, bytes_read, language, area.width, wrap);
                        if !wrap && area.height > 2 && area.width > 7 {
                            let visible_cols = area.width.saturating_sub(5) as usize;
                            let max_x = text_cache.max_code_columns().saturating_sub(visible_cols);
                            horizontal_x = horizontal_x.min(max_x);
                            if max_x > 0 {
                                horizontal_bar = Some((Rect::new(area.x + 5, area.bottom() - 1,
                                    area.width.saturating_sub(6), 1), text_cache.max_code_columns().saturating_sub(1)));
                                area.height -= 1;
                            }
                        } else { horizontal_x = 0; }
                        if let Some(line) = pending_line {
                            scroll = pending_find_byte
                                .map_or_else(|| text_cache.source_to_row(line),
                                    |byte| text_cache.source_byte_to_row(line, byte));
                        }
                        draw_preview(
                            drawer.frame,
                            area,
                            &theme,
                            if wrap { PreviewView::TextRows { rows: text_cache.rows() } }
                            else { PreviewView::TextRowsScrolled {
                                rows: text_cache.rows(),
                                horizontal_offset: horizontal_x,
                            } },
                            &mut scroll,
                            &mut cache,
                        )
                    }
                    PreviewView::Diff { path, lines, hunk_start, hunk_end, .. }
                        if !model.config.wrap_code =>
                    {
                        let half_content = (area.width as usize / 2).saturating_sub(7);
                        let max_x = cache.max_code_columns(lines).saturating_sub(half_content);
                        horizontal_x = horizontal_x.min(max_x);
                        if max_x > 0 && area.height > 2 {
                            horizontal_bar = Some((Rect::new(area.x, area.bottom() - 1,
                                area.width.saturating_sub(1), 1),
                                cache.max_code_columns(lines).saturating_add(area.width.saturating_sub(1) as usize).saturating_sub(half_content)));
                            area.height -= 1;
                        }
                        draw_preview(drawer.frame, area, &theme,
                            PreviewView::DiffScrolled { path, lines, hunk_start, hunk_end,
                                horizontal_offset: horizontal_x },
                            &mut scroll, &mut cache)
                    }
                    other => {
                        draw_preview(drawer.frame, area, &theme, other, &mut scroll, &mut cache)
                    }
                };
                model.preview_state.preview_scroll_x = horizontal_x;
                model.preview_state.split_cache = cache;
                model.preview_state.text_rows = text_cache;
                model.preview_state.preview_scroll = scroll;
                model.session.layout.preview_inner = inner;
                // Snapshot rendered cells before overlaying the selection.
                // This preserves Markdown table borders and wrapped CJK.
                let identity = if let Some(diff) = &model.preview_state.file_diff {
                    format!("diff:{}:{:?}", diff.path, model.preview_state.diff_source)
                } else if let Some(preview) = &model.preview_state.preview {
                    format!(
                        "file:{}:{}",
                        preview.path.display(),
                        model.preview_state.markdown_code
                    )
                } else {
                    "empty".to_owned()
                };
                model.preview_state.selection.observe(
                    drawer.frame,
                    inner,
                    scroll,
                    &theme,
                    &identity,
                    numbered,
                );
                if let Some(bar) = blame_bar {
                    draw_inline_blame(drawer.frame, inner, bar, &model, self.palette);
                }
                model.draw_inline_diff(drawer.frame, inner);
                model.session.layout.file_find_rect = Rect::default();
                if model.preview_state.find.open {
                    super::file_find_ui::highlight_file_find_matches(
                        drawer.frame,
                        inner,
                        &model,
                        &theme,
                    );
                    model.session.layout.file_find_rect = super::file_find_ui::draw_file_find_bar(
                        drawer.frame,
                        inner,
                        &model,
                        self.palette,
                    );
                }
                if let Some(ruler) = ruler_area {
                    // Clear the dedicated lane, then show search/Git locations and viewport.
                    for y in ruler.y..ruler.bottom() {
                        for x in ruler.x..ruler.right() {
                            drawer.frame.buffer_mut()[(x, y)]
                                .set_symbol(" ")
                                .set_style(Style::default().bg(self.palette.bg));
                        }
                    }
                    draw_file_overview(drawer.frame, ruler, &model, self.palette);
                    if let Some((source_total, _)) = model.overview_git_data() {
                        let first_line = model.preview_state.text_rows.source_line_at(scroll)
                            .unwrap_or(1).saturating_sub(1);
                        VerticalScrollbar::new(ruler, first_line, source_total)
                            .draw(drawer.frame, self.palette.bg, self.palette.fg_dim, self.palette.accent);
                    } else {
                        VerticalScrollbar::new(ruler, scroll, model.preview_total_lines())
                            .draw(drawer.frame, self.palette.bg, self.palette.fg_dim, self.palette.accent);
                    }
                    model.session.layout.overview_ruler = ruler;
                    model.session.layout.preview_scrollbar = Rect::default();
                } else {
                    model.session.layout.overview_ruler = Rect::default();
                    if let Some(lane) = scrollbar_lane {
                        // Match the actual viewport after optional bottom bars.
                        let lane = Rect::new(lane.x, inner.y, lane.width, inner.height);
                        let total = model.preview_total_lines();
                        if VerticalScrollbar::new(lane, scroll, total).geometry().is_some() {
                            model.session.layout.preview_scrollbar = lane;
                            VerticalScrollbar::new(lane, scroll, total)
                                .draw(drawer.frame, self.palette.bg, self.palette.fg_dim, self.palette.accent);
                        } else {
                            model.session.layout.preview_scrollbar = Rect::default();
                        }
                    } else {
                        model.session.layout.preview_scrollbar = Rect::default();
                    }
                }
                if let Some((bar, total)) = horizontal_bar {
                    draw_horizontal_position(drawer.frame, bar, model.preview_state.preview_scroll_x, total, self.palette);
                }
            }
            PaneKind::Divider => {
                drawer.frame.render_widget(
                    Block::default()
                        .borders(Borders::LEFT)
                        .border_style(Style::default().fg(self.palette.border)),
                    area,
                );
            }
            PaneKind::Terminal => {
                let inner = terminal_tabs::draw_tab_strip(drawer.frame, area, &mut model, self.palette);
                if let Some(term) = &mut model.terminal {
                    term.render(drawer.frame, inner, self.palette.bg, self.palette.fg);
                }
            }
            PaneKind::Footer => {
                model.session.layout.git_sync_status =
                    draw_workspace_footer(drawer.frame, area, &model, self.palette);
                model.session.layout.footer = area;
                model.session.layout.hotkeys = Rect::new(
                    area.x,
                    area.bottom().saturating_sub(1),
                    area.width,
                    area.height.min(1),
                );
            }
            PaneKind::Menu => {
                if let Some(menu) = &mut model.session.context_menu {
                    forge_ui::draw_context_menu(drawer.frame, &theme, menu);
                    area = menu.area;
                }
            }
            PaneKind::Help => {
                if model.keys.open {
                    keybindings::draw(drawer.frame, area, &model.keys, self.palette);
                    return;
                }
                draw_help_guide(drawer.frame, area, &mut model, self.palette);
            }
            PaneKind::Tools => draw_tools(drawer.frame, area, &mut model, self.palette),
            PaneKind::Editor => editor::draw_editor(drawer.frame, area, &mut model, self.palette),
            PaneKind::Blame => draw_blame_popup(drawer.frame, area, &mut model, self.palette),
        }
        model.session.layout.window = drawer.frame.area();
        model.session.layout.dual_pane = drawer.frame.area().width >= 80;
        // Return terminal-default surfaces to Reset after the RGB-only bridge.
        let base_bg = forge_ui::to_color(theme.panel_bg);
        let base_fg = forge_ui::to_color(theme.fg);
        let selection_bg = forge_ui::to_color(theme.selection_bg);
        let selection_fg = forge_ui::to_color(theme.selection_fg);
        area = area.intersection(drawer.frame.area());
        let buffer = drawer.frame.buffer_mut();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let cell = &mut buffer[(x, y)];
                if cell.bg == base_bg {
                    cell.set_bg(self.palette.bg);
                }
                if cell.bg == selection_bg && cell.fg == selection_fg {
                    cell.set_fg(self.palette.on_accent);
                } else if cell.fg == base_fg {
                    cell.set_fg(self.palette.fg);
                }
            }
        }
    }
}

// Keep the component tree and layout here; drawing implementations live in
// independent modules so UI changes stay scoped to their specific panel.
mod decorations;
mod help_guide;
mod preview_content;
mod tools_panel;
mod workspace;

pub(crate) use decorations::draw_blame_popup;
use decorations::*;
use help_guide::*;
pub(crate) use preview_content::bridge_theme;
use preview_content::*;
use tools_panel::*;
use workspace::*;

#[cfg(test)]
mod tests;

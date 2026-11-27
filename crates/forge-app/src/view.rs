//! Kit components. Workspace state is scoped to this tree; only the palette is
//! process-wide. Draft text lives exclusively in the mounted commit editor.
use super::*;
use crate::splitter::{Axis as SplitterAxis, Splitter};
use crossterm::event::{Event, KeyEventKind};
use forge_ui::{
    draw_file_list, draw_scm_sidebar_rows, FileListState, HorizontalScrollbar, PreviewView,
    StagingState,
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
mod activity_bar;
pub(crate) mod commit_detail;
mod dialog;
mod terminal_tabs;

// Keep the terminal's own foreground and background. Accent belongs to small
// focus marks; selection is a quiet neutral surface, like NSH's TUI controls.
pub(crate) fn workspace_palette() -> Palette {
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
    pub(super) keyboard_enhancement: Option<std::sync::Arc<super::KeyboardEnhancement>>,
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
            model.flush_workspace_changes();
            model.try_reveal();
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
            if notice_app.read().write_notice_needs_tick() {
                notice_app.write().expire_write_notice();
            }
            if notice_app.read().terminal_copy_notice_expired() {
                notice_app.write().expire_terminal_copy_notice();
            }
        }
    });
    // The edge timer sleeps completely when there is no host drag.
    let selection_app = app;
    let selection_notify = selection_app.read().selection_scroll_notify.clone();
    hooks.use_future(async move {
        loop {
            selection_notify.notified().await;
            while selection_app
                .read()
                .terminals
                .active
                .as_ref()
                .is_some_and(|t| t.selection_autoscroll_active())
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if selection_app
                    .read()
                    .terminals
                    .active
                    .as_ref()
                    .is_some_and(|t| t.selection_autoscroll_active())
                {
                    if let Some(term) = &mut selection_app.write().terminals.active {
                        term.tick_selection_autoscroll();
                    }
                }
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
            let profiles_ready = terminal_app
                .read()
                .shell_discovery
                .as_ref()
                .is_some_and(|r| r.lock().is_ok_and(|slot| slot.is_some()));
            if profiles_ready {
                terminal_app.write().poll_shell_discovery();
            }
            // A child TUI can request clipboard transfer over OSC 52 after
            // handling its own Ctrl+C. vt100's callback gives us the already
            // validated base64 bytes; forward them to the *outer* terminal,
            // not to Forge's local clipboard or the child's input stream.
            // Stdout's lock keeps each escape sequence from interleaving with
            // ratatui's frame writes.
            let clipboard = {
                let model = terminal_app.read();
                model
                    .terminals
                    .active
                    .as_ref()
                    .and_then(|t| t.take_clipboard_request())
                    .or_else(|| {
                        model
                            .terminals
                            .inactive()
                            .iter()
                            .find_map(|t| t.take_clipboard_request())
                    })
            };
            if let Some(data) = clipboard {
                use std::io::Write;
                let mut stdout = std::io::stdout().lock();
                let result = stdout
                    .write_all(b"\x1b]52;c;")
                    .and_then(|_| stdout.write_all(&data))
                    .and_then(|_| stdout.write_all(b"\x07"))
                    .and_then(|_| stdout.flush());
                drop(stdout);
                // OSC 52 has no clipboard-write acknowledgement from the host.
                terminal_app.write().show_terminal_copy_notice(
                    if result.is_ok() {
                        "Copy sent to clipboard"
                    } else {
                        "Clipboard unavailable"
                    },
                    result.is_err(),
                );
            }
            let changed = {
                let model = terminal_app.read();
                model.terminals.active.as_ref().is_some_and(|t| {
                    t.is_visible()
                        && (t.revision.load(std::sync::atomic::Ordering::Acquire)
                            != t.drawn_revision
                            || (t.ended.load(std::sync::atomic::Ordering::Acquire)
                                && t.is_focused())
                            || t.tab_title_changed()
                            || model
                                .terminals
                                .inactive()
                                .iter()
                                .any(|tab| tab.tab_title_changed()))
                })
            };
            if changed {
                // A mutable state access is required to wake ratatui-kit;
                // merely acquiring a write guard does not trigger a frame.
                let mut model = terminal_app.write();
                if let Some(term) = &mut model.terminals.active {
                    if term.ended.load(std::sync::atomic::Ordering::Acquire) && term.is_focused() {
                        term.set_focused(false);
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
    // The shallow watcher cannot see every collapsed directory. A bounded,
    // low-frequency repository reconciliation runs independently of visibility
    // and even if watcher initialization fails.
    let freshness_app = app;
    hooks.use_future(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if freshness_app.read().periodic_git_status_due(Instant::now()) {
                freshness_app.write().tick_git_status_at(Instant::now());
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
            let (next_root, desired, git_watch, document_watch) = {
                let model = observed_app.read();
                let next_root = model.files.root().to_path_buf();
                let mut desired = HashSet::from([next_root.clone()]);
                // Listings exist only for expanded directories. No recursive
                // project walk is needed, even when node_modules contains
                // hundreds of thousands of nested subdirectories.
                for folder in &model.files.expanded {
                    if folder.starts_with(&next_root) && model.files.has_listing(folder) {
                        desired.insert(folder.clone());
                    }
                }
                let git_root = model
                    .scm.status()
                    .as_ref()
                    .map(|s| s.repo_root.clone())
                    .filter(|p| next_root.starts_with(p))
                    .unwrap_or_else(|| next_root.clone());
                let git_watch = super::auto_refresh::GitWatchPaths::resolve(&git_root);
                if let Some(paths) = &git_watch {
                    desired.extend(paths.watch_directories());
                }
                let document_watch = model.watched_document(&git_root);
                if let Some(parent) = document_watch.as_deref().and_then(Path::parent) {
                    if parent.starts_with(&git_root) {
                        desired.insert(parent.to_path_buf());
                    }
                }
                (next_root, desired, git_watch, document_watch)
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
                    let document_changed = document_watch.as_ref().is_some_and(|path|
                        changes.iter().any(|event| super::auto_refresh::touches_document(event, path)));
                    if !git_metadata_changed && !document_changed
                        && !changes.iter().any(|event| super::auto_refresh::relevant(event, &root))
                    {
                        continue;
                    }
                    let mut changed_paths: Vec<PathBuf> = changes
                        .iter()
                        .filter(|event| super::auto_refresh::relevant(event, &root))
                        .flat_map(|event| event.paths.iter().cloned())
                        .collect();
                    if document_changed {
                        changed_paths.extend(document_watch);
                    }
                    let mut model = observed_app.write();
                    if model.files.root() == root {
                        model.on_workspace_change(workspace_changes::WorkspaceChange::External {
                            git_metadata_changed,
                            changed_paths,
                            overflowed,
                        });
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    let (blame_due, inline_due) = {
                        let model = observed_app.read();
                        (model.needs_blame_tick(), model.needs_inline_diff_tick())
                    };
                    if blame_due || inline_due {
                        let mut model = observed_app.write();
                        if blame_due {
                            model.tick_blame();
                        }
                        if inline_due {
                            model.tick_inline_diff();
                        }
                    }
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
            "editor:syntax",
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
        let mut model = app.write_no_update();
        let nav_before = input_trace::nav_state(&model);
        let input_event = event.clone();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            Event::Paste(text) => {
                if !model.handle_paste(&text) {
                    return EventResult::Ignored;
                }
            }
            _ => return EventResult::Ignored,
        }
        input_trace::event(&input_event, nav_before, input_trace::nav_state(&model));
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
    // The kit renders once more before processing a requested exit. Restore
    // keyboard flags while the alternate screen is still active, including
    // external-editor handoffs. Test harnesses have no keyboard controller.
    if let Some(keyboard) = &props.keyboard_enhancement {
        let leaving = model.session.should_quit
            || props
                .editor_handoff
                .as_ref()
                .is_some_and(|handoff| handoff.lock().is_ok_and(|resume| resume.is_some()));
        if leaving {
            keyboard.disable();
        } else {
            keyboard.enable();
        }
    }
    input_trace::render(&model);
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
    let terminal_open = model
        .terminals
        .active
        .as_ref()
        .is_some_and(|t| t.is_visible());
    let terminal_maximized = model
        .terminals
        .active
        .as_ref()
        .is_some_and(|t| t.is_visible() && t.maximized);
    let layout_state::PanePlan {
        dual,
        sidebar_width,
        terminal_height,
        show_list,
        show_preview,
    } = model.layout_state.plan(
        width,
        height,
        focus,
        terminal_open,
        terminal_maximized,
        activity_bar::activity_bar_width(),
    );
    let screen = (width, height);
    let tools_size = model
        .tools
        .panel
        .as_ref()
        .map(|panel| dialog::DialogSize::tools(panel, screen));
    let commit_size = dialog::DialogSize::commit(screen);
    drop(model);
    {
        let mut model = app.write_no_update();
        model.layout_state.geometry = PaneLayout::default();
        // Maximized Terminal and hidden panes must not retain hit regions from
        // a previously painted Activity Bar or Sidebar collapse button.
        model.activity_hits.clear();
        model.commit_file_links.clear();
    }
    element!(PaletteProvider(palette: palette) {
        ContextProvider(value: Context::owned(app)) {
            View(flex_direction: Direction::Vertical) {
                if terminal_maximized {
                    View(height: Constraint::Min(0)) {
                        NativePane(kind: PaneKind::Terminal)
                    }
                } else {
                View(height: Constraint::Length(1)) {
                    NativePane(kind: PaneKind::Header)
                }
                View(flex_direction: Direction::Horizontal, height: Constraint::Min(0)) {
                    View(width: Constraint::Length(activity_bar::activity_bar_width().min(width))) {
                        NativePane(kind: PaneKind::ActivityBar)
                    }
                    if show_list {
                        View(width: if dual { Constraint::Length(sidebar_width) } else { Constraint::Percentage(100) }) {
                            NativePane(kind: PaneKind::Sidebar)
                        }
                    }
                    if dual && show_list {
                        View(width: Constraint::Length(1)) { NativePane(kind: PaneKind::Divider) }
                    }
                    if show_preview {
                        View(width: Constraint::Min(0)) {
                            NativePane(kind: PaneKind::Preview)
                        }
                    }
                }
                if terminal_height > 0 {
                    // A dedicated one-cell splitter prevents its drag gesture
                    // from being delivered to the child PTY or the preview.
                    View(height: Constraint::Length(1)) {
                        NativePane(kind: PaneKind::TerminalDivider)
                    }
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
                Modal(open: true, width: Constraint::Length(commit_size.width), height: Constraint::Length(commit_size.height)) {
                    CommitEditor()
                }
            }
            if help_open && !keys_open {
                Modal(open: true, width: Constraint::Percentage(90), height: Constraint::Percentage(85)) {
                    Help()
                }
            }
            if tools_open {
                Modal(open: true, width: Constraint::Length(tools_size.map_or(80, |s| s.width)), height: Constraint::Length(tools_size.map_or(15, |s| s.height))) {
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
        // Kit 0.10 renders after every dispatched terminal event. A reactive
        // write here leaves a second wakeup queued and paints each key twice.
        // Background job completions still use reactive writes in Workspace.
        let mut model = app.write_no_update();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            Event::Paste(text) => {
                model.handle_paste(&text);
            }
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
        let mut model = app.write_no_update();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            Event::Paste(text) => {
                model.handle_paste(&text);
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
        let mut model = app.write_no_update();
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
        let mut model = app.write_no_update();
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
        let mut model = app.write_no_update();
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_key(key),
            Event::Mouse(mouse) => model.handle_mouse(mouse),
            Event::Paste(text) => {
                model.handle_paste(&text);
            }
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
    ActivityBar,
    #[default]
    Sidebar,
    Preview,
    Terminal,
    Divider,
    TerminalDivider,
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
/// A compact 2-column file overview: markers next to the existing position thumb.
/// Only project known rows; no I/O or scanning occurs during drawing.
fn overview_marker_y(row: usize, total: usize, height: usize) -> usize {
    if height <= 1 || total <= 1 {
        return 0;
    }
    row.min(total - 1).saturating_mul(height - 1) / (total - 1)
}

fn draw_file_overview(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) {
    if area.width < 3
        || area.height < 3
        || !matches!(model.preview_state.preview_mode, PreviewMode::File)
        || (model.markdown_available() && !model.preview_state.markdown_code)
    {
        return;
    }
    let total = model
        .overview_git_data()
        .map_or_else(|| model.preview_total_lines(), |(n, _)| n);
    let height = area.height as usize;
    if total <= height {
        return;
    }
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
        && model
            .preview_state
            .preview
            .as_ref()
            .is_some_and(|preview| preview.path == model.preview_state.find.path)
    {
        for hit in &model.preview_state.find.matches {
            let row = if let Some(source_lines) = known_lines {
                Some(overview_marker_y(
                    hit.line.saturating_sub(1),
                    source_lines.max(1),
                    height,
                ))
            } else {
                model
                    .preview_state
                    .presentation
                    .text_rows
                    .source_row_range(hit.line)
                    .map(|range| overview_marker_y(range.start, total, height))
            };
            if let Some(y) = row {
                marks[y] = Some(("•", Color::Yellow));
            }
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
    // The modal owns input; the workspace behind it does not need to be
    // laid out and painted again for every editor keystroke.
    editor_backdrop: Option<(forge_theme::Theme, ratatui::buffer::Buffer)>,
}

impl Component for NativePane {
    type Props<'a> = NativePaneProps;
    fn new(props: &Self::Props<'_>) -> Self {
        Self {
            app: None,
            kind: props.kind,
            palette: Palette::default(),
            editor_backdrop: None,
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
        let theme = bridge_theme(self.palette);
        let cache_area = area.intersection(drawer.frame.area());
        let cache_backdrop = model.editing.active()
            && matches!(
                self.kind,
                PaneKind::Header
                    | PaneKind::ActivityBar
                    | PaneKind::Sidebar
                    | PaneKind::Preview
                    | PaneKind::Terminal
                    | PaneKind::Divider
                    | PaneKind::TerminalDivider
                    | PaneKind::Footer
            );
        if cache_backdrop {
            if let Some((cached_theme, cached)) = &self.editor_backdrop {
                if *cached_theme == theme && cached.area == cache_area {
                    copy_pane_cells(cached, drawer.frame.buffer_mut(), cache_area);
                    return;
                }
            }
        } else {
            self.editor_backdrop = None;
        }
        // Ratatui Reset is transparent to terminal background. Paint every
        // cell owned by a pane before drawing text, including blank rows.
        // This keeps all panels consistent across terminal color profiles.
        drawer.frame.buffer_mut().set_style(
            area,
            Style::default().bg(self.palette.bg).fg(self.palette.fg),
        );
        match self.kind {
            PaneKind::Header => {
                draw_workspace_header(drawer.frame, area, &mut model, self.palette);
                model.layout_state.geometry.header = area;
            }
            PaneKind::ActivityBar => {
                activity_bar::draw_activity_bar(drawer.frame, area, &mut model, self.palette);
            }
            PaneKind::Sidebar => {
                area = activity_bar::draw_sidebar_heading(
                    drawer.frame,
                    area,
                    &mut model,
                    self.palette,
                );
                model.layout_state.geometry.list = area;
                if model.in_git_sidebar() {
                    let mut scroll = model.scm.scroll_scm;
                    let mut sticky_headers = std::mem::take(&mut model.scm.sticky_headers);
                    let sticky_scroll = model.config.tree_sticky_scroll;
                    let (unstaged, staged) = model.scm_slices();
                    let focused = model.sidebar_focused();
                    let history = model.history_view();
                    let rows = model.scm_rows();
                    let layout = draw_scm_sidebar_rows(
                        drawer.frame,
                        area,
                        &theme,
                        &mut StagingState {
                            unstaged,
                            staged,
                            expanded_unstaged: model.scm.expanded(StagingColumn::Unstaged),
                            expanded_staged: model.scm.expanded(StagingColumn::Staged),
                            history: &history,
                            selected: model.scm.scm_selected,
                            scroll: &mut scroll,
                            focused,
                            sticky_scroll,
                            compact_folders: model.config.compact_folders,
                            follow_selection: model.scm.follow_selection,
                            sticky_headers: &mut sticky_headers,
                        },
                        &rows,
                    );
                    model.scm.sticky_headers = sticky_headers;
                    model.layout_state.geometry.staging = layout;
                    model.layout_state.geometry.list_inner = layout.inner;
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
                            git: model.scm.status().as_ref(),
                            focused: model.sidebar_focused(),
                            sticky_scroll,
                            follow_selection: model.files.follow_selection,
                            sticky_headers: &mut sticky_headers,
                        },
                    );
                    model.files.sticky_headers = sticky_headers;
                    model.layout_state.geometry.list_inner = inner;
                    model.files.scroll = scroll;
                }
                draw_marks(drawer.frame, &model, self.palette);
                let (first, total) = if model.in_git_sidebar() {
                    (model.scm.scroll_scm, model.scm_rows().len())
                } else {
                    (model.files.scroll, model.files.visible.len())
                };
                let lane = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
                let bar = VerticalScrollbar::new(lane, first, total);
                model.layout_state.geometry.sidebar_scrollbar = if bar.geometry().is_some() {
                    lane
                } else {
                    Rect::default()
                };
                bar.draw(
                    drawer.frame,
                    self.palette.bg,
                    self.palette.fg_dim,
                    self.palette.accent,
                );
            }
            PaneKind::Preview => {
                model.commit_file_links.clear();
                model.layout_state.geometry.preview = area;
                // This region belongs to the last painted preview, not to the
                // previous file, diff, or wrap setting.
                model.layout_state.geometry.preview_horizontal_scrollbar = Rect::default();
                model.layout_state.geometry.preview_horizontal_max = 0;
                area = draw_preview_heading(drawer.frame, area, &mut model, self.palette);
                if commit_detail::draw(drawer.frame, area, &mut model, self.palette) {
                    model.layout_state.geometry.overview_ruler = Rect::default();
                    model.layout_state.geometry.preview_scrollbar = Rect::default();
                    model.layout_state.geometry.file_find_rect = Rect::default();
                } else {
                    // A 3-column focus ruler, not a code minimap: preserve source width.
                    let ruler_area = if model.config.show_overview_ruler
                        && area.width >= 36
                        && area.height >= 5
                        && matches!(model.preview_state.preview_mode, PreviewMode::File)
                        && (!model.markdown_available() || model.preview_state.markdown_code)
                        && matches!(
                            model.preview_state.preview.as_ref().map(|p| &p.content),
                            Some(PreviewContent::Text { .. })
                        ) {
                        let sidebar = Rect::new(area.right() - 3, area.y, 3, area.height);
                        area.width -= 3;
                        Some(sidebar)
                    } else {
                        None
                    };
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
                    let previous_width = model.preview_state.presentation.text_rows.width();
                    let clicked_source = if previous_width != area.width
                        || model.preview_state.presentation.text_rows.wrap_enabled()
                            != model.config.wrap_code
                    {
                        model.blame.cursor.as_ref().and_then(|(path, row, scroll)| {
                            (Some(path) == model.preview_state.preview.as_ref().map(|p| &p.path)
                                && *scroll == model.preview_state.presentation.preview_scroll)
                                .then(|| {
                                    model
                                        .preview_state
                                        .presentation
                                        .text_rows
                                        .source_line_at(*row)
                                })
                                .flatten()
                                .map(|line| (path.clone(), line, *scroll))
                        })
                    } else {
                        None
                    };
                    // Rebuild source-row mapping before asking whether inline
                    // Blame has a line to show. A same-file external refresh
                    // clears this cache; querying attribution first would hide
                    // its status row for one frame, expand the text viewport,
                    // and permanently clamp a reader positioned at EOF.
                    if model.config.git.blame.show_inline
                        && !model.preview_state.preview_loading
                        && matches!(model.preview_state.preview_mode, PreviewMode::File)
                    {
                        let wrap = model.config.wrap_code;
                        model
                            .preview_state
                            .prepare_source_rows(&theme, area.width, wrap);
                        if let Some((path, source, _)) = clicked_source {
                            model.blame.cursor = Some((
                                path,
                                model
                                    .preview_state
                                    .presentation
                                    .text_rows
                                    .source_to_row(source),
                                model.preview_state.presentation.preview_scroll,
                            ));
                        }
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
                    let markdown_available = model.markdown_available();
                    let hint = if model.preview_state.needs_empty_diff_hint() {
                        model.empty_diff_hint()
                    } else {
                        ""
                    };
                    let wrap = model.config.wrap_code;
                    let root = model.files.root().to_path_buf();
                    let preview_state::PreviewFrame {
                        inner,
                        scroll,
                        horizontal_bar,
                    } = model.preview_state.render_frame(
                        drawer.frame,
                        area,
                        &theme,
                        &root,
                        markdown_available,
                        hint,
                        wrap,
                    );
                    model.layout_state.geometry.preview_inner = inner;
                    if let Some(bar) = blame_bar {
                        draw_inline_blame(drawer.frame, inner, bar, &model, self.palette);
                    }
                    model.draw_inline_diff(drawer.frame, inner);
                    model.layout_state.geometry.file_find_rect = Rect::default();
                    if model.preview_state.find.open {
                        super::file_find_ui::highlight_file_find_matches(
                            drawer.frame,
                            inner,
                            &model,
                            &theme,
                        );
                        model.layout_state.geometry.file_find_rect =
                            super::file_find_ui::draw_file_find_bar(
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
                            let first_line = model
                                .preview_state
                                .presentation
                                .text_rows
                                .source_line_at(scroll)
                                .unwrap_or(1)
                                .saturating_sub(1);
                            VerticalScrollbar::new(ruler, first_line, source_total).draw(
                                drawer.frame,
                                self.palette.bg,
                                self.palette.fg_dim,
                                self.palette.accent,
                            );
                        } else {
                            VerticalScrollbar::new(ruler, scroll, model.preview_total_lines())
                                .draw(
                                    drawer.frame,
                                    self.palette.bg,
                                    self.palette.fg_dim,
                                    self.palette.accent,
                                );
                        }
                        model.layout_state.geometry.overview_ruler = ruler;
                        model.layout_state.geometry.preview_scrollbar = Rect::default();
                    } else {
                        model.layout_state.geometry.overview_ruler = Rect::default();
                        if let Some(lane) = scrollbar_lane {
                            // Match the actual viewport after optional bottom bars.
                            let lane = Rect::new(lane.x, inner.y, lane.width, inner.height);
                            let total = model.preview_total_lines();
                            if VerticalScrollbar::new(lane, scroll, total)
                                .geometry()
                                .is_some()
                            {
                                model.layout_state.geometry.preview_scrollbar = lane;
                                VerticalScrollbar::new(lane, scroll, total).draw(
                                    drawer.frame,
                                    self.palette.bg,
                                    self.palette.fg_dim,
                                    self.palette.accent,
                                );
                            } else {
                                model.layout_state.geometry.preview_scrollbar = Rect::default();
                            }
                        } else {
                            model.layout_state.geometry.preview_scrollbar = Rect::default();
                        }
                    }
                    if let Some((bar, max_x)) = horizontal_bar {
                        let scrollbar = HorizontalScrollbar::new(
                            bar,
                            model.preview_state.presentation.preview_scroll_x,
                            (bar.width as usize).saturating_add(max_x),
                        );
                        if scrollbar.geometry().is_some() {
                            model.layout_state.geometry.preview_horizontal_scrollbar = bar;
                            model.layout_state.geometry.preview_horizontal_max = max_x;
                            scrollbar.draw(
                                drawer.frame,
                                self.palette.bg,
                                self.palette.border,
                                self.palette.accent,
                            );
                        }
                    }
                }
            }
            PaneKind::Divider => {
                model.layout_state.geometry.sidebar_divider = area;
                Splitter::draw(
                    drawer.frame,
                    area,
                    SplitterAxis::Vertical,
                    model
                        .layout_state
                        .splitter
                        .highlighted(SplitterAxis::Vertical),
                    self.palette.border,
                    self.palette.accent,
                );
            }
            PaneKind::TerminalDivider => {
                model.layout_state.geometry.terminal_divider = area;
                Splitter::draw(
                    drawer.frame,
                    area,
                    SplitterAxis::Horizontal,
                    model
                        .layout_state
                        .splitter
                        .highlighted(SplitterAxis::Horizontal),
                    self.palette.border,
                    self.palette.accent,
                );
            }
            PaneKind::Terminal => {
                let inner =
                    terminal_tabs::draw_tab_strip(drawer.frame, area, &mut model, self.palette);
                // Cursor ownership follows exactly the normal text-input route;
                // dialogs, editors and active find inputs must never inherit it.
                let owns_keyboard = model.typing_owner() == input_router::KeyOwner::Terminal;
                if let Some(term) = &mut model.terminals.active {
                    // The child PTY's default colors belong to the outer terminal, not
                    // Forge's theme. Explicit ANSI/RGB colors still come from vt100.
                    term.render(
                        drawer.frame,
                        inner,
                        Color::Reset,
                        Color::Reset,
                        owns_keyboard,
                    );
                }
                terminal_tabs::draw_copy_notice(drawer.frame, inner, &model, self.palette);
                terminal_tabs::draw_tab_tooltip(drawer.frame, area, &model, self.palette);
            }
            PaneKind::Footer => {
                model.layout_state.geometry.git_sync_status =
                    draw_workspace_footer(drawer.frame, area, &model, self.palette);
                model.layout_state.geometry.footer = area;
                model.layout_state.geometry.hotkeys = Rect::new(
                    area.x,
                    area.bottom().saturating_sub(1),
                    area.width,
                    area.height.min(1),
                );
                activity_bar::draw_workspace_tooltip(drawer.frame, &model, self.palette);
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
        model.layout_state.geometry.window = drawer.frame.area();
        model.layout_state.geometry.dual_pane = drawer.frame.area().width >= 80;
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
        if cache_backdrop {
            let mut cached = ratatui::buffer::Buffer::empty(cache_area);
            copy_pane_cells(buffer, &mut cached, cache_area);
            self.editor_backdrop = Some((theme, cached));
        }
    }
}

fn copy_pane_cells(
    source: &ratatui::buffer::Buffer,
    target: &mut ratatui::buffer::Buffer,
    area: Rect,
) {
    for y in area.y..area.bottom() {
        if area.width > 0 {
            let source_start = source.index_of(area.x, y);
            let target_start = target.index_of(area.x, y);
            let width = area.width as usize;
            target.content[target_start..target_start + width]
                .clone_from_slice(&source.content[source_start..source_start + width]);
        }
    }
}

// Keep the component tree and layout here; drawing implementations live in
// independent modules so UI changes stay scoped to their specific panel.
mod decorations;
mod footer;
mod help_guide;
mod preview_content;
mod tools_panel;
mod workspace;

pub(crate) use decorations::draw_blame_popup;
use decorations::*;
use footer::*;
use help_guide::*;
pub(crate) use preview_content::bridge_theme;
use preview_content::*;
use tools_panel::*;
use workspace::*;

#[cfg(test)]
mod tests;

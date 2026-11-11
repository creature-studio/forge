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

// Keep the terminal's own foreground and background. Accent belongs to small
// focus marks; selection is a quiet neutral surface, like NSH's TUI controls.
fn workspace_palette() -> Palette {
    let mut palette = Palette::default();
    palette.selection = Color::DarkGray;
    palette.on_accent = Color::White;
    palette
}

static PALETTE: Atom<Palette> = Atom::new(workspace_palette);

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
    let palette = hooks.use_atom(&PALETTE).get();
    let completed = app.read().services.jobs.completion_notifier();
    hooks.use_future(async move {
        loop {
            completed.notified().await;
            let mut model = app.write();
            model.poll_pending_list();
            model.poll_pending_status();
            model.poll_pending_preview();
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
    // Watch only the root and the directories actually opened in the tree.
    // Registering Recursive on an entire Java/Node monorepo is synchronous in
    // notify/inotify and can block the Tokio UI thread for seconds/minutes,
    // leaving the Files panel stuck at "listing ...". Git metadata gets its
    // own shallow watch so stage/branch changes still refresh.
    let observed_app = app;
    hooks.use_future(async move {
        use notify::{RecursiveMode, Watcher};
        use std::collections::HashSet;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    let _ = sender.send(event);
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
                    while let Ok(event) = receiver.try_recv() {
                        changes.push(event);
                    }
                    let git_metadata_changed = git_watch.as_ref().is_some_and(|paths|
                        changes.iter().any(|event| paths.matches_event(event))
                    );
                    if !git_metadata_changed
                        && !changes.iter().any(|event| super::auto_refresh::relevant(event, &root))
                    {
                        continue;
                    }
                    let mut model = observed_app.write();
                    if model.files.tree_root == root {
                        model.refresh_external_changes(git_metadata_changed);
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
                View(height: Constraint::Length(if height >= 12 { 3 } else if height >= 5 { 2 } else { 1 })) {
                    NativePane(kind: PaneKind::Footer)
                }
            }
            if menu_open {
                Modal(open: true, width: Constraint::Length(0), height: Constraint::Length(0)) {
                    Menu()
                }
            }
            if commit_open {
                Modal(open: true, width: Constraint::Percentage(70), height: Constraint::Length(7)) {
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

#[component]
fn CommitEditor(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app = *hooks.use_context::<State<App>>();
    let palette = hooks.use_palette();
    let draft = hooks.use_state(tui_input::Input::default);
    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, move |event| {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => {
                    let mut model = app.write();
                    model.session.commit_open = false;
                    model.session.status = "commit cancelled".into();
                }
                KeyCode::Enter => {
                    let message = draft.read().value().trim().to_owned();
                    let mut model = app.write();
                    if message.is_empty() {
                        model.session.status = "empty commit message".into();
                    } else {
                        model.session.commit_open = false;
                        model.request_commit(message);
                    }
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {}
                _ => {
                    draft.write().handle_event(&Event::Key(key));
                }
            },
            Event::Paste(text) => {
                let mut input = draft.write();
                for ch in text.chars().filter(|ch| !ch.is_control()) {
                    input.handle_event(&Event::Key(KeyEvent::new(
                        KeyCode::Char(ch),
                        KeyModifiers::NONE,
                    )));
                }
            }
            _ => {}
        }
        EventResult::Consumed
    });
    element!(Border(top_title: Line::from(" Commit "), flex_direction: Direction::Vertical) {
        View(height: Constraint::Length(1)) { Text(text: "Commits ALL staged files in this repository", style: Style::default().fg(palette.fg_dim)) }
        View(height: Constraint::Length(1)) { Input(input: draft.read().clone(), placeholder: "Commit message") }
        View(height: Constraint::Length(1)) { Text(text: "Enter commit   Esc cancel", style: Style::default().fg(palette.fg_dim)) }
        View(height: Constraint::Length(1)) { Text(text: app.read().session.status.clone(), style: Style::default().fg(palette.warning)) }
    })
}

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
            }
            PaneKind::Preview => {
                model.session.layout.preview = area;
                area = draw_preview_heading(drawer.frame, area, &mut model, self.palette);
                if model.preview_state.text_rows.width() != area.width {
                    model.blame.cursor = None;
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
                let (_, content) = preview_content(&model);
                let numbered = matches!(content, PreviewView::Text { .. });
                let inner = match content {
                    PreviewView::Text {
                        lines,
                        truncated,
                        bytes_read,
                        language,
                    } => {
                        text_cache
                            .ensure(&theme, lines, truncated, bytes_read, language, area.width);
                        if let Some(line) = pending_line {
                            scroll = text_cache.source_to_row(line);
                        }
                        draw_preview(
                            drawer.frame,
                            area,
                            &theme,
                            PreviewView::TextRows {
                                rows: text_cache.rows(),
                            },
                            &mut scroll,
                            &mut cache,
                        )
                    }
                    other => {
                        draw_preview(drawer.frame, area, &theme, other, &mut scroll, &mut cache)
                    }
                };
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
            }
            PaneKind::Divider => {
                drawer.frame.render_widget(
                    Block::default()
                        .borders(Borders::LEFT)
                        .border_style(Style::default().fg(self.palette.border)),
                    area,
                );
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
                drawer.frame.render_widget(Clear, area);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Git help ")
                    .title_bottom(" Esc / ? / q Close · j/k Scroll ")
                    .style(Style::default().fg(self.palette.fg).bg(self.palette.bg));
                let inner = block.inner(area);
                let lines = wrap_help(inner.width.max(1) as usize);
                model.session.help_scroll_limit = lines
                    .len()
                    .saturating_sub(inner.height as usize)
                    .min(u16::MAX as usize)
                    as u16;
                model.session.help_scroll = model
                    .session
                    .help_scroll
                    .min(model.session.help_scroll_limit);
                drawer.frame.render_widget(block, area);
                drawer.frame.render_widget(
                    Paragraph::new(lines).scroll((model.session.help_scroll, 0)),
                    inner,
                );
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

enum MarkRow {
    File(PathBuf),
    Git(PathBuf, DiffSide),
    None,
}

fn draw_inline_blame(frame: &mut Frame<'_>, inner: Rect, fallback: Rect, model: &App, p: Palette) {
    use unicode_width::UnicodeWidthStr;
    let Some((blame, line)) = model.current_blame() else {
        return;
    };
    let label = if blame.uncommitted() {
        "Uncommitted changes".to_string()
    } else {
        format!(
            "{} · {} · {} {}",
            blame.author,
            super::blame::relative_time(blame.time),
            &blame.oid[..blame.oid.len().min(8)],
            blame.summary
        )
    };
    // Place the attribution after the current source row when space allows.
    // Full-width source stays intact; its attribution uses the reserved strip.
    let row = model
        .blame
        .cursor
        .as_ref()
        .filter(|(_, _, scroll)| *scroll == model.preview_state.preview_scroll)
        .map_or(model.preview_state.preview_scroll, |(_, row, _)| *row);
    let y = inner
        .y
        .saturating_add(row.saturating_sub(model.preview_state.preview_scroll) as u16);
    let mut end = inner.x;
    if y < inner.bottom() {
        for x in inner.x..inner.right() {
            if let Some(cell) = frame.buffer_mut().cell((x, y)) {
                if cell.symbol() != " " {
                    end = end.max(x.saturating_add(cell.symbol().width() as u16));
                }
            }
        }
    }
    let available = inner.right().saturating_sub(end.saturating_add(3));
    let area = if y < inner.bottom() && available >= label.width().min(40) as u16 {
        Rect::new(end.saturating_add(3), y, available, 1)
    } else {
        fallback
    };
    let text = if area == fallback {
        format!("L{line}  {label}")
    } else {
        label
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(p.fg_dim)),
        area,
    );
}

pub(super) fn draw_blame_popup(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let Some(popup) = &mut model.blame.popup else {
        return;
    };
    frame.render_widget(Clear, area);
    let committed = !popup.oid.bytes().all(|b| b == b'0');
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", popup.title))
        .title_bottom(if committed {
            " Esc Close · d Diff · h History · c Copy SHA · F6 Details "
        } else {
            " Esc Close · Uncommitted changes "
        })
        .style(Style::default().fg(p.fg).bg(p.bg));
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if !popup.message.is_empty() && inner.height > 1 {
        frame.render_widget(
            Paragraph::new(popup.message.clone()).style(Style::default().fg(p.fg_dim)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
        inner.height -= 1;
    }
    let rows = popup
        .text
        .lines()
        .flat_map(|line| forge_ui::wrap_text_line(&Line::from(line.to_owned()), inner.width.max(1)))
        .collect::<Vec<_>>();
    popup.max_scroll = rows
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    popup.scroll = popup.scroll.min(popup.max_scroll);
    let visible = rows
        .into_iter()
        .skip(popup.scroll as usize)
        .take(inner.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_marks(frame: &mut Frame<'_>, model: &App, p: Palette) {
    let area = model.session.layout.list_inner;
    if area.width == 0 {
        return;
    }
    let rows: Vec<MarkRow> = if model.in_git_sidebar() {
        let root = model.repo_root().unwrap_or_default();
        let (u, s) = model.scm_slices();
        let rows = model.scm_rows();
        let indices = model
            .scm
            .sticky_headers
            .iter()
            .copied()
            .chain(model.scm.scroll_scm..rows.len());
        indices
            .take(area.height as usize)
            .map(|index| match &rows[index] {
                ScmRow::Folder { path, column, .. } => MarkRow::Git(root.join(path), column.side()),
                row @ ScmRow::Change { .. } => scm_entry(row, u, s)
                    .map(|entry| MarkRow::Git(root.join(&entry.path), entry.side))
                    .unwrap_or(MarkRow::None),
                _ => MarkRow::None,
            })
            .collect()
    } else {
        let indices = model
            .files
            .sticky_headers
            .iter()
            .copied()
            .chain(model.files.scroll..model.files.visible.len());
        indices
            .take(area.height as usize)
            .map(|index| MarkRow::File(model.files.visible[index].entry.path.clone()))
            .collect()
    };
    for (offset, row) in rows.iter().take(area.height as usize).enumerate() {
        let marked = match row {
            MarkRow::File(path) => model.tools.marks.contains_file(path),
            MarkRow::Git(path, side) => model.tools.marks.contains_git(path, *side),
            MarkRow::None => false,
        };
        if marked {
            frame.render_widget(
                Paragraph::new("●").style(Style::default().fg(p.warning)),
                Rect::new(area.x, area.y + offset as u16, 1, 1),
            );
        }
    }
}

fn draw_tools(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    if model
        .tools
        .panel
        .as_ref()
        .is_some_and(|panel| matches!(panel.kind, tools::PanelKind::Settings))
    {
        draw_settings(frame, area, model, p);
        return;
    }
    let loading = model.tools.pending_read.is_some();
    let context = model.tools.content_context.as_ref();
    let context_path = model.tools.context_path.as_deref();
    let Some(panel) = &mut model.tools.panel else {
        return;
    };
    panel.area = area;
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", panel.title))
        .title_bottom(" Esc Back · ↑/↓,PgUp/PgDn List · Ctrl+PgUp/PgDn Details ")
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let input_height = if panel.is_input() { 2 } else { 0 };
    let list_height = if panel.rows.is_empty() {
        0
    } else if matches!(panel.kind, tools::PanelKind::Commands) {
        // Command search favors visible matches, not a half-screen blank detail.
        inner.height.saturating_sub(input_height + 2).max(1)
    } else {
        (inner.height.saturating_sub(input_height + 3) / 2).max(1)
    };
    let rows = Layout::vertical([
        Constraint::Length(input_height),
        Constraint::Length(1),
        Constraint::Length(list_height),
        Constraint::Min(0),
    ])
    .split(inner);
    if panel.is_input() {
        let value = panel.input.value();
        let cursor = panel.input.visual_cursor();
        let start = cursor.saturating_sub(rows[0].width.saturating_sub(4) as usize);
        let mut skipped = 0;
        let text: String = value
            .chars()
            .skip_while(|ch| {
                let width = unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0);
                if skipped + width <= start {
                    skipped += width;
                    true
                } else {
                    false
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(format!("> {text}")), rows[0]);
        if rows[0].width > 2 {
            frame.set_cursor_position((
                rows[0].x + 2 + (cursor.saturating_sub(skipped) as u16).min(rows[0].width - 3),
                rows[0].y,
            ));
        }
    }
    let message = if loading
        && !matches!(
            panel.kind,
            tools::PanelKind::Files | tools::PanelKind::Content
        ) {
        format!("Loading… {}", panel.message)
    } else {
        panel.message.clone()
    };
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(p.warning)),
        rows[1],
    );
    if matches!(panel.kind, tools::PanelKind::Content) {
        let body = Rect::new(
            inner.x,
            rows[2].y,
            inner.width,
            inner.bottom().saturating_sub(rows[2].y),
        );
        draw_content_search(
            frame,
            body,
            panel,
            &model.files.tree_root,
            context_path,
            context,
            loading,
            p,
        );
        return;
    }
    panel.list_area = rows[2];
    panel.list_offset = panel.list_offset.min(
        panel
            .rows
            .len()
            .saturating_sub(panel.list_area.height as usize),
    );
    let offset = panel.list_offset;
    for (i, row) in panel
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows[2].height as usize)
    {
        let label: String = row
            .label
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let style = if i == panel.selected {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        frame.render_widget(
            Paragraph::new(format!(
                "{} {label}",
                if i == panel.selected { "›" } else { " " }
            ))
            .style(style),
            Rect::new(rows[2].x, rows[2].y + (i - offset) as u16, rows[2].width, 1),
        );
    }
    let detail_lines = wrap_detail(&panel.detail, rows[3].width.max(1) as usize);
    panel.scroll_limit = detail_lines.len().saturating_sub(rows[3].height as usize);
    let details = Paragraph::new(detail_lines);
    panel.scroll = panel.scroll.min(panel.scroll_limit);
    frame.render_widget(
        details
            .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0))
            .style(Style::default().fg(p.fg_dim)),
        rows[3],
    );
}

/// Crop by display cells around the first match, preserving UTF-8 byte offsets.
fn content_line(
    text: &str,
    width: usize,
    matcher: Option<&forge_fs::workspace::ContentMatcher>,
    p: Palette,
) -> Line<'static> {
    let ranges = matcher.map(|m| m.ranges(text)).unwrap_or_default();
    let hit = ranges.first().map_or(0, |r| r.start);
    let before = unicode_width::UnicodeWidthStr::width(&text[..hit]);
    let skip = before.saturating_sub(width / 3);
    let mut cells = 0;
    let mut used = 0;
    let mut spans = Vec::new();
    if skip > 0 && width > 0 {
        spans.push(Span::raw("…"));
        used += 1;
    }
    for (byte, ch) in text.char_indices() {
        let ch = if ch.is_control() { ' ' } else { ch };
        let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        cells += size;
        if cells <= skip {
            continue;
        }
        if used + size > width {
            break;
        }
        used += size;
        let style = if ranges.iter().any(|r| r.contains(&byte)) {
            Style::default()
                .fg(p.bg)
                .bg(p.warning)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
fn draw_content_search(
    frame: &mut Frame<'_>,
    area: Rect,
    panel: &mut tools::Panel,
    root: &Path,
    context_path: Option<&Path>,
    context: Option<&std::result::Result<forge_fs::workspace::ContentContext, String>>,
    updating: bool,
    p: Palette,
) {
    let panes = if area.width >= 95 {
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area)
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area)
    };
    let matcher =
        if updating || panel.message.starts_with("Failed:") || panel.input.value().is_empty() {
            None
        } else {
            forge_fs::workspace::ContentMatcher::new(panel.input.value(), panel.regex).ok()
        };
    let list_block = Block::default().borders(Borders::ALL).title(format!(
        " {} results{} ",
        panel.rows.len(),
        if updating { " · Updating" } else { "" }
    ));
    let list = list_block.inner(panes[0]);
    frame.render_widget(list_block, panes[0]);
    panel.list_area = list;
    panel.list_offset = panel
        .list_offset
        .min(panel.rows.len().saturating_sub(list.height as usize));
    let offset = panel.list_offset;
    for (index, row) in panel
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
    {
        let boundary = index == 0
            || match (&panel.rows[index - 1].value, &row.value) {
                (tools::RowValue::File(a, _), tools::RowValue::File(b, _)) => a != b,
                _ => true,
            };
        let style = if index == panel.selected {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        let mut line = content_line(
            &row.label,
            list.width.saturating_sub(2) as usize,
            matcher.as_ref(),
            p,
        );
        line.spans.insert(
            0,
            Span::raw(if index == panel.selected {
                "› "
            } else if boundary {
                "◆ "
            } else {
                "│ "
            }),
        );
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(list.x, list.y + (index - offset) as u16, list.width, 1),
        );
    }
    let selected = panel.rows.get(panel.selected).and_then(|r| match &r.value {
        tools::RowValue::File(path, line) => Some((path, *line, &r.label)),
        _ => None,
    });
    let title = selected
        .map(|(path, line, _)| {
            let count = panel
                .rows
                .iter()
                .filter(|r| matches!(&r.value, tools::RowValue::File(p, _) if p == path))
                .count();
            let column = context
                .filter(|_| context_path == Some(path.as_path()))
                .and_then(|c| c.as_ref().ok())
                .and_then(|c| c.lines.get(line.saturating_sub(1)))
                .and_then(|text| {
                    matcher.as_ref().and_then(|m| {
                        m.ranges(text)
                            .first()
                            .map(|r| text[..r.start].chars().count() + 1)
                    })
                })
                .map(|column| format!(":{column}"))
                .unwrap_or_default();
            format!(
                " {}:{line}{column} · {count} matches ",
                path.strip_prefix(root).unwrap_or(path).display()
            )
        })
        .unwrap_or_else(|| " Context ".into());
    let block = Block::default().borders(Borders::ALL).title(title);
    let preview = block.inner(panes[1]);
    frame.render_widget(block, panes[1]);
    let Some((path, selected_line, fallback)) = selected else {
        frame.render_widget(Paragraph::new("Type to search · Ctrl-R regex\nSelect a result to preview surrounding code\nEnter opens the exact line"), preview);
        return;
    };
    let context = if context_path == Some(path.as_path()) {
        context
    } else {
        None
    };
    match context {
        Some(Ok(context)) => {
            let height = preview.height as usize;
            let start = selected_line
                .saturating_sub(1)
                .saturating_sub((height / 2).min(5));
            panel.scroll_limit = context.lines.len().saturating_sub(start + height);
            panel.scroll = panel.scroll.min(panel.scroll_limit);
            for (index, text) in context
                .lines
                .iter()
                .enumerate()
                .skip(start + panel.scroll)
                .take(height)
            {
                let active = index + 1 == selected_line;
                let prefix = format!("{}{:>5} │ ", if active { "›" } else { " " }, index + 1);
                let mut line = content_line(
                    text,
                    preview.width.saturating_sub(9) as usize,
                    matcher.as_ref(),
                    p,
                );
                line.spans.insert(
                    0,
                    Span::styled(
                        prefix,
                        Style::default().fg(if active { p.accent } else { p.fg_dim }),
                    ),
                );
                let style = if active {
                    Style::default().bg(p.selection).fg(p.on_accent)
                } else {
                    Style::default().fg(p.fg)
                };
                frame.render_widget(
                    Paragraph::new(line).style(style),
                    Rect::new(
                        preview.x,
                        preview.y + (index - start - panel.scroll) as u16,
                        preview.width,
                        1,
                    ),
                );
            }
        }
        other => {
            let message = match other {
                Some(Err(error)) => error.as_str(),
                _ => "Loading context…",
            };
            frame.render_widget(
                Paragraph::new(format!("{message}\n{fallback}"))
                    .style(Style::default().fg(p.fg_dim)),
                preview,
            );
        }
    }
}

fn draw_settings(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let Some(panel) = model.tools.panel.as_mut() else {
        return;
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Settings ")
        .title_bottom(" Up/Down Select · Enter/Space Toggle · Esc Close ")
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    panel.area = area;
    panel.list_area = Rect::default();
    if inner.height < 3 || inner.width < 10 {
        return;
    }

    let header = " APPEARANCE & NAVIGATION ";
    frame.render_widget(
        Paragraph::new(header).style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let rows_y = inner.y + 2;
    let visible = (inner.bottom().saturating_sub(rows_y)).min(4);
    panel.list_area = Rect::new(inner.x, rows_y, inner.width, visible);

    for i in 0..visible as usize {
        let chosen = i == panel.selected;
        let style = if chosen {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg).bg(p.bg)
        };
        let label = match i {
            0 => "Tree Sticky Scroll",
            1 => "Compact Folders",
            2 => "Show hidden files",
            _ => "Keyboard shortcuts",
        };
        let value = match i {
            0 => {
                if model.config.tree_sticky_scroll {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            1 => {
                if model.config.compact_folders {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            2 => {
                if model.config.show_hidden_files {
                    "[ On ]"
                } else {
                    "[ Off ]"
                }
            }
            _ => "[ Open F4 ]",
        };
        let width = inner.width as usize;
        let content = format!(
            " {} {:width$} {}",
            if chosen { "›" } else { " " },
            label,
            value,
            width = width.saturating_sub(value.len() + 5).min(45)
        );
        frame.render_widget(
            Paragraph::new(content).style(style),
            Rect::new(inner.x, rows_y + i as u16, inner.width, 1),
        );
    }
    if inner.height >= 10 {
        let detail_y = rows_y + 6;
        let desc = match panel.selected {
            0 => "Pin expanded parent directories above the scrolled tree.\nApplies to Files, Git Changes/Staged and Commit History.",
            1 => "Join consecutive single-child folder paths into one row.\nApplies to Files, Git Changes/Staged and Commit History.",
            2 => "Show dotfiles in the Files tree.\n.git remains hidden; tracked changes in Git are unaffected.",
            _ => "Open the searchable F4 keyboard shortcuts view.\nExisting custom bindings remain unchanged.",
        };
        let detail = format!(" DESCRIPTION\n\n{desc}\n\nChanges save to forge/config.toml and survive a restart.\n{}",
            if panel.message.is_empty() { "" } else { &panel.message });
        frame.render_widget(
            Paragraph::new(detail).style(Style::default().fg(p.fg_dim)),
            Rect::new(
                inner.x,
                detail_y,
                inner.width,
                inner.bottom().saturating_sub(detail_y),
            ),
        );
    }
}

fn wrap_detail(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cells = 0;
        let mut part = String::new();
        for ch in line.chars() {
            let ch = if ch == '\t' { ' ' } else { ch };
            let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cells + size > width && !part.is_empty() {
                out.push(Line::from(std::mem::take(&mut part)));
                cells = 0;
            }
            part.push(ch);
            cells += size;
        }
        out.push(Line::from(part));
    }
    out
}

fn draw_workspace_header(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    // The footer owns the branch and upstream divergence. Use the header
    // for complementary workspace counts instead of duplicating or cropping
    // a raw "## branch...origin" line.
    let branch = if area.width >= 70 {
        if let Some(status) = &model.scm.git_status {
            Line::from(vec![
                Span::styled(
                    format!("{} changed", status.unstaged.len()),
                    Style::default().fg(p.warning),
                ),
                Span::styled(
                    format!("  {} staged", status.staged.len()),
                    Style::default().fg(p.success),
                ),
            ])
        } else {
            Line::default()
        }
    } else {
        Line::default()
    };
    let right_width = branch
        .width()
        .min(area.width.saturating_sub(4) as usize / 2);
    let path_width = (area.width as usize)
        .saturating_sub(14 + right_width + if right_width > 0 { 2 } else { 0 });
    draw_balanced_line(
        frame,
        rows[0],
        Line::from(vec![
            Span::styled(
                "forge",
                Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  /  ", Style::default().fg(p.fg_dim)),
            Span::styled(
                fit_label(
                    &model.files.tree_root.display().to_string(),
                    path_width,
                    true,
                ),
                Style::default().fg(p.fg_dim),
            ),
        ]),
        branch,
        p,
    );
    let tabs = if area.height > 1 { rows[1] } else { rows[0] };
    let widths = if area.width >= 32 {
        (14, 12)
    } else {
        (area.width.min(9), area.width.saturating_sub(9).min(7))
    };
    let files = Rect::new(tabs.x, tabs.y, widths.0, tabs.height.min(1));
    let git = Rect::new(files.right(), tabs.y, widths.1, tabs.height.min(1));
    for (rect, label, active) in [
        (
            files,
            if area.width >= 32 {
                " F1  Files "
            } else {
                " Files "
            },
            !model.in_git_sidebar(),
        ),
        (
            git,
            if area.width >= 32 {
                " F2  Git "
            } else {
                " Git "
            },
            model.in_git_sidebar(),
        ),
    ] {
        let style = if active {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.fg_dim).bg(p.bg)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
    }
    model.session.layout.files_tab = files;
    model.session.layout.git_tab = git;
}

fn draw_pane_heading(
    frame: &mut Frame<'_>,
    area: Rect,
    p: Palette,
    title: &str,
    detail: &str,
    focused: bool,
) -> Rect {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let title_style = Style::default().fg(if focused { p.fg } else { p.fg_dim });
    let detail_width = (area.width as usize).saturating_sub(title.len() + 7);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if focused { " › " } else { "   " },
                Style::default().fg(p.accent),
            ),
            Span::styled(
                title.to_owned(),
                if focused {
                    title_style.add_modifier(Modifier::BOLD)
                } else {
                    title_style
                },
            ),
            Span::styled(
                format!("  {}", fit_label(detail, detail_width, true)),
                Style::default().fg(p.fg_dim),
            ),
        ])),
        rows[0],
    );
    let body = rows[1];
    // A single blank row gives headings space on a normal-sized terminal.
    let gap = u16::from(area.height >= 6);
    Rect::new(
        body.x,
        body.y + gap,
        body.width,
        body.height.saturating_sub(gap),
    )
}

fn draw_preview_heading(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) -> Rect {
    model.session.layout.markdown_preview_tab = Rect::default();
    model.session.layout.markdown_code_tab = Rect::default();
    model.session.layout.open_file_tab = Rect::default();
    let open_file_chip = if area.width >= 34
        && matches!(model.preview_state.preview_mode, PreviewMode::Diff)
        && model.preview_state.file_diff.as_ref().is_some_and(|diff| {
            diff.repo_root.join(&diff.path).is_file() && model.diff_matches_selection(diff)
        }) {
        14
    } else {
        0
    };
    let tabs_width = if model.markdown_available() && area.width >= 24 {
        area.width.saturating_sub(12).min(24)
    } else {
        0
    };
    let (mut detail, _) = preview_content(model);
    if model.inline_diff_hint() {
        detail = format!("F7 Peek · {detail}");
    }
    let body = draw_pane_heading(
        frame,
        Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(tabs_width + open_file_chip),
            area.height,
        ),
        p,
        if model.session.activity == ActivityTab::Git
            && matches!(model.selected_scm_row(), Some(ScmRow::Commit { .. }))
        {
            "Commit"
        } else if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
            "Diff"
        } else {
            "Preview"
        },
        &detail,
        model.session.focus == PaneFocus::Preview,
    );
    if open_file_chip > 0 {
        let rect = Rect::new(area.right() - open_file_chip, area.y, open_file_chip, 1);
        frame.render_widget(
            Paragraph::new(" [Open file] ").style(
                Style::default()
                    .fg(p.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            rect,
        );
        model.session.layout.open_file_tab = rect;
    }
    if tabs_width > 0 {
        draw_markdown_tabs(
            frame,
            Rect::new(
                area.right() - tabs_width,
                area.y,
                tabs_width,
                area.height.min(1),
            ),
            model,
            p,
        );
    }
    Rect::new(body.x, body.y, area.width, body.height)
}

fn draw_markdown_tabs(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    let compact = area.width < 19;
    let preview_width = area.width.min(if compact { 8 } else { 11 });
    let preview = Rect::new(area.x, area.y, preview_width, area.height.min(1));
    let code = Rect::new(
        preview.right(),
        area.y,
        area.width.saturating_sub(preview_width).min(8),
        area.height.min(1),
    );
    for (rect, label, active) in [
        (
            preview,
            if compact { "Preview" } else { " Preview " },
            !model.preview_state.markdown_code,
        ),
        (
            code,
            if compact { "Code" } else { " Code " },
            model.preview_state.markdown_code,
        ),
    ] {
        let style = if active {
            Style::default()
                .fg(p.on_accent)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.fg_dim).bg(p.bg)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
    }
    if area.width >= 24 {
        frame.render_widget(
            Paragraph::new("F3").style(Style::default().fg(p.fg_dim)),
            Rect::new(code.right() + 1, area.y, 2, area.height.min(1)),
        );
    }
    model.session.layout.markdown_preview_tab = preview;
    model.session.layout.markdown_code_tab = code;
}

fn draw_workspace_footer(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) -> Rect {
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    if rows[0].height > 0 {
        let rule_height = u16::from(rows[0].height >= 2);
        if rule_height > 0 {
            frame.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(p.border)),
                rows[0],
            );
        }
        let status_area = Rect::new(area.x, rows[0].y + rule_height, area.width, 1);
        let context = model.context_description();
        let error_status =
            |s: &str| s.contains("error") || s.contains("failed:") || s == "empty commit message";
        let status = if error_status(&model.session.status) {
            &model.session.status
        } else {
            model.session.write_status.as_deref().unwrap_or_else(|| {
                if model.session.status.is_empty() {
                    &context
                } else {
                    &model.session.status
                }
            })
        };
        let is_error = error_status(status);
        let position = if model.session.focus == PaneFocus::Preview {
            format!("Preview  line {}", model.preview_state.preview_scroll + 1)
        } else if model.in_git_sidebar() {
            format!(
                "Git  {} / {}",
                model.scm.scm_selected + 1,
                model.scm_rows().len()
            )
        } else {
            String::new()
        };
        draw_balanced_line(
            frame,
            status_area,
            Line::from(Span::styled(
                status.to_owned(),
                Style::default().fg(if is_error { p.error } else { p.fg_dim }),
            )),
            Line::from(Span::styled(position, Style::default().fg(p.fg_dim))),
            p,
        );
    }
    if area.width < 10 {
        frame.render_widget(
            Paragraph::new(if model.session.commit_open {
                "Esc Cancel"
            } else if model.session.help_open {
                "? Close"
            } else {
                "? Help"
            }),
            rows[1],
        );
        return Rect::default();
    }
    // A persistent, clickable Git status chip lives in the bottom-left
    // status line, VS Code style. It only opens the Sync panel: never Push.
    let chip = if !model.session.commit_open
        && !model.session.help_open
        && model.session.context_menu.is_none()
    {
        git_status_chip(model, p, (area.width as usize / 2).min(46))
    } else {
        None
    };
    let mut hitbox = Rect::default();
    let mut keys_area = rows[1];
    if let Some((content, width)) = chip {
        hitbox = Rect::new(keys_area.x, keys_area.y, width as u16, 1);
        frame.render_widget(Paragraph::new(content), hitbox);
        keys_area.x += width as u16;
        keys_area.width = keys_area.width.saturating_sub(width as u16);
    }
    let keys = footer_shortcuts(model, keys_area.width as usize);
    let mut spans = vec![Span::raw(" ")];
    for interaction::Shortcut { key, label } in keys {
        spans.push(Span::styled(
            format!(" {} ", model.keys.label(key)),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{label}  "),
            Style::default().fg(p.fg_dim),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), keys_area);
    hitbox
}

/// A compact branch/tracking chip. Counts come only from the Git status
/// response for the *current* branch. No upstream = no invented ↓0 ↑0.
fn git_status_chip(model: &App, p: Palette, max_width: usize) -> Option<(Line<'static>, usize)> {
    use super::git_sync_badge::GitSyncBadge;
    use unicode_width::UnicodeWidthStr;
    let badge = GitSyncBadge::parse(&model.scm.branch, &model.tools.sync)?;
    if model.scm.git_status.is_none() || max_width < 10 {
        return None;
    }

    let prefix = " ⎇ ";
    let suffix = " ↻ ";
    let counter = badge
        .upstream
        .map(|(behind, ahead)| format!(" ↓{behind} ↑{ahead}"));
    let missing = badge
        .upstream_missing
        .then_some(" upstream gone".to_owned());
    let candidate = counter.or(missing).unwrap_or_default();
    let available = max_width.saturating_sub(prefix.width() + suffix.width() + 1);
    let extra = if candidate.width() + 3 <= available {
        candidate
    } else {
        String::new()
    };
    let branch_width = available.saturating_sub(extra.width());
    if branch_width == 0 {
        return None;
    }
    let branch = fit_label(&badge.branch, branch_width, false);
    let mut spans = vec![
        Span::styled(prefix.to_string(), Style::default().fg(p.accent)),
        Span::styled(
            branch,
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if !extra.is_empty() {
        if let Some((behind, ahead)) = badge.upstream {
            spans.push(Span::styled(
                format!(" ↓{behind}"),
                Style::default().fg(p.fg_dim),
            ));
            spans.push(Span::styled(
                format!(" ↑{ahead}"),
                Style::default().fg(p.fg_dim),
            ));
        } else {
            spans.push(Span::styled(extra, Style::default().fg(p.warning)));
        }
    }
    spans.push(Span::styled(
        suffix.to_string(),
        Style::default().fg(p.accent),
    ));
    let line = Line::from(spans);
    let width = line.width();
    if width > max_width {
        return None;
    }
    Some((line, width))
}

fn footer_shortcuts(model: &App, width: usize) -> Vec<interaction::Shortcut> {
    let help_label = if model.session.help_open {
        "Close"
    } else {
        "Help"
    };
    let reserved = if model.session.commit_open {
        1
    } else {
        help_label.len() + 7
    };
    let mut remaining = width.saturating_sub(reserved);
    let mut keys = Vec::new();
    let candidates = if model.session.commit_open {
        vec![
            interaction::Shortcut {
                key: "Enter",
                label: "Commit staged",
            },
            interaction::Shortcut {
                key: "Esc",
                label: "Cancel",
            },
        ]
    } else if model.session.help_open {
        vec![
            interaction::Shortcut {
                key: "Esc",
                label: "Close help",
            },
            interaction::Shortcut {
                key: "j/k",
                label: "Scroll help",
            },
        ]
    } else if model.session.context_menu.is_some() {
        vec![
            interaction::Shortcut {
                key: "Enter",
                label: "Run action",
            },
            interaction::Shortcut {
                key: "j/k",
                label: "Select action",
            },
            interaction::Shortcut {
                key: "Esc",
                label: "Close menu",
            },
        ]
    } else {
        let mut shortcuts = model.context_shortcuts();
        if (60..80).contains(&width) {
            shortcuts.insert(
                0,
                interaction::Shortcut {
                    key: "Tab",
                    label: if model.session.focus == PaneFocus::Preview {
                        "Files"
                    } else {
                        "Preview"
                    },
                },
            );
        }
        shortcuts
    };
    for shortcut in candidates {
        let size = model.keys.label(shortcut.key).len() + shortcut.label.len() + 5;
        if size <= remaining {
            remaining -= size;
            keys.push(shortcut);
        }
    }
    if !model.session.commit_open {
        if remaining >= 18 {
            keys.push(interaction::Shortcut {
                key: "F4",
                label: "Shortcuts",
            });
        }
        keys.push(interaction::Shortcut {
            key: "?",
            label: if width >= 10 { help_label } else { "" },
        });
    }
    keys
}

fn wrap_help(width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    let mut lines = Vec::new();
    for source in interaction::HELP_TEXT.lines() {
        let mut line = String::new();
        for word in source.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(Line::from(std::mem::take(&mut line)));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(Line::from(line));
    }
    lines
}

/// Fit both ends independently so a long path or status cannot hide metadata.
fn draw_balanced_line(
    frame: &mut Frame<'_>,
    area: Rect,
    left: Line<'_>,
    right: Line<'_>,
    p: Palette,
) {
    let inner = Rect::new(
        area.x + area.width.min(2),
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );
    let right_width = right.width().min(inner.width as usize / 2) as u16;
    let gap = u16::from(right_width > 0) * 2;
    let left_width = inner.width.saturating_sub(right_width + gap);
    frame.render_widget(
        Paragraph::new(left).style(Style::default().fg(p.fg)),
        Rect::new(inner.x, inner.y, left_width, inner.height),
    );
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect::new(
            inner.right().saturating_sub(right_width),
            inner.y,
            right_width,
            inner.height,
        ),
    );
}

fn fit_label(text: &str, width: usize, keep_end: bool) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut remaining = width - 1;
    let chars: Vec<_> = if keep_end {
        text.chars().rev().collect()
    } else {
        text.chars().collect()
    };
    let mut kept = Vec::new();
    for ch in chars {
        let cells = ch.width().unwrap_or(0);
        if cells > remaining {
            break;
        }
        remaining -= cells;
        kept.push(ch);
    }
    if keep_end {
        format!("…{}", kept.into_iter().rev().collect::<String>())
    } else {
        format!("{}…", kept.into_iter().collect::<String>())
    }
}

fn preview_content(model: &App) -> (String, PreviewView<'_>) {
    if model.preview_state.preview_loading {
        return ("Loading".into(), PreviewView::Loading);
    }
    if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
        return match &model.preview_state.file_diff {
            Some(diff) => {
                let hunks = diff.hunks();
                let hunk = model.preview_state.hunk_index.and_then(|i| hunks.get(i));
                (
                    format!(
                        "{} · {}{}",
                        model.preview_state.diff_label,
                        diff.path,
                        model
                            .preview_state
                            .hunk_index
                            .map(|i| format!(" · hunk {}/{}", i + 1, hunks.len()))
                            .unwrap_or_default()
                    ),
                    PreviewView::Diff {
                        path: &diff.path,
                        side: &model.preview_state.diff_label,
                        lines: &diff.lines,
                        hunk_start: hunk.map(|h| h.start),
                        hunk_end: hunk.map(|h| h.end),
                        hunk_idx: model.preview_state.hunk_index.map(|i| (i, hunks.len())),
                    },
                )
            }
            None => (
                String::new(),
                PreviewView::Hint {
                    text: model.empty_diff_hint(),
                },
            ),
        };
    }
    let Some(preview) = &model.preview_state.preview else {
        return (String::new(), PreviewView::Idle);
    };
    let title = preview
        .path
        .strip_prefix(&model.files.tree_root)
        .unwrap_or(&preview.path)
        .display()
        .to_string();
    let content = match &preview.content {
        PreviewContent::Empty => PreviewView::Idle,
        PreviewContent::Directory {
            name,
            children,
            truncated,
        } => PreviewView::Directory {
            name,
            children,
            truncated: *truncated,
        },
        PreviewContent::Binary { notice, .. } => PreviewView::Binary { notice },
        PreviewContent::Error { message } => PreviewView::Error { message },
        PreviewContent::Text { .. }
            if model.markdown_available() && !model.preview_state.markdown_code =>
        {
            PreviewView::Markdown {
                preview: &model.preview_state.markdown,
            }
        }
        PreviewContent::Text {
            lines,
            truncated,
            bytes_read,
        } => PreviewView::Text {
            lines,
            truncated: *truncated,
            bytes_read: *bytes_read,
            language: TextLang::from_name(
                preview
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            ),
        },
    };
    (title, content)
}

/// Adapt semantic kit colors for the existing syntax/diff renderers. Reset
/// colors use neutral fallbacks because the legacy token type stores RGB only.
fn bridge_theme(p: Palette) -> forge_theme::Theme {
    use forge_theme::{vga, Rgb};
    fn rgb(c: Color, fallback: Rgb) -> Rgb {
        match c {
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
            Color::Black => vga::BLACK,
            Color::White => vga::WHITE,
            Color::Gray => vga::LIGHT_GRAY,
            Color::Cyan => vga::CYAN,
            Color::DarkGray => vga::DARK_GRAY,
            Color::Green => vga::GREEN,
            Color::Red => vga::RED,
            Color::Yellow => vga::BROWN,
            Color::Blue => vga::BLUE,
            _ => fallback,
        }
    }
    let mut t = forge_theme::Theme::modern_ide();
    t.name = "forge-terminal".into();
    t.bg = rgb(p.bg, Rgb::new(0, 0, 0));
    t.panel_bg = t.bg;
    t.fg = rgb(p.fg, Rgb::new(229, 229, 229));
    t.muted = rgb(p.fg_dim, t.muted);
    t.accent = rgb(p.accent, t.accent);
    t.border = rgb(p.border, t.muted);
    t.border_focus = rgb(p.border_active, t.accent);
    t.selection_bg = rgb(p.selection, t.accent);
    t.selection_fg = rgb(p.on_accent, t.bg);
    t.pill_bg = t.bg;
    t.menu_bg = t.bg;
    t.menu_fg = t.fg;
    t.git_added = rgb(p.success, t.git_added);
    t.git_deleted = rgb(p.error, t.git_deleted);
    t.git_modified = rgb(p.warning, t.git_modified);
    t.git_conflicted = t.git_deleted;
    t.git_untracked = t.git_added;
    t.git_ignored = t.muted;
    t.git_renamed = rgb(p.info, t.git_renamed);
    t.directory = rgb(p.info, t.directory);
    t.diff_add = t.git_added;
    t.diff_del = t.git_deleted;
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_kit::test_util::render_frames;

    #[tokio::test]
    async fn content_context_layout_highlights_crops_and_keeps_mouse_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.rs");
        std::fs::write(
            &path,
            format!(
                "before-context\n{}needle[界]\nafter-context\n",
                "字".repeat(600)
            ),
        )
        .unwrap();
        for width in [130, 70, 24] {
            let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
            app.open_tools(tools::PanelKind::Content);
            let panel = app.tools.panel.as_mut().unwrap();
            panel.input = tui_input::Input::new("needle[界]".into());
            panel.rows = vec![tools::ToolRow {
                label: "code.rs:2 needle[界]".into(),
                value: tools::RowValue::File(path.clone(), 2),
            }];
            app.tools.context_path = Some(path.clone());
            app.tools.content_context = Some(Ok(forge_fs::workspace::ContentContext::load(
                &path,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap()));
            // Cancel the empty initial search; this frame represents completed results.
            app.tools.pending_read = None;
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 26)).unwrap();
            terminal
                .draw(|frame| draw_tools(frame, frame.area(), &mut app, Palette::default()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
            if width >= 70 {
                assert!(screen.contains("before-context"), "{screen}");
                assert!(screen.contains("after-context"));
                assert!(screen.contains("needle["));
                assert!(screen.contains(":2:601"));
                assert!(buffer
                    .content()
                    .iter()
                    .any(|cell| cell.bg == Palette::default().warning));
            }
            let list = app.tools.panel.as_ref().unwrap().list_area;
            app.handle_tools_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: list.x,
                row: list.y,
                modifiers: KeyModifiers::NONE,
            });
            assert_eq!(app.tools.panel.as_ref().unwrap().selected, 0);
            app.tools.panel.as_mut().unwrap().rows.clear();
            terminal
                .draw(|frame| draw_tools(frame, frame.area(), &mut app, Palette::default()))
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(screen.contains("Type to search") || width < 30);
        }
    }

    fn fixture(root: &Path) -> App {
        let mut app = create_workspace_at(ColorMode::TrueColor, root.to_owned());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.files.children.insert(
            root.to_owned(),
            vec![Entry {
                id: forge_model::PathId(1),
                name: "中文文件.rs".into(),
                path: root.join("中文文件.rs"),
                kind: EntryKind::File,
            }],
        );
        app.rebuild_visible();
        app.preview_state.preview = Some(Preview {
            path: root.join("中文文件.rs"),
            content: PreviewContent::Text {
                lines: vec!["fn main() {}".into()],
                truncated: false,
                bytes_read: 12,
            },
        });
        app
    }

    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        use unicode_width::UnicodeWidthStr;
        let mut skip = 0;
        buffer
            .content()
            .iter()
            .filter_map(|cell| {
                if skip > 0 {
                    skip -= 1;
                    return None;
                }
                skip = cell.symbol().width().saturating_sub(1);
                Some(cell.symbol())
            })
            .collect()
    }

    fn markdown_fixture(root: &Path) -> App {
        let mut app = fixture(root);
        let entry = &mut app.files.children.get_mut(root).unwrap()[0];
        entry.name = "README.MD".into();
        entry.path = root.join("README.MD");
        app.rebuild_visible();
        app.preview_state.preview = Some(Preview {
            path: root.join("README.MD"),
            content: PreviewContent::Text {
                lines: vec!["# Markdown 标题".into(), "".into(), "**bold** body".into()],
                truncated: false,
                bytes_read: 42,
            },
        });
        app
    }

    #[tokio::test]
    async fn footer_git_chip_shows_upstream_counts_and_opens_sync_panel() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.scm.git_status = Some(StatusSnapshot::new(dir.path().to_owned()));
        app.scm.branch = "dev".into();
        app.tools.sync = "dev...origin/dev [ahead 1]".into();

        for width in [100u16, 55, 35] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 12)).unwrap();
            let mut hitbox = Rect::default();
            terminal
                .draw(|frame| {
                    hitbox = draw_workspace_footer(
                        frame,
                        Rect::new(0, 9, width, 3),
                        &app,
                        workspace_palette(),
                    );
                })
                .unwrap();
            assert!(hitbox.width > 0);
            assert!(hitbox.right() <= width);
            assert_eq!(hitbox.y, 11);
            let line = (0..width)
                .map(|x| terminal.backend().buffer()[(x, 11)].symbol())
                .collect::<String>();
            assert!(line.contains("dev"), "{line}");
            assert!(
                line.contains("↓0") && line.contains("↑1"),
                "tracking counts must remain visible even at width {width}: {line}"
            );
        }

        let chip = git_status_chip(&app, workspace_palette(), 38).unwrap();
        assert_eq!(
            chip.0
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            " ⎇ dev ↓0 ↑1 ↻ "
        );
        app.session.layout.git_sync_status = Rect::new(0, 11, chip.1 as u16, 1);
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            app.tools.panel.as_ref().map(|p| &p.kind),
            Some(tools::PanelKind::Git(forge_git::GitQuery::Sync))
        ));
        assert!(
            app.scm.pending_write.is_none(),
            "click must NOT push or pull"
        );
    }

    #[tokio::test]
    async fn footer_git_chip_handles_no_upstream_and_narrow_or_non_git_windows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.scm.git_status = Some(StatusSnapshot::new(dir.path().to_owned()));
        app.scm.branch = "new-branch".into();
        app.tools.sync = "new-branch".into();
        let (line, width) = git_status_chip(&app, workspace_palette(), 35).unwrap();
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(text.contains("new-branch") && text.contains("↻"));
        assert!(
            !text.contains("↑") && !text.contains("↓"),
            "no upstream isn't zero divergence"
        );
        assert!(width <= 35);
        app.tools.sync = "new-branch...origin/new-branch [gone]".into();
        let (line, _) = git_status_chip(&app, workspace_palette(), 40).unwrap();
        assert!(line.spans.iter().any(|s| s.content.contains("gone")));
        assert!(git_status_chip(&app, workspace_palette(), 9).is_none());
        app.scm.git_status = None;
        assert!(git_status_chip(&app, workspace_palette(), 40).is_none());
        app.scm.branch.clear();
        assert!(git_status_chip(&app, workspace_palette(), 40).is_none());
    }

    #[tokio::test]
    async fn diff_heading_open_file_is_clickable_and_keeps_directory_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("中文文件.rs"), "new source\n").unwrap();
        let mut app = fixture(dir.path());
        let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
        snapshot.push_change(
            "中文文件.rs",
            forge_model::GitStatus::Modified,
            DiffSide::Unstaged,
        );
        app.scm.git_status = Some(snapshot);
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.scm.scm_selected = scm_jump_to_column(&app.scm_rows(), StagingColumn::Unstaged);
        app.set_file_diff(FileDiff {
            repo_root: dir.path().to_owned(),
            path: "中文文件.rs".into(),
            side: DiffSide::Unstaged,
            lines: vec![],
        });
        app.preview_state.preview_mode = PreviewMode::Diff;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| {
                let _ = draw_preview_heading(
                    frame,
                    Rect::new(40, 2, 75, 22),
                    &mut app,
                    workspace_palette(),
                );
            })
            .unwrap();
        let chip = app.session.layout.open_file_tab;
        assert_eq!(chip.width, 14);
        let first_line = (chip.x..chip.right())
            .map(|x| terminal.backend().buffer()[(x, chip.y)].symbol())
            .collect::<String>();
        assert!(first_line.contains("Open file"), "{first_line}");
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: chip.x + 3,
            row: chip.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(
            app.files.visible[app.files.selected].entry.path,
            dir.path().join("中文文件.rs")
        );
    }

    #[tokio::test]
    async fn markdown_tabs_switch_source_with_mouse_and_keyboard() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = markdown_fixture(dir.path());
        assert!(app.markdown_available());
        assert!(matches!(
            preview_content(&app).1,
            PreviewView::Markdown { .. }
        ));
        assert!(app.context_shortcuts().iter().any(|s| s.key == "F3"));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                let body = draw_preview_heading(frame, frame.area(), &mut app, workspace_palette());
                assert_eq!(body, Rect::new(0, 2, 40, 8));
            })
            .unwrap();
        let tab = app.session.layout.markdown_code_tab;
        assert_eq!(tab.y, 0);
        assert!(app.session.layout.markdown_preview_tab.x >= 40 / 2 - 4);
        assert!(text(terminal.backend().buffer()).contains("F3"));
        app.preview_state.preview_scroll = 10;
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x + 1,
            row: tab.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.preview_state.markdown_code);
        assert_eq!(app.preview_state.preview_scroll, 0);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(matches!(
            preview_content(&app).1,
            PreviewView::Text {
                language: TextLang::Markdown,
                ..
            }
        ));
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
        // Clicking the selected tab preserves the current position.
        let tab = app.session.layout.markdown_preview_tab;
        app.preview_state.preview_scroll = 2;
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x + 1,
            row: tab.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.preview_state.preview_scroll, 2);
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
        assert!(!app.markdown_available());
        app = fixture(dir.path());
        assert!(!app.markdown_available());
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
    }

    #[tokio::test]
    async fn markdown_renders_preview_and_code_at_all_workspace_sizes() {
        let dir = tempfile::tempdir().unwrap();
        for (width, height) in [(120, 24), (50, 14), (24, 8), (8, 4), (1, 1)] {
            for code in [false, true] {
                let mut app = markdown_fixture(dir.path());
                app.session.focus = PaneFocus::Preview;
                app.preview_state.markdown_code = code;
                let buffer =
                    render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
                if width >= 50 {
                    let screen = text(&buffer);
                    assert!(
                        screen.contains("Preview") && screen.contains("Code"),
                        "{screen}"
                    );
                    assert_eq!(screen.contains("# Markdown"), code, "{screen}");
                    assert_eq!(screen.contains("**bold**"), code, "{screen}");
                }
                if width >= 24 {
                    let heading_y = if height >= 12 { 2 } else { 1 };
                    let footer_height = if height >= 12 { 3 } else { 2 };
                    let pane_height = height - heading_y - footer_height;
                    let heading = (0..width)
                        .map(|x| buffer[(x, heading_y)].symbol())
                        .collect::<String>();
                    assert!(heading.contains("Code"), "{heading}");
                    // The first content line follows the usual heading gap.
                    let content_y = heading_y + 1 + u16::from(pane_height >= 6);
                    let first_content = (0..width)
                        .map(|x| buffer[(x, content_y)].symbol())
                        .collect::<String>();
                    assert!(first_content.contains("Markdown"), "{first_content}");
                }
            }
        }
    }

    #[tokio::test]
    async fn markdown_reveal_uses_source_lines_and_refresh_clears_layout() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = markdown_fixture(dir.path());
        let path = dir.path().join("README.MD");
        app.tools.reveal = Some((path.clone(), 3));
        let preview = app.preview_state.preview.clone().unwrap();
        app.apply_preview_result(path, Ok(Ok(preview)));
        assert!(app.preview_state.markdown_code);
        assert_eq!(app.preview_state.preview_scroll, 2);
        assert_eq!(app.preview_source_line(), 3);
        app.set_markdown_code(false);
        app.preview_state.markdown.ensure(
            &["text ".repeat(50)],
            false,
            250,
            12,
            &forge_theme::Theme::default(),
        );
        let total = app.preview_total_lines();
        assert!(total > 3);
        app.session.layout.preview_inner = Rect::new(0, 0, 12, 3);
        app.scroll_preview(isize::MAX);
        assert_eq!(app.preview_state.preview_scroll, total - 3);
        assert_eq!(app.preview_source_line(), 1);
        app.request_preview_for_selection();
        assert_eq!(app.preview_state.markdown.line_count(), 0);
    }

    #[tokio::test]
    async fn sticky_title_click_jumps_to_directory_without_replacing_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let original_preview = app.preview_state.preview.clone();
        let mut file = app.files.visible[0].clone();
        file.depth = 1;
        let folder = TreeRow {
            entry: Entry {
                id: forge_model::PathId(77),
                name: "src".into(),
                path: dir.path().join("src"),
                kind: EntryKind::Directory,
            },
            depth: 0,
            expanded: true,
        };
        app.files.visible = vec![folder, file];
        app.files.selected = 1;
        app.files.scroll = 1;
        app.files.sticky_headers = vec![0];
        app.session.layout.list = Rect::new(0, 2, 30, 15);
        app.session.layout.list_inner = Rect::new(0, 2, 30, 15);
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.files.selected, 0);
        assert_eq!(app.files.scroll, 0);
        assert_eq!(app.preview_state.preview, original_preview);
        assert!(
            app.files.expanded.is_empty(),
            "sticky click should not toggle folder"
        );
    }

    #[tokio::test]
    async fn files_view_omits_sidebar_count_and_footer_position() {
        let dir = tempfile::tempdir().unwrap();
        let app = fixture(dir.path());
        let count = app.files.visible.len();
        assert!(count > 0);
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 120, 30, 3);
        let screen = text(&buffer);
        assert!(screen.contains("F1  Files"), "{screen}");
        assert!(!screen.contains(&format!("Files  {count}")), "{screen}");
        assert!(!screen.contains("Files  1 /"), "{screen}");
    }

    #[tokio::test]
    async fn workspace_uses_one_divider_and_neutral_full_width_selection() {
        let dir = tempfile::tempdir().unwrap();
        let buffer = render_frames(
            element!(Workspace(initial: Some(fixture(dir.path())))),
            120,
            30,
            3,
        );
        let screen = text(&buffer);
        assert!(screen.contains("Files"), "{screen}");
        assert!(screen.contains("中文文件.rs"), "{screen}");
        assert!(screen.contains("fn main() {}"), "{screen}");
        assert!(buffer.content().iter().any(|cell| cell.fg == Color::Cyan));
        assert_eq!(buffer[(1, 3)].bg, Color::Reset);
        assert_eq!(buffer[(30, 4)].symbol(), "│");
        assert_eq!(buffer[(0, 2)].symbol(), "›");
        assert_eq!(buffer[(29, 2)].bg, Color::DarkGray);
        assert!(!screen.contains('┌') && !screen.contains('┐'), "{screen}");
        assert!(!buffer.content().iter().any(|cell| cell.bg == Color::Cyan));
    }

    #[tokio::test]
    async fn sidebar_history_renders_lanes_badges_and_commit_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let head = forge_git::GraphCommit {
            id: "a".repeat(40),
            short: "aaaaaaa".into(),
            parents: vec!["b".repeat(40)],
            refs: vec![
                forge_git::GraphRef {
                    name: "main".into(),
                    kind: forge_git::GraphRefKind::LocalBranch,
                },
                forge_git::GraphRef {
                    name: "HEAD".into(),
                    kind: forge_git::GraphRefKind::Head,
                },
            ],
            author: "fxh".into(),
            time: 1_700_000_000,
            subject: "feat: sidebar history".into(),
        };
        let base = forge_git::GraphCommit {
            id: "b".repeat(40),
            short: "bbbbbbb".into(),
            parents: Vec::new(),
            refs: Vec::new(),
            author: "fxh".into(),
            time: 1_699_000_000,
            subject: "chore: base".into(),
        };
        let oid = head.id.clone();
        app.session.activity = ActivityTab::Git;
        app.history.expanded = true;
        app.history.seed(vec![head, base]);
        app.history.files.insert(
            oid.clone(),
            vec![forge_git::CommitChange {
                path: "crates/forge-ui/src/graph.rs".into(),
                old_path: None,
                kind: forge_git::CommitChangeKind::Modified,
            }],
        );
        app.history.expanded_commits.insert(oid.clone());
        for dir in ["crates", "crates/forge-ui", "crates/forge-ui/src"] {
            app.history
                .expanded_dirs
                .insert(forge_ui::dir_key(&oid, dir));
        }

        let buffer = render_frames(element!(Workspace(initial: Some(app))), 140, 32, 3);
        let screen = text(&buffer);
        assert!(screen.contains("HISTORY"), "{screen}");
        // Subjects are truncated to the pane width, so match a prefix.
        assert!(screen.contains("feat: sidebar hi"), "{screen}");
        assert!(screen.contains("chore: base"), "{screen}");
        assert!(screen.contains("main"), "{screen}");
        assert!(
            screen.contains("crates") && screen.contains("forge-ui") && screen.contains("graph.rs"),
            "{screen}"
        );
        assert!(screen.contains('●'), "{screen}");
    }

    #[tokio::test]
    async fn footer_advertises_the_history_entry_in_both_tabs() {
        let dir = tempfile::tempdir().unwrap();
        for width in [100, 110, 130] {
            let app = fixture(dir.path());
            let screen = text(&render_frames(
                element!(Workspace(initial: Some(app))),
                width,
                30,
                3,
            ));
            let footer = screen.lines().find(|l| l.contains("Help")).unwrap_or("");
            assert!(footer.contains("G History"), "files w={width}: {footer}");
        }
        // The Git footer is full of contextual hints; the history entry must not
        // push them out.
        let mut app = fixture(dir.path());
        app.enter_git_sidebar();
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            30,
            3,
        ));
        let footer = screen.lines().find(|l| l.contains("Help")).unwrap_or("");
        assert!(footer.contains("Select a file"), "{footer}");
        let mut app = fixture(dir.path());
        assert!(app.history.expanded);
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        assert!(app.history.expanded);
    }

    #[tokio::test]
    async fn narrow_workspace_shows_focused_preview_and_handles_tiny_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 45, 12, 3);
        assert!(text(&buffer).contains("fn main() {}"));
        for (w, h) in [(1, 1), (1, 12), (20, 3), (40, 12), (79, 10), (80, 12)] {
            let _ = render_frames(
                element!(Workspace(initial: Some(fixture(dir.path())))),
                w,
                h,
                3,
            );
        }
    }

    #[tokio::test]
    async fn footer_preserves_errors_and_changes_shortcuts_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.status = "commit failed: rejected".into();
        app.session.activity = ActivityTab::Git;
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 100, 25, 3);
        let screen = text(&buffer);
        assert!(screen.contains("commit failed: rejected"), "{screen}");
        assert!(
            screen.contains("Select a file") && screen.contains("? Help"),
            "{screen}"
        );
        assert!(!screen.contains("s/u"), "{screen}");
        assert_eq!(buffer[(2, 23)].fg, Color::Red);
        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 80, 20, 3);
        let screen = text(&buffer);
        assert!(
            screen.contains("j/k") && screen.contains("Scroll"),
            "{screen}"
        );
        assert!(screen.contains("? Help"), "{screen}");
    }

    #[tokio::test]
    async fn files_and_git_keep_separate_previews_without_rendering_directory_details() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let original = app.preview_state.preview.clone();
        app.preview_state.preview_scroll = 2;
        app.files.children.get_mut(dir.path()).unwrap().insert(
            0,
            Entry {
                id: forge_model::PathId(3),
                name: "src".into(),
                path: dir.path().join("src"),
                kind: EntryKind::Directory,
            },
        );
        app.rebuild_visible();
        app.files.selected = 0; // Deliberately select a directory.
        app.enter_git_sidebar();
        assert!(matches!(app.preview_state.preview_mode, PreviewMode::Diff));
        app.preview_state.preview = None;
        app.leave_git_sidebar();
        assert_eq!(app.preview_state.preview, original);
        assert_eq!(app.preview_state.preview_scroll, 2);
        assert!(app.files.pending_lists.is_empty());
        assert!(app.preview_state.pending_preview.is_none());
    }

    #[tokio::test]
    async fn compact_commands_and_narrow_tab_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.open_tools(tools::PanelKind::Commands);
        let panel = app.tools.panel.as_mut().unwrap();
        panel.input = tui_input::Input::new("stage".into());
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 120, 30, 3);
        let lines: Vec<String> = (0..30)
            .map(|row| {
                (0..120)
                    .map(|x| buffer[(x, row)].symbol())
                    .collect::<String>()
            })
            .collect();
        let title = lines
            .iter()
            .position(|line| line.contains("Commands"))
            .unwrap();
        assert!(
            title > 5,
            "command palette should be compact: title at row {title}"
        );

        let app = fixture(dir.path());
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 75, 24, 3);
        let footer: String = (0..75).map(|x| buffer[(x, 23)].symbol()).collect();
        assert!(
            footer.contains("Tab") && footer.contains("Preview"),
            "{footer}"
        );
    }

    #[tokio::test]
    async fn files_and_git_tabs_show_active_state_and_accept_mouse_and_function_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        terminal
            .draw(|frame| {
                draw_workspace_header(frame, Rect::new(0, 0, 90, 2), &mut app, workspace_palette())
            })
            .unwrap();
        let screen = text(terminal.backend().buffer());
        assert!(screen.contains("F1  Files") && screen.contains("F2  Git"));
        let files = app.session.layout.files_tab;
        let git = app.session.layout.git_tab;
        assert_eq!(
            terminal.backend().buffer()[(files.x, files.y)].bg,
            workspace_palette().selection
        );
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: git.x + 1,
            row: git.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.in_git_sidebar());
        terminal
            .draw(|frame| {
                draw_workspace_header(frame, Rect::new(0, 0, 90, 2), &mut app, workspace_palette())
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(git.x, git.y)].bg,
            workspace_palette().selection
        );
        assert_ne!(
            terminal.backend().buffer()[(files.x, files.y)].bg,
            workspace_palette().selection
        );
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: files.x + 1,
            row: files.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(!app.in_git_sidebar());
        assert!(app
            .context_shortcuts()
            .iter()
            .all(|s| s.key != "s" && s.key != "u"));
    }

    #[tokio::test]
    async fn commit_modal_and_context_menu_render_under_input_layers() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.commit_open = true;
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            25,
            3,
        ));
        // The block cursor covers the first placeholder character.
        assert!(screen.contains("ommit message"), "{screen}");
        let mut app = fixture(dir.path());
        app.open_file_context_menu(0, 5, 5);
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            25,
            3,
        ));
        assert!(screen.contains("Copy path"), "{screen}");
        for (width, height) in [(1, 1), (12, 5)] {
            let mut app = fixture(dir.path());
            app.open_file_context_menu(0, 50, 50);
            let _ = render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
        }
    }

    #[tokio::test]
    async fn command_and_confirmation_dialogs_render_including_tiny_windows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.open_tools(tools::PanelKind::Commands);
        let screen = text(&render_frames(
            element!(Workspace(initial:Some(app))),
            100,
            30,
            3,
        ));
        assert!(screen.contains("Commands"), "{screen}");
        assert!(screen.contains("Find file"), "{screen}");
        for (width, height) in [(1, 1), (12, 5), (40, 10), (80, 24)] {
            let mut app = fixture(dir.path());
            app.open_tools(tools::PanelKind::Commands);
            let panel = app.tools.panel.as_mut().unwrap();
            panel.input = tui_input::Input::new("中文中文中文中文中文中文".into());
            let _ = render_frames(element!(Workspace(initial:Some(app))), width, height, 3);
            let mut app = fixture(dir.path());
            app.run_command(tools::Command::Delete);
            let _ = render_frames(element!(Workspace(initial:Some(app))), width, height, 3);
        }
    }

    #[tokio::test]
    async fn footer_names_file_folder_and_hunk_actions_even_in_narrow_windows() {
        let dir = tempfile::tempdir().unwrap();
        for (side, focus, folder, expected) in [
            (DiffSide::Unstaged, PaneFocus::List, false, "Stage file"),
            (DiffSide::Staged, PaneFocus::List, false, "Unstage file"),
            (DiffSide::Unstaged, PaneFocus::List, true, "Stage folder"),
            (DiffSide::Staged, PaneFocus::List, true, "Unstage folder"),
            (DiffSide::Unstaged, PaneFocus::Preview, false, "Stage hunk"),
            (DiffSide::Staged, PaneFocus::Preview, false, "Unstage hunk"),
        ] {
            for width in [32, 45, 80, 120] {
                let mut app = fixture(dir.path());
                app.session.status.clear();
                app.session.activity = ActivityTab::Git;
                app.session.focus = focus;
                let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
                let path = if folder {
                    "folder/中文文件.rs"
                } else {
                    "中文文件.rs"
                };
                snapshot.push_change(path, forge_model::GitStatus::Modified, side);
                app.scm.git_status = Some(snapshot);
                app.scm.scm_selected = if folder {
                    app.scm_rows()
                        .iter()
                        .position(
                            |r| matches!(r, ScmRow::Folder { column, .. } if column.side() == side),
                        )
                        .unwrap()
                } else {
                    scm_jump_to_column(
                        &app.scm_rows(),
                        if side == DiffSide::Unstaged {
                            StagingColumn::Unstaged
                        } else {
                            StagingColumn::Staged
                        },
                    )
                };
                app.preview_state.preview_mode = PreviewMode::Diff;
                app.preview_state.hunk_index = Some(0);
                app.set_file_diff(FileDiff {
                    repo_root: dir.path().to_owned(),
                    path: "中文文件.rs".into(),
                    side,
                    lines: vec![forge_model::DiffLine {
                        kind: forge_model::DiffLineKind::Header,
                        text: "@@ -1 +1 @@".into(),
                    }],
                });
                let buffer = render_frames(element!(Workspace(initial: Some(app))), width, 20, 3);
                let footer: String = (0..width).map(|x| buffer[(x, 19)].symbol()).collect();
                assert!(
                    footer.contains(expected) && footer.contains("? Help"),
                    "{width}: {footer}"
                );
                assert!(!footer.contains("s/u") && !footer.contains("sync"));
            }
        }
    }

    #[tokio::test]
    async fn help_wraps_and_renders_at_small_sizes_without_changing_focus() {
        let dir = tempfile::tempdir().unwrap();
        for (width, height) in [(120, 35), (45, 12), (20, 5), (1, 1)] {
            let mut app = fixture(dir.path());
            app.session.help_open = true;
            app.session.focus = PaneFocus::Preview;
            let buffer = render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
            if width >= 45 {
                assert!(text(&buffer).contains("Git help"));
            }
        }
        assert!(wrap_help(30).len() > wrap_help(100).len());
    }

    #[tokio::test]
    async fn git_marks_count_each_side() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let path = "中文文件.rs";
        let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
        snapshot.push_change(path, forge_model::GitStatus::Modified, DiffSide::Unstaged);
        snapshot.push_change(path, forge_model::GitStatus::Modified, DiffSide::Staged);
        snapshot.finish();
        app.scm.git_status = Some(snapshot);
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        app.scm.scm_selected = 1;
        for side in [DiffSide::Unstaged, DiffSide::Staged] {
            app.tools.marks.git.insert(crate::tools::GitMark {
                path: dir.path().join(path),
                side,
            });
        }
        assert_eq!(app.tools.marks.len(), 2);
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 110, 30, 3);
        let screen = text(&buffer);
        assert!(screen.contains("CHANGES (1)"), "{screen}");
        assert!(screen.contains("STAGED (1)"), "{screen}");
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "●")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn stale_preview_is_rejected_and_modes_keep_file_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.apply_preview_result(
            dir.path().join("old.rs"),
            Ok(Ok(Preview::error(dir.path().join("old.rs"), "stale"))),
        );
        assert_eq!(
            app.preview_state.preview.as_ref().unwrap().path,
            dir.path().join("中文文件.rs")
        );
        app.enter_git_sidebar();
        app.leave_git_sidebar();
        assert_eq!(app.files.selected, 0);
        assert_eq!(app.selected_entry().unwrap().name, "中文文件.rs");
    }
}

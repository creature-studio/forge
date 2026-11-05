//! Kit components. Workspace state is scoped to this tree; only the palette is
//! process-wide. Draft text lives exclusively in the mounted commit editor.
use super::*;
use crossterm::event::{Event, KeyEventKind};
use forge_ui::{
    draw_file_list, draw_preview, draw_scm_sidebar, FileListState, PreviewView, StagingState,
    TextLang,
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
            if help_open {
                Modal(open: true, width: Constraint::Percentage(90), height: Constraint::Percentage(85)) {
                    Help()
                }
            }
            if tools_open {
                Modal(open: true, width: Constraint::Percentage(92), height: Constraint::Percentage(90)) {
                    Tools()
                }
            }
            if editing_open {
                Modal(open: true, width: Constraint::Percentage(96), height: Constraint::Percentage(94)) {
                    TextEditor()
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
            Event::Key(key) if key.kind == KeyEventKind::Press => model.handle_editor_key(key),
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
    let mut exit = hooks.use_exit();
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
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => exit(),
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
                let git = model.in_git_sidebar();
                let count = if git {
                    model
                        .scm
                        .git_status
                        .as_ref()
                        .map(|s| s.unstaged.len() + s.staged.len())
                        .unwrap_or(0)
                } else {
                    model.files.visible.len()
                };
                let heading_detail = if model.tools.marks.is_empty() {
                    count.to_string()
                } else {
                    format!("{} selected", model.tools.marks.len())
                };
                area = draw_pane_heading(
                    drawer.frame,
                    area,
                    self.palette,
                    if git { "Source control" } else { "Files" },
                    &heading_detail,
                    model.sidebar_focused(),
                );
                if model.in_git_sidebar() {
                    let mut scroll = model.scm.scroll_scm;
                    let (unstaged, staged) = model.scm_slices();
                    let focused = model.sidebar_focused();
                    let layout = draw_scm_sidebar(
                        drawer.frame,
                        area,
                        &theme,
                        &mut StagingState {
                            unstaged,
                            staged,
                            expanded_unstaged: &model.scm.scm_expanded_unstaged,
                            expanded_staged: &model.scm.scm_expanded_staged,
                            selected: model.scm.scm_selected,
                            scroll: &mut scroll,
                            focused,
                        },
                    );
                    model.session.layout.staging = layout;
                    model.session.layout.list_inner = layout.inner;
                    model.scm.scroll_scm = scroll;
                } else {
                    let mut scroll = model.files.scroll;
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
                        },
                    );
                    model.session.layout.list_inner = inner;
                    model.files.scroll = scroll;
                }
                draw_marks(drawer.frame, &model, self.palette);
            }
            PaneKind::Preview => {
                model.session.layout.preview = area;
                let (title, _) = preview_content(&model);
                area = draw_pane_heading(
                    drawer.frame,
                    area,
                    self.palette,
                    if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
                        "Diff"
                    } else {
                        "Preview"
                    },
                    &title,
                    model.session.focus == PaneFocus::Preview,
                );
                let mut scroll = model.preview_state.preview_scroll;
                // The cache is temporarily detached to keep borrowed content immutable.
                let mut cache = std::mem::take(&mut model.preview_state.split_cache);
                let (_, content) = preview_content(&model);
                let inner =
                    draw_preview(drawer.frame, area, &theme, content, &mut scroll, &mut cache);
                model.preview_state.split_cache = cache;
                model.preview_state.preview_scroll = scroll;
                model.session.layout.preview_inner = inner;
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

fn draw_marks(frame: &mut Frame<'_>, model: &App, p: Palette) {
    let area = model.session.layout.list_inner;
    if area.width == 0 {
        return;
    }
    let rows: Vec<MarkRow> = if model.in_git_sidebar() {
        let root = model.repo_root().unwrap_or_default();
        let (u, s) = model.scm_slices();
        model
            .scm_rows()
            .iter()
            .skip(model.scm.scroll_scm)
            .map(|row| match row {
                ScmRow::Folder { path, column, .. } => {
                    MarkRow::Git(root.join(path), column.side())
                }
                ScmRow::Change { .. } => scm_entry(row, u, s)
                    .map(|entry| MarkRow::Git(root.join(&entry.path), entry.side))
                    .unwrap_or(MarkRow::None),
                ScmRow::Section { .. } => MarkRow::None,
            })
            .collect()
    } else {
        model
            .files
            .visible
            .iter()
            .skip(model.files.scroll)
            .map(|row| MarkRow::File(row.entry.path.clone()))
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
    let loading = model.tools.pending_read.is_some();
    let Some(panel) = &mut model.tools.panel else {
        return;
    };
    panel.area = area;
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", panel.title))
        .title_bottom(" Esc Close · ↑/↓ Select · Enter Open · PgUp/PgDn Details ")
        .style(Style::default().fg(p.fg).bg(p.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let input_height = if panel.is_input() { 2 } else { 0 };
    let list_height = if panel.rows.is_empty() {
        0
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
    let message = if loading {
        format!("Loading… {}", panel.message)
    } else {
        panel.message.clone()
    };
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(p.warning)),
        rows[1],
    );
    panel.list_area = rows[2];
    let offset = panel
        .selected
        .saturating_sub(rows[2].height.saturating_sub(1) as usize);
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
    let branch = if model.scm.branch.is_empty() {
        Line::default()
    } else {
        let mut spans = vec![Span::styled(
            format!(
                "git {}",
                fit_label(
                    if model.tools.sync.is_empty() {
                        &model.scm.branch
                    } else {
                        &model.tools.sync
                    },
                    (area.width as usize / 4).clamp(3, 24),
                    true
                )
            ),
            Style::default().fg(p.fg_dim),
        )];
        if area.width >= 100 {
            if let Some(status) = &model.scm.git_status {
                spans.push(Span::styled(
                    format!("  {} changed", status.unstaged.len()),
                    Style::default().fg(p.warning),
                ));
                spans.push(Span::styled(
                    format!("  {} staged", status.staged.len()),
                    Style::default().fg(p.success),
                ));
            }
        }
        Line::from(spans)
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

fn draw_workspace_footer(frame: &mut Frame<'_>, area: Rect, model: &App, p: Palette) {
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
            format!(
                "Files  {} / {}",
                if model.files.visible.is_empty() {
                    0
                } else {
                    model.files.selected + 1
                },
                model.files.visible.len()
            )
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
        return;
    }
    let keys = footer_shortcuts(model, area.width as usize);
    let mut spans = vec![Span::raw(" ")];
    for interaction::Shortcut { key, label } in keys {
        spans.push(Span::styled(format!(" {key} "), Style::default().fg(p.fg)));
        spans.push(Span::styled(
            format!("{label}  "),
            Style::default().fg(p.fg_dim),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[1]);
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
        model.context_shortcuts()
    };
    for shortcut in candidates {
        let size = shortcut.key.len() + shortcut.label.len() + 5;
        if size <= remaining {
            remaining -= size;
            keys.push(shortcut);
        }
    }
    if !model.session.commit_open {
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
                        diff.side.label(),
                        diff.path,
                        model
                            .preview_state
                            .hunk_index
                            .map(|i| format!(" · hunk {}/{}", i + 1, hunks.len()))
                            .unwrap_or_default()
                    ),
                    PreviewView::Diff {
                        path: &diff.path,
                        side: diff.side.label(),
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
        assert_eq!(buffer[(0, 4)].symbol(), "›");
        assert_eq!(buffer[(29, 4)].bg, Color::DarkGray);
        assert!(!screen.contains('┌') && !screen.contains('┐'), "{screen}");
        assert!(!buffer.content().iter().any(|cell| cell.bg == Color::Cyan));
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

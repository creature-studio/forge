//! Application orchestration: state machine, modes, and event loop.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use forge_config::Config;
use forge_fs::{Entry, EntryKind, FsError, FsService, Preview, PreviewContent, TextPage};
use forge_git::{
    DiffHunk, DiffSide, FileDiff, GitReadError, GitService, GitWriteError, StatusSnapshot,
};
use forge_jobs::{JobError, JobHandle, JobPriority, JobScheduler};
use forge_model::JobId;
use forge_theme::{detect_color_mode, ColorMode};
use forge_ui::{
    build_scm_rows, compact_scm_rows, compact_tree_rows, diff_items, diff_render_hunk_start,
    expand_ancestors_for_path, file_list_items, find_parent_row, find_scm_parent_row, flatten_tree,
    init_icons_from_env, scm_entry, scm_jump_to_column, scroll_to_hunk, set_color_mode,
    staging_items, sticky_index_at, ActivityTab, ContextAction, ContextMenu, ContextTarget,
    ScrollbarDrag, VerticalScrollbar,
    MarkdownPreview, PaneFocus, PaneLayout, ScmRow, SplitRowsCache, StagingColumn, TextLang,
    TextRowsCache, TreeRow,
};
mod auto_refresh;
mod blame;
mod file_find;
mod file_find_ui;
mod inline_diff;
mod preview_selection;
mod pending_request;
mod input;
use preview_selection::PreviewSelection;
mod editor;
mod git_sync_badge;
mod history;
mod interaction;
mod keybindings;
mod tools;
mod view;
mod embedded_terminal;
use ratatui_kit::prelude::*;

/// Double-click window for directory enter.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

struct PendingList {
    #[allow(dead_code)]
    id: JobId,
    path: PathBuf,
    handle: JobHandle<Result<Vec<Entry>, FsError>>,
}

struct PendingPreviewPage {
    path: PathBuf,
    from_offset: u64,
    handle: JobHandle<Result<TextPage, FsError>>,
}

struct PendingStatus {
    #[allow(dead_code)]
    id: JobId,
    key: PathBuf,
    handle: JobHandle<Result<Option<StatusSnapshot>, GitReadError>>,
}

struct PendingPreview {
    #[allow(dead_code)]
    id: JobId,
    path: PathBuf,
    handle: JobHandle<Result<Preview, FsError>>,
}

/// Where a previewed diff comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffSource {
    /// Working tree / index diff for one path.
    Working(DiffSide),
    /// One file as changed by a commit (read-only history view).
    Commit { oid: String, short: String },
}

struct PendingDiff {
    #[allow(dead_code)]
    id: JobId,
    path: String,
    source: DiffSource,
    handle: JobHandle<Result<FileDiff, GitWriteError>>,
}

#[allow(dead_code)]
enum WriteOp {
    Stage,
    Unstage,
    StageHunk,
    UnstageHunk,
    Commit(String),
    Push,
    Pull,
}

struct PendingWrite {
    #[allow(dead_code)]
    id: JobId,
    op: WriteOp,
    target: String,
    handle: JobHandle<Result<Option<String>, GitWriteError>>,
}

/// What the preview pane is showing.
enum PreviewMode {
    File,
    Diff,
}

/// Capabilities shared by the workspace; never part of a component's draft state.
struct Services {
    jobs: JobScheduler,
    fs: FsService,
    git: GitService,
}

struct FilesState {
    tree_root: PathBuf,
    expanded: HashSet<PathBuf>,
    /// Directory whose unloaded descendants should be expanded automatically
    /// as one compact folder click. None when a branch or file is encountered.
    compact_expand_pending: Option<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    visible: Vec<TreeRow>,
    selected: usize,
    scroll: usize,
    follow_selection: bool,
    restore_select: Option<String>,
    external_anchor: Option<(PathBuf, usize)>,
    /// Keep a deliberately revealed path and its ancestors visible even
    /// through extension/status/exclusion filters.
    revealed_path: Option<PathBuf>,
    pending_lists: Vec<PendingList>,
    last_list_click: Option<(Instant, usize)>,
    sticky_headers: Vec<usize>,
}

struct ScmState {
    pending_status: Option<PendingStatus>,
    /// A refresh arrived while a status job was already running. Blocking
    /// workers cannot be aborted, so re-run only after the current one ends.
    status_rerun: bool,
    pending_write: Option<PendingWrite>,
    git_status: Option<StatusSnapshot>,
    branch: String,
    scm_selected: usize,
    scroll_scm: usize,
    follow_selection: bool,
    /// When a whole file moves between Changes and Staged, follow its path
    /// after the asynchronous Git snapshot, not the old numeric row index.
    pending_transfer: Option<(String, StagingColumn)>,
    last_scm_click: Option<(Instant, usize)>,
    scm_expanded_unstaged: HashSet<String>,
    scm_expanded_staged: HashSet<String>,
    sticky_headers: Vec<usize>,
}

struct PreviewState {
    preview_scroll: usize,
    /// Horizontal displacement of source code columns (gutter stays fixed).
    preview_scroll_x: usize,
    find: file_find::FileFindState,
    markdown_code: bool,
    markdown: MarkdownPreview,
    selection: PreviewSelection,
    text_rows: TextRowsCache,
    /// Source file line to seek once Code rows have been measured for width.
    pending_source_line: Option<usize>,
    /// Source-line jump requested from the overview ruler; survives preview paging.
    pending_overview_line: Option<usize>,
    /// UTF-8 byte column of a pending in-file Find result.
    pending_find_byte: Option<usize>,
    split_cache: SplitRowsCache,
    diff_row_count: usize,
    pending_preview: Option<PendingPreview>,
    pending_page: Option<PendingPreviewPage>,
    pending_diff: Option<PendingDiff>,
    preview: Option<Preview>,
    preview_loading: bool,
    preview_mode: PreviewMode,
    /// Which kind of diff `file_diff` holds.
    diff_source: DiffSource,
    /// Header chip for the loaded diff (`unstaged`, `staged`, or a short oid).
    diff_label: String,
    file_diff: Option<FileDiff>,
    hunk_index: Option<usize>,
}

struct SessionState {
    status: String,
    write_status: Option<String>,
    /// Transient write-operation notification; tracked separately from
    /// long-lived context/status so old success/error messages expire.
    notice_lease: Option<(String, String, Instant)>,
    help_open: bool,
    help_reference: bool,
    help_scroll: u16,
    help_scroll_limit: u16,
    help_area: ratatui::layout::Rect,
    help_close_rect: ratatui::layout::Rect,
    help_overview_rect: ratatui::layout::Rect,
    help_reference_rect: ratatui::layout::Rect,
    activity: ActivityTab,
    commit_open: bool,
    context_menu: Option<ContextMenu>,
    should_quit: bool,
    layout: PaneLayout,
    focus: PaneFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollbarTarget { Sidebar, Preview }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalTabAction {
    Activate(u64),
    Close(u64),
    New,
    Previous,
    Next,
    Maximize,
    Help,
    Hide,
}

struct App {
    services: Services,
    config: Config,
    files: FilesState,
    scm: ScmState,
    preview_state: PreviewState,
    // Preserve the previous Files preview while the Git tab owns the viewer.
    // A directory selection must not initiate a new preview.
    files_preview: Option<(Option<Preview>, usize, bool)>,
    session: SessionState,
    tools: tools::ToolsState,
    editing: editor::EditingState,
    keys: keybindings::Keybindings,
    history: history::HistoryState,
    blame: blame::BlameState,
    inline_diff: inline_diff::InlineDiff,
    terminal: Option<embedded_terminal::EmbeddedTerminal>,
    /// Inactive PTY sessions remain alive; only the active terminal is drawn.
    terminal_tabs: Vec<embedded_terminal::EmbeddedTerminal>,
    /// Exact hit targets for controls painted in the Terminal header.
    terminal_tab_hits: Vec<(ratatui::layout::Rect, TerminalTabAction)>,
    terminal_tab_bar: ratatui::layout::Rect,
    terminal_help_return: bool,
    terminal_notify: std::sync::Arc<tokio::sync::Notify>,
    terminal_frame_epoch: u64,
    /// Pointer capture persists regardless of horizontal mouse movement.
    scrollbar_drag: Option<(ScrollbarTarget, ScrollbarDrag)>,
}

/// Read the current branch name from `.git/HEAD` (best-effort, sync).
fn read_branch_name(repo_root: &Path) -> Option<String> {
    let marker = repo_root.join(".git");
    let git_dir = if marker.is_file() {
        let text = std::fs::read_to_string(&marker).ok()?;
        repo_root.join(text.trim().strip_prefix("gitdir: ")?)
    } else {
        marker
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(name) = head.strip_prefix("ref: refs/heads/") {
        return Some(name.to_string());
    }
    if head.len() >= 7 {
        Some(format!("{}...", &head[..7]))
    } else {
        Some(head.to_string())
    }
}

/// CLI / env options that affect terminal color emission.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Force `Color::Rgb` (also sets `COLORTERM=truecolor` for this process).
    pub force_truecolor: bool,
    /// Force ANSI-256 indexed colors.
    pub force_ansi256: bool,
    /// Force named 16-color VGA palette (default; works on basic GNOME Terminal).
    pub force_ansi16: bool,
    /// Print a colored smoke strip to stdout and exit (no TUI).
    pub demo_colors: bool,
}

/// Run the forge TUI application with default color detection.
pub fn run() -> Result<()> {
    run_with_options(RunOptions::default())
}

/// Run the forge TUI with explicit color-mode overrides.
pub fn run_with_options(opts: RunOptions) -> Result<()> {
    ensure_usable_term();
    let mode = resolve_color_mode(opts);
    apply_color_mode(mode);
    init_icons_from_env();

    let term = std::env::var("TERM").unwrap_or_default();
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    let icons = forge_ui::icon_mode().as_str();
    eprintln!(
        "forge: palette={}  icons={icons}  TERM={term:?}  COLORTERM={colorterm:?}  (default icons=builtin | FORGE_ICONS=nerd needs a Nerd Font | flags: --force-ansi16|--force-ansi256|--force-truecolor|--demo-colors | FORGE_COLOR=16|256|truecolor | FORGE_ICONS=builtin|nerd|ascii)",
        mode.as_str()
    );

    if opts.demo_colors {
        return demo_colors(mode);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads())
        .max_blocking_threads(blocking_threads())
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    rt.block_on(run_async(mode))
}

/// Tokio workers for the async I/O and the TUI loop.
///
/// One worker per logical core is unnecessary here (blocking git work runs on
/// `spawn_blocking`) and every extra worker keeps its own glibc malloc arena
/// alive. Capping the pool also keeps the render task's temporary buffers from
/// being spread across a dozen per-core arenas. `FORGE_WORKERS` overrides.
fn worker_threads() -> usize {
    std::env::var("FORGE_WORKERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|workers| *workers > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(2, |cores| cores.get().min(4))
        })
}

/// Blocking pool size for git reads, previews and directory listings.
///
/// Tokio's default (512) lets every queued blocking task grow its own thread,
/// and every one of those threads keeps a glibc arena's high-water mark after
/// it exits. Git status alone builds a walk of the whole worktree (~230 bytes
/// per entry), so a dozen transient threads can pin tens of MiB. A small pool
/// keeps that high-water bounded. `FORGE_BLOCKING_THREADS` overrides.
fn blocking_threads() -> usize {
    std::env::var("FORGE_BLOCKING_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(4)
}

/// Make the process environment able to emit ANSI colors.
///
/// Cursor / CI agent shells often set `NO_COLOR=1` and `TERM=dumb`, which makes
/// crossterm emit empty `\x1b[m` instead of real colors — the TUI looks
/// monochrome white-on-black. Forge is a color-first Turbo/Edit UI, so we clear
/// those blocks (with a stderr note) unless the user asked for no color via
/// `FORGE_COLOR=none`.
fn ensure_usable_term() {
    if let Ok(v) = std::env::var("FORGE_COLOR") {
        let v = v.to_ascii_lowercase();
        if matches!(v.as_str(), "none" | "off" | "mono" | "0" | "false") {
            eprintln!("forge: FORGE_COLOR={v} — leaving NO_COLOR alone (monochrome)");
            return;
        }
    }

    let term = std::env::var("TERM").unwrap_or_default();
    if term.is_empty() || term == "dumb" {
        eprintln!(
            "forge: warning: TERM={term:?} cannot carry colors — setting TERM=xterm-256color for this process"
        );
        std::env::set_var("TERM", "xterm-256color");
    }

    if std::env::var_os("NO_COLOR").is_some() {
        eprintln!(
            "forge: clearing NO_COLOR={:?} (it made crossterm emit empty \\e[m and killed all color)",
            std::env::var_os("NO_COLOR")
        );
        std::env::remove_var("NO_COLOR");
    }

    // FORCE_COLOR=0 also disables colored crates; force-enable for this process.
    match std::env::var("FORCE_COLOR") {
        Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off") => {
            eprintln!("forge: FORCE_COLOR={v} → setting FORCE_COLOR=1");
            std::env::set_var("FORCE_COLOR", "1");
        }
        Err(_) => {
            std::env::set_var("FORCE_COLOR", "1");
        }
        Ok(_) => {}
    }
}

fn resolve_color_mode(opts: RunOptions) -> ColorMode {
    if opts.force_truecolor {
        ColorMode::TrueColor
    } else if opts.force_ansi256 {
        ColorMode::Ansi256
    } else if opts.force_ansi16 {
        ColorMode::Ansi16
    } else {
        detect_color_mode()
    }
}

fn apply_color_mode(mode: ColorMode) {
    set_color_mode(mode);
    match mode {
        ColorMode::TrueColor => {
            std::env::set_var("COLORTERM", "truecolor");
            std::env::set_var("FORGE_COLOR", "truecolor");
        }
        ColorMode::Ansi256 => {
            std::env::set_var("FORGE_COLOR", "256");
        }
        ColorMode::Ansi16 => {
            std::env::set_var("FORGE_COLOR", "16");
        }
    }
}

/// Smoke test: print named ANSI color samples then exit.
fn demo_colors(mode: ColorMode) -> Result<()> {
    use std::io::Write;

    let mut out = io::stdout();
    writeln!(
        out,
        "forge --demo-colors  palette={}  (expect visible Blue / Cyan / Yellow / Green)",
        mode.as_str()
    )?;

    // Classic 16-color SGR — works even when crossterm would strip colors.
    // 44=blue bg, 46=cyan bg, 103;30=bright yellow bg / black fg, etc.
    let samples: &[(&str, &str)] = &[
        ("Black on Blue (desktop)", "\x1b[97;44m"),
        ("Black on Cyan (selection)", "\x1b[30;46m"),
        ("Yellow on Black (modified)", "\x1b[93;40m"),
        ("Green on Black (added)", "\x1b[92;40m"),
        ("Red on Black (deleted)", "\x1b[91;40m"),
        ("White on DarkGrey (menu)", "\x1b[97;100m"),
        ("Black on Grey (chrome)", "\x1b[30;47m"),
    ];

    for (label, sgr) in samples {
        write!(out, "{sgr}  {label:<32}  \x1b[0m\n")?;
    }
    write!(
        out,
        "raw CSI: \x1b[44m BLUE-BG \x1b[0m \x1b[46m CYAN-BG \x1b[0m \x1b[103;30m YELLOW \x1b[0m\n"
    )?;
    out.flush()?;
    eprintln!(
        "forge: demo-colors done — if the lines above are NOT colored, your terminal is ignoring ANSI (check NO_COLOR / TERM)."
    );
    Ok(())
}

fn create_workspace(color_mode: ColorMode) -> App {
    create_workspace_at(
        color_mode,
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    )
}

fn create_workspace_at(color_mode: ColorMode, cwd: PathBuf) -> App {
    let (config, config_warning) = match Config::load_checked() {
        Ok(config) => (config, None),
        Err(error) => (Config::default(), Some(format!("Config error: {error}"))),
    };
    let git = GitService::new();
    let tree_root = git.find_repo_root(&cwd).unwrap_or_else(|| cwd.clone());

    let mut app = App {
        services: Services {
            jobs: JobScheduler::new(),
            fs: FsService::new(),
            git,
        },
        config,
        files: FilesState {
            tree_root,
            expanded: HashSet::new(),
            compact_expand_pending: None,
            children: HashMap::new(),
            visible: Vec::new(),
            selected: 0,
            scroll: 0,
            follow_selection: true,
            restore_select: None,
            external_anchor: None,
            revealed_path: None,
            pending_lists: Vec::new(),
            last_list_click: None,
            sticky_headers: Vec::new(),
        },
        scm: ScmState {
            git_status: None,
            branch: String::new(),
            scm_selected: 0,
            scroll_scm: 0,
            follow_selection: true,
            pending_transfer: None,
            last_scm_click: None,
            scm_expanded_unstaged: HashSet::new(),
            scm_expanded_staged: HashSet::new(),
            sticky_headers: Vec::new(),
            pending_status: None,
            status_rerun: false,
            pending_write: None,
        },
        preview_state: PreviewState {
            preview_scroll: 0,
            preview_scroll_x: 0,
            find: file_find::FileFindState::default(),
            markdown_code: false,
            markdown: MarkdownPreview::default(),
            selection: PreviewSelection::default(),
            text_rows: TextRowsCache::default(),
            pending_source_line: None,
            pending_overview_line: None,
            pending_find_byte: None,
            split_cache: SplitRowsCache::default(),
            diff_row_count: 0,
            pending_preview: None,
            pending_page: None,
            pending_diff: None,
            preview: None,
            preview_loading: false,
            preview_mode: PreviewMode::File,
            diff_source: DiffSource::Working(DiffSide::Unstaged),
            diff_label: String::new(),
            file_diff: None,
            hunk_index: None,
        },
        files_preview: None,
        session: SessionState {
            status: config_warning.unwrap_or_else(|| format!("palette:{}", color_mode.as_str())),
            write_status: None,
            notice_lease: None,
            help_open: false,
            help_reference: false,
            help_scroll: 0,
            help_scroll_limit: 0,
            help_area: ratatui::layout::Rect::default(),
            help_close_rect: ratatui::layout::Rect::default(),
            help_overview_rect: ratatui::layout::Rect::default(),
            help_reference_rect: ratatui::layout::Rect::default(),
            activity: ActivityTab::Files,
            commit_open: false,
            context_menu: None,
            should_quit: false,
            layout: PaneLayout::default(),
            focus: PaneFocus::List,
        },
        tools: tools::ToolsState::default(),
        editing: editor::EditingState::default(),
        keys: keybindings::Keybindings::default(),
        history: history::HistoryState::default(),
        blame: blame::BlameState::default(),
        inline_diff: inline_diff::InlineDiff::default(),
        terminal: None,
        terminal_tabs: Vec::new(),
        terminal_tab_hits: Vec::new(),
        terminal_tab_bar: ratatui::layout::Rect::default(),
        terminal_help_return: false,
        terminal_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
        terminal_frame_epoch: 0,
        scrollbar_drag: None,
    };
    app.tools.filter.show_hidden = app.config.show_hidden_files;
    app.request_list();
    app.request_status();

    app
}

async fn run_async(color_mode: ColorMode) -> Result<()> {
    struct RestoreTerminal;
    impl Drop for RestoreTerminal {
        fn drop(&mut self) {
            let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture);
            ratatui::restore();
        }
    }
    let _restore = RestoreTerminal;
    let handoff = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut initial = None;
    loop {
        crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
        element!(view::Workspace(color_mode: color_mode, initial: initial, editor_handoff: Some(handoff.clone())))
            .fullscreen().await?;
        crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
        ratatui::restore();
        let resume = handoff.lock().expect("editor handoff").take();
        let Some(resume) = resume else {
            break;
        };
        initial = Some(resume.edit(color_mode).await);
    }
    Ok(())
}

impl App {


    fn hint_bar(&self) -> String {
        String::new()
    }

    /// An operation notification is not permanent state. Give it time to be
    /// read, then return the footer to contextual information. An in-flight
    /// Git write must never have its "working" notice prematurely cleared.
    fn expire_write_notice(&mut self) {
        let Some(message) = self.session.write_status.clone() else {
            self.session.notice_lease = None;
            return;
        };
        match &self.session.notice_lease {
            Some((previous, previous_status, since))
                if *previous == message
                    && since.elapsed() >= Duration::from_secs(8)
                    && self.scm.pending_write.is_none()
                    && self.tools.pending_task.is_none() =>
            {
                if self.session.status == *previous_status {
                    self.session.status.clear();
                }
                self.session.write_status = None;
                self.session.notice_lease = None;
            }
            Some((previous, _, _)) if *previous == message => {}
            _ => {
                self.session.notice_lease =
                    Some((message, self.session.status.clone(), Instant::now()));
            }
        }
    }

    fn repo_root(&self) -> Option<PathBuf> {
        self.scm
            .git_status
            .as_ref()
            .map(|s| s.repo_root.clone())
            .or_else(|| self.services.git.find_repo_root(&self.files.tree_root))
    }

    fn scm_slices(&self) -> (&[forge_git::ChangeEntry], &[forge_git::ChangeEntry]) {
        match &self.scm.git_status {
            Some(snap) => (snap.unstaged.as_slice(), snap.staged.as_slice()),
            None => (&[], &[]),
        }
    }

    fn scm_rows(&self) -> Vec<ScmRow> {
        let (u, s) = self.scm_slices();
        let rows = build_scm_rows(
            u,
            s,
            &self.scm.scm_expanded_unstaged,
            &self.scm.scm_expanded_staged,
            &self.history_view(),
        );
        if self.config.compact_folders {
            compact_scm_rows(&rows)
        } else {
            rows
        }
    }

    fn selected_scm_entry(&self) -> Option<&forge_git::ChangeEntry> {
        let rows = self.scm_rows();
        let row = rows.get(self.scm.scm_selected)?;
        let (u, s) = self.scm_slices();
        scm_entry(row, u, s)
    }

    fn selected_scm_row(&self) -> Option<ScmRow> {
        self.scm_rows().get(self.scm.scm_selected).cloned()
    }

    fn clamp_scm_selection(&mut self) {
        let len = self.scm_rows().len();
        if len == 0 {
            self.scm.scm_selected = 0;
        } else if self.scm.scm_selected >= len {
            self.scm.scm_selected = len - 1;
        }
    }

    fn scm_expanded_mut(&mut self, column: StagingColumn) -> &mut HashSet<String> {
        match column {
            StagingColumn::Unstaged => &mut self.scm.scm_expanded_unstaged,
            StagingColumn::Staged => &mut self.scm.scm_expanded_staged,
        }
    }

    /// Toggle expand/collapse for the folder at `index` (no-op for other rows).
    /// Expand all implicit directory segments represented by one compact row.
    fn compact_expand_paths<'a>(
        start: &str,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let paths: Vec<&str> = paths.into_iter().collect();
        let mut chain = vec![start.to_owned()];
        let mut current = start.to_owned();
        for _ in 0..128 {
            let prefix = format!("{current}/");
            let mut only_child: Option<&str> = None;
            for path in &paths {
                let Some(relative) = path.strip_prefix(&prefix) else {
                    continue;
                };
                let Some((candidate, _)) = relative.split_once('/') else {
                    return chain;
                };
                if candidate.is_empty() || only_child.is_some_and(|dir| dir != candidate) {
                    return chain;
                }
                only_child = Some(candidate);
            }
            let Some(child) = only_child else {
                break;
            };
            current.push('/');
            current.push_str(child);
            chain.push(current.clone());
        }
        chain
    }

    fn toggle_scm_expand_at(&mut self, index: usize) {
        let rows = self.scm_rows();
        let Some(ScmRow::Folder { column, path, .. }) = rows.get(index).cloned() else {
            if rows.get(index).and_then(ScmRow::commit).is_some()
                || matches!(rows.get(index), Some(ScmRow::HistorySection { .. }))
            {
                self.toggle_history_row(index);
            }
            return;
        };
        let collapsing = self.scm_expanded_mut(column).contains(&path);
        let chain = if !collapsing && self.config.compact_folders {
            let (unstaged, staged) = self.scm_slices();
            let entries = match column {
                StagingColumn::Unstaged => unstaged,
                StagingColumn::Staged => staged,
            };
            Self::compact_expand_paths(&path, entries.iter().map(|entry| entry.path.as_str()))
        } else {
            vec![path.clone()]
        };
        let set = self.scm_expanded_mut(column);
        if collapsing {
            set.remove(&path);
            let descendants = format!("{path}/");
            set.retain(|value| !value.starts_with(&descendants));
        } else {
            for part in &chain {
                set.insert(part.clone());
            }
        }
        let target = if collapsing {
            &path
        } else {
            chain.last().unwrap_or(&path)
        };
        let rows = self.scm_rows();
        if let Some(i) = rows.iter().position(|r| {
            matches!(r, ScmRow::Folder { column: c, path: p, .. } if *c == column && *p == *target)
        }) {
            self.scm.scm_selected = i;
        } else {
            self.clamp_scm_selection();
        }
    }

    /// Expand folder, or open diff for a file row.
    fn scm_open_or_expand(&mut self) {
        match self.selected_scm_row() {
            Some(ScmRow::Folder { .. }) => {
                self.toggle_scm_expand_at(self.scm.scm_selected);
            }
            Some(ScmRow::Change { .. }) => {
                self.request_diff_for_scm_selection();
                self.session.focus = PaneFocus::Preview;
            }
            Some(ScmRow::Section { column, .. }) => {
                // Jump into first change if present; else no-op.
                let rows = self.scm_rows();
                self.scm.scm_selected = scm_jump_to_column(&rows, column);
                if self.selected_scm_entry().is_some() {
                    self.request_diff_for_scm_selection();
                }
            }
            Some(
                ScmRow::HistorySection { .. }
                | ScmRow::Commit { .. }
                | ScmRow::CommitFolder { .. }
                | ScmRow::CommitFile { .. },
            ) => self.toggle_history_row(self.scm.scm_selected),
            None => {}
        }
    }

    /// `h` / Left: collapse expanded folder, else select parent, else leave Git.
    fn scm_tree_back(&mut self) {
        self.scm.follow_selection = true;
        let Some(row) = self.selected_scm_row() else {
            self.leave_git_sidebar();
            return;
        };
        match row {
            ScmRow::Folder {
                column,
                path,
                expanded,
                ..
            } if expanded => {
                self.scm_expanded_mut(column).remove(&path);
                // Reselect the same folder (now collapsed).
                let rows = self.scm_rows();
                if let Some(i) = rows.iter().position(|r| {
                    matches!(
                        r,
                        ScmRow::Folder { column: c, path: p, .. } if *c == column && *p == path
                    )
                }) {
                    self.scm.scm_selected = i;
                }
            }
            ScmRow::Folder { .. } | ScmRow::Change { .. } => {
                let rows = self.scm_rows();
                if let Some(parent) = find_scm_parent_row(&rows, self.scm.scm_selected) {
                    self.scm.scm_selected = parent;
                    if self.selected_scm_entry().is_none() {
                        self.preview_state.preview_mode = PreviewMode::Diff;
                        self.clear_diff_view();
                        self.preview_state.preview_loading = false;
                        self.services.jobs.cancel_key("git:diff");
                        self.preview_state.pending_diff = None;
                    }
                } else {
                    self.leave_git_sidebar();
                }
            }
            ScmRow::Section { .. } => self.leave_git_sidebar(),
            ScmRow::HistorySection { .. } => self.history.expanded = false,
            ScmRow::Commit { .. } | ScmRow::CommitFolder { .. } | ScmRow::CommitFile { .. } => {
                self.collapse_history_row(self.scm.scm_selected)
            }
        }
    }

    fn in_git_sidebar(&self) -> bool {
        matches!(self.session.activity, ActivityTab::Git)
    }

    fn sidebar_focused(&self) -> bool {
        matches!(self.session.focus, PaneFocus::List | PaneFocus::Staging)
    }

    fn git_sync_shortcut_enabled(&self) -> bool {
        self.in_git_sidebar() && self.sidebar_focused()
    }

    fn request_list(&mut self) {
        self.invalidate_search_cache();
        // Manual refresh (r), Git writes, and settings changes must not
        // destroy the current file reader or its list viewport. A genuinely
        // new workspace starts with an empty visible list and still resets.
        if !self.files.visible.is_empty() {
            self.record_refresh_anchor();
            let shown_file = self.preview_state.preview.as_ref().map(|p| p.path.clone());
            self.refresh_tree_in_place();
            if let Some(path) = shown_file {
                self.refresh_preview_if_affected(&[path]);
            }
            return;
        }
        // Cold start / explicit workspace change.
        self.record_refresh_anchor();
        self.files.children.clear();
        self.cancel_pending_lists();
        self.preview_state.preview = None;
        self.preview_state.preview_loading = false;
        self.preview_state.preview_scroll = 0;
        self.preview_state.preview_mode = PreviewMode::File;
        self.clear_diff_view();
        self.preview_state.pending_preview = None;
        self.cancel_preview_page();
        self.preview_state.pending_diff = None;
        self.services.jobs.cancel_key("fs:preview");
        self.services.jobs.cancel_key("git:diff");
        self.request_dir(self.files.tree_root.clone());
    }

    fn record_refresh_anchor(&mut self) {
        if self.files.external_anchor.is_none() {
            self.files.external_anchor = self
                .selected_entry()
                .map(|entry| (entry.path.clone(), self.files.scroll));
        }
    }

    /// Refresh directory contents without replacing the currently displayed
    /// tree. Reload every expanded directory as well as root; stale children
    /// stay visible until their respective async results arrive.
    fn refresh_tree_in_place(&mut self) {
        // Do not abandon handles without cancelling their scheduled work.
        // A new refresh must start one coherent generation of listings.
        self.cancel_pending_lists();
        let root = self.files.tree_root.clone();
        self.request_dir(root.clone());
        let mut expanded: Vec<_> = self.files.expanded.iter()
            .filter(|path| *path != &root && path.starts_with(&root))
            .cloned()
            .collect();
        expanded.sort();
        expanded.dedup();
        for path in expanded {
            self.request_dir(path);
        }
    }

    /// Refresh the directory cache and Git snapshot without dropping focus,
    /// expansion, or a loaded diff/preview. Editing drafts are never touched.
    ///
    /// `git_metadata_changed` is true when the watcher saw HEAD, packed-refs
    /// or `refs/` move; only then can the commit graph differ from what the
    /// sidebar already holds. Worktree writes (saves, builds, external edits)
    /// refresh status and the listing but leave history alone.
    fn refresh_external_changes(&mut self, git_metadata_changed: bool) {
        if self.editing.saving() || self.scm.pending_write.is_some() {
            return; // Own writes refresh on completion.
        }
        self.invalidate_search_cache();
        self.reset_inline_diff();
        self.record_refresh_anchor();
        self.refresh_tree_in_place();
        self.request_status();
        if git_metadata_changed && self.history.expanded {
            self.request_history();
        }
    }

    fn cancel_pending_lists(&mut self) {
        for pending in self.files.pending_lists.drain(..) {
            self.services.jobs.cancel(pending.id);
        }
    }

    fn request_dir(&mut self, path: PathBuf) {
        // Rebuilding the tree after each completed directory could otherwise
        // re-enqueue other still-pending expanded directories repeatedly.
        if self.files.pending_lists.iter().any(|pending| pending.path == path) {
            return;
        }
        let handle = self.services.fs.list(&self.services.jobs, path.clone());
        self.session.status = format!("listing {}...", path.display());
        self.files.pending_lists.push(PendingList {
            id: handle.id,
            path,
            handle,
        });
    }

    fn request_missing_expanded(&mut self) {
        let missing: Vec<PathBuf> = self
            .files
            .expanded
            .iter()
            .filter(|p| !self.files.children.contains_key(p.as_path()))
            .cloned()
            .collect();
        for path in missing {
            self.request_dir(path);
        }
    }

    fn rebuild_visible(&mut self) {
        self.files.visible = flatten_tree(
            &self.files.tree_root,
            &self.files.children,
            &self.files.expanded,
        );
        let visible = std::mem::take(&mut self.files.visible);
        let filtered: Vec<_> = visible
            .into_iter()
            .filter(|r| self.filter_entry(&r.entry))
            .collect();
        self.files.visible = if self.config.compact_folders {
            compact_tree_rows(&filtered, &self.files.children)
        } else {
            filtered
        };
        if self.files.visible.is_empty() {
            self.files.selected = 0;
            self.files.scroll = 0;
        } else if self.files.selected >= self.files.visible.len() {
            self.files.selected = self.files.visible.len() - 1;
        }
    }

    /// Rebuild after a filter change while keeping selection identity stable.
    ///
    /// The selected path wins even when its row index moved. If the path is
    /// filtered out, the nearest visible ancestor directory is selected;
    /// otherwise the previous index is clamped as a last resort. Expansion and
    /// focus are untouched so closing a filter restores the same tree shape.
    fn rebuild_visible_preserving_selection(&mut self) {
        let anchor = self.selected_entry().map(|entry| entry.path.clone());
        let old_selected = self.files.selected;
        let old_scroll = self.files.scroll;
        let viewport = self.session.layout.list_inner.height as usize;
        self.rebuild_visible();
        let len = self.files.visible.len();
        if len == 0 {
            return;
        }
        let found = anchor.and_then(|path| {
            if let Some(index) = self
                .files
                .visible
                .iter()
                .position(|row| row.entry.path == path)
            {
                return Some(index);
            }
            // When an expanded chain becomes compact, its original folder
            // is represented by the deepest directory in that same chain.
            // Keep the selected folder instead of jumping up to a sibling.
            if let Some(index) = self.files.visible.iter().position(|row| {
                matches!(row.entry.kind, EntryKind::Directory)
                    && row.entry.name.contains(" / ")
                    && row.entry.path.starts_with(&path)
            }) {
                return Some(index);
            }
            let mut parent = path.parent();
            while let Some(candidate) = parent {
                if let Some(index) = self
                    .files
                    .visible
                    .iter()
                    .position(|row| row.entry.path == candidate)
                {
                    return Some(index);
                }
                if candidate == self.files.tree_root {
                    break;
                }
                parent = candidate.parent();
            }
            None
        });
        self.files.selected = found.unwrap_or_else(|| old_selected.min(len - 1));
        if !self.files.follow_selection {
            self.files.scroll = old_scroll.min(len - 1);
            return;
        }
        let max_scroll = len.saturating_sub(viewport.max(1));
        if viewport == 0 {
            self.files.scroll = old_scroll.min(self.files.selected);
        } else if self.files.selected >= old_scroll
            && self.files.selected < old_scroll.saturating_add(viewport)
        {
            self.files.scroll = old_scroll.min(max_scroll);
        } else if self.files.selected < old_scroll {
            self.files.scroll = self.files.selected;
        } else {
            self.files.scroll = self
                .files
                .selected
                .saturating_sub(viewport.saturating_sub(1))
                .min(max_scroll);
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.files
            .visible
            .get(self.files.selected)
            .map(|r| &r.entry)
    }

    fn request_status(&mut self) {
        // The gix status call runs on a blocking worker that cannot be
        // aborted, so a second request while one is in flight would only add
        // another full fan-out of short-lived threads. Remember it and run
        // once more when the current job finishes.
        if self.scm.pending_status.is_some() {
            self.scm.status_rerun = true;
            return;
        }
        let start = self.files.tree_root.clone();
        let job_key = format!("git:status:{}", start.display());
        let git = self.services.git;
        let start_for_job = start.clone();
        let handle = self
            .services
            .jobs
            .spawn(JobPriority::Normal, Some(job_key), async move {
                match tokio::task::spawn_blocking(move || git.status(&start_for_job)).await {
                    Ok(Ok(snap)) => Ok(Some(snap)),
                    Ok(Err(GitReadError::NotARepository(_))) => Ok(None),
                    Ok(Err(GitReadError::Discover(_))) => Ok(None),
                    Ok(Err(err)) => Err(err),
                    Err(join) => Err(GitReadError::Status(format!("status worker: {join}"))),
                }
            });
        self.scm.pending_status = Some(PendingStatus {
            id: handle.id,
            key: start,
            handle,
        });
    }

    fn request_preview_for_selection(&mut self) {
        // Folders navigate the tree without replacing the viewer or its scroll.
        if self
            .selected_entry()
            .is_some_and(|entry| matches!(entry.kind, EntryKind::Directory))
        {
            return;
        }
        if self
            .selected_entry()
            .is_some_and(|entry| self.preview_state.find.path != entry.path)
        {
            self.reset_file_find();
        }
        self.cancel_preview_page();
        self.preview_state.preview_scroll_x = 0;
        self.reset_blame();
        self.reset_inline_diff();
        self.preview_state.preview_mode = PreviewMode::File;
        self.preview_state.selection.clear();
        self.preview_state.markdown = MarkdownPreview::default();
        self.preview_state.text_rows.clear();
        self.preview_state.pending_source_line = None;
        self.preview_state.pending_overview_line = None;
        self.preview_state.pending_find_byte = None;
        self.clear_diff_view();
        self.services.jobs.cancel_key("git:diff");
        self.preview_state.pending_diff = None;

        let Some(entry) = self.selected_entry().cloned() else {
            self.preview_state.preview = None;
            self.preview_state.preview_loading = false;
            self.preview_state.preview_scroll = 0;
            self.preview_state.pending_preview = None;
            self.services.jobs.cancel_key("fs:preview");
            return;
        };

        self.preview_state.preview_loading = true;
        self.preview_state.preview_scroll = 0;
        let path = entry.path.clone();
        let handle = self
            .services
            .fs
            .preview(&self.services.jobs, entry.path, entry.kind);
        self.preview_state.pending_preview = Some(PendingPreview {
            id: handle.id,
            path,
            handle,
        });
    }

    /// Reload the selected file only when the watcher actually reports a
    /// change to that file. Keep the old preview visible and preserve scroll
    /// while its replacement is read asynchronously.
    fn refresh_preview_if_affected(&mut self, changed_paths: &[PathBuf]) {
        if !matches!(self.preview_state.preview_mode, PreviewMode::File) {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if matches!(entry.kind, EntryKind::Directory)
            || !changed_paths.iter().any(|path| path == &entry.path)
        {
            return;
        }
        let same_preview = self
            .preview_state
            .preview
            .as_ref()
            .is_some_and(|preview| preview.path == entry.path);
        let same_pending = self
            .preview_state
            .pending_preview
            .as_ref()
            .is_some_and(|pending| pending.path == entry.path);
        if !same_preview && !same_pending {
            return; // The next directory load will request the new selection.
        }
        self.cancel_preview_page();
        self.refresh_blame_in_place();
        // Read through the visible source line before swapping the new
        // snapshot in. Replacing a 2,000-line preview with the first 400
        // lines would otherwise make the renderer clamp scroll to 0.
        let viewport = self.session.layout.preview_inner.height as usize;
        let through_line = if self.preview_state.text_rows.line_count() > 0 {
            self.preview_state.text_rows.row_to_source(
                self.preview_state.preview_scroll.saturating_add(viewport),
            ).saturating_add(16)
        } else {
            self.preview_state.preview_scroll.saturating_add(viewport).saturating_add(32)
        };
        let path = entry.path.clone();
        let handle = self.services.fs.preview_through_line(
            &self.services.jobs, path.clone(), entry.kind, through_line,
        );
        self.preview_state.pending_preview = Some(PendingPreview {
            id: handle.id,
            path,
            handle,
        });
        // Showing the generic Loading view would clamp its one-row content
        // and silently reset the scroll before the new file bytes arrive.
        self.preview_state.preview_loading = !same_preview;
    }

    /// Reset the preview for a new diff and return the repository root.
    fn begin_diff(&mut self, source: DiffSource) -> Option<PathBuf> {
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return None;
        };
        self.cancel_preview_page();
        self.preview_state.preview_mode = PreviewMode::Diff;
        self.preview_state.preview_scroll_x = 0;
        self.preview_state.preview_loading = true;
        self.preview_state.preview_scroll = 0;
        self.preview_state.diff_label = match &source {
            DiffSource::Working(side) => side.label().to_string(),
            DiffSource::Commit { short, .. } => short.clone(),
        };
        self.preview_state.diff_source = source;
        self.clear_diff_view();
        self.services.jobs.cancel_key("fs:preview");
        self.preview_state.pending_preview = None;
        // Status middle stays blank while loading; left chip shows basename.
        self.session.status.clear();
        Some(repo)
    }

    fn request_diff(&mut self, rela: String, side: DiffSide) {
        let source = DiffSource::Working(side);
        let Some(repo) = self.begin_diff(source.clone()) else {
            return;
        };
        let git = self.services.git;
        let path = rela.clone();
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:diff".into()), async move {
                    git.file_diff(&repo, &path, side).await
                });
        self.preview_state.pending_diff = Some(PendingDiff {
            id: handle.id,
            path: rela,
            source,
            handle,
        });
    }

    /// Diff of one file as changed by `oid` (read-only history view).
    fn request_commit_diff(&mut self, oid: String, short: String, rela: String) {
        let source = DiffSource::Commit {
            oid: oid.clone(),
            short,
        };
        let Some(repo) = self.begin_diff(source.clone()) else {
            return;
        };
        let git = self.services.git;
        let path = rela.clone();
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:diff".into()), async move {
                    git.commit_file_diff(&repo, &oid, &path).await
                });
        self.preview_state.pending_diff = Some(PendingDiff {
            id: handle.id,
            path: rela,
            source,
            handle,
        });
    }

    fn scm_directory_selected(&self) -> bool {
        matches!(
            self.selected_scm_row(),
            Some(ScmRow::Folder { .. } | ScmRow::CommitFolder { .. })
        )
    }

    fn request_diff_for_scm_selection(&mut self) {
        if self.scm_directory_selected() {
            return;
        }
        if let Some(ScmRow::Commit { index }) = self.selected_scm_row() {
            if let Some(commit) = self.history.commits.get(index) {
                self.request_commit_details(commit.id.clone());
                return;
            }
        }
        if let Some(ScmRow::CommitFile { commit, index, .. }) = self.selected_scm_row() {
            let commit = self.history.commits.get(commit).cloned();
            let path = commit
                .as_ref()
                .and_then(|c| self.history.files.get(&c.id))
                .and_then(|changes| changes.get(index))
                .map(|change| change.path.clone());
            if let (Some(commit), Some(path)) = (commit, path) {
                self.request_commit_diff(commit.id, commit.short, path);
                return;
            }
        }
        let Some(entry) = self.selected_scm_entry().cloned() else {
            self.session.status.clear();
            self.preview_state.preview_mode = PreviewMode::Diff;
            self.services.jobs.cancel_key("git:diff");
            self.preview_state.pending_diff = None;
            self.clear_diff_view();
            self.preview_state.preview_loading = false;
            return;
        };
        self.request_diff(entry.path, entry.side);
    }

    fn request_write_stage(&mut self, stage: bool) {
        let action = self.selection_stage_action(stage);
        let target = match self.stage_target(action) {
            Ok(target) => target,
            Err(reason) => {
                self.session.status = reason.into();
                return;
            }
        };
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };

        let scope = if matches!(
            action,
            ContextAction::StageFolder | ContextAction::UnstageFolder
        ) {
            format!(
                "folder {}/ · {} files",
                target.path.display(),
                target.paths.len()
            )
        } else {
            format!("file {}", target.path.display())
        };
        // Only transfer the focused single *file* row. Folder, batch, and
        // hunk operations have different selection semantics.
        let transfer = if self.session.focus == PaneFocus::List
            && matches!(self.selected_scm_row(), Some(ScmRow::Change { .. }))
            && target.paths.len() == 1
        {
            Some((
                target.path.to_string_lossy().into_owned(),
                if stage {
                    StagingColumn::Staged
                } else {
                    StagingColumn::Unstaged
                },
            ))
        } else {
            None
        };
        let git = self.services.git;
        let paths_job = target.paths;
        let op_label = if stage { "staging" } else { "unstaging" };
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    let r = if stage {
                        git.stage_paths(&repo, &paths_job).await
                    } else {
                        git.unstage_paths(&repo, &paths_job).await
                    };
                    r.map(|_| None)
                });
        self.session.status = format!("{op_label} {scope}...");
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_transfer = transfer;
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: if stage {
                WriteOp::Stage
            } else {
                WriteOp::Unstage
            },
            target: scope,
            handle,
        });
    }

    fn request_commit(&mut self, message: String) {
        if self.write_busy() {
            return;
        }
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };
        let git = self.services.git;
        let msg = message.clone();
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    git.commit(&repo, msg).await.map(|_| None)
                });
        self.session.status = "committing...".into();
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Commit(message),
            target: "repository staged changes".into(),
            handle,
        });
    }

    fn clear_diff_view(&mut self) {
        self.preview_state.selection.clear();
        self.preview_state.file_diff = None;
        self.preview_state.hunk_index = None;
        self.preview_state.split_cache.clear();
        self.preview_state.diff_row_count = 0;
    }

    fn set_file_diff(&mut self, diff: FileDiff) {
        self.preview_state.selection.clear();
        self.preview_state
            .split_cache
            .set_full_new_lines(diff.full_new_lines.as_deref());
        let count = if diff.lines.is_empty() && diff.full_new_lines.is_none() {
            1
        } else {
            self.preview_state.split_cache.ensure(&diff.lines).max(1)
        };
        self.preview_state.diff_row_count = count;
        self.preview_state.file_diff = Some(diff);
    }

    fn markdown_available(&self) -> bool {
        !self.in_git_sidebar()
            && !self.preview_state.preview_loading
            && matches!(self.preview_state.preview_mode, PreviewMode::File)
            && self.preview_state.preview.as_ref().is_some_and(|preview| {
                matches!(preview.content, PreviewContent::Text { .. })
                    && TextLang::from_name(&preview.path.to_string_lossy()) == TextLang::Markdown
            })
    }

    fn set_markdown_code(&mut self, code: bool) {
        if self.markdown_available() && self.preview_state.markdown_code != code {
            self.preview_state.markdown_code = code;
            self.preview_state.selection.clear();
            self.preview_state.preview_scroll = 0;
        }
    }

    /// Rendered Markdown rows do not correspond to source line numbers.
    fn preview_source_line(&self) -> usize {
        if self.markdown_available() && !self.preview_state.markdown_code {
            1
        } else if matches!(
            self.preview_state.preview.as_ref().map(|p| &p.content),
            Some(PreviewContent::Text { .. })
        ) && self.preview_state.text_rows.line_count() > 0
        {
            self.preview_state
                .text_rows
                .row_to_source(self.preview_state.preview_scroll)
        } else {
            self.preview_state.preview_scroll + 1
        }
    }

    /// Line count for preview scrolling, including cached Markdown wrapping.
    fn preview_total_lines(&self) -> usize {
        if self.markdown_available() && !self.preview_state.markdown_code {
            return self.preview_state.markdown.line_count().max(1);
        }
        match self.preview_state.preview_mode {
            PreviewMode::Diff => {
                if self.preview_state.file_diff.is_some() {
                    self.preview_state.diff_row_count.max(1)
                } else {
                    1
                }
            }
            PreviewMode::File => match self.preview_state.preview.as_ref().map(|p| &p.content) {
                None | Some(PreviewContent::Empty) => 1,
                Some(PreviewContent::Error { .. }) => 2,
                Some(PreviewContent::Binary { .. }) => 4,
                Some(PreviewContent::Directory { .. }) => 5,
                Some(PreviewContent::Text {
                    lines, truncated, ..
                }) => {
                    if self.preview_state.text_rows.line_count() > 0 {
                        self.preview_state.text_rows.line_count()
                    } else {
                        let base = if lines.is_empty() { 1 } else { lines.len() };
                        if *truncated {
                            base + 2
                        } else {
                            base
                        }
                    }
                }
            },
        }
    }

    fn poll_pending_list(&mut self) -> bool {
        let finished = pending_request::drain_ready(
            &mut self.files.pending_lists,
            |pending| pending.handle.try_join(),
        );
        let progressed = !finished.is_empty();
        for (pending, result) in finished {
            self.apply_list_result(pending.path, result);
        }
        progressed
    }

    fn poll_pending_status(&mut self) -> bool {
        if let Some((pending, result)) = pending_request::take_ready(
            &mut self.scm.pending_status,
            |pending| pending.handle.try_join(),
        ) {
            let key = pending.key;
            self.apply_status_result(key, result);
            // A status-filtered search may have started before this snapshot arrived.
            if self.tools.filter.status != 0 {
                self.invalidate_search_cache();
            }
            if std::mem::take(&mut self.scm.status_rerun) {
                self.request_status();
            }
            true
        } else {
            false
        }
    }

    fn poll_pending_preview(&mut self) -> bool {
        if let Some((pending, result)) = pending_request::take_ready(
            &mut self.preview_state.pending_preview,
            |pending| pending.handle.try_join(),
        ) {
            self.apply_preview_result(pending.path, result);
            true
        } else {
            false
        }
    }

    fn poll_pending_page(&mut self) -> bool {
        let Some((pending, result)) = pending_request::take_ready(
            &mut self.preview_state.pending_page,
            |pending| pending.handle.try_join(),
        ) else {
            return false;
        };
        let path = pending.path;
        let from_offset = pending.from_offset;
        if !matches!(self.preview_state.preview_mode, PreviewMode::File)
            || self.preview_state.pending_preview.is_some()
            || !self.selected_entry().is_some_and(|entry| {
                entry.path == path || matches!(entry.kind, EntryKind::Directory)
            })
        {
            return true;
        }
        let Some(preview) = self.preview_state.preview.as_mut() else {
            return true;
        };
        if preview.path != path {
            return true;
        }
        let PreviewContent::Text {
            lines,
            truncated,
            bytes_read,
        } = &mut preview.content
        else {
            return true;
        };
        if !*truncated || *bytes_read as u64 != from_offset {
            return true; // Stale page from an older file/selection generation.
        }
        match result {
            Ok(Ok(page)) if page.next_offset > from_offset => {
                lines.extend(page.lines);
                *bytes_read = page.next_offset as usize;
                *truncated = !page.eof;
                // Keep scroll/focus intact. TextRowsCache will rebuild with
                // updated byte and line counts at the next draw.
            }
            Ok(Ok(_)) => {
                *truncated = false; // Do not loop forever on a stalled EOF.
            }
            Ok(Err(err)) => self.session.status = format!("preview page error: {err}"),
            Err(JobError::Cancelled | JobError::Shutdown) => {}
        }
        self.advance_file_find_jump();
        self.advance_overview_jump();
        true
    }

    /// Advance a focus-ruler jump, loading additional pages only as needed.
    fn advance_overview_jump(&mut self) {
        let Some(target) = self.preview_state.pending_overview_line else { return; };
        let Some(preview) = self.preview_state.preview.as_ref() else { return; };
        let PreviewContent::Text { lines, truncated, .. } = &preview.content else {
            self.preview_state.pending_overview_line = None;
            return;
        };
        if target <= lines.len() || !*truncated {
            let target = target.min(lines.len().max(1));
            self.preview_state.pending_overview_line = None;
            self.preview_state.pending_source_line = Some(target);
            self.preview_state.pending_find_byte = None;
            self.preview_state.preview_scroll = self.preview_state.text_rows.source_to_row(target);
        } else {
            self.maybe_request_preview_page();
        }
    }

    /// Marker priority is search over Git; clicking empty track seeks proportionally.
    fn overview_click_target(&self, row: usize, height: usize) -> Option<usize> {
        let source_total = self.overview_git_data().map(|(n, _)| n)
            .or_else(|| match &self.preview_state.preview.as_ref()?.content {
                PreviewContent::Text { lines, .. } => Some(lines.len()),
                _ => None,
            })?.max(1);
        let scale = |line: usize| line.saturating_sub(1)
            .saturating_mul(height.saturating_sub(1)) / source_total.saturating_sub(1).max(1);
        let y = row.min(height.saturating_sub(1));
        let find = &self.preview_state.find;
        if find.open && !find.query.is_empty()
            && self.preview_state.preview.as_ref().is_some_and(|p| p.path == find.path) {
            if let Some(hit) = find.matches.iter()
                .filter(|hit| scale(hit.line) == y)
                .min_by_key(|hit| hit.line.abs_diff(y * source_total / height.max(1)))
            { return Some(hit.line); }
        }
        if let Some((_, git)) = self.overview_git_data() {
            if let Some(&(line, _)) = git.iter()
                .filter(|(line, _)| scale(*line) == y)
                .min_by_key(|(line, color)| {
                    (usize::from(*color == ratatui::style::Color::Blue),
                     line.abs_diff(y * source_total / height.max(1)))
                })
            { return Some(line); }
        }
        Some(1 + y.saturating_mul(source_total.saturating_sub(1))
            / height.saturating_sub(1).max(1))
    }

    fn cancel_preview_page(&mut self) {
        self.preview_state.pending_page = None;
        self.services.jobs.cancel_key("fs:preview:page");
    }

    /// Fetch the next source-line page when the viewport approaches the
    /// current end, without eagerly scanning an entire large source file.
    fn maybe_request_preview_page(&mut self) {
        if !matches!(self.preview_state.preview_mode, PreviewMode::File)
            || self.preview_state.preview_loading
            || self.preview_state.pending_preview.is_some()
            || self.preview_state.pending_page.is_some()
        {
            return;
        }
        let Some(preview) = self.preview_state.preview.as_ref() else {
            return;
        };
        let PreviewContent::Text {
            truncated: true,
            bytes_read,
            ..
        } = &preview.content
        else {
            return;
        };
        let Some(entry) = self.selected_entry() else {
            return;
        };
        // Selecting a folder deliberately leaves the previous file visible.
        // It must remain scrollable and pageable in that situation too.
        if entry.path != preview.path && !matches!(entry.kind, EntryKind::Directory) {
            return;
        }
        let viewport = self.session.layout.preview_inner.height as usize;
        let remaining = self
            .preview_total_lines()
            .saturating_sub(self.preview_state.preview_scroll.saturating_add(viewport));
        if remaining > viewport.max(32)
            && self.preview_state.find.jump_target.is_none()
            && self.preview_state.pending_overview_line.is_none()
        {
            return;
        }
        let path = preview.path.clone();
        let from_offset = *bytes_read as u64;
        let handle = self
            .services
            .fs
            .preview_page(&self.services.jobs, path.clone(), from_offset);
        self.preview_state.pending_page = Some(PendingPreviewPage {
            path,
            from_offset,
            handle,
        });
    }

    fn poll_pending_diff(&mut self) -> bool {
        if let Some((pending, result)) = pending_request::take_ready(
            &mut self.preview_state.pending_diff,
            |pending| pending.handle.try_join(),
        ) {
            let path = pending.path;
            let source = pending.source;
            match result {
                Ok(Ok(diff)) => {
                    if diff.path != path
                        || !matches!(self.preview_state.preview_mode, PreviewMode::Diff)
                        || self.preview_state.diff_source != source
                        || (!self.diff_matches_selection(&diff) && !self.scm_directory_selected())
                    {
                        self.preview_state.preview_loading = false;
                        return true;
                    }
                    let hunk_count = diff.hunks().len();
                    self.preview_state.hunk_index = if hunk_count == 0 { None } else { Some(0) };
                    self.set_file_diff(diff);
                    self.preview_state.preview_loading = false;
                    self.preview_state.preview_mode = PreviewMode::Diff;
                    self.preview_state.preview_scroll = 0;
                    self.ensure_hunk_visible();
                    if let Some(resume) = self.tools.resume_diff.take().filter(|r| {
                        r.path == path
                            && matches!(&source, DiffSource::Working(side) if *side == r.side)
                    }) {
                        self.preview_state.preview_scroll = resume
                            .scroll
                            .min(self.preview_state.diff_row_count.saturating_sub(1));
                        self.preview_state.hunk_index = resume
                            .hunk
                            .map(|i| i.min(hunk_count.saturating_sub(1)))
                            .filter(|_| hunk_count > 0);
                        self.session.focus = resume.focus;
                    }
                    // Left chip + preview header carry identity; middle status blank.
                    self.session.status.clear();
                }
                Ok(Err(err)) => {
                    self.preview_state.preview_loading = false;
                    self.session.status = format!("diff error: {err}");
                }
                Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
            }
            true
        } else {
            false
        }
    }

    fn poll_pending_write(&mut self) -> bool {
        if let Some((pending, result)) = pending_request::take_ready(
            &mut self.scm.pending_write,
            |pending| pending.handle.try_join(),
        ) {
            let op = match pending.op {
                WriteOp::Stage => "staged",
                WriteOp::Unstage => "unstaged",
                WriteOp::StageHunk => "hunk staged",
                WriteOp::UnstageHunk => "hunk unstaged",
                WriteOp::Commit(_) => "committed",
                WriteOp::Push => "pushed",
                WriteOp::Pull => "pulled",
            }.to_string();
            let refresh = op != "pushed";
            let target = pending.target;
            let remote = matches!(op.as_str(), "pulled" | "pushed");
            match result {
                Ok(Ok(msg)) => {
                    self.session.write_status = Some(format!("{op}: {target}"));
                    let detail = msg.unwrap_or_default();
                    if remote {
                        self.tools.remote_result = format!("{op}: {detail}");
                        self.request_sync_status();
                    }
                    if refresh {
                        self.session.status = if detail.is_empty() {
                            format!("{op} - refreshing...")
                        } else {
                            format!("{op} - {detail}")
                        };
                        self.request_status();
                        if self.in_git_sidebar() && self.history.expanded {
                            // A commit rewrote the graph; reload it (cheap, cached jobs).
                            self.request_history();
                        }
                        // Keep diff open after hunk ops — reload it.
                        if matches!(
                            op.as_str(),
                            "hunk staged" | "hunk unstaged" | "staged" | "unstaged" | "committed"
                        ) {
                            if let Some(diff) = self.preview_state.file_diff.clone() {
                                let path = diff.path.clone();
                                let side = diff.side;
                                self.request_diff(path, side);
                            } else {
                                self.request_list();
                            }
                        } else {
                            self.request_list();
                        }
                    } else {
                        self.session.status = if detail.is_empty() {
                            op
                        } else {
                            format!("{op} - {detail}")
                        };
                    }
                }
                Ok(Err(err)) => {
                    self.scm.pending_transfer = None;
                    self.session.status = format!("{op} failed: {err}");
                    self.session.write_status = Some(self.session.status.clone());
                    if remote {
                        self.tools.remote_result = self.session.status.clone();
                        let message = self.session.status.clone();
                        self.open_tools(tools::PanelKind::Text);
                        if let Some(panel) = &mut self.tools.panel {
                            panel.title = "Remote operation failed".into();
                            panel.detail=format!("{message}\n\nUse : Commands → Conflicts to resolve conflicts. Pull uses --ff-only and refuses divergent histories.");
                        }
                        self.request_status();
                    }
                }
                Err(JobError::Cancelled) | Err(JobError::Shutdown) => {
                    self.scm.pending_transfer = None;
                    self.session.write_status = Some(format!("{op} interrupted: {target}"));
                }
            }
            true
        } else {
            false
        }
    }

    fn apply_list_result(
        &mut self,
        path: PathBuf,
        result: Result<Result<Vec<Entry>, FsError>, JobError>,
    ) {
        match result {
            Ok(Ok(entries)) => {
                self.files.children.insert(path.clone(), entries);
                let compact_selected = self.advance_compact_folder_expansion();
                self.rebuild_visible();
                if let Some(target) = compact_selected {
                    if let Some(index) = self
                        .files
                        .visible
                        .iter()
                        .position(|r| r.entry.path == target)
                    {
                        self.files.selected = index;
                    }
                }

                if path == self.files.tree_root {
                    if let Some(name) = self.files.restore_select.take() {
                        if let Some(i) = self
                            .files
                            .visible
                            .iter()
                            .position(|r| r.depth == 0 && r.entry.name == name)
                        {
                            self.files.selected = i;
                        }
                    }
                }

                if self.files.follow_selection && self.files.selected < self.files.scroll {
                    self.files.scroll = self.files.selected;
                }
                if self.files.visible.is_empty() {
                    self.files.scroll = 0;
                }

                self.request_missing_expanded();
                if let Some((path, old_scroll)) = self.files.external_anchor.clone() {
                    if let Some(index) = self
                        .files
                        .visible
                        .iter()
                        .position(|row| row.entry.path == path)
                    {
                        self.files.selected = index;
                        self.files.scroll = if self.files.follow_selection {
                            old_scroll.min(index)
                        } else {
                            old_scroll
                        };
                        // Subsequent expanded-directory results can still shift
                        // row indices. Retain the original anchor until the
                        // entire refresh is complete, not just the root load.
                        if self.files.pending_lists.is_empty() {
                            self.files.external_anchor = None;
                        }
                    } else if self.files.pending_lists.is_empty() {
                        // Removed paths fall back to their closest visible ancestor.
                        let mut parent = path.parent();
                        while let Some(folder) = parent {
                            if let Some(index) = self
                                .files
                                .visible
                                .iter()
                                .position(|row| row.entry.path == folder)
                            {
                                self.files.selected = index;
                                break;
                            }
                            parent = folder.parent();
                        }
                        self.files.external_anchor = None;
                    }
                }
                self.session.status = self.hint_bar();
                // A reveal can depend on several asynchronously loaded parent
                // directories. Resolve it immediately when any listing arrives,
                // even if this was the last pending job. Do not preview the
                // previous selection while the requested target is loading.
                if self.tools.reveal.is_some() {
                    self.try_reveal();
                } else if matches!(self.preview_state.preview_mode, PreviewMode::File)
                    && self.files.external_anchor.is_none()
                {
                    // Rebuilding a directory is not a file selection change.
                    // In particular, other files and Git metadata can refresh
                    // the tree while the user is reading far down a preview.
                    // Only request a preview if its selected path truly changed
                    // (or no preview has been loaded/requested yet).
                    if let Some(entry) = self.selected_entry() {
                        let path = &entry.path;
                        let same_preview = self
                            .preview_state
                            .preview
                            .as_ref()
                            .is_some_and(|preview| &preview.path == path);
                        let same_pending = self
                            .preview_state
                            .pending_preview
                            .as_ref()
                            .is_some_and(|pending| &pending.path == path);
                        if !same_preview && !same_pending {
                            self.request_preview_for_selection();
                        }
                    } else if self.preview_state.preview.is_none()
                        && self.preview_state.pending_preview.is_none()
                    {
                        self.request_preview_for_selection();
                    }
                }
            }
            Ok(Err(err)) => {
                self.session.status = format!("list error: {err}");
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn apply_status_result(
        &mut self,
        key: PathBuf,
        result: Result<Result<Option<StatusSnapshot>, GitReadError>, JobError>,
    ) {
        // A directory can change while its blocking status worker is still
        // running. A shared Git repository is NOT enough to make a snapshot
        // from the old Files root valid for the current workspace.
        if key != self.files.tree_root {
            return;
        }
        match result {
            Ok(Ok(Some(snap))) => {
                self.scm.branch =
                    read_branch_name(&snap.repo_root).unwrap_or_else(|| "HEAD".into());
                self.scm.git_status = Some(snap);
                self.rebuild_visible_preserving_selection();
                self.request_sync_status();
                // Do not jump on an out-of-order snapshot from before the
                // write completed. The rerun will contain the new side.
                if self.scm.pending_write.is_none() {
                    if let Some((path, side)) = self.scm.pending_transfer.as_ref().cloned() {
                        let has_target = self
                            .scm_slices()
                            .0
                            .iter()
                            .chain(self.scm_slices().1.iter())
                            .any(|entry| entry.path == path && entry.side == side.side());
                        if has_target {
                            expand_ancestors_for_path(self.scm_expanded_mut(side), &path);
                            let rows = self.scm_rows();
                            let (unstaged, staged) = self.scm_slices();
                            if let Some(index) = rows.iter().position(|row| {
                                scm_entry(row, unstaged, staged).is_some_and(|entry| {
                                    entry.path == path && entry.side == side.side()
                                })
                            }) {
                                self.scm.scm_selected = index;
                                self.scm.follow_selection = true;
                                self.scm.pending_transfer = None;
                            }
                        } else if !self.scm.status_rerun {
                            // The file may have vanished or another refresh
                            // superseded the write. Never retain a stale jump.
                            self.scm.pending_transfer = None;
                        }
                    }
                }
                if let Some(resume) = &self.tools.resume_diff {
                    let column = if resume.side == DiffSide::Unstaged {
                        StagingColumn::Unstaged
                    } else {
                        StagingColumn::Staged
                    };
                    let path = resume.path.clone();
                    expand_ancestors_for_path(self.scm_expanded_mut(column), &path);
                    let rows = self.scm_rows();
                    let (u, s) = self.scm_slices();
                    if let Some(i) = rows.iter().position(|r| {
                        scm_entry(r, u, s)
                            .is_some_and(|e| e.path == path && e.side == column.side())
                    }) {
                        self.scm.scm_selected = i;
                    } else {
                        self.tools.resume_diff = None;
                    }
                }
                self.clamp_scm_selection();
                if self.in_git_sidebar() && self.selected_scm_entry().is_some() {
                    self.request_diff_for_scm_selection();
                } else if self.in_git_sidebar() {
                    self.preview_state.preview_mode = PreviewMode::Diff;
                    self.clear_diff_view();
                    self.preview_state.preview_loading = false;
                }
                if self.files.pending_lists.is_empty() && !self.in_git_sidebar() {
                    self.session.status = self.hint_bar();
                }
            }
            Ok(Ok(None)) => {
                self.scm.pending_transfer = None;
                if key == self.files.tree_root
                    || self
                        .scm
                        .git_status
                        .as_ref()
                        .is_some_and(|s| !self.files.tree_root.starts_with(&s.repo_root))
                {
                    self.scm.git_status = None;
                    self.scm.branch.clear();
                }
                self.clamp_scm_selection();
                if self.files.pending_lists.is_empty() {
                    self.session.status = self.hint_bar();
                }
            }
            Ok(Err(err)) => {
                self.session.status = format!("git status error: {err}");
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn apply_preview_result(
        &mut self,
        path: PathBuf,
        result: Result<Result<Preview, FsError>, JobError>,
    ) {
        let current = self.selected_entry();
        let refresh_anchor_matches = self
            .files
            .external_anchor
            .as_ref()
            .is_some_and(|(selected, _)| selected == &path);
        if !refresh_anchor_matches
            && !current.is_some_and(|entry| {
                entry.path == path || matches!(entry.kind, EntryKind::Directory)
            })
        {
            return;
        }
        if !matches!(self.preview_state.preview_mode, PreviewMode::File) {
            return;
        }

        match result {
            Ok(Ok(preview)) => {
                let same_file = self
                    .preview_state
                    .preview
                    .as_ref()
                    .is_some_and(|old| old.path == path);
                self.preview_state.selection.clear();
                self.preview_state.markdown = MarkdownPreview::default();
                self.preview_state.text_rows.clear();
                self.preview_state.pending_source_line = None;
                self.preview_state.preview = Some(preview);
                self.preview_state.preview_loading = false;
                if same_file
                    && self.preview_state.find.open
                    && !self.preview_state.find.query.is_empty()
                {
                    self.start_file_find();
                }
                if !same_file {
                    self.preview_state.preview_scroll = 0;
                }
                if self.tools.reveal.as_ref().is_some_and(|(p, _)| p == &path) {
                    if let Some((_, line)) = self.tools.reveal.take() {
                        if self.markdown_available() {
                            self.preview_state.markdown_code = true;
                        }
                        self.preview_state.preview_scroll = line.saturating_sub(1);
                        self.preview_state.pending_source_line = Some(line);
                        self.session.focus =
                            self.tools.return_focus.take().unwrap_or(PaneFocus::Preview);
                    }
                }
            }
            Ok(Err(err)) => {
                self.preview_state.selection.clear();
                self.preview_state.text_rows.clear();
                self.preview_state.pending_source_line = None;
                self.preview_state.preview = Some(Preview::error(path, err.to_string()));
                self.preview_state.preview_loading = false;
                self.preview_state.preview_scroll = 0;
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn dismiss_menu(&mut self) {
        self.session.context_menu = None;
    }

    fn toggle_scm_sidebar(&mut self) {
        if self.in_git_sidebar() {
            self.leave_git_sidebar();
        } else {
            self.enter_git_sidebar();
        }
    }

    fn enter_git_sidebar(&mut self) {
        self.close_file_find();
        self.tools.marks.clear();
        if !self.in_git_sidebar() {
            self.files_preview = Some((
                self.preview_state.preview.clone(),
                self.preview_state.preview_scroll,
                self.preview_state.markdown_code,
            ));
            self.services.jobs.cancel_key("fs:preview");
            self.preview_state.pending_preview = None;
            self.preview_state.preview_loading = false;
        }
        self.session.activity = ActivityTab::Git;
        self.preview_state.selection.clear();
        self.preview_state.preview_mode = PreviewMode::Diff;
        self.session.focus = PaneFocus::List;
        self.clamp_scm_selection();
        // Prefer first file: expand its ancestors so the change row is visible.
        if self.scm.scm_expanded_unstaged.is_empty() && self.scm.scm_expanded_staged.is_empty() {
            let (u, s) = self.scm_slices();
            let first_u = u.first().map(|e| e.path.clone());
            let first_s = s.first().map(|e| e.path.clone());
            if let Some(path) = first_u {
                expand_ancestors_for_path(&mut self.scm.scm_expanded_unstaged, &path);
            } else if let Some(path) = first_s {
                expand_ancestors_for_path(&mut self.scm.scm_expanded_staged, &path);
            }
        }
        let rows = self.scm_rows();
        if let Some(i) = rows
            .iter()
            .position(|r| r.is_change())
            .filter(|_| self.scm.scm_selected == 0)
        {
            self.scm.scm_selected = i;
        }
        self.request_diff_for_scm_selection();
        if self.history.expanded {
            self.request_history();
        }
        self.session.status = "source control".into();
    }

    fn leave_git_sidebar(&mut self) {
        self.tools.marks.clear();
        self.session.activity = ActivityTab::Files;
        self.session.focus = PaneFocus::List;
        self.services.jobs.cancel_key("git:diff");
        self.preview_state.pending_diff = None;
        self.preview_state.preview_mode = PreviewMode::File;
        self.preview_state.preview_loading = false;
        self.clear_diff_view();
        self.preview_state.markdown = MarkdownPreview::default();
        if let Some((preview, scroll, markdown_code)) = self.files_preview.take() {
            self.preview_state.preview = preview;
            self.preview_state.preview_scroll = scroll;
            self.preview_state.markdown_code = markdown_code;
        }
        // A directory does not trigger a preview. For a selected file, a
        // cancelled or missing preview may be requested again as needed.
        if self
            .selected_entry()
            .is_some_and(|entry| !matches!(entry.kind, EntryKind::Directory))
            && self.preview_state.preview.is_none()
        {
            self.request_preview_for_selection();
        }
        self.session.status = self.hint_bar();
    }

    fn expand_staging(&mut self) {
        // Compatibility alias: open Git sidebar.
        if !self.in_git_sidebar() {
            self.enter_git_sidebar();
        } else {
            self.session.focus = PaneFocus::List;
            self.clamp_scm_selection();
        }
    }

    fn open_file_context_menu(&mut self, index: usize, col: u16, row: u16) {
        let Some(row_ref) = self.files.visible.get(index) else {
            return;
        };
        let entry = &row_ref.entry;
        let path = entry.path.clone();
        self.files.selected = index;
        let is_dir = entry.kind == EntryKind::Directory;
        let mut items = file_list_items(is_dir);
        if self.has_discardable_changes_under(&path) {
            items.insert(
                0,
                forge_ui::ContextMenuItem {
                    action: ContextAction::DiscardChanges,
                    enabled: true,
                },
            );
        }
        self.session.context_menu =
            ContextMenu::new(items, col, row, ContextTarget::FileList { path });
        self.session.focus = PaneFocus::List;
    }

    fn open_staging_context_menu(
        &mut self,
        column: StagingColumn,
        index: usize,
        col: u16,
        row: u16,
    ) {
        self.expand_staging();
        // Map column+index into flat SCM selection.
        let rows = self.scm_rows();
        if let Some(flat) = rows.iter().position(|r| {
            matches!(r, ScmRow::Change { column: c, index: i, .. } if *c == column && *i == index)
        }) {
            self.scm.scm_selected = flat;
        }
        self.session.focus = PaneFocus::List;
        let has_staged = self
            .scm
            .git_status
            .as_ref()
            .map(|s| !s.staged.is_empty())
            .unwrap_or(false);
        let in_repo = self.scm.git_status.is_some();
        let mut items = staging_items(column, has_staged, in_repo);
        let can_open = self
            .selected_scm_entry()
            .and_then(|entry| self.repo_root().map(|root| root.join(&entry.path)))
            .is_some_and(|path| path.is_file());
        for item in &mut items {
            if item.action == ContextAction::OpenFile {
                item.enabled = can_open;
            }
        }
        self.update_stage_menu(&mut items);
        self.session.context_menu =
            ContextMenu::new(items, col, row, ContextTarget::Staging { column, index });
    }

    fn open_staging_folder_menu(&mut self, flat: usize, col: u16, row: u16) {
        let Some(ScmRow::Folder { column, path, .. }) = self.scm_rows().get(flat).cloned() else {
            return;
        };
        self.scm.scm_selected = flat;
        self.session.focus = PaneFocus::List;
        self.request_diff_for_scm_selection();
        let has_staged = self
            .scm
            .git_status
            .as_ref()
            .is_some_and(|s| !s.staged.is_empty());
        let mut items = staging_items(column, has_staged, self.scm.git_status.is_some());
        for item in &mut items {
            item.action = match item.action {
                ContextAction::Stage => ContextAction::StageFolder,
                ContextAction::Unstage => ContextAction::UnstageFolder,
                ContextAction::Diff => ContextAction::Open,
                action => action,
            };
        }
        items.retain(|item| item.action != ContextAction::OpenFile);
        let can_discard = self.repo_root().is_some_and(|root|
            self.has_discardable_changes_under(&root.join(&path))
        );
        if !can_discard {
            items.retain(|item| item.action != ContextAction::DiscardChanges);
        }
        self.update_stage_menu(&mut items);
        self.session.context_menu = ContextMenu::new(
            items,
            col,
            row,
            ContextTarget::StagingFolder { column, path },
        );
    }

    fn open_staging_section_menu(&mut self, flat: usize, col: u16, row: u16) {
        let Some(ScmRow::Section { column, .. }) = self.scm_rows().get(flat).cloned() else {
            return;
        };
        let Some(status) = self.scm.git_status.as_ref() else {
            return;
        };
        let has_unstaged = !status.unstaged.is_empty();
        let has_staged = !status.staged.is_empty();
        let conflicted = status
            .unstaged
            .iter()
            .any(|e| e.status == forge_git::GitStatus::Conflicted);
        let discardable = status.unstaged.iter().any(|e| {
            !matches!(
                e.status,
                forge_git::GitStatus::Untracked | forge_git::GitStatus::Conflicted
            )
        });
        use forge_ui::ContextMenuItem;
        let items = match column {
            StagingColumn::Unstaged => vec![
                ContextMenuItem {
                    action: ContextAction::DiscardAllChanges,
                    enabled: discardable,
                },
                ContextMenuItem {
                    action: ContextAction::StageAllChanges,
                    enabled: has_unstaged,
                },
                ContextMenuItem {
                    action: ContextAction::StashUnstagedChanges,
                    enabled: has_unstaged && !conflicted,
                },
                ContextMenuItem {
                    action: ContextAction::StashAllChanges,
                    enabled: (has_unstaged || has_staged) && !conflicted,
                },
            ],
            StagingColumn::Staged => vec![ContextMenuItem {
                action: ContextAction::UnstageAllChanges,
                enabled: has_staged,
            }],
        };
        self.scm.scm_selected = flat;
        self.session.focus = PaneFocus::List;
        self.session.context_menu =
            ContextMenu::new(items, col, row, ContextTarget::StagingSection { column });
    }

    fn request_stage_all_changes(&mut self, stage: bool) {
        if self.write_busy() {
            return;
        }
        let Some(status) = self.scm.git_status.as_ref() else {
            self.session.status = "Refresh Git status first".into();
            return;
        };
        if (if stage {
            &status.unstaged
        } else {
            &status.staged
        })
        .is_empty()
        {
            self.session.status = "No changes in this Git section".into();
            return;
        }
        let repo = status.repo_root.clone();
        let git = self.services.git;
        // The Git backend sets cwd to the repository root. A single literal
        // "." pathspec covers the whole worktree including deletions/renames.
        let paths = vec![std::path::PathBuf::from(".")];
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    let result = if stage {
                        git.stage_paths(&repo, &paths).await
                    } else {
                        git.unstage_paths(&repo, &paths).await
                    };
                    result.map(|_| None)
                });
        self.scm.pending_transfer = None;
        self.session.status = if stage {
            "staging all changes..."
        } else {
            "unstaging all changes..."
        }
        .into();
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: if stage {
                WriteOp::Stage
            } else {
                WriteOp::Unstage
            },
            target: "all repository changes".into(),
            handle,
        });
    }

    fn file_stage_flags(&self, abs: &Path) -> (bool, bool) {
        let Some(snap) = &self.scm.git_status else {
            return (false, false);
        };
        let Ok(rel) = abs.strip_prefix(&snap.repo_root) else {
            return (false, false);
        };
        let rela = forge_model::path_to_rela(rel);
        let can_stage = snap.unstaged.iter().any(|c| c.path == rela);
        let can_unstage = snap.staged.iter().any(|c| c.path == rela);
        // Untracked files are in unstaged list — stageable. Also allow staging clean? no.
        // If file has any unstaged-ish status including untracked via status_for:
        let status = snap.status_for(abs);
        let can_stage = can_stage
            || matches!(
                status,
                forge_model::GitStatus::Modified
                    | forge_model::GitStatus::Deleted
                    | forge_model::GitStatus::Untracked
                    | forge_model::GitStatus::Added
                    | forge_model::GitStatus::Renamed
            ) && !can_unstage;
        (can_stage, can_unstage)
    }

    fn apply_context_action(&mut self, action: ContextAction) {
        let Some(menu) = self.session.context_menu.clone() else {
            return;
        };
        self.dismiss_menu();
        match menu.target {
            ContextTarget::FileList { path } => {
                let Some(index) = self.files.visible.iter().position(|r| r.entry.path == path)
                else {
                    self.session.status =
                        "Selection changed; select the file or folder again".into();
                    return;
                };
                self.files.selected = index;
                let marks = std::mem::take(&mut self.tools.marks);
                match action {
                    ContextAction::NewFile => self.run_command(tools::Command::NewFile),
                    ContextAction::NewDirectory => self.run_command(tools::Command::NewDirectory),
                    ContextAction::Rename => self.run_command(tools::Command::Rename),
                    ContextAction::Copy => self.run_command(tools::Command::Copy),
                    ContextAction::Move => self.run_command(tools::Command::Move),
                    ContextAction::DeleteFile | ContextAction::DeleteDirectory => {
                        self.confirm_delete_paths(vec![path])
                    }
                    ContextAction::EditText => self.open_text_editor(path, 1),
                    ContextAction::ExternalEditor => self.open_editor(Some(path), 1),
                    ContextAction::Open => self.open_selected(),
                    ContextAction::DiscardChanges => self.discard_scope(vec![path]),
                    ContextAction::Preview => {
                        self.request_preview_for_selection();
                        self.session.focus = PaneFocus::Preview;
                    }
                    ContextAction::CopyPath => self.copy_selected_path(),
                    _ => {}
                }
                self.tools.marks = marks;
            }
            ContextTarget::FileRoot { path } => {
                if path != self.files.tree_root {
                    self.session.status = "Workspace changed; open the menu again".into();
                    return;
                }
                match action {
                    ContextAction::NewFile | ContextAction::NewDirectory => {
                        self.new_item_in(&path, action == ContextAction::NewDirectory)
                    }
                    _ => {}
                }
            }
            ContextTarget::Staging { column, index } => {
                self.expand_staging();
                let rows = self.scm_rows();
                let Some(flat) = rows.iter().position(|r| {
                    matches!(r, ScmRow::Change { column: c, index: i, .. } if *c == column && *i == index)
                }) else {
                    self.session.status = "Git changes have changed; open the menu again".into();
                    return;
                };
                self.scm.scm_selected = flat;
                self.session.focus = PaneFocus::List;
                match action {
                    ContextAction::Stage => self.request_write_stage(true),
                    ContextAction::Unstage => self.request_write_stage(false),
                    ContextAction::Diff => {
                        self.request_diff_for_scm_selection();
                        self.session.focus = PaneFocus::Preview;
                    }
                    ContextAction::OpenFile => self.open_selected_worktree_file(),
                    ContextAction::DiscardChanges => {
                        let path = self.selected_scm_entry().map(|e| e.path.clone());
                        if let (Some(root), Some(path)) = (self.repo_root(), path) {
                            self.discard_scope(vec![root.join(path)]);
                        }
                    }
                    ContextAction::Commit => self.open_commit(),
                    ContextAction::CopyPath => self.copy_staging_path(),
                    ContextAction::Push => self.request_push(),
                    ContextAction::Pull => self.request_pull(),
                    _ => {}
                }
            }
            ContextTarget::StagingFolder { column, path } => {
                let rows = self.scm_rows();
                let Some(flat) = rows.iter().position(|r| matches!(r, ScmRow::Folder { column: c, path: p, .. } if *c == column && *p == path)) else {
                    self.session.status = "Folder changes have changed; select the folder again".into();
                    return;
                };
                self.scm.scm_selected = flat;
                self.session.focus = PaneFocus::List;
                match action {
                    ContextAction::StageFolder => self.request_write_stage(true),
                    ContextAction::UnstageFolder => self.request_write_stage(false),
                    ContextAction::Open => self.toggle_scm_expand_at(flat),
                    ContextAction::DiscardChanges => {
                        if let Some(root) = self.repo_root() {
                            self.discard_scope(vec![root.join(&path)]);
                        }
                    }
                    ContextAction::CopyPath => self.copy_staging_path(),
                    ContextAction::Commit => self.open_commit(),
                    ContextAction::Push => self.request_push(),
                    ContextAction::Pull => self.request_pull(),
                    _ => {}
                }
            }
            ContextTarget::StagingSection { column } => {
                if self.scm.git_status.is_none() {
                    self.session.status = "Refresh Git status first".into();
                    return;
                }
                match (column, action) {
                    (StagingColumn::Unstaged, ContextAction::DiscardAllChanges) => {
                        if let Some(root) = self.repo_root() {
                            // Protect untracked/conflicted files and the index, like
                            // the existing scoped Discard action.
                            self.discard_scope(vec![root]);
                        }
                    }
                    (StagingColumn::Unstaged, ContextAction::StageAllChanges) => {
                        self.request_stage_all_changes(true)
                    }
                    (StagingColumn::Staged, ContextAction::UnstageAllChanges) => {
                        self.request_stage_all_changes(false)
                    }
                    (StagingColumn::Unstaged, ContextAction::StashUnstagedChanges) => {
                        self.prompt(tools::PromptAction::StashUnstaged, "Stash Unstaged Changes", String::new(),
                            "Message · staged/index changes will be preserved · confirmation follows");
                    }
                    (StagingColumn::Unstaged, ContextAction::StashAllChanges) => {
                        self.prompt(tools::PromptAction::Stash, "Stash All Changes", String::new(),
                            "Message · includes staged, unstaged and untracked files · confirmation follows");
                    }
                    _ => {}
                }
            }
            ContextTarget::Diff => {
                self.session.focus = PaneFocus::Preview;
                match action {
                    ContextAction::StageHunk => self.request_hunk_write(true),
                    ContextAction::UnstageHunk => self.request_hunk_write(false),
                    ContextAction::OpenFile => self.open_selected_worktree_file(),
                    ContextAction::CopyPath => self.copy_diff_path(),
                    _ => {}
                }
            }
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        match copy_path_clipboard(text) {
            Ok(()) => self.session.status = format!("copied - {text}"),
            Err(err) => self.session.status = format!("clipboard unavailable ({err}) - {text}"),
        }
    }

    fn copy_selected_path(&mut self) {
        if let Some(entry) = self.selected_entry() {
            let s = entry.path.display().to_string();
            self.copy_to_clipboard(&s);
        }
    }

    fn copy_staging_path(&mut self) {
        let path = match self.selected_scm_row() {
            Some(ScmRow::Folder { path, .. }) => Some(path),
            _ => self.selected_scm_entry().map(|entry| entry.path.clone()),
        };
        if let Some(path) = path {
            let s = if let Some(repo) = self.repo_root() {
                repo.join(&path).display().to_string()
            } else {
                path
            };
            self.copy_to_clipboard(&s);
        }
    }

    fn copy_diff_path(&mut self) {
        if let Some(diff) = self.preview_state.file_diff.as_ref() {
            let s = diff.repo_root.join(&diff.path).display().to_string();
            self.copy_to_clipboard(&s);
        }
    }

    fn current_hunks(&self) -> Vec<DiffHunk> {
        self.preview_state
            .file_diff
            .as_ref()
            .map(|d| d.hunks())
            .unwrap_or_default()
    }

    fn ensure_hunk_visible(&mut self) {
        let Some(idx) = self.preview_state.hunk_index else {
            return;
        };
        let Some(diff) = self.preview_state.file_diff.take() else {
            return;
        };
        let hunks = diff.hunks();
        let Some(hunk) = hunks.get(idx).cloned() else {
            self.preview_state.file_diff = Some(diff);
            return;
        };
        // Diff body starts at line 0 (title chip is outside the scroll area).
        // Map unified hunk index → split-row index (meta lines collapsed).
        let rendered_start =
            diff_render_hunk_start(&diff.lines, hunk.start, &mut self.preview_state.split_cache);
        let total = if diff.lines.is_empty() {
            1
        } else {
            self.preview_state.split_cache.ensure(&diff.lines).max(1)
        };
        self.preview_state.diff_row_count = total;
        let viewport = self.session.layout.preview_inner.height as usize;
        scroll_to_hunk(
            &mut self.preview_state.preview_scroll,
            viewport,
            total,
            rendered_start,
        );
        self.preview_state.file_diff = Some(diff);
    }

    fn move_hunk(&mut self, delta: isize) {
        let hunks = self.current_hunks();
        if hunks.is_empty() {
            self.session.status = "no hunks in this diff".into();
            return;
        }
        let cur = self.preview_state.hunk_index.unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(hunks.len() as isize) as usize;
        self.preview_state.hunk_index = Some(next);
        self.ensure_hunk_visible();
        self.session.focus = PaneFocus::Preview;
        self.session.status = format!("hunk {}/{}", next + 1, hunks.len());
    }

    fn request_hunk_write(&mut self, stage: bool) {
        let target = match self.stage_target(if stage {
            ContextAction::StageHunk
        } else {
            ContextAction::UnstageHunk
        }) {
            Ok(target) => target,
            Err(reason) => {
                self.session.status = reason.into();
                return;
            }
        };
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };
        let Some(diff) = self.preview_state.file_diff.clone() else {
            self.session.status = "open a diff first (d)".into();
            return;
        };
        let hunks = diff.hunks();
        let Some(idx) = target.hunk else {
            self.session.status = "no hunk selected (n/N)".into();
            return;
        };
        let Some(hunk) = hunks.get(idx).cloned() else {
            self.session.status = "hunk out of range".into();
            return;
        };
        if stage && !matches!(diff.side, DiffSide::Unstaged) {
            self.session.status = "switch to unstaged diff to stage hunk".into();
            return;
        }
        if !stage && !matches!(diff.side, DiffSide::Staged) {
            self.session.status = "switch to staged diff to unstage hunk".into();
            return;
        }
        let git = self.services.git;
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    let r = if stage {
                        git.stage_hunk(&repo, &diff, &hunk).await
                    } else {
                        git.unstage_hunk(&repo, &diff, &hunk).await
                    };
                    r.map(|_| None)
                });
        self.session.status = if stage {
            format!("staging hunk {}/{}...", idx + 1, hunks.len())
        } else {
            format!("unstaging hunk {}/{}...", idx + 1, hunks.len())
        };
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: if stage {
                WriteOp::StageHunk
            } else {
                WriteOp::UnstageHunk
            },
            target: format!(
                "{} · hunk {}/{}",
                target.path.display(),
                idx + 1,
                hunks.len()
            ),
            handle,
        });
    }

    /// Only a status line for the exact current branch can establish that
    /// it has no upstream. A stale/empty status cannot authorize publishing.
    fn unpublished_branch(&self) -> Option<String> {
        let branch = self.scm.branch.trim();
        if branch.is_empty() || branch == "HEAD" {
            return None;
        }
        let status = self.tools.sync.lines().next()?.trim();
        let status = status.strip_prefix("## ").unwrap_or(status);
        (status == branch).then(|| branch.to_owned())
    }

    fn request_push(&mut self) {
        if self.write_busy() {
            return;
        }
        if let Some(branch) = self.unpublished_branch() {
            self.open_tools(tools::PanelKind::Git(forge_git::GitQuery::PublishRemotes(
                branch,
            )));
            return;
        }
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };
        let git = self.services.git;
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    git.push(&repo).await.map(Some)
                });
        self.session.status = "pushing...".into();
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Push,
            target: "repository".into(),
            handle,
        });
    }

    fn request_pull(&mut self) {
        if self.write_busy() {
            return;
        }
        let Some(repo) = self.repo_root() else {
            self.session.status = "not a git repository".into();
            return;
        };
        let git = self.services.git;
        let handle =
            self.services
                .jobs
                .spawn(JobPriority::High, Some("git:write".into()), async move {
                    git.pull(&repo).await.map(Some)
                });
        self.session.status = "pulling...".into();
        self.session.write_status = Some(self.session.status.clone());
        self.scm.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Pull,
            target: "repository".into(),
            handle,
        });
    }

    fn open_diff_context_menu(&mut self, col: u16, row: u16) {
        if !self.in_git_sidebar() {
            return;
        }
        let Some(diff) = self.preview_state.file_diff.as_ref() else {
            return;
        };
        let has_hunk = self.preview_state.hunk_index.is_some();
        let mut items = diff_items(matches!(diff.side, DiffSide::Unstaged), has_hunk);
        let can_open =
            diff.repo_root.join(&diff.path).is_file() && self.diff_matches_selection(diff);
        for item in &mut items {
            if item.action == ContextAction::OpenFile {
                item.enabled = can_open;
            }
        }
        self.update_stage_menu(&mut items);
        self.session.context_menu = ContextMenu::new(items, col, row, ContextTarget::Diff);
        self.session.focus = PaneFocus::Preview;
    }


}

#[allow(dead_code)]
fn path_is_under(child: &Path, root: &Path) -> bool {
    child.starts_with(root)
}

/// Copy `text` to the system clipboard. Falls back with an error on headless hosts.
fn copy_path_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

//! Application orchestration: state machine, modes, and event loop.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use forge_config::Config;
use forge_theme::{detect_color_mode, ColorMode};
use forge_fs::{Entry, EntryKind, FsError, FsService, Preview, PreviewContent};
use forge_git::{
    DiffHunk, DiffSide, FileDiff, GitReadError, GitService, GitWriteError, StatusSnapshot,
};
use forge_jobs::{JobError, JobHandle, JobPriority, JobScheduler};
use forge_model::JobId;
use forge_ui::{
    build_scm_rows, diff_items, diff_render_hunk_start, file_list_items, find_parent_row,
    flatten_tree, init_icons_from_env, list_index_at, preview_line_count, scm_entry, scm_index_at,
    scm_jump_to_column, scroll_to_hunk, staging_items, set_color_mode, ActivityTab, ContextAction,
    ContextMenu, ContextTarget, FileListState, IconMode, PaneFocus, PaneLayout, PreviewView, ScmRow,
    ShellState, StagingColumn, StagingState, TextLang, TreeRow, UiShell,
};
use forge_theme::Theme;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Double-click window for directory enter.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

struct PendingList {
    #[allow(dead_code)]
    id: JobId,
    path: PathBuf,
    handle: JobHandle<Result<Vec<Entry>, FsError>>,
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

struct PendingDiff {
    #[allow(dead_code)]
    id: JobId,
    path: String,
    side: DiffSide,
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
    handle: JobHandle<Result<Option<String>, GitWriteError>>,
}

/// What the preview pane is showing.
enum PreviewMode {
    File,
    Diff,
}

struct App {
    /// Tree root (repo root when known, else initial cwd).
    tree_root: PathBuf,
    /// Expanded directory paths.
    expanded: HashSet<PathBuf>,
    /// Cached children keyed by directory path.
    children: HashMap<PathBuf, Vec<Entry>>,
    /// Depth-first visible rows derived from tree_root/children/expanded.
    visible: Vec<TreeRow>,
    selected: usize,
    scroll: usize,
    preview_scroll: usize,
    restore_select: Option<String>,
    status: String,
    pending_lists: Vec<PendingList>,
    pending_status: Option<PendingStatus>,
    pending_preview: Option<PendingPreview>,
    pending_diff: Option<PendingDiff>,
    pending_write: Option<PendingWrite>,
    git_status: Option<StatusSnapshot>,
    branch: String,
    activity: ActivityTab,
    preview: Option<Preview>,
    preview_loading: bool,
    preview_mode: PreviewMode,
    file_diff: Option<FileDiff>,
    /// Active hunk index inside `file_diff` (None when no hunks / not in diff).
    hunk_index: Option<usize>,
    /// Flat selection index into the SCM sidebar (`build_scm_rows`).
    scm_selected: usize,
    scroll_scm: usize,
    /// `Some` means commit modal is open with the current buffer.
    commit_input: Option<String>,
    /// Floating right-click menu.
    context_menu: Option<ContextMenu>,
    should_quit: bool,
    jobs: JobScheduler,
    fs: FsService,
    git: GitService,
    ui: UiShell,
    layout: PaneLayout,
    focus: PaneFocus,
    last_list_click: Option<(Instant, usize)>,
}

/// Restores terminal modes even if the event loop panics / returns early.

/// Read the current branch name from `.git/HEAD` (best-effort, sync).
fn read_branch_name(repo_root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(repo_root.join(".git/HEAD")).ok()?;
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

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
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
    let icons = match forge_ui::icon_mode() {
        IconMode::Nerd => "nerd",
        IconMode::Ascii => "ascii",
    };
    eprintln!(
        "forge: palette={}  icons={icons}  TERM={term:?}  COLORTERM={colorterm:?}  (flags: --force-ansi16|--force-ansi256|--force-truecolor|--demo-colors | FORGE_COLOR=16|256|truecolor | FORGE_ICONS=nerd|ascii)",
        mode.as_str()
    );

    if opts.demo_colors {
        return demo_colors(mode);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    rt.block_on(run_async(mode))
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

async fn run_async(color_mode: ColorMode) -> Result<()> {
    let _config = Config::load_default();

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let git = GitService::new();
    let tree_root = git.find_repo_root(&cwd).unwrap_or_else(|| cwd.clone());

    let mut app = App {
        tree_root,
        expanded: HashSet::new(),
        children: HashMap::new(),
        visible: Vec::new(),
        selected: 0,
        scroll: 0,
        preview_scroll: 0,
        restore_select: None,
        status: format!("modern-ide | palette:{}", color_mode.as_str()),
        pending_lists: Vec::new(),
        pending_status: None,
        pending_preview: None,
        pending_diff: None,
        pending_write: None,
        git_status: None,
        branch: String::new(),
        activity: ActivityTab::Files,
        preview: None,
        preview_loading: false,
        preview_mode: PreviewMode::File,
        file_diff: None,
        hunk_index: None,
        scm_selected: 0,
        scroll_scm: 0,
        commit_input: None,
        context_menu: None,
        should_quit: false,
        jobs: JobScheduler::new(),
        fs: FsService::new(),
        git,
        ui: UiShell::new(if matches!(color_mode, ColorMode::Ansi16) {
            Theme::modern_ide_vga()
        } else {
            Theme::modern_ide()
        }),
        layout: PaneLayout::default(),
        focus: PaneFocus::List,
        last_list_click: None,
    };
    app.request_list();
    app.request_status();

    let mut stdout = io::stdout();
    enable_raw_mode().context("enable raw mode")?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen + mouse")?;
    let _guard = TerminalGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;

    let result = event_loop(&mut terminal, &mut app).await;

    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    let _ = terminal.show_cursor();
    std::mem::forget(_guard);

    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| {
            let cwd = app.tree_root.display().to_string();
            let git_label = match &app.git_status {
                Some(snap) => {
                    // Quiet header cue; detailed counts live in the bottom strip.
                    let root = snap
                        .repo_root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "repo".into());
                    format!("- {root}")
                }
                None => String::new(),
            };
            let in_repo = app.git_status.is_some();

            let preview_title;
            let preview_view;
            match app.preview_mode {
                PreviewMode::Diff => {
                    if let Some(diff) = app.file_diff.as_ref() {
                        // Basename for file-type label; chip draws muted side + cyan name.
                        preview_title = diff
                            .path
                            .rsplit('/')
                            .next()
                            .unwrap_or(diff.path.as_str())
                            .to_string();
                        let hunks = diff.hunks();
                        let (hunk_start, hunk_end, hunk_idx) = match app.hunk_index {
                            Some(i) if i < hunks.len() => (
                                Some(hunks[i].start),
                                Some(hunks[i].end),
                                Some((i, hunks.len())),
                            ),
                            _ if !hunks.is_empty() => (None, None, Some((0, hunks.len()))),
                            _ => (None, None, None),
                        };
                        preview_view = PreviewView::Diff {
                            path: &diff.path,
                            side: diff.side.label(),
                            lines: &diff.lines,
                            hunk_start,
                            hunk_end,
                            hunk_idx,
                        };
                    } else if app.preview_loading {
                        preview_title = "diff".into();
                        preview_view = PreviewView::Loading;
                    } else {
                        preview_title = String::new();
                        let clean = app
                            .git_status
                            .as_ref()
                            .is_some_and(|s| s.unstaged.is_empty() && s.staged.is_empty());
                        preview_view = PreviewView::Hint {
                            text: if clean {
                                "Working tree clean"
                            } else {
                                "No changes selected"
                            },
                        };
                    }
                }
                PreviewMode::File => {
                    if app.preview_loading {
                        preview_title = app
                            .visible
                            .get(app.selected)
                            .map(|r| {
                                r.entry
                                    .path
                                    .strip_prefix(&app.tree_root)
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|_| r.entry.name.clone())
                            })
                            .unwrap_or_default();
                        preview_view = PreviewView::Loading;
                    } else if let Some(preview) = app.preview.as_ref() {
                        // Relative path so the preview title can show muted dirname + basename chip.
                        preview_title = preview
                            .path
                            .strip_prefix(&app.tree_root)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| {
                                preview
                                    .path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| preview.path.display().to_string())
                            });
                        preview_view = match &preview.content {
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
                    } else {
                        preview_title = String::new();
                        preview_view = PreviewView::Idle;
                    }
                }
            }

            let (unstaged, staged) = match &app.git_status {
                Some(snap) => (snap.unstaged.as_slice(), snap.staged.as_slice()),
                None => (&[][..], &[][..]),
            };

            let focus = app.focus;
            let commit_ref = app.commit_input.as_deref();
            let branch = app.branch.as_str();
            let activity = app.activity;
            let mut shell = ShellState {
                cwd: &cwd,
                git_label: &git_label,
                branch,
                status: &app.status,
                list: FileListState {
                    rows: &app.visible,
                    selected: app.selected,
                    scroll: &mut app.scroll,
                    git: app.git_status.as_ref(),
                    focused: false,
                },
                preview_title: &preview_title,
                preview: preview_view,
                preview_scroll: &mut app.preview_scroll,
                staging: StagingState {
                    unstaged,
                    staged,
                    selected: app.scm_selected,
                    scroll: &mut app.scroll_scm,
                    focused: false,
                },
                focus,
                commit_input: commit_ref,
                context_menu: app.context_menu.as_mut(),
                in_repo,
                activity,
            };
            app.layout = app.ui.draw(frame, &mut shell);
        })?;

        app.poll_pending_list();
        app.poll_pending_status();
        app.poll_pending_preview();
        app.poll_pending_diff();
        app.poll_pending_write();

        if event::poll(Duration::from_millis(16)).context("poll events")? {
            match event::read().context("read event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                }
                Event::Mouse(mouse) => {
                    if app.commit_input.is_none() {
                        app.handle_mouse(mouse);
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        tokio::task::yield_now().await;
    }
    Ok(())
}

impl App {
    fn hint_bar(&self) -> String {
        String::new()
    }

    fn repo_root(&self) -> Option<PathBuf> {
        self.git_status
            .as_ref()
            .map(|s| s.repo_root.clone())
            .or_else(|| self.git.find_repo_root(&self.tree_root))
    }

    fn scm_slices(&self) -> (&[forge_git::ChangeEntry], &[forge_git::ChangeEntry]) {
        match &self.git_status {
            Some(snap) => (snap.unstaged.as_slice(), snap.staged.as_slice()),
            None => (&[], &[]),
        }
    }

    fn scm_rows(&self) -> Vec<ScmRow> {
        let (u, s) = self.scm_slices();
        build_scm_rows(u, s)
    }

    fn selected_scm_entry(&self) -> Option<&forge_git::ChangeEntry> {
        let rows = self.scm_rows();
        let row = *rows.get(self.scm_selected)?;
        let (u, s) = self.scm_slices();
        scm_entry(row, u, s)
    }

    fn selected_scm_column(&self) -> Option<StagingColumn> {
        let rows = self.scm_rows();
        rows.get(self.scm_selected).map(|r| r.column())
    }

    fn clamp_scm_selection(&mut self) {
        let len = self.scm_rows().len();
        if len == 0 {
            self.scm_selected = 0;
        } else if self.scm_selected >= len {
            self.scm_selected = len - 1;
        }
    }

    fn in_git_sidebar(&self) -> bool {
        matches!(self.activity, ActivityTab::Git)
    }

    fn sidebar_focused(&self) -> bool {
        matches!(self.focus, PaneFocus::List | PaneFocus::Staging)
    }

    fn request_list(&mut self) {
        // Full refresh: keep expand set, reload cached children from disk.
        self.children.clear();
        self.rebuild_visible();
        self.pending_lists.clear();
        self.preview = None;
        self.preview_loading = false;
        self.preview_scroll = 0;
        self.preview_mode = PreviewMode::File;
        self.file_diff = None;
        self.hunk_index = None;
        self.pending_preview = None;
        self.pending_diff = None;
        self.jobs.cancel_key("fs:preview");
        self.jobs.cancel_key("git:diff");
        self.request_dir(self.tree_root.clone());
    }

    fn request_dir(&mut self, path: PathBuf) {
        let handle = self.fs.list(&self.jobs, path.clone());
        self.status = format!("listing {}...", path.display());
        self.pending_lists.push(PendingList {
            id: handle.id,
            path,
            handle,
        });
    }

    fn request_missing_expanded(&mut self) {
        let missing: Vec<PathBuf> = self
            .expanded
            .iter()
            .filter(|p| !self.children.contains_key(p.as_path()))
            .cloned()
            .collect();
        for path in missing {
            self.request_dir(path);
        }
    }

    fn rebuild_visible(&mut self) {
        self.visible = flatten_tree(&self.tree_root, &self.children, &self.expanded);
        if self.visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if self.selected >= self.visible.len() {
            self.selected = self.visible.len() - 1;
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.visible.get(self.selected).map(|r| &r.entry)
    }

    fn request_status(&mut self) {
        let start = self.tree_root.clone();
        let job_key = format!("git:status:{}", start.display());
        let git = self.git;
        let start_for_job = start.clone();
        let handle = self.jobs.spawn(
            JobPriority::Normal,
            Some(job_key),
            async move {
                match tokio::task::spawn_blocking(move || git.status(&start_for_job)).await {
                    Ok(Ok(snap)) => Ok(Some(snap)),
                    Ok(Err(GitReadError::NotARepository(_))) => Ok(None),
                    Ok(Err(GitReadError::Discover(_))) => Ok(None),
                    Ok(Err(err)) => Err(err),
                    Err(join) => Err(GitReadError::Status(format!("status worker: {join}"))),
                }
            },
        );
        self.pending_status = Some(PendingStatus {
            id: handle.id,
            key: start,
            handle,
        });
    }

    fn request_preview_for_selection(&mut self) {
        self.preview_mode = PreviewMode::File;
        self.file_diff = None;
        self.jobs.cancel_key("git:diff");
        self.pending_diff = None;

        let Some(entry) = self.selected_entry().cloned() else {
            self.preview = None;
            self.preview_loading = false;
            self.preview_scroll = 0;
            self.pending_preview = None;
            self.jobs.cancel_key("fs:preview");
            return;
        };

        self.preview_loading = true;
        self.preview_scroll = 0;
        let path = entry.path.clone();
        let handle = self.fs.preview(&self.jobs, entry.path, entry.kind);
        self.pending_preview = Some(PendingPreview {
            id: handle.id,
            path,
            handle,
        });
    }

    fn request_diff(&mut self, rela: String, side: DiffSide) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        self.preview_mode = PreviewMode::Diff;
        self.preview_loading = true;
        self.preview_scroll = 0;
        self.file_diff = None;
        self.hunk_index = None;
        self.jobs.cancel_key("fs:preview");
        self.pending_preview = None;

        let git = self.git;
        let path_for_job = rela.clone();
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:diff".into()),
            async move { git.file_diff(&repo, &path_for_job, side).await },
        );
        self.pending_diff = Some(PendingDiff {
            id: handle.id,
            path: rela,
            side,
            handle,
        });
        // Status middle stays blank while loading; left chip shows basename.
        self.status.clear();
    }

    fn request_diff_for_scm_selection(&mut self) {
        let Some(entry) = self.selected_scm_entry().cloned() else {
            self.status = "no change selected".into();
            self.preview_mode = PreviewMode::Diff;
            self.file_diff = None;
            self.preview_loading = false;
            self.hunk_index = None;
            return;
        };
        self.request_diff(entry.path, entry.side);
    }

    fn request_write_stage(&mut self, stage: bool) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };

        // Prefer SCM sidebar selection when in Git mode; else file-list selection.
        let paths: Vec<PathBuf> = if self.in_git_sidebar() {
            match self.selected_scm_entry() {
                Some(e) => {
                    let col = self.selected_scm_column().unwrap_or(StagingColumn::Unstaged);
                    if stage && !matches!(col, StagingColumn::Unstaged) {
                        self.status = "switch to Changes to stage ([)".into();
                        return;
                    }
                    if !stage && !matches!(col, StagingColumn::Staged) {
                        self.status = "switch to Staged to unstage (])".into();
                        return;
                    }
                    vec![PathBuf::from(&e.path)]
                }
                None => {
                    self.status = "no change selected".into();
                    return;
                }
            }
        } else {
            // From file list: stage/unstage selected file path relative to repo.
            let Some(entry) = self.selected_entry() else {
                self.status = "no file selected".into();
                return;
            };
            if matches!(entry.kind, EntryKind::Directory) {
                self.status = "select a file (or use staging panel)".into();
                return;
            }
            let Ok(rel) = entry.path.strip_prefix(&repo) else {
                self.status = "file outside repository".into();
                return;
            };
            vec![rel.to_path_buf()]
        };

        let git = self.git;
        let paths_job = paths.clone();
        let op_label = if stage { "staging" } else { "unstaging" };
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:write".into()),
            async move {
                let r = if stage {
                    git.stage_paths(&repo, &paths_job).await
                } else {
                    git.unstage_paths(&repo, &paths_job).await
                };
                r.map(|_| None)
            },
        );
        self.status = format!("{op_label} {}...", paths[0].display());
        self.pending_write = Some(PendingWrite {
            id: handle.id,
            op: if stage { WriteOp::Stage } else { WriteOp::Unstage },
            handle,
        });
    }

    fn request_commit(&mut self, message: String) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        let git = self.git;
        let msg = message.clone();
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:write".into()),
            async move { git.commit(&repo, msg).await.map(|_| None) },
        );
        self.status = "committing...".into();
        self.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Commit(message),
            handle,
        });
    }

    fn poll_pending_list(&mut self) {
        let mut i = 0;
        while i < self.pending_lists.len() {
            if let Some(result) = self.pending_lists[i].handle.try_join() {
                let pending = self.pending_lists.remove(i);
                self.apply_list_result(pending.path, result);
            } else {
                i += 1;
            }
        }
    }

    fn poll_pending_status(&mut self) {
        let done = match self.pending_status.as_mut() {
            Some(pending) => pending
                .handle
                .try_join()
                .map(|result| (pending.key.clone(), result)),
            None => None,
        };
        if let Some((key, result)) = done {
            self.pending_status = None;
            self.apply_status_result(key, result);
        }
    }

    fn poll_pending_preview(&mut self) {
        let done = match self.pending_preview.as_mut() {
            Some(pending) => pending
                .handle
                .try_join()
                .map(|result| (pending.path.clone(), result)),
            None => None,
        };
        if let Some((path, result)) = done {
            self.pending_preview = None;
            self.apply_preview_result(path, result);
        }
    }

    fn poll_pending_diff(&mut self) {
        let done = match self.pending_diff.as_mut() {
            Some(pending) => pending
                .handle
                .try_join()
                .map(|result| (pending.path.clone(), pending.side, result)),
            None => None,
        };
        if let Some((_path, _side, result)) = done {
            self.pending_diff = None;
            match result {
                Ok(Ok(diff)) => {
                    let hunk_count = diff.hunks().len();
                    self.hunk_index = if hunk_count == 0 { None } else { Some(0) };
                    self.file_diff = Some(diff);
                    self.preview_loading = false;
                    self.preview_mode = PreviewMode::Diff;
                    self.preview_scroll = 0;
                    self.ensure_hunk_visible();
                    // Left chip + preview header carry identity; middle status blank.
                    self.status.clear();
                }
                Ok(Err(err)) => {
                    self.preview_loading = false;
                    self.status = format!("diff error: {err}");
                }
                Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
            }
        }
    }

    fn poll_pending_write(&mut self) {
        let done = match self.pending_write.as_mut() {
            Some(pending) => pending.handle.try_join().map(|result| {
                let op = match &pending.op {
                    WriteOp::Stage => "staged",
                    WriteOp::Unstage => "unstaged",
                    WriteOp::StageHunk => "hunk staged",
                    WriteOp::UnstageHunk => "hunk unstaged",
                    WriteOp::Commit(_) => "committed",
                    WriteOp::Push => "pushed",
                    WriteOp::Pull => "pulled",
                };
                let refresh = !matches!(pending.op, WriteOp::Push);
                (op.to_string(), refresh, result)
            }),
            None => None,
        };
        if let Some((op, refresh, result)) = done {
            self.pending_write = None;
            match result {
                Ok(Ok(msg)) => {
                    let detail = msg.unwrap_or_default();
                    if refresh {
                        self.status = if detail.is_empty() {
                            format!("{op} - refreshing...")
                        } else {
                            format!("{op} - {detail}")
                        };
                        self.request_status();
                        // Keep diff open after hunk ops — reload it.
                        if matches!(
                            op.as_str(),
                            "hunk staged" | "hunk unstaged" | "staged" | "unstaged" | "committed"
                        ) {
                            if let Some(diff) = self.file_diff.clone() {
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
                        self.status = if detail.is_empty() { op } else { format!("{op} - {detail}") };
                    }
                }
                Ok(Err(err)) => {
                    self.status = format!("{op} failed: {err}");
                }
                Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
            }
        }
    }

    fn apply_list_result(
        &mut self,
        path: PathBuf,
        result: Result<Result<Vec<Entry>, FsError>, JobError>,
    ) {
        match result {
            Ok(Ok(entries)) => {
                self.children.insert(path.clone(), entries);
                self.rebuild_visible();

                if path == self.tree_root {
                    if let Some(name) = self.restore_select.take() {
                        if let Some(i) = self
                            .visible
                            .iter()
                            .position(|r| r.depth == 0 && r.entry.name == name)
                        {
                            self.selected = i;
                        }
                    }
                }

                if self.selected < self.scroll {
                    self.scroll = self.selected;
                }
                if self.visible.is_empty() {
                    self.scroll = 0;
                }

                self.request_missing_expanded();
                self.status = self.hint_bar();
                if matches!(self.preview_mode, PreviewMode::File) {
                    self.request_preview_for_selection();
                }
            }
            Ok(Err(err)) => {
                self.status = format!("list error: {err}");
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn apply_status_result(
        &mut self,
        key: PathBuf,
        result: Result<Result<Option<StatusSnapshot>, GitReadError>, JobError>,
    ) {
        match result {
            Ok(Ok(Some(snap))) => {
                if !self.tree_root.starts_with(&snap.repo_root) && key != self.tree_root {
                    return;
                }
                self.branch = read_branch_name(&snap.repo_root).unwrap_or_else(|| "HEAD".into());
                self.git_status = Some(snap);
                self.clamp_scm_selection();
                if self.in_git_sidebar() && self.selected_scm_entry().is_some() {
                    self.request_diff_for_scm_selection();
                } else if self.in_git_sidebar() {
                    self.preview_mode = PreviewMode::Diff;
                    self.file_diff = None;
                    self.preview_loading = false;
                    self.hunk_index = None;
                }
                if self.pending_lists.is_empty() && !self.in_git_sidebar() {
                    self.status = self.hint_bar();
                }
            }
            Ok(Ok(None)) => {
                if key == self.tree_root
                    || self
                        .git_status
                        .as_ref()
                        .is_some_and(|s| !self.tree_root.starts_with(&s.repo_root))
                {
                    self.git_status = None;
                    self.branch.clear();
                }
                self.clamp_scm_selection();
                if self.pending_lists.is_empty() {
                    self.status = self.hint_bar();
                }
            }
            Ok(Err(err)) => {
                self.status = format!("git status error: {err}");
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn apply_preview_result(
        &mut self,
        path: PathBuf,
        result: Result<Result<Preview, FsError>, JobError>,
    ) {
        let current = self.selected_entry().map(|e| e.path.as_path());
        if current != Some(path.as_path()) {
            return;
        }
        if !matches!(self.preview_mode, PreviewMode::File) {
            return;
        }

        match result {
            Ok(Ok(preview)) => {
                self.preview = Some(preview);
                self.preview_loading = false;
                self.preview_scroll = 0;
            }
            Ok(Err(err)) => {
                self.preview = Some(Preview::error(path, err.to_string()));
                self.preview_loading = false;
                self.preview_scroll = 0;
            }
            Err(JobError::Cancelled) | Err(JobError::Shutdown) => {}
        }
    }

    fn dismiss_menu(&mut self) {
        self.context_menu = None;
    }

    fn toggle_scm_sidebar(&mut self) {
        if self.in_git_sidebar() {
            self.leave_git_sidebar();
        } else {
            self.enter_git_sidebar();
        }
    }

    fn enter_git_sidebar(&mut self) {
        self.activity = ActivityTab::Git;
        self.focus = PaneFocus::List;
        self.clamp_scm_selection();
        // Prefer first change row when landing on a section header.
        let rows = self.scm_rows();
        if let Some(ScmRow::Section { .. }) = rows.get(self.scm_selected) {
            if let Some(i) = rows.iter().position(|r| r.is_change()) {
                self.scm_selected = i;
            }
        }
        self.request_diff_for_scm_selection();
        self.status = "source control".into();
    }

    fn leave_git_sidebar(&mut self) {
        self.activity = ActivityTab::Files;
        self.focus = PaneFocus::List;
        self.preview_mode = PreviewMode::File;
        self.file_diff = None;
        self.hunk_index = None;
        self.request_preview_for_selection();
        self.status = self.hint_bar();
    }

    fn expand_staging(&mut self) {
        // Compatibility alias: open Git sidebar.
        if !self.in_git_sidebar() {
            self.enter_git_sidebar();
        } else {
            self.focus = PaneFocus::List;
            self.clamp_scm_selection();
        }
    }

    fn open_file_context_menu(&mut self, index: usize, col: u16, row: u16) {
        let Some(row_ref) = self.visible.get(index) else {
            return;
        };
        let entry = &row_ref.entry;
        self.selected = index;
        let is_dir = matches!(entry.kind, EntryKind::Directory | EntryKind::Symlink);
        let (can_stage, can_unstage) = if is_dir {
            (false, false)
        } else {
            self.file_stage_flags(entry.path.as_path())
        };
        let items = file_list_items(is_dir, can_stage, can_unstage);
        self.context_menu = ContextMenu::new(
            items,
            col,
            row,
            ContextTarget::FileList { index },
        );
        self.focus = PaneFocus::List;
    }

    fn open_staging_context_menu(&mut self, column: StagingColumn, index: usize, col: u16, row: u16) {
        self.expand_staging();
        // Map column+index into flat SCM selection.
        let rows = self.scm_rows();
        if let Some(flat) = rows.iter().position(|r| {
            matches!(r, ScmRow::Change { column: c, index: i } if *c == column && *i == index)
        }) {
            self.scm_selected = flat;
        }
        self.focus = PaneFocus::List;
        let has_staged = self
            .git_status
            .as_ref()
            .map(|s| !s.staged.is_empty())
            .unwrap_or(false);
        let in_repo = self.git_status.is_some();
        let items = staging_items(column, has_staged, in_repo);
        self.context_menu = ContextMenu::new(
            items,
            col,
            row,
            ContextTarget::Staging { column, index },
        );
    }

    fn file_stage_flags(&self, abs: &Path) -> (bool, bool) {
        let Some(snap) = &self.git_status else {
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
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        self.dismiss_menu();
        match menu.target {
            ContextTarget::FileList { index } => {
                if index < self.visible.len() {
                    self.selected = index;
                }
                match action {
                    ContextAction::Open => self.open_selected(),
                    ContextAction::Preview => {
                        self.request_preview_for_selection();
                        self.focus = PaneFocus::Preview;
                    }
                    ContextAction::Stage => {
                        self.focus = PaneFocus::List;
                        self.request_write_stage(true);
                    }
                    ContextAction::Unstage => {
                        self.focus = PaneFocus::List;
                        self.request_write_stage(false);
                    }
                    ContextAction::Diff => self.diff_selected_file(),
                    ContextAction::CopyPath => self.copy_selected_path(),
                    ContextAction::Commit => {
                        self.commit_input = Some(String::new());
                        self.status = "commit message...".into();
                    }
                    ContextAction::Push => self.request_push(),
                    ContextAction::Pull => self.request_pull(),
                    ContextAction::StageHunk => self.request_hunk_write(true),
                    ContextAction::UnstageHunk => self.request_hunk_write(false),
                }
            }
            ContextTarget::Staging { column, index } => {
                self.expand_staging();
                let rows = self.scm_rows();
                if let Some(flat) = rows.iter().position(|r| {
                    matches!(r, ScmRow::Change { column: c, index: i } if *c == column && *i == index)
                }) {
                    self.scm_selected = flat;
                }
                self.focus = PaneFocus::List;
                match action {
                    ContextAction::Stage => self.request_write_stage(true),
                    ContextAction::Unstage => self.request_write_stage(false),
                    ContextAction::Diff => {
                        self.request_diff_for_scm_selection();
                        self.focus = PaneFocus::Preview;
                    }
                    ContextAction::Commit => {
                        self.commit_input = Some(String::new());
                        self.status = "commit message...".into();
                    }
                    ContextAction::CopyPath => self.copy_staging_path(),
                    ContextAction::Push => self.request_push(),
                    ContextAction::Pull => self.request_pull(),
                    ContextAction::Open
                    | ContextAction::Preview
                    | ContextAction::StageHunk
                    | ContextAction::UnstageHunk => {}
                }
            }
            ContextTarget::Diff => {
                self.focus = PaneFocus::Preview;
                match action {
                    ContextAction::StageHunk => self.request_hunk_write(true),
                    ContextAction::UnstageHunk => self.request_hunk_write(false),
                    ContextAction::CopyPath => self.copy_diff_path(),
                    _ => {}
                }
            }
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        match copy_path_clipboard(text) {
            Ok(()) => self.status = format!("copied - {text}"),
            Err(err) => self.status = format!("clipboard unavailable ({err}) - {text}"),
        }
    }

    fn copy_selected_path(&mut self) {
        if let Some(entry) = self.selected_entry() {
            let s = entry.path.display().to_string();
            self.copy_to_clipboard(&s);
        }
    }

    fn copy_staging_path(&mut self) {
        if let Some(entry) = self.selected_scm_entry() {
            let path = entry.path.clone();
            let s = if let Some(repo) = self.repo_root() {
                repo.join(&path).display().to_string()
            } else {
                path
            };
            self.copy_to_clipboard(&s);
        }
    }

    fn copy_diff_path(&mut self) {
        if let Some(diff) = self.file_diff.as_ref() {
            let s = diff.repo_root.join(&diff.path).display().to_string();
            self.copy_to_clipboard(&s);
        }
    }

    fn current_hunks(&self) -> Vec<DiffHunk> {
        self.file_diff
            .as_ref()
            .map(|d| d.hunks())
            .unwrap_or_default()
    }

    fn ensure_hunk_visible(&mut self) {
        let Some(idx) = self.hunk_index else {
            return;
        };
        let Some(diff) = self.file_diff.as_ref() else {
            return;
        };
        let hunks = diff.hunks();
        let Some(hunk) = hunks.get(idx) else {
            return;
        };
        // Diff body starts at line 0 (title chip is outside the scroll area).
        // Map unified hunk index → split-row index (meta lines collapsed).
        let rendered_start = diff_render_hunk_start(&diff.lines, hunk.start);
        let total = preview_line_count(self.current_preview_view());
        let viewport = self.layout.preview_inner.height as usize;
        scroll_to_hunk(&mut self.preview_scroll, viewport, total, rendered_start);
    }

    fn move_hunk(&mut self, delta: isize) {
        let hunks = self.current_hunks();
        if hunks.is_empty() {
            self.status = "no hunks in this diff".into();
            return;
        }
        let cur = self.hunk_index.unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(hunks.len() as isize) as usize;
        self.hunk_index = Some(next);
        self.ensure_hunk_visible();
        self.focus = PaneFocus::Preview;
        self.status = format!("hunk {}/{}", next + 1, hunks.len());
    }

    fn request_hunk_write(&mut self, stage: bool) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        let Some(diff) = self.file_diff.clone() else {
            self.status = "open a diff first (d)".into();
            return;
        };
        let hunks = diff.hunks();
        let Some(idx) = self.hunk_index else {
            self.status = "no hunk selected (n/N)".into();
            return;
        };
        let Some(hunk) = hunks.get(idx).cloned() else {
            self.status = "hunk out of range".into();
            return;
        };
        if stage && !matches!(diff.side, DiffSide::Unstaged) {
            self.status = "switch to unstaged diff to stage hunk".into();
            return;
        }
        if !stage && !matches!(diff.side, DiffSide::Staged) {
            self.status = "switch to staged diff to unstage hunk".into();
            return;
        }
        let git = self.git;
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:write".into()),
            async move {
                let r = if stage {
                    git.stage_hunk(&repo, &diff, &hunk).await
                } else {
                    git.unstage_hunk(&repo, &diff, &hunk).await
                };
                r.map(|_| None)
            },
        );
        self.status = if stage {
            format!("staging hunk {}/{}...", idx + 1, hunks.len())
        } else {
            format!("unstaging hunk {}/{}...", idx + 1, hunks.len())
        };
        self.pending_write = Some(PendingWrite {
            id: handle.id,
            op: if stage {
                WriteOp::StageHunk
            } else {
                WriteOp::UnstageHunk
            },
            handle,
        });
    }

    fn request_push(&mut self) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        let git = self.git;
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:write".into()),
            async move { git.push(&repo).await.map(Some) },
        );
        self.status = "pushing...".into();
        self.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Push,
            handle,
        });
    }

    fn request_pull(&mut self) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        let git = self.git;
        let handle = self.jobs.spawn(
            JobPriority::High,
            Some("git:write".into()),
            async move { git.pull(&repo).await.map(Some) },
        );
        self.status = "pulling...".into();
        self.pending_write = Some(PendingWrite {
            id: handle.id,
            op: WriteOp::Pull,
            handle,
        });
    }

    fn open_diff_context_menu(&mut self, col: u16, row: u16) {
        let Some(diff) = self.file_diff.as_ref() else {
            return;
        };
        let has_hunk = self.hunk_index.is_some();
        let items = diff_items(matches!(diff.side, DiffSide::Unstaged), has_hunk);
        self.context_menu = ContextMenu::new(items, col, row, ContextTarget::Diff);
        self.focus = PaneFocus::Preview;
    }

    fn handle_key(&mut self, key: KeyEvent) {

        // Commit modal captures all keys.
        if let Some(buf) = self.commit_input.as_mut() {
            match key.code {
                KeyCode::Esc => {
                    self.commit_input = None;
                    self.status = "commit cancelled".into();
                }
                KeyCode::Enter => {
                    let msg = buf.clone();
                    self.commit_input = None;
                    if msg.trim().is_empty() {
                        self.status = "empty commit message".into();
                    } else {
                        self.request_commit(msg);
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.push(c);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {}
            }
            return;
        }

        // Context menu captures navigation / confirm / dismiss.
        if self.context_menu.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.dismiss_menu();
                    self.status = self.hint_bar();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(menu) = self.context_menu.as_mut() {
                        menu.move_sel(-1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(menu) = self.context_menu.as_mut() {
                        menu.move_sel(1);
                    }
                }
                KeyCode::Enter => {
                    let action = self
                        .context_menu
                        .as_ref()
                        .and_then(|m| m.current_action());
                    if let Some(action) = action {
                        self.apply_context_action(action);
                    }
                }
                KeyCode::Char('q') => {
                    self.dismiss_menu();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Esc => {
                if self.in_git_sidebar() && self.sidebar_focused() {
                    self.leave_git_sidebar();
                } else if self.in_git_sidebar() && matches!(self.focus, PaneFocus::Preview) {
                    // From diff preview back to SCM list (keep diff loaded).
                    self.focus = PaneFocus::List;
                } else if matches!(self.preview_mode, PreviewMode::Diff) {
                    self.preview_mode = PreviewMode::File;
                    self.file_diff = None;
                    self.hunk_index = None;
                    self.request_preview_for_selection();
                    self.focus = PaneFocus::List;
                }
            }
            KeyCode::Char('c') => {
                if self.repo_root().is_none() {
                    self.status = "not a git repository".into();
                } else {
                    self.commit_input = Some(String::new());
                    self.status = "commit message...".into();
                }
            }
            KeyCode::Char('s') => {
                if matches!(self.preview_mode, PreviewMode::Diff) && self.file_diff.is_some() {
                    self.request_hunk_write(true);
                } else {
                    self.request_write_stage(true);
                }
            }
            KeyCode::Char('u') => {
                if matches!(self.preview_mode, PreviewMode::Diff) && self.file_diff.is_some() {
                    self.request_hunk_write(false);
                } else {
                    self.request_write_stage(false);
                }
            }
            KeyCode::Char('n')
                if matches!(self.preview_mode, PreviewMode::Diff) =>
            {
                self.move_hunk(1);
            }
            KeyCode::Char('N')
                if matches!(self.preview_mode, PreviewMode::Diff) =>
            {
                self.move_hunk(-1);
            }
            KeyCode::Char('p') => self.request_pull(),
            KeyCode::Char('P') => self.request_push(),
            KeyCode::Char('d')
                if self.in_git_sidebar() && self.sidebar_focused() =>
            {
                self.request_diff_for_scm_selection();
                self.focus = PaneFocus::Preview;
            }
            KeyCode::Char('d') => {
                self.diff_selected_file();
            }
            KeyCode::Char('g') => {
                self.toggle_scm_sidebar();
            }
            KeyCode::Char('[') => {
                self.expand_staging();
                let rows = self.scm_rows();
                self.scm_selected = scm_jump_to_column(&rows, StagingColumn::Unstaged);
                self.focus = PaneFocus::List;
                self.request_diff_for_scm_selection();
            }
            KeyCode::Char(']') => {
                self.expand_staging();
                let rows = self.scm_rows();
                self.scm_selected = scm_jump_to_column(&rows, StagingColumn::Staged);
                self.focus = PaneFocus::List;
                self.request_diff_for_scm_selection();
            }
            KeyCode::Char('j') | KeyCode::Down => match self.focus {
                PaneFocus::Preview => self.scroll_preview(1),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                    self.focus = PaneFocus::List;
                    self.move_scm(1);
                }
                _ => {
                    self.focus = PaneFocus::List;
                    self.move_sel(1);
                }
            },
            KeyCode::Char('k') | KeyCode::Up => match self.focus {
                PaneFocus::Preview => self.scroll_preview(-1),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => {
                    self.focus = PaneFocus::List;
                    self.move_scm(-1);
                }
                _ => {
                    self.focus = PaneFocus::List;
                    self.move_sel(-1);
                }
            },
            KeyCode::Char(' ') => {
                if matches!(self.focus, PaneFocus::List) && !self.in_git_sidebar() {
                    self.toggle_expand_at(self.selected);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                if self.in_git_sidebar() && self.sidebar_focused() {
                    self.request_diff_for_scm_selection();
                    self.focus = PaneFocus::Preview;
                } else {
                    self.open_selected();
                }
            }
            KeyCode::Char('h') | KeyCode::Backspace => {
                if matches!(self.focus, PaneFocus::Preview) {
                    self.focus = PaneFocus::List;
                } else if self.in_git_sidebar() {
                    self.leave_git_sidebar();
                } else {
                    self.tree_back();
                }
            }
            KeyCode::Char('r') => {
                self.request_list();
                self.request_status();
            }
            KeyCode::Tab => {
                // Sidebar <-> Preview only (Git reuses sidebar via activity).
                self.focus = match self.focus {
                    PaneFocus::List | PaneFocus::Staging => PaneFocus::Preview,
                    PaneFocus::Preview => PaneFocus::List,
                };
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    PaneFocus::List | PaneFocus::Staging => PaneFocus::Preview,
                    PaneFocus::Preview => PaneFocus::List,
                };
            }
            KeyCode::Home => {
                self.focus = PaneFocus::List;
                if self.in_git_sidebar() {
                    self.scm_selected = 0;
                    self.request_diff_for_scm_selection();
                } else {
                    self.selected = 0;
                    self.request_preview_for_selection();
                }
            }
            KeyCode::End => {
                self.focus = PaneFocus::List;
                if self.in_git_sidebar() {
                    self.scm_selected = self.scm_rows().len().saturating_sub(1);
                    self.request_diff_for_scm_selection();
                } else {
                    self.selected = self.visible.len().saturating_sub(1);
                    self.request_preview_for_selection();
                }
            }
            KeyCode::PageDown => match self.focus {
                PaneFocus::Preview => self.scroll_preview(10),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => self.move_scm(5),
                _ => self.move_sel(10),
            },
            KeyCode::PageUp => match self.focus {
                PaneFocus::Preview => self.scroll_preview(-10),
                PaneFocus::List | PaneFocus::Staging if self.in_git_sidebar() => self.move_scm(-5),
                _ => self.move_sel(-10),
            },
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        // Context menu: consume clicks while open.
        if let Some(menu) = self.context_menu.clone() {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Down(MouseButton::Right) => {
                    if menu.contains(col, row) {
                        if let Some((idx, action)) = self
                            .context_menu
                            .as_ref()
                            .and_then(|m| m.action_at_row(col, row))
                        {
                            if let Some(m) = self.context_menu.as_mut() {
                                m.selected = idx;
                            }
                            let enabled = self
                                .context_menu
                                .as_ref()
                                .and_then(|m| m.items.get(idx))
                                .map(|i| i.enabled)
                                .unwrap_or(false);
                            if enabled {
                                self.apply_context_action(action);
                            }
                        }
                    } else {
                        self.dismiss_menu();
                        // Fall through so a left click still selects underneath.
                        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                            self.handle_mouse_content(mouse);
                        } else if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
                            self.handle_mouse_content(mouse);
                        }
                    }
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    // Ignore wheel over menu / dismiss on wheel outside.
                    if !menu.contains(col, row) {
                        self.dismiss_menu();
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(m) = self.context_menu.as_mut() {
                        m.hover_at(col, row);
                    }
                }
                _ => {}
            }
            return;
        }

        self.handle_mouse_content(mouse);
    }

    fn handle_mouse_content(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;
        let over_list = self.layout.contains_list(col, row);
        let over_preview = self.layout.contains_preview(col, row);
        let git = self.in_git_sidebar();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if over_preview && matches!(self.preview_mode, PreviewMode::Diff) {
                    self.open_diff_context_menu(col, row);
                } else if over_list && git {
                    let rows = self.scm_rows();
                    if let Some(flat) = scm_index_at(
                        self.layout.list_inner,
                        self.scroll_scm,
                        rows.len(),
                        col,
                        row,
                    ) {
                        if let Some(ScmRow::Change { column, index }) = rows.get(flat).copied() {
                            self.open_staging_context_menu(column, index, col, row);
                        }
                    }
                } else if over_list {
                    if let Some(index) = list_index_at(
                        self.layout.list_inner,
                        self.scroll,
                        self.visible.len(),
                        col,
                        row,
                    ) {
                        self.open_file_context_menu(index, col, row);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if over_list && git {
                    self.focus = PaneFocus::List;
                    let rows = self.scm_rows();
                    if let Some(flat) = scm_index_at(
                        self.layout.list_inner,
                        self.scroll_scm,
                        rows.len(),
                        col,
                        row,
                    ) {
                        if flat != self.scm_selected {
                            self.scm_selected = flat;
                        }
                        if self.selected_scm_entry().is_some() {
                            self.request_diff_for_scm_selection();
                        } else {
                            self.preview_mode = PreviewMode::Diff;
                            self.file_diff = None;
                            self.preview_loading = false;
                            self.hunk_index = None;
                            self.jobs.cancel_key("git:diff");
                            self.pending_diff = None;
                            self.status = "No changes selected".into();
                        }
                    }
                } else if over_list {
                    self.focus = PaneFocus::List;
                    // Stay in Files when clicking the file tree.
                    if !matches!(self.activity, ActivityTab::Files) {
                        self.activity = ActivityTab::Files;
                        self.preview_mode = PreviewMode::File;
                    }
                    if let Some(index) = list_index_at(
                        self.layout.list_inner,
                        self.scroll,
                        self.visible.len(),
                        col,
                        row,
                    ) {
                        let now = Instant::now();
                        let is_double = self
                            .last_list_click
                            .map(|(t, i)| i == index && now.duration_since(t) <= DOUBLE_CLICK)
                            .unwrap_or(false);
                        self.last_list_click = Some((now, index));

                        let is_dir = self
                            .visible
                            .get(index)
                            .is_some_and(|r| matches!(r.entry.kind, EntryKind::Directory));

                        if index != self.selected {
                            self.selected = index;
                            self.request_preview_for_selection();
                        }

                        if is_double {
                            if !is_dir {
                                self.open_selected();
                            }
                            self.last_list_click = None;
                        } else if is_dir {
                            self.toggle_expand_at(index);
                        }
                    }
                } else if over_preview {
                    self.focus = PaneFocus::Preview;
                }
            }
            MouseEventKind::ScrollUp => {
                if over_list || (!over_preview && self.sidebar_focused()) {
                    self.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.move_scm(-1);
                    } else {
                        self.move_sel(-1);
                    }
                } else if over_preview || matches!(self.focus, PaneFocus::Preview) {
                    self.focus = PaneFocus::Preview;
                    self.scroll_preview(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if over_list || (!over_preview && self.sidebar_focused()) {
                    self.focus = PaneFocus::List;
                    if self.in_git_sidebar() {
                        self.move_scm(1);
                    } else {
                        self.move_sel(1);
                    }
                } else if over_preview || matches!(self.focus, PaneFocus::Preview) {
                    self.focus = PaneFocus::Preview;
                    self.scroll_preview(3);
                }
            }
            _ => {}
        }
    }

    fn diff_selected_file(&mut self) {
        let Some(repo) = self.repo_root() else {
            self.status = "not a git repository".into();
            return;
        };
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if matches!(entry.kind, EntryKind::Directory) {
            self.status = "select a file to diff".into();
            return;
        }
        let Ok(rel) = entry.path.strip_prefix(&repo) else {
            self.status = "file outside repository".into();
            return;
        };
        let rela = forge_model::path_to_rela(rel);
        let side = if let Some(snap) = &self.git_status {
            if snap.unstaged.iter().any(|c| c.path == rela) {
                DiffSide::Unstaged
            } else if snap.staged.iter().any(|c| c.path == rela) {
                DiffSide::Staged
            } else {
                DiffSide::Unstaged
            }
        } else {
            DiffSide::Unstaged
        };
        self.request_diff(rela, side);
        self.focus = PaneFocus::Preview;
    }

    fn scroll_preview(&mut self, delta: isize) {
        let view = self.current_preview_view();
        let total = preview_line_count(view);
        let viewport = self.layout.preview_inner.height as usize;
        let max_scroll = total.saturating_sub(viewport);
        let next = (self.preview_scroll as isize + delta).clamp(0, max_scroll as isize) as usize;
        self.preview_scroll = next;
    }

    fn current_preview_view(&self) -> PreviewView<'_> {
        if matches!(self.preview_mode, PreviewMode::Diff) {
            if let Some(diff) = self.file_diff.as_ref() {
                let hunks = diff.hunks();
                let (hunk_start, hunk_end, hunk_idx) = match self.hunk_index {
                    Some(i) if i < hunks.len() => (
                        Some(hunks[i].start),
                        Some(hunks[i].end),
                        Some((i, hunks.len())),
                    ),
                    _ if !hunks.is_empty() => (None, None, Some((0, hunks.len()))),
                    _ => (None, None, None),
                };
                return PreviewView::Diff {
                    path: &diff.path,
                    side: diff.side.label(),
                    lines: &diff.lines,
                    hunk_start,
                    hunk_end,
                    hunk_idx,
                };
            }
            if self.preview_loading {
                return PreviewView::Loading;
            }
            return PreviewView::Hint {
                text: "No changes selected",
            };
        }
        if self.preview_loading {
            return PreviewView::Loading;
        }
        let Some(preview) = self.preview.as_ref() else {
            return PreviewView::Idle;
        };
        match &preview.content {
            PreviewContent::Empty => PreviewView::Idle,
            PreviewContent::Directory { name, children, truncated } => PreviewView::Directory {
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
        }
    }

    fn move_sel(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let len = self.visible.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        if next != self.selected {
            self.selected = next;
            self.request_preview_for_selection();
        }
    }

    fn move_scm(&mut self, delta: isize) {
        let len = self.scm_rows().len();
        if len == 0 {
            return;
        }
        let next = (self.scm_selected as isize + delta).clamp(0, (len as isize) - 1) as usize;
        if next != self.scm_selected {
            self.scm_selected = next;
            // Auto-load diff for change rows; clear when on a section header.
            if self.selected_scm_entry().is_some() {
                self.request_diff_for_scm_selection();
            } else {
                self.preview_mode = PreviewMode::Diff;
                self.file_diff = None;
                self.preview_loading = false;
                self.hunk_index = None;
                self.jobs.cancel_key("git:diff");
                self.pending_diff = None;
                self.status = "No changes selected".into();
            }
        }
    }

    fn toggle_expand_at(&mut self, index: usize) {
        let Some(row) = self.visible.get(index) else {
            return;
        };
        if !matches!(row.entry.kind, EntryKind::Directory) {
            return;
        }
        let path = row.entry.path.clone();
        let was_expanded = self.expanded.contains(&path);
        if was_expanded {
            self.expanded.remove(&path);
        } else {
            self.expanded.insert(path.clone());
            if !self.children.contains_key(&path) {
                self.request_dir(path);
            }
        }
        // Keep selection on the same directory after rebuild.
        let keep = self.visible.get(index).map(|r| r.entry.path.clone());
        self.rebuild_visible();
        if let Some(path) = keep {
            if let Some(i) = self.visible.iter().position(|r| r.entry.path == path) {
                self.selected = i;
            }
        }
        self.status = self.hint_bar();
        if matches!(self.preview_mode, PreviewMode::File) {
            self.request_preview_for_selection();
        }
    }

    fn open_selected(&mut self) {
        let Some(row) = self.visible.get(self.selected) else {
            return;
        };
        if matches!(row.entry.kind, EntryKind::Directory) {
            self.toggle_expand_at(self.selected);
            return;
        }
        self.status = format!("previewing {}", row.entry.name);
        self.request_preview_for_selection();
    }

    /// `h` / Backspace: collapse expanded dir, else select parent, else re-root up.
    fn tree_back(&mut self) {
        let Some(row) = self.visible.get(self.selected).cloned() else {
            self.re_root_up();
            return;
        };
        if matches!(row.entry.kind, EntryKind::Directory) && row.expanded {
            self.toggle_expand_at(self.selected);
            return;
        }
        if row.depth > 0 {
            if let Some(parent_idx) = find_parent_row(&self.visible, self.selected) {
                self.selected = parent_idx;
                self.request_preview_for_selection();
            }
            return;
        }
        self.re_root_up();
    }

    fn re_root_up(&mut self) {
        let Some(parent) = self.tree_root.parent() else {
            self.status = "already at root".into();
            return;
        };
        let parent = parent.to_path_buf();
        if parent == self.tree_root {
            self.status = "already at root".into();
            return;
        }
        let came_from = self
            .tree_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned());
        self.tree_root = parent;
        self.expanded.clear();
        self.children.clear();
        self.visible.clear();
        self.selected = 0;
        self.scroll = 0;
        self.preview_scroll = 0;
        self.restore_select = came_from;
        self.last_list_click = None;
        self.focus = PaneFocus::List;
        self.request_list();
        self.request_status();
        if let Some(name) = &self.restore_select {
            self.status = format!("up - restoring {name}");
        }
    }
}

#[allow(dead_code)]
fn path_is_under(child: &Path, root: &Path) -> bool {
    child.starts_with(root)
}

/// Copy `text` to the system clipboard. Falls back with an error on headless hosts.
fn copy_path_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text.to_string()).map_err(|e| e.to_string())
}

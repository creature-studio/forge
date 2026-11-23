//! Searchable workspace commands and focused dialogs. All I/O runs in jobs.
use super::*;
use forge_fs::workspace::{FileIndex, FileOperation, SearchOptions};
use forge_git::{GitAction, GitItem, GitQuery};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tui_input::backend::crossterm::EventHandler;

mod actions;
mod controls;
#[cfg(test)]
mod tests;

/// One explicitly selected Git row: absolute path (file or folder) plus side.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct GitMark {
    pub path: PathBuf,
    pub side: DiffSide,
}

/// Selection state for both pages. Git marks carry a side so CHANGES and
/// STAGED rows of the same file are independent; file marks stay path-only.
#[derive(Default, Clone)]
pub(super) struct Marks {
    pub files: HashSet<PathBuf>,
    pub git: HashSet<GitMark>,
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.git.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len() + self.git.len()
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.git.clear();
    }

    pub fn contains_file(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    pub fn toggle_file(&mut self, path: PathBuf) -> bool {
        if self.files.remove(&path) {
            false
        } else {
            self.files.insert(path);
            true
        }
    }

    pub fn extend_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.files.extend(paths);
    }

    pub fn toggle_git(&mut self, path: PathBuf, side: DiffSide) -> bool {
        let mark = GitMark { path, side };
        if self.git.remove(&mark) {
            false
        } else {
            self.git.insert(mark);
            true
        }
    }

    pub fn contains_git(&self, path: &Path, side: DiffSide) -> bool {
        self.git.contains(&GitMark {
            path: path.to_owned(),
            side,
        })
    }

    pub fn git_paths(&self, side: DiffSide) -> Vec<PathBuf> {
        self.git
            .iter()
            .filter(|mark| mark.side == side)
            .map(|mark| mark.path.clone())
            .collect()
    }
}

/// Per-workspace content-search draft and regex mode. Filename pickers open
/// empty so a new target is never appended to an old query.
#[derive(Default, Clone)]
struct SearchSession {
    content: String,
    regex: bool,
}

#[derive(Default)]
pub(super) struct ToolsState {
    pub context_path: Option<PathBuf>,
    pub content_context: Option<std::result::Result<forge_fs::workspace::ContentContext, String>>,
    pending_context: Option<JobHandle<Result<forge_fs::workspace::ContentContext>>>,
    context_cancel: Option<Arc<AtomicBool>>,
    pub panel: Option<Panel>,
    history_parents: Vec<Panel>,
    search_sessions: HashMap<PathBuf, SearchSession>,
    pub pending_read: Option<JobHandle<Result<ReadReply>>>,
    pub pending_task: Option<JobHandle<Result<String>>>,
    pub pending_sync: Option<JobHandle<Result<String>>>,
    cancel: Option<Arc<AtomicBool>>,
    file_index: Option<Arc<FileIndex>>,
    pending_index: Option<JobHandle<Result<Arc<FileIndex>>>>,
    index_key: Option<(PathBuf, SearchOptions)>,
    index_cancel: Option<Arc<AtomicBool>>,
    search_generation: u64,
    search_hits: Option<Mutex<std::sync::mpsc::Receiver<(u64, forge_fs::workspace::SearchHit)>>>,
    streamed: bool,
    after: Option<PanelKind>,
    pub marks: Marks,
    pub filter: FileFilter,
    pub editor: Option<EditorRequest>,
    pub reveal: Option<(PathBuf, usize)>,
    pub return_focus: Option<PaneFocus>,
    pub resume_diff: Option<DiffResume>,
    pub sync: String,
    pub last_fetch: String,
    pub remote_result: String,
    pub pinned: Option<GitItem>,
    worktree_navigation: HashMap<PathBuf, WorkspaceNavigation>,
}

/// Navigation state is local to a worktree. Never reuse a directory's
/// expansion/selection in another worktree with similar filenames.
#[derive(Clone)]
struct WorkspaceNavigation {
    selected_path: Option<PathBuf>,
    selected_scroll: usize,
    expanded: HashSet<PathBuf>,
    filter: FileFilter,
    staged_folders: HashSet<String>,
    unstaged_folders: HashSet<String>,
    scm_selected: usize,
    scm_scroll: usize,
    activity: ActivityTab,
    focus: PaneFocus,
}

#[derive(Default, Clone)]
pub(super) struct FileFilter {
    pub status: u8, // 0 all, 1 modified, 2 untracked
    pub extension: String,
    pub show_hidden: bool,
    pub excluded: String,
}

#[derive(Clone)]
pub(super) struct EditorRequest {
    pub path: PathBuf,
    pub line: usize,
}

pub(super) struct DiffResume {
    pub path: String,
    pub side: DiffSide,
    pub scroll: usize,
    pub focus: PaneFocus,
    pub hunk: Option<usize>,
}

pub(super) struct EditorResume {
    request: EditorRequest,
    root: PathBuf,
    path: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    unstaged: HashSet<String>,
    staged: HashSet<String>,
    scm_selected: usize,
    activity: ActivityTab,
    focus: PaneFocus,
    scroll: usize,
    filter: FileFilter,
    marks: Marks,
    list_scroll: usize,
    scm_scroll: usize,
    git_selection: Option<(String, DiffSide)>,
    hunk: Option<usize>,
}

impl EditorResume {
    pub fn capture(app: &mut App) -> Option<Self> {
        let request = app.tools.editor.take()?;
        let opened_path = request.path.clone();
        Some(Self {
            request,
            root: app.files.tree_root.clone(),
            path: Some(opened_path),
            expanded: app.files.expanded.clone(),
            unstaged: app.scm.scm_expanded_unstaged.clone(),
            staged: app.scm.scm_expanded_staged.clone(),
            scm_selected: app.scm.scm_selected,
            activity: app.session.activity,
            focus: app.session.focus,
            scroll: app.preview_source_line().saturating_sub(1),
            filter: app.tools.filter.clone(),
            marks: app.tools.marks.clone(),
            list_scroll: app.files.scroll,
            scm_scroll: app.scm.scroll_scm,
            git_selection: app.selected_scm_entry().map(|e| (e.path.clone(), e.side)),
            hunk: app.preview_state.hunk_index,
        })
    }
    pub async fn edit(self, mode: ColorMode) -> App {
        let result = run_editor(&self.request).await;
        self.restore(mode, result)
    }
    fn restore(self, mode: ColorMode, result: Result<()>) -> App {
        let mut app = create_workspace_at(mode, self.root);
        app.files.expanded = self.expanded;
        app.scm.scm_expanded_unstaged = self.unstaged;
        app.scm.scm_expanded_staged = self.staged;
        app.scm.scm_selected = self.scm_selected;
        app.session.activity = self.activity;
        app.session.focus = self.focus;
        app.tools.filter = self.filter;
        app.tools.marks = self.marks;
        app.files.scroll = self.list_scroll;
        app.scm.scroll_scm = self.scm_scroll;
        if self.activity == ActivityTab::Git {
            if let Some((path, side)) = self.git_selection {
                app.tools.resume_diff = Some(DiffResume {
                    path,
                    side,
                    scroll: self.scroll,
                    focus: self.focus,
                    hunk: self.hunk,
                });
            }
        } else if let Some(path) = self.path {
            app.tools.reveal = Some((path, self.scroll + 1));
        }
        app.tools.return_focus = Some(self.focus);
        app.request_missing_expanded();
        if let Err(e) = result {
            app.session.write_status = Some(format!("Editor failed: {e}"));
        }
        app
    }
}

pub(super) async fn run_editor(request: &EditorRequest) -> Result<()> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".into());
    let words = shell_words::split(&editor).context("Invalid $EDITOR quoting")?;
    let (program, args) = words.split_first().context("$EDITOR is empty")?;
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    let name = Path::new(program)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    if matches!(name.as_ref(), "vi" | "vim" | "nvim" | "nano" | "emacs") {
        command.arg(format!("+{}", request.line));
    }
    command.arg(&request.path);
    command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let status = command.status().await?;
    if !status.success() {
        anyhow::bail!("{program} exited with {status}");
    }
    Ok(())
}

#[derive(Clone)]
pub(super) enum PanelKind {
    Commands,
    Files,
    Content,
    Filters,
    Settings,
    Workspaces,
    Git(GitQuery),
    Lines(FileDiff),
    Prompt(PromptAction),
    Confirm(Operation),
    Text,
}

#[derive(Clone)]
pub(super) enum PromptAction {
    NewFile,
    NewDirectory,
    Rename(PathBuf),
    Copy(Vec<PathBuf>),
    Move(Vec<PathBuf>),
    Extension,
    Exclusions,
    Branch,
    RenameBranch(String),
    Stash,
    StashUnstaged,
    Amend,
    Worktree,
    OpenWorkspace,
}

#[derive(Clone)]
pub(super) enum Operation {
    Files(FileOperation),
    Git(GitAction),
    Stage { paths: Vec<PathBuf>, stage: bool },
    Line { diff: FileDiff, index: usize },
    DiscardHunk { diff: FileDiff, hunk: DiffHunk },
}

#[derive(Clone)]
pub(super) enum RowValue {
    Command(Command),
    File(PathBuf, usize),
    Git(GitItem),
    Workspace(PathBuf),
    Line(usize),
}
#[derive(Clone)]
pub(super) struct ToolRow {
    pub label: String,
    pub value: RowValue,
}
pub(super) struct ReadReply {
    generation: Option<u64>,
    history_has_older: bool,
    rows: Vec<ToolRow>,
    detail: String,
}

fn search_row(root: &Path, hit: forge_fs::workspace::SearchHit) -> ToolRow {
    ToolRow {
        // Filename matches have no content hit or meaningful line number.
        // Line one is kept only for the reveal target.
        label: if !hit.content {
            hit.path
                .strip_prefix(root)
                .unwrap_or(&hit.path)
                .display()
                .to_string()
        } else {
            format!(
                "{}:{}  {}",
                hit.path.strip_prefix(root).unwrap_or(&hit.path).display(),
                hit.line,
                if hit.text.is_empty() {
                    "(empty line)"
                } else {
                    &hit.text
                }
            )
        },
        value: RowValue::File(hit.path, hit.line),
    }
}

pub(super) struct Panel {
    history_has_older: bool,
    pub kind: PanelKind,
    pub title: String,
    pub input: tui_input::Input,
    pub rows: Vec<ToolRow>,
    pub selected: usize,
    // A streamed search initially selects the first arriving hit, which may
    // later sort anywhere. Only preserve it across sorting if the user moved.
    pub search_selection_touched: bool,
    /// Detail pane scroll. List scrolling is independent of detail scrolling.
    pub scroll: usize,
    pub scroll_limit: usize,
    pub list_offset: usize,
    pub last_click: Option<(usize, std::time::Instant)>,
    pub detail: String,
    pub message: String,
    pub regex: bool,
    pub area: ratatui::layout::Rect,
    pub list_area: ratatui::layout::Rect,
    /// Form button hit regions are updated only by the shared renderer.
    pub form_cancel: ratatui::layout::Rect,
    pub form_submit: ratatui::layout::Rect,
}

#[derive(Clone, Copy)]
pub(super) enum Command {
    FindFile,
    FindContent,
    NewFile,
    NewDirectory,
    Rename,
    Copy,
    Move,
    Delete,
    Mark,
    MarkAll,
    ClearMarks,
    Stage,
    Unstage,
    Lines,
    FileHistory,
    History,
    Graph,
    Editor,
    EditText,
    OpenFile,
    Filters,
    All,
    Modified,
    Untracked,
    Extension,
    Hidden,
    Exclusions,
    TreeStickyScroll,
    CompactFolders,
    Settings,
    CycleTheme,
    ToggleStagedGutter,
    ToggleUnstagedGutter,
    ToggleCodeWrap,
    ToggleOverviewRuler,
    ShortcutSettings,
    Discard,
    Branches,
    Stashes,
    Conflicts,
    Amend,
    Sync,
    Fetch,
    Worktrees,
    Workspaces,
    AbortMerge,
    AbortCherryPick,
    AbortRevert,
    ContinueCherryPick,
    ContinueRevert,
}

const COMMANDS: &[(Command, &str)] = &[
    (
        Command::EditText,
        "Edit text in Forge                       Ctrl-E",
    ),
    (
        Command::FindFile,
        "Find file by fuzzy name                  Ctrl-P",
    ),
    (
        Command::FindContent,
        "Search file contents / regex             Ctrl-Shift-F",
    ),
    (
        Command::OpenFile,
        "Open changed file in Files                 o",
    ),
    (Command::NewFile, "New file…"),
    (Command::NewDirectory, "New directory…"),
    (Command::Rename, "Rename selected file or directory…"),
    (Command::Copy, "Copy selected items to directory…"),
    (Command::Move, "Move selected items to directory…"),
    (Command::Delete, "Delete selected items…"),
    (Command::Mark, "Toggle selection                         v"),
    (
        Command::MarkAll,
        "Select all visible items                 V",
    ),
    (Command::ClearMarks, "Clear selection"),
    (Command::Stage, "Stage selected files / folders"),
    (Command::Unstage, "Unstage selected files / folders"),
    (Command::Lines, "Stage / Unstage individual diff lines    I"),
    (
        Command::FileHistory,
        "File history and compare versions        H",
    ),
    (
        Command::History,
        "Commit history                           L",
    ),
    (Command::Graph, "Commit history in the Git sidebar        G"),
    (
        Command::Editor,
        "Open selected file in $EDITOR            E",
    ),
    (
        Command::Filters,
        "File filters                             F",
    ),
    (Command::All, "Filter: all files"),
    (
        Command::Modified,
        "Filter: modified / added / deleted files",
    ),
    (Command::Untracked, "Filter: untracked files"),
    (Command::Extension, "Filter: extension…"),
    (Command::Hidden, "Filter: toggle hidden files"),
    (Command::Exclusions, "Filter: excluded directory names…"),
    (Command::TreeStickyScroll, "View: toggle Tree Sticky Scroll"),
    (Command::CompactFolders, "View: toggle Compact Folders"),
    (
        Command::Settings,
        "Settings · Appearance / Navigation / Keyboard",
    ),
    (
        Command::ShortcutSettings,
        "Settings: keyboard shortcuts (F4)",
    ),
    (
        Command::Discard,
        "Discard unstaged file changes / current hunk…   x",
    ),
    (
        Command::Branches,
        "Branches: create / switch / rename / delete / diff   B",
    ),
    (
        Command::Stashes,
        "Stashes: save / view / apply / drop       Z",
    ),
    (
        Command::Conflicts,
        "Conflicts: ours / theirs / edit / resolve",
    ),
    (Command::Amend, "Amend latest commit…"),
    (Command::Sync, "Remote status / fetch / pull / push      S"),
    (Command::Fetch, "Fetch all remotes"),
    (
        Command::Workspaces,
        "Switch workspace: recent directories / open folder  Alt-O",
    ),
    (
        Command::Worktrees,
        "Worktrees: list / create / switch / remove    W",
    ),
    (Command::AbortMerge, "Abort merge…"),
    (Command::AbortCherryPick, "Abort cherry-pick…"),
    (Command::AbortRevert, "Abort revert…"),
    (Command::ContinueCherryPick, "Continue cherry-pick"),
    (Command::ContinueRevert, "Continue revert"),
];

impl Panel {
    fn replace_rows(&mut self, rows: Vec<ToolRow>) {
        let selected = self
            .rows
            .get(self.selected)
            .and_then(|row| match &row.value {
                RowValue::File(path, line) => Some((path.clone(), *line)),
                _ => None,
            });
        self.rows = rows;
        self.last_click = None; // reordering invalidates a prior row-position click
        self.selected = selected
            .and_then(|(path, line)| {
                self.rows.iter().position(
                    |row| matches!(&row.value, RowValue::File(p, l) if p == &path && *l == line),
                )
            })
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
        self.list_offset = self.list_offset.min(self.rows.len().saturating_sub(1));
        self.show_selected();
    }

    /// One navigation contract for all command/search/Git lists: the selected
    /// row and viewport position are separate from the detail scroll.
    fn page_size(&self) -> usize {
        (self.list_area.height as usize).max(1)
    }

    fn show_selected(&mut self) {
        let page = self.page_size();
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset.saturating_add(page) {
            self.list_offset = self.selected.saturating_add(1).saturating_sub(page);
        }
        self.list_offset = self.list_offset.min(self.rows.len().saturating_sub(page));
    }

    fn select_row(&mut self, next: usize) {
        self.last_click = None;
        self.selected = next.min(self.rows.len().saturating_sub(1));
        self.show_selected();
        self.scroll = 0;
    }

    fn scroll_rows(&mut self, delta: isize) {
        self.last_click = None;
        let max = self.rows.len().saturating_sub(self.page_size());
        self.list_offset = self.list_offset.saturating_add_signed(delta).min(max);
    }

    pub(super) fn new(kind: PanelKind, title: &str) -> Self {
        Self {
            kind,
            history_has_older: false,
            title: title.into(),
            input: tui_input::Input::default(),
            rows: Vec::new(),
            selected: 0,
            search_selection_touched: false,
            scroll: 0,
            scroll_limit: 0,
            list_offset: 0,
            last_click: None,
            detail: String::new(),
            message: String::new(),
            regex: false,
            area: Default::default(),
            list_area: Default::default(),
            form_cancel: Default::default(),
            form_submit: Default::default(),
        }
    }
    pub fn is_input(&self) -> bool {
        matches!(
            self.kind,
            PanelKind::Commands
                | PanelKind::Files
                | PanelKind::Content
                | PanelKind::Workspaces
                | PanelKind::Prompt(_)
        )
    }
}

impl App {
    pub(super) fn new_item_in(&mut self, parent: &Path, directory: bool) {
        let prefix = parent.strip_prefix(&self.files.tree_root).unwrap_or(parent);
        let prefix = if prefix.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.display())
        };
        self.prompt(
            if directory {
                PromptAction::NewDirectory
            } else {
                PromptAction::NewFile
            },
            if directory { "New folder" } else { "New file" },
            prefix,
            "Path relative to workspace · existing items are never overwritten",
        );
    }

    pub(super) fn confirm_delete_paths(&mut self, paths: Vec<PathBuf>) {
        let detail = format!(
            "Permanently delete {} items and their contents?\n{}",
            paths.len(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        self.confirm(Operation::Files(FileOperation::Delete(paths)), detail, None);
    }

    pub(super) fn open_file_root_menu(&mut self, col: u16, row: u16) {
        self.session.context_menu = ContextMenu::new(
            vec![
                forge_ui::ContextMenuItem {
                    action: ContextAction::NewFile,
                    enabled: true,
                },
                forge_ui::ContextMenuItem {
                    action: ContextAction::NewDirectory,
                    enabled: true,
                },
            ],
            col,
            row,
            ContextTarget::FileRoot {
                path: self.files.tree_root.clone(),
            },
        );
    }

    pub(super) fn current_path(&self) -> Option<PathBuf> {
        if self.in_git_sidebar() {
            match self.selected_scm_row()? {
                ScmRow::Folder { path, .. } => Some(self.repo_root()?.join(path)),
                ScmRow::Change { .. } => {
                    Some(self.repo_root()?.join(&self.selected_scm_entry()?.path))
                }
                ScmRow::CommitFile { commit, index, .. } => {
                    let oid = self.history.commits.get(commit)?.id.clone();
                    let path = self.history.files.get(&oid)?.get(index)?.path.clone();
                    Some(self.repo_root()?.join(path))
                }
                _ => None,
            }
        } else {
            self.selected_entry().map(|e| e.path.clone())
        }
    }

    /// Editing from the preview targets the file displayed on the right,
    /// even when a directory is temporarily selected in the left tree.
    pub(super) fn editor_target(&self) -> Option<PathBuf> {
        if self.session.focus == PaneFocus::Preview
            && matches!(self.preview_state.preview_mode, PreviewMode::File)
        {
            if let Some(preview) = &self.preview_state.preview {
                if preview.path.is_file() {
                    return Some(preview.path.clone());
                }
            }
        }
        self.current_path()
    }

    fn operation_paths(&self) -> Vec<PathBuf> {
        if self.tools.marks.files.is_empty() {
            self.current_path().into_iter().collect()
        } else {
            let files: Vec<_> = self.tools.marks.files.iter().cloned().collect();
            forge_fs::workspace::unique_roots(&files)
        }
    }

    pub(super) fn toggle_mark(&mut self) {
        if self.in_git_sidebar() {
            let Some(row) = self.selected_scm_row() else {
                self.session.status = "Select a file or folder in Git first".into();
                return;
            };
            let Some(repo) = self.repo_root() else {
                self.session.status = "Not a Git repository".into();
                return;
            };
            let (path, side) = match row {
                ScmRow::Change { .. } => {
                    let Some(entry) = self.selected_scm_entry() else {
                        return;
                    };
                    (repo.join(&entry.path), entry.side)
                }
                ScmRow::Folder { path, column, .. } => (repo.join(path), column.side()),
                ScmRow::Section { .. } => {
                    self.session.status =
                        "Select a file or folder; group headings are navigation only".into();
                    return;
                }
                ScmRow::HistorySection { .. }
                | ScmRow::Commit { .. }
                | ScmRow::CommitFolder { .. }
                | ScmRow::CommitFile { .. } => {
                    self.session.status =
                        "History is read-only; open the file in the working tree to stage it"
                            .into();
                    return;
                }
            };
            self.tools.marks.toggle_git(path, side);
            self.session.status = format!(
                "{} selected · {} · s Stage / u Unstage only this side · Esc clear",
                self.tools.marks.len(),
                side.label()
            );
            return;
        }
        if let Some(path) = self.current_path() {
            self.tools.marks.toggle_file(path);
            self.session.status = format!(
                "{} selected · : Copy / Move / Delete · Esc clear",
                self.tools.marks.len()
            );
        }
    }

    fn mark_all(&mut self) {
        if self.in_git_sidebar() {
            let Some(repo) = self.repo_root() else {
                self.session.status = "Cannot mark Git changes: repository root unavailable".into();
                return;
            };
            let changes: Vec<(PathBuf, DiffSide)> = {
                let (u, s) = self.scm_slices();
                self.scm_rows()
                    .iter()
                    .filter_map(|row| {
                        scm_entry(row, u, s).map(|entry| (repo.join(&entry.path), entry.side))
                    })
                    .collect()
            };
            for (path, side) in changes {
                self.tools.marks.git.insert(GitMark { path, side });
            }
        } else {
            let paths = self
                .files
                .visible
                .iter()
                .map(|r| r.entry.path.clone())
                .collect::<Vec<_>>();
            self.tools.marks.extend_files(paths);
        }
        self.session.status = format!(
            "{} selected · {}",
            self.tools.marks.len(),
            if self.in_git_sidebar() {
                "s Stage selected sides · u Unstage selected sides"
            } else {
                ": batch operations"
            }
        );
    }

    pub(super) fn filter_entry(&self, entry: &Entry) -> bool {
        // An explicit Open file / Reveal request must be visible even if a
        // previous extension, status or excluded-folder filter hides its path.
        // Never bypass the special .git directory exclusion.
        if entry.name != ".git"
            && self.files.revealed_path.as_ref().is_some_and(|target| {
                entry.path == *target
                    || (entry.kind == EntryKind::Directory && target.starts_with(&entry.path))
            })
        {
            return true;
        }
        let options = self.search_options(false, false);
        if entry.name == ".git"
            || (entry.kind == EntryKind::Directory && options.excluded.contains(&entry.name))
            || (!options.show_hidden && entry.name.starts_with('.'))
        {
            return false;
        }
        if let Some(allowed) = &options.allowed {
            if !allowed.iter().any(|p| {
                p == &entry.path
                    || (entry.kind == EntryKind::Directory && p.starts_with(&entry.path))
            }) {
                return false;
            }
        }
        entry.kind == EntryKind::Directory
            || options.extension.is_empty()
            || entry.path.extension().and_then(|e| e.to_str())
                == Some(options.extension.trim_start_matches('.'))
    }

    fn search_options(&self, content: bool, regex: bool) -> SearchOptions {
        let mut options = SearchOptions {
            content,
            regex,
            show_hidden: self.tools.filter.show_hidden,
            extension: self.tools.filter.extension.clone(),
            ..Default::default()
        };
        if !self.tools.filter.excluded.is_empty() {
            options.excluded.extend(
                self.tools
                    .filter
                    .excluded
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned),
            );
        }
        if self.tools.filter.status != 0 {
            let wanted = self.tools.filter.status;
            options.allowed = Some(
                self.scm
                    .git_status
                    .as_ref()
                    .map(|s| {
                        s.entries
                            .iter()
                            .filter(|(_, v)| {
                                if wanted == 2 {
                                    **v == forge_git::GitStatus::Untracked
                                } else {
                                    !matches!(
                                        v,
                                        forge_git::GitStatus::Clean
                                            | forge_git::GitStatus::Untracked
                                            | forge_git::GitStatus::Ignored
                                    )
                                }
                            })
                            .map(|(p, _)| s.repo_root.join(p))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
        }
        options
    }

    fn cancel_tool_read(&mut self) {
        self.tools.search_generation = self.tools.search_generation.wrapping_add(1);
        self.tools.search_hits = None;
        if let Some(flag) = self.tools.cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.services.jobs.cancel_key("tools:read");
        self.tools.pending_read = None;
    }

    fn cancel_file_index(&mut self) {
        if let Some(flag) = self.tools.index_cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.services.jobs.cancel_key("tools:index");
        self.tools.pending_index = None;
        self.tools.index_key = None;
    }

    pub(super) fn invalidate_search_cache(&mut self) {
        self.clear_search_context();
        self.cancel_file_index();
        self.tools.file_index = None;
        if self
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PanelKind::Files | PanelKind::Content))
        {
            self.reload_tools();
        }
    }

    fn remember_search_panel(&mut self) {
        let Some(panel) = self.tools.panel.as_ref() else {
            return;
        };
        if !matches!(panel.kind, PanelKind::Content) {
            return;
        }
        let session = self
            .tools
            .search_sessions
            .entry(self.files.tree_root.clone())
            .or_default();
        if matches!(panel.kind, PanelKind::Content) {
            session.content = panel.input.value().to_owned();
            session.regex = panel.regex;
        }
    }

    pub(super) fn close_tools(&mut self) {
        self.remember_search_panel();
        self.clear_search_context();
        self.cancel_tool_read();
        self.cancel_file_index();
        self.tools.panel = None;
        self.tools.history_parents.clear();
    }

    fn open_history_child(&mut self, kind: PanelKind) {
        let parent = self.tools.panel.take();
        let mut parents = std::mem::take(&mut self.tools.history_parents);
        if let Some(parent) = parent {
            parents.push(parent);
        }
        self.open_tools(kind);
        self.tools.history_parents = parents;
    }

    pub(super) fn tools_has_parent(&self) -> bool {
        !self.tools.history_parents.is_empty()
    }
    /// Single source for the Escape action advertised by panel borders
    /// and the workspace footer. The actual behavior is back_tools().
    pub(super) fn tools_escape_label(&self) -> &'static str {
        match self.tools.panel.as_ref().map(|p| &p.kind) {
            Some(PanelKind::Prompt(_) | PanelKind::Confirm(_)) => "Cancel",
            Some(_) if self.tools_has_parent() => "Back",
            _ => "Close",
        }
    }

    fn back_tools(&mut self) {
        self.cancel_tool_read();
        if let Some(parent) = self.tools.history_parents.pop() {
            // The parent's read is cancelled when a child opens. If someone
            // opens a prompt while the list still says Loading, resume the
            // read on return instead of restoring a permanently empty panel.
            let retry = parent.message == "Loading…" && parent.rows.is_empty();
            self.tools.panel = Some(parent);
            if retry {
                self.reload_tools();
            }
        } else {
            self.close_tools();
        }
    }

    pub(super) fn open_tools(&mut self, kind: PanelKind) {
        self.remember_search_panel();
        self.cancel_tool_read();
        self.cancel_file_index();
        self.tools.history_parents.clear();
        self.session.context_menu = None;
        let title = match &kind {
            PanelKind::Commands => "Commands",
            PanelKind::Files => "Find file · fuzzy name",
            PanelKind::Content => "Search contents",
            PanelKind::Filters => "File filters",
            PanelKind::Settings => "Settings",
            PanelKind::Workspaces => "Workspaces · O open folder",
            PanelKind::Git(q) => match q {
                GitQuery::History(Some(_)) | GitQuery::HistoryPage { path: Some(_), .. } => {
                    "File history"
                }
                GitQuery::History(None) | GitQuery::HistoryPage { path: None, .. } => {
                    "Commit history"
                }
                GitQuery::Commit(_) => "Commit details · Enter file diff",
                GitQuery::Branches => "Branches",
                GitQuery::Stashes => "Stashes",
                GitQuery::StashDiff(_) => "Stash diff · Esc Stashes · PgUp/PgDn Scroll",
                GitQuery::Conflicts => "Conflicts",
                GitQuery::Worktrees => "Worktrees",
                GitQuery::Sync => "Remote synchronization",
                GitQuery::PublishRemotes(_) => "Publish Branch · Choose remote",
                _ => "Git diff",
            },
            PanelKind::Lines(_) => "Diff lines · s Stage line · u Unstage line",
            PanelKind::Prompt(_) => "Input",
            PanelKind::Confirm(_) => "Confirm operation",
            PanelKind::Text => "Details",
        };
        let mut panel = Panel::new(kind, title);
        if let Some(previous) = self.tools.search_sessions.get(&self.files.tree_root) {
            if matches!(panel.kind, PanelKind::Content) {
                panel.input = tui_input::Input::new(previous.content.clone());
                panel.regex = previous.regex;
                panel.title = format!(
                    "Search contents · {}",
                    if panel.regex { "regex" } else { "literal" }
                );
            }
        }
        if matches!(panel.kind, PanelKind::Content) {
            panel.detail = "Type code to search · Alt-R toggle regex · Enter jump · results limited to 2,000, text files ≤ 2 MiB".into();
        }
        if matches!(panel.kind, PanelKind::Files) {
            panel.detail = "Type a few filename characters · Enter reveal file · Ctrl-E Edit text · E External editor · best 2,000 matching files".into();
        }
        self.tools.panel = Some(panel);
        self.reload_tools();
    }

    fn reload_tools(&mut self) {
        let has_git_repo = self.repo_root().is_some();
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        if !matches!(panel.kind, PanelKind::Files | PanelKind::Content) {
            panel.rows.clear();
            panel.selected = 0;
            panel.list_offset = 0;
            panel.last_click = None;
            panel.scroll = 0;
        } else {
            // Never allow Enter to activate a hit from a previous query.
            // Search results belong to a single generation; clear the old
            // list immediately while the debounced request is in flight.
            panel.rows.clear();
            panel.selected = 0;
            panel.list_offset = 0;
            panel.last_click = None;
            panel.scroll = 0;
            panel.search_selection_touched = false;
        }
        let kind = panel.kind.clone();
        let query = panel.input.value().to_owned();
        let regex = panel.regex;
        if matches!(kind, PanelKind::Workspaces) {
            let mut recent = vec![self.files.tree_root.clone()];
            recent.extend(self.config.recent_workspaces.iter().cloned());
            let query = query.to_ascii_lowercase();
            let mut seen = HashSet::new();
            panel.rows = recent
                .into_iter()
                .filter(|path| seen.insert(path.clone()))
                .filter(|path| path.to_string_lossy().to_ascii_lowercase().contains(&query))
                .map(|path| ToolRow {
                    label: format!(
                        "{}  {}",
                        if path == self.files.tree_root {
                            "●"
                        } else {
                            " "
                        },
                        path.display()
                    ),
                    value: RowValue::Workspace(path),
                })
                .collect();
            panel.message = if panel.rows.is_empty() {
                "No matching recent directories · press O to open a folder".into()
            } else {
                String::new()
            };
            panel.detail = "Enter switch · O open any directory · type to filter
Git Worktrees are managed separately with W."
                .into();
            return;
        }
        if matches!(kind, PanelKind::Settings) {
            panel.rows = vec![
                ToolRow {
                    label: format!(
                        "Tree Sticky Scroll [{}]",
                        if self.config.tree_sticky_scroll {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::TreeStickyScroll),
                },
                ToolRow {
                    label: format!(
                        "Compact Folders [{}]",
                        if self.config.compact_folders {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::CompactFolders),
                },
                ToolRow {
                    label: format!(
                        "Show hidden files [{}]",
                        if self.config.show_hidden_files {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::Hidden),
                },
                ToolRow {
                    label: format!(
                        "Color theme [{}]",
                        self.config.theme.as_deref().unwrap_or("current")
                    ),
                    value: RowValue::Command(Command::CycleTheme),
                },
                ToolRow {
                    label: format!(
                        "Code auto wrap [{}]",
                        if self.config.wrap_code { "On" } else { "Off" }
                    ),
                    value: RowValue::Command(Command::ToggleCodeWrap),
                },
                ToolRow {
                    label: format!(
                        "Focus Overview [{}]",
                        if self.config.show_overview_ruler {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::ToggleOverviewRuler),
                },
                ToolRow {
                    label: format!(
                        "Git Gutter: staged [{}]",
                        if self.config.git.show_staged_gutter {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::ToggleStagedGutter),
                },
                ToolRow {
                    label: format!(
                        "Git Gutter: unstaged [{}]",
                        if self.config.git.show_unstaged_gutter {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    value: RowValue::Command(Command::ToggleUnstagedGutter),
                },
                ToolRow {
                    label: "Keyboard shortcuts [F4]".into(),
                    value: RowValue::Command(Command::ShortcutSettings),
                },
            ];
            panel.detail = "DISPLAY & NAVIGATION\nFocus Overview shows Git changes, search matches and viewport position; click to jump.\nTree Sticky Scroll pins parent folders while scrolling.\nCompact Folders joins loaded unbranched directory chains.\nShow hidden files displays dotfiles in Files.\n\nKEYBOARD\nF4 opens searchable keyboard shortcuts.\n\nSettings save to forge/config.toml. Enter or Space to toggle; Esc closes.".into();
            return;
        }
        if matches!(kind, PanelKind::Commands | PanelKind::Filters) {
            panel.rows = COMMANDS
                .iter()
                // Commands that require a Git repository must not appear as
                // executable actions in an unrelated plain directory.
                .filter(|(c, _)| {
                    has_git_repo
                        || !matches!(
                            c,
                            Command::Stage
                                | Command::Unstage
                                | Command::Lines
                                | Command::FileHistory
                                | Command::History
                                | Command::Graph
                                | Command::Branches
                                | Command::Stashes
                                | Command::Conflicts
                                | Command::Amend
                                | Command::Sync
                                | Command::Fetch
                                | Command::Worktrees
                                | Command::Discard
                                | Command::AbortMerge
                                | Command::AbortCherryPick
                                | Command::AbortRevert
                                | Command::ContinueCherryPick
                                | Command::ContinueRevert
                        )
                })
                .filter(|(c, _)| {
                    self.session.activity == ActivityTab::Git
                        || !matches!(c, Command::Stage | Command::Unstage | Command::Lines)
                })
                .filter(|(c, _)| {
                    !matches!(kind, PanelKind::Filters)
                        || matches!(
                            c,
                            Command::All
                                | Command::Modified
                                | Command::Untracked
                                | Command::Extension
                                | Command::Hidden
                                | Command::Exclusions
                        )
                })
                .filter(|(_, label)| label.to_lowercase().contains(&query.to_lowercase()))
                .map(|(c, label)| ToolRow {
                    label: if matches!(c, Command::TreeStickyScroll) {
                        format!(
                            "View: Tree Sticky Scroll [{}] · toggle",
                            if self.config.tree_sticky_scroll {
                                "On"
                            } else {
                                "Off"
                            }
                        )
                    } else if matches!(c, Command::CompactFolders) {
                        format!(
                            "View: Compact Folders [{}] · toggle",
                            if self.config.compact_folders {
                                "On"
                            } else {
                                "Off"
                            }
                        )
                    } else {
                        self.keys.command_label(label)
                    },
                    value: RowValue::Command(*c),
                })
                .collect();
            panel.message = if panel.rows.is_empty() {
                if !has_git_repo {
                    "No matching commands · Git operations need a repository".into()
                } else {
                    "No matching commands".into()
                }
            } else {
                String::new()
            };
            panel.detail = format!(
                "{} selected · filters: status={} extension={} hidden={} · Enter run",
                self.tools.marks.len(),
                self.tools.filter.status,
                self.tools.filter.extension,
                self.tools.filter.show_hidden
            );
            return;
        }
        if let PanelKind::Lines(diff) = kind {
            panel.rows = diff
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    matches!(
                        l.kind,
                        forge_git::DiffLineKind::Add | forge_git::DiffLineKind::Delete
                    )
                })
                .map(|(i, l)| ToolRow {
                    label: format!("{:>5} {}", i + 1, l.text),
                    value: RowValue::Line(i),
                })
                .collect();
            panel.detail = format!(
                "{} · {} · one changed line per operation; all other edits are preserved",
                diff.path,
                diff.side.label()
            );
            return;
        }
        let is_search = matches!(kind, PanelKind::Files | PanelKind::Content);
        let root = if is_search {
            self.files.tree_root.clone()
        } else {
            self.repo_root()
                .unwrap_or_else(|| self.files.tree_root.clone())
        };
        let git = self.services.git;
        let options = self.search_options(matches!(kind, PanelKind::Content), regex);
        self.cancel_tool_read();
        let generation = self.tools.search_generation;
        let index = if matches!(kind, PanelKind::Files) {
            if let Some(index) = self
                .tools
                .file_index
                .as_ref()
                .filter(|i| i.matches(&root, &options))
            {
                Some(index.clone())
            } else {
                let key = (root.clone(), options.clone());
                if self.tools.index_key.as_ref() != Some(&key) {
                    self.cancel_file_index();
                    self.tools.file_index = None;
                    let flag = Arc::new(AtomicBool::new(false));
                    self.tools.index_cancel = Some(flag.clone());
                    self.tools.index_key = Some(key);
                    let root = root.clone();
                    let options = options.clone();
                    self.tools.pending_index = Some(self.services.jobs.spawn(
                        JobPriority::High,
                        Some("tools:index".into()),
                        async move {
                            tokio::task::spawn_blocking(move || {
                                FileIndex::build(&root, &options, &flag).map(Arc::new)
                            })
                            .await?
                        },
                    ));
                }
                self.tools.panel.as_mut().unwrap().message = "Searching… indexing files".into();
                return;
            }
        } else {
            None
        };
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(flag) = self.tools.cancel.replace(cancel.clone()) {
            flag.store(true, Ordering::Relaxed);
        }
        if !is_search && !matches!(kind, PanelKind::Git(_)) {
            return;
        }
        if let Some(panel) = &mut self.tools.panel {
            panel.message = if is_search {
                "Searching…"
            } else {
                "Loading…"
            }
            .into();
        }
        let (hit_tx, hit_rx) = std::sync::mpsc::sync_channel(2000);
        self.tools.search_hits = is_search.then_some(Mutex::new(hit_rx));
        self.tools.streamed = false;
        let wake = self.services.jobs.completion_notifier();
        self.tools.pending_read = Some(self.services.jobs.spawn(JobPriority::High, Some("tools:read".into()), async move {
            if is_search {
                // Trailing-edge debounce for both filename and content searches.
                // The scheduler cancels the previous generation whenever the input
                // changes, so only the most recent non-empty query reaches a worker.
                // Empty queries render immediately (including the initial file list).
                if !query.is_empty() {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                let search_root=root.clone();
                let index_detail = index.as_ref().map(|index| {
                    let mut detail = String::new();
                    if index.truncated { detail.push_str(" · index capped at 100,000 paths / 32 MiB; narrow workspace to search remaining files"); }
                    if index.skipped > 0 { detail.push_str(&format!(" · {} unreadable paths skipped", index.skipped)); }
                    detail
                }).unwrap_or_default();
                let hits = tokio::task::spawn_blocking(move || {
                    if let Some(index) = index { return Ok(index.query(&query, &cancel)); }
                    let mut_count = std::sync::atomic::AtomicUsize::new(0);
                    forge_fs::workspace::search_stream(&search_root, &query, &options, &cancel, |hit| {
                        let _ = hit_tx.try_send((generation, hit.clone()));
                        if mut_count.fetch_add(1, Ordering::Relaxed) % 64 == 0 { wake.notify_one(); }
                    })
                }).await??;
                let count = hits.len();
                let detail = if matches!(kind, PanelKind::Files) {
                    format!("{count} matching files · Enter open · max 2,000 results{index_detail}")
                } else {
                    format!("{count} matches · Enter jump · Alt-R regex · max 2,000 results / 2 MiB per file{index_detail}")
                };
                Ok(ReadReply { generation: Some(generation), history_has_older: false,
                    rows: hits.into_iter().map(|h| search_row(&root, h)).collect(), detail })
            } else if let PanelKind::Git(q) = kind {
                let history = match &q {
                    GitQuery::History(path) => Some((path.clone(), 0)),
                    GitQuery::HistoryPage { path, skip } => Some((path.clone(), *skip)),
                    _ => None,
                };
                let (items, mut detail) = git.write.query(&root,q).await?;
                let mut history_has_older = false;
                if let Some((path, skip)) = history {
                    // A full page alone cannot distinguish 200 from 201 commits.
                    if items.len() == 200 {
                        let (older, _) = git.write.query(&root, GitQuery::HistoryPage {
                            path, skip: skip.saturating_add(200),
                        }).await?;
                        history_has_older = !older.is_empty();
                    }
                    let range = if items.is_empty() { "No commits found".into() }
                        else { format!("Commits {}–{}", skip + 1, skip + items.len()) };
                    detail = format!("{range} · {} · {} · Enter details · Space pin version · D compare pinned → selected · C cherry-pick · R revert",
                        if skip == 0 { "First page" } else { "[ newer" },
                        if history_has_older { "] older" } else { "Last page" });
                }
                Ok(ReadReply { generation: None, history_has_older, rows: items.into_iter().map(|item| ToolRow { label: item.label.clone(), value: RowValue::Git(item) }).collect(), detail })
            } else { unreachable!() }
        }));
    }

    pub(super) fn prompt(
        &mut self,
        action: PromptAction,
        title: &str,
        value: String,
        detail: &str,
    ) {
        // Any panel opening a child prompt must be preserved. Explicitly
        // enumerating parent kinds used to drop navigation history for
        // Workspaces and new workflows, making Esc close every dialog.
        self.open_history_child(PanelKind::Prompt(action));
        if let Some(panel) = &mut self.tools.panel {
            panel.title = title.into();
            panel.input = tui_input::Input::new(value);
            panel.detail = detail.into();
        }
    }

    fn confirm(&mut self, operation: Operation, detail: String, after: Option<PanelKind>) {
        // Confirmation is always a child of the page (or prompt) that
        // requested it. On Esc, restore that exact page and its draft.
        let title = if matches!(&operation, Operation::Files(FileOperation::Delete(_))) {
            "Delete items"
        } else {
            "Confirm"
        };
        self.open_history_child(PanelKind::Confirm(operation));
        self.tools.after = after;
        if let Some(panel) = &mut self.tools.panel {
            panel.title = title.into();
            panel.detail = detail;
            panel.message.clear(); // Shared dialog renders both actions.
        }
    }

    pub(super) fn poll_tools(&mut self) {
        self.request_search_context();
        if let Some((_, result)) =
            pending_request::take_ready(&mut self.tools.pending_context, JobHandle::try_join)
        {
            self.tools.content_context = Some(match result {
                Ok(Ok(context)) => Ok(context),
                Ok(Err(error)) => Err(format!("Context unavailable: {error}")),
                Err(error) => Err(format!("Context job failed: {error}")),
            });
        }
        if let Some((_, result)) =
            pending_request::take_ready(&mut self.tools.pending_index, JobHandle::try_join)
        {
            self.tools.index_key = None;
            self.tools.index_cancel = None;
            match result {
                Ok(Ok(index)) => {
                    self.tools.file_index = Some(index);
                    if self
                        .tools
                        .panel
                        .as_ref()
                        .is_some_and(|p| matches!(p.kind, PanelKind::Files))
                    {
                        self.reload_tools();
                    }
                }
                Ok(Err(error)) => {
                    if let Some(panel) = &mut self.tools.panel {
                        panel.message = format!("Failed: {error}");
                    }
                }
                _ => {}
            }
        }
        if let Some(receiver) = &self.tools.search_hits {
            let rows: Vec<_> = receiver
                .lock()
                .unwrap()
                .try_iter()
                .filter(|(generation, _)| *generation == self.tools.search_generation)
                .map(|(_, hit)| search_row(&self.files.tree_root, hit))
                .collect();
            if !rows.is_empty() {
                if let Some(panel) = &mut self.tools.panel {
                    if !self.tools.streamed {
                        panel.replace_rows(rows);
                        if matches!(panel.kind, PanelKind::Content)
                            && !panel.search_selection_touched
                        {
                            // Never inherit the old query's selected row.
                            panel.selected = 0;
                            panel.list_offset = 0;
                            panel.scroll = 0;
                        }
                        self.tools.streamed = true;
                    } else {
                        panel.rows.extend(rows);
                    }
                    panel.message = format!("Searching… {} results", panel.rows.len());
                }
            }
        }
        if let Some((_, result)) =
            pending_request::take_ready(&mut self.tools.pending_read, JobHandle::try_join)
        {
            self.tools.search_hits = None;
            if let Some(panel) = &mut self.tools.panel {
                match result {
                    Ok(Ok(reply))
                        if reply
                            .generation
                            .is_none_or(|g| g == self.tools.search_generation) =>
                    {
                        panel.history_has_older = reply.history_has_older;
                        let first_load = panel.rows.is_empty();
                        panel.replace_rows(reply.rows);
                        // A branch manager should open on the checked-out
                        // branch, not the first alphabetically sorted branch.
                        // Don't steal selection on later background refreshes.
                        if first_load && matches!(panel.kind, PanelKind::Git(GitQuery::Branches)) {
                            if let Some(index) = panel.rows.iter().position(|row|
                                matches!(&row.value, RowValue::Git(item) if item.id == self.scm.branch)
                            ) {
                                panel.select_row(index);
                            }
                        }
                        if matches!(panel.kind, PanelKind::Content)
                            && !panel.search_selection_touched
                        {
                            // Parallel hits arrive unordered. The final result
                            // list is sorted: default to its true first row,
                            // not the first hit that happened to arrive.
                            panel.selected = 0;
                            panel.list_offset = 0;
                            panel.scroll = 0;
                        }
                        panel.detail = reply.detail;
                        if matches!(panel.kind, PanelKind::Git(GitQuery::Stashes)) {
                            // Title remains readable in 40-column terminals;
                            // actions live in the scrollable help/detail area.
                            panel.detail = format!(
                                "Enter View Diff  ·  A Save
                                 a Apply  ·  I Restore staged
                                 p Pop  ·  X Drop

{}",
                                panel.detail
                            );
                        }
                        panel.message = if matches!(panel.kind, PanelKind::Git(GitQuery::Sync)) {
                            "Tracking details below · P Push / Publish".into()
                        } else if matches!(panel.kind, PanelKind::Git(GitQuery::PublishRemotes(_)))
                            && panel.rows.is_empty()
                        {
                            "No remotes configured · add a Git remote before publishing".into()
                        } else if panel.rows.is_empty() {
                            "No results · read details below".into()
                        } else {
                            String::new()
                        };
                        if matches!(panel.kind, PanelKind::Git(GitQuery::Sync)) {
                            self.tools.sync =
                                panel.detail.lines().next().unwrap_or_default().into();
                            panel.detail.push_str(&format!(
                                "\nLast session fetch: {}\nLast Pull/Push: {}",
                                self.tools.last_fetch, self.tools.remote_result
                            ));
                        }
                    }
                    Ok(Err(e)) => {
                        panel.message = format!("Failed: {e}");
                    }
                    _ => {}
                }
            }
        }
        self.request_search_context();
        if let Some((_, result)) =
            pending_request::take_ready(&mut self.tools.pending_task, JobHandle::try_join)
        {
            let show_completion = self
                .tools
                .panel
                .as_ref()
                .is_some_and(|p| p.title == "Working");
            match result {
                Ok(Ok(message)) => {
                    if message.starts_with("Fetch completed:") {
                        self.tools.last_fetch = message.clone();
                    }
                    if message.starts_with("Published ") {
                        self.tools.remote_result = message.clone();
                        self.request_sync_status();
                    }
                    self.tools.marks.clear();
                    self.request_list();
                    self.request_status();
                    let after = self.tools.after.take();
                    if show_completion {
                        if let Some(kind) = after {
                            self.open_tools(kind);
                        } else {
                            self.tools.panel = Some(Panel::new(PanelKind::Text, "Completed"));
                        }
                        if let Some(p) = &mut self.tools.panel {
                            p.message = message.clone();
                        }
                    }
                    self.session.write_status = Some(message);
                }
                Ok(Err(e)) => {
                    let message = format!("Failed: {e}");
                    if matches!(self.tools.after, Some(PanelKind::Git(GitQuery::Sync))) {
                        self.tools.last_fetch = message.clone();
                    }
                    self.session.status = message.clone();
                    self.session.write_status = Some(message.clone());
                    self.tools.panel = Some(Panel::new(PanelKind::Text, "Operation failed"));
                    if let Some(p) = &mut self.tools.panel {
                        p.detail=format!("{message}\n\nUse Conflicts for conflicts; recovery commands are in Ctrl+K Commands.\nEsc closes this view.");
                    }
                    self.request_status();
                }
                _ => {}
            }
        }
        if let Some((_, Ok(Ok(summary)))) =
            pending_request::take_ready(&mut self.tools.pending_sync, JobHandle::try_join)
        {
            self.tools.sync = summary;
        }
        self.try_reveal();
    }

    pub(super) fn request_sync_status(&mut self) {
        let Some(repo) = self.repo_root() else {
            return;
        };
        let git = self.services.git;
        self.tools.pending_sync = Some(self.services.jobs.spawn(
            JobPriority::Normal,
            Some("tools:sync".into()),
            async move {
                let (_, detail) = git.write.query(&repo, GitQuery::Sync).await?;
                Ok(detail
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("## ")
                    .into())
            },
        ));
    }

    pub(super) fn open_editor(&mut self, path: Option<PathBuf>, line: usize) {
        if self.write_busy() {
            return;
        }
        if let Some(path) = path.filter(|p| !p.is_dir()) {
            self.tools.editor = Some(EditorRequest { path, line });
        } else {
            self.session.status = "Select a file to edit".into();
        }
    }

    pub(super) fn reveal_file(&mut self, path: PathBuf, line: usize) {
        // Git may discover a repository above the current Files root. The
        // target cannot appear in that narrower tree until it is rooted at
        // the repository, so switch only when the target is actually outside.
        if !path.starts_with(&self.files.tree_root) {
            let repo = self.repo_root();
            if let Some(repo) = repo.filter(|repo| path.starts_with(repo)) {
                if self.write_busy() {
                    self.session.status = "Cannot navigate while a write is in progress".into();
                    return;
                }
                self.switch_workspace(repo);
            } else {
                self.session.status = "File is outside the current workspace".into();
                return;
            }
        }
        self.session.activity = ActivityTab::Files;
        self.session.focus = PaneFocus::List;
        self.files.external_anchor = None; // old refresh anchor must not steal selection
        self.files.last_list_click = None;
        self.files.revealed_path = Some(path.clone());
        self.tools.reveal = Some((path.clone(), line));
        let mut parent = path.parent();
        while let Some(p) = parent {
            if !p.starts_with(&self.files.tree_root) {
                break;
            }
            self.files.expanded.insert(p.to_owned());
            parent = p.parent();
        }
        self.request_missing_expanded();
        self.rebuild_visible();
        self.try_reveal();
    }

    pub(super) fn try_reveal(&mut self) {
        let Some((path, line)) = self.tools.reveal.clone() else {
            return;
        };
        let Some(index) = self.files.visible.iter().position(|r| r.entry.path == path) else {
            return; // Ancestor listings have not arrived yet.
        };
        let pending_target = self
            .preview_state
            .pending_preview
            .as_ref()
            .is_some_and(|pending| pending.path == path);
        let already_displayed = matches!(self.preview_state.preview_mode, PreviewMode::File)
            && self
                .preview_state
                .preview
                .as_ref()
                .is_some_and(|preview| preview.path == path)
            && self.preview_state.pending_preview.is_none();
        self.files.selected = index;
        self.files.follow_selection = true;
        let viewport = self.session.layout.list_inner.height as usize;
        self.files.scroll = index.saturating_sub(viewport.saturating_sub(1) / 2);
        self.files.external_anchor = None;

        if already_displayed {
            // No asynchronous preview completion will follow. Complete the
            // explicit reveal now, including its focus/scroll handoff, or a
            // later reveal could accidentally inherit this operation's focus.
            self.tools.reveal = None;
            if self.markdown_available() {
                self.preview_state.markdown_code = true;
            }
            self.preview_state.preview_scroll = line.saturating_sub(1);
            self.preview_state.pending_source_line = Some(line);
            self.session.focus = self.tools.return_focus.take().unwrap_or(PaneFocus::Preview);
        } else if !pending_target {
            self.request_preview_for_selection();
        }
    }

    pub(super) fn handle_tools_mouse(&mut self, mouse: MouseEvent) {
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        let location = ratatui::layout::Position::new(mouse.column, mouse.row);
        // Form buttons share the exact keyboard confirmation/cancellation
        // path, including history return and deferred-operation cleanup.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && matches!(panel.kind, PanelKind::Prompt(_) | PanelKind::Confirm(_))
        {
            let action = if panel.form_submit.contains(location) {
                Some(KeyCode::Enter)
            } else if panel.form_cancel.contains(location) {
                Some(KeyCode::Esc)
            } else {
                None
            };
            if let Some(key) = action {
                self.handle_tools_key(KeyEvent::new(key, KeyModifiers::NONE));
            }
            return;
        }
        let in_list = panel.list_area.contains(location);
        let previous_selected = panel.selected;
        let mut activate: Option<(PanelKind, RowValue)> = None;
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if matches!(mouse.kind, MouseEventKind::ScrollDown) {
                    3
                } else {
                    -3
                };
                if in_list {
                    // A wheel gesture scrolls the hovered list without
                    // silently changing the selected row or its detail.
                    panel.scroll_rows(delta);
                } else {
                    panel.scroll = panel
                        .scroll
                        .saturating_add_signed(delta)
                        .min(panel.scroll_limit);
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) if in_list => {
                let index = (mouse.row - panel.list_area.y) as usize + panel.list_offset;
                if index < panel.rows.len() {
                    let now = std::time::Instant::now();
                    let is_double_click = panel.last_click.is_some_and(|(old, when)| {
                        old == index && now.duration_since(when) <= Duration::from_millis(450)
                    });
                    panel.selected = index;
                    // Single-click Settings rows toggle immediately (the
                    // existing behavior). No second toggle on double click.
                    let settings = matches!(panel.kind, PanelKind::Settings);
                    if settings || is_double_click {
                        activate = Some((panel.kind.clone(), panel.rows[index].value.clone()));
                        panel.last_click = None;
                    } else {
                        panel.last_click = Some((index, now));
                    }
                }
            }
            _ => return,
        }
        if let Some(panel) = &mut self.tools.panel {
            if panel.selected != previous_selected {
                if matches!(panel.kind, PanelKind::Content) {
                    panel.search_selection_touched = true;
                }
                panel.scroll = 0;
            }
        }
        if let Some((kind, value)) = activate {
            self.activate_tool_row(kind, value);
        } else {
            self.request_search_context();
        }
    }

    fn clear_search_context(&mut self) {
        if let Some(cancel) = self.tools.context_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.services.jobs.cancel_key("tools:context");
        self.tools.pending_context = None;
        self.tools.context_path = None;
        self.tools.content_context = None;
    }

    fn request_search_context(&mut self) {
        let path = self.tools.panel.as_ref().and_then(|panel| {
            if !matches!(panel.kind, PanelKind::Content) {
                return None;
            }
            match &panel.rows.get(panel.selected)?.value {
                RowValue::File(path, _) => Some(path.clone()),
                _ => None,
            }
        });
        if path == self.tools.context_path {
            return;
        }
        self.clear_search_context();
        let Some(path) = path else {
            return;
        };
        self.tools.context_path = Some(path.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.tools.context_cancel = Some(cancel.clone());
        self.tools.pending_context = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("tools:context".into()),
            async move {
                tokio::task::spawn_blocking(move || {
                    forge_fs::workspace::ContentContext::load(&path, &cancel)
                })
                .await?
            },
        ));
    }

    pub(super) fn switch_workspace(&mut self, path: PathBuf) {
        if self.write_busy() || path == self.files.tree_root {
            return;
        }
        // Capture identity (absolute path) rather than a fragile row index.
        let current_root = self.files.tree_root.clone();
        self.config
            .recent_workspaces
            .retain(|p| p != &path && p != &current_root);
        self.config
            .recent_workspaces
            .insert(0, current_root.clone());
        self.config.recent_workspaces.insert(0, path.clone());
        self.config.recent_workspaces.truncate(12);
        // Test fixtures switch workspaces often; never rewrite the developer's
        // real ~/.config/forge/config.toml from a unit test.
        if !cfg!(test) {
            if let Err(error) = self.config.save() {
                self.session.status = format!("Workspace history not saved: {error}");
            }
        }
        let navigation = WorkspaceNavigation {
            selected_path: self.selected_entry().map(|entry| entry.path.clone()),
            selected_scroll: self.files.scroll,
            expanded: self.files.expanded.clone(),
            filter: self.tools.filter.clone(),
            staged_folders: self.scm.scm_expanded_staged.clone(),
            unstaged_folders: self.scm.scm_expanded_unstaged.clone(),
            scm_selected: self.scm.scm_selected,
            scm_scroll: self.scm.scroll_scm,
            activity: self.session.activity,
            focus: self.session.focus,
        };
        self.tools
            .worktree_navigation
            .insert(current_root, navigation);
        let restore = self.tools.worktree_navigation.get(&path).cloned();
        if self
            .tools
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PanelKind::Files | PanelKind::Content))
        {
            self.close_tools();
        }
        self.tools.marks.clear();
        self.tools.filter = restore
            .as_ref()
            .map(|n| n.filter.clone())
            .unwrap_or_default();
        if restore.is_none() {
            self.tools.filter.show_hidden = self.config.show_hidden_files;
        }
        self.tools.sync.clear();
        self.tools.remote_result.clear();
        self.tools.last_fetch.clear();
        self.files.expanded = restore
            .as_ref()
            .map(|n| n.expanded.clone())
            .unwrap_or_default();
        self.files.compact_expand_pending = None;
        self.files.tree_root = path;
        self.files_preview = None;
        self.files.external_anchor = restore.as_ref().and_then(|n| {
            n.selected_path
                .as_ref()
                .map(|path| (path.clone(), n.selected_scroll))
        });
        self.files.revealed_path = None;
        self.files.visible.clear();
        self.files.selected = 0;
        self.files.scroll = restore.as_ref().map(|n| n.selected_scroll).unwrap_or(0);
        self.files.follow_selection = true;
        self.scm.follow_selection = true;
        self.scm.pending_transfer = None;
        self.files.sticky_headers.clear();
        self.scm.sticky_headers.clear();
        self.scm.git_status = None;
        self.scm.branch.clear();
        // Graph/commit details from the previous Git repository are never
        // valid in a different worktree. Load history lazily again on demand.
        self.history = history::HistoryState::default();
        self.scm.scm_expanded_staged = restore
            .as_ref()
            .map(|n| n.staged_folders.clone())
            .unwrap_or_default();
        self.scm.scm_expanded_unstaged = restore
            .as_ref()
            .map(|n| n.unstaged_folders.clone())
            .unwrap_or_default();
        self.scm.scm_selected = restore.as_ref().map(|n| n.scm_selected).unwrap_or(0);
        self.scm.scroll_scm = restore.as_ref().map(|n| n.scm_scroll).unwrap_or(0);
        self.session.activity = restore
            .as_ref()
            .map(|n| n.activity)
            .unwrap_or(ActivityTab::Files);
        self.session.focus = restore.as_ref().map(|n| n.focus).unwrap_or(PaneFocus::List);
        self.request_list();
        self.request_status();
    }
}

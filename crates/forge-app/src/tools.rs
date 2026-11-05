//! Searchable workspace commands and focused dialogs. All I/O runs in jobs.
use super::*;
use forge_fs::workspace::{FileOperation, SearchOptions};
use forge_git::{GitAction, GitItem, GitQuery};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tui_input::backend::crossterm::EventHandler;

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

#[derive(Default)]
pub(super) struct ToolsState {
    pub panel: Option<Panel>,
    pub pending_read: Option<JobHandle<Result<ReadReply>>>,
    pub pending_task: Option<JobHandle<Result<String>>>,
    pub pending_sync: Option<JobHandle<Result<String>>>,
    cancel: Option<Arc<AtomicBool>>,
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
        Some(Self {
            request,
            root: app.files.tree_root.clone(),
            path: app.current_path(),
            expanded: app.files.expanded.clone(),
            unstaged: app.scm.scm_expanded_unstaged.clone(),
            staged: app.scm.scm_expanded_staged.clone(),
            scm_selected: app.scm.scm_selected,
            activity: app.session.activity,
            focus: app.session.focus,
            scroll: app.preview_state.preview_scroll,
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
    Amend,
    Worktree,
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
    Line(usize),
}
#[derive(Clone)]
pub(super) struct ToolRow {
    pub label: String,
    pub value: RowValue,
}
pub(super) struct ReadReply {
    rows: Vec<ToolRow>,
    detail: String,
}

pub(super) struct Panel {
    pub kind: PanelKind,
    pub title: String,
    pub input: tui_input::Input,
    pub rows: Vec<ToolRow>,
    pub selected: usize,
    pub scroll: usize,
    pub scroll_limit: usize,
    pub detail: String,
    pub message: String,
    pub regex: bool,
    pub area: ratatui::layout::Rect,
    pub list_area: ratatui::layout::Rect,
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
    Editor,
    EditText,
    Filters,
    All,
    Modified,
    Untracked,
    Extension,
    Hidden,
    Exclusions,
    Discard,
    Branches,
    Stashes,
    Conflicts,
    Amend,
    Sync,
    Fetch,
    Worktrees,
    AbortMerge,
    AbortCherryPick,
    AbortRevert,
    ContinueCherryPick,
    ContinueRevert,
}

const COMMANDS: &[(Command, &str)] = &[
    (
        Command::EditText,
        "Edit text in Forge                       e",
    ),
    (
        Command::FindFile,
        "Find file by fuzzy name                  Ctrl-P",
    ),
    (
        Command::FindContent,
        "Search file contents / regex             Ctrl-F",
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
    fn new(kind: PanelKind, title: &str) -> Self {
        Self {
            kind,
            title: title.into(),
            input: tui_input::Input::default(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            scroll_limit: 0,
            detail: String::new(),
            message: String::new(),
            regex: false,
            area: Default::default(),
            list_area: Default::default(),
        }
    }
    pub fn is_input(&self) -> bool {
        matches!(
            self.kind,
            PanelKind::Commands | PanelKind::Files | PanelKind::Content | PanelKind::Prompt(_)
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
                _ => None,
            }
        } else {
            self.selected_entry().map(|e| e.path.clone())
        }
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
                    self.session.status = "Select a file or folder; group headings are navigation only"
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
            let repo = self.repo_root().unwrap_or_default();
            let changes: Vec<(PathBuf, DiffSide)> = {
                let (u, s) = self.scm_slices();
                self.scm_rows()
                    .iter()
                    .filter_map(|row| {
                        scm_entry(row, u, s)
                            .map(|entry| (repo.join(&entry.path), entry.side))
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
            options.excluded = self
                .tools
                .filter
                .excluded
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect();
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
        if let Some(flag) = self.tools.cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.services.jobs.cancel_key("tools:read");
        self.tools.pending_read = None;
    }

    pub(super) fn close_tools(&mut self) {
        self.cancel_tool_read();
        self.tools.panel = None;
    }

    pub(super) fn open_tools(&mut self, kind: PanelKind) {
        self.cancel_tool_read();
        self.session.context_menu = None;
        let title = match &kind {
            PanelKind::Commands => "Commands",
            PanelKind::Files => "Find file · fuzzy name",
            PanelKind::Content => "Search contents",
            PanelKind::Filters => "File filters",
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
                GitQuery::Conflicts => "Conflicts",
                GitQuery::Worktrees => "Worktrees",
                GitQuery::Sync => "Remote synchronization",
                _ => "Git diff",
            },
            PanelKind::Lines(_) => "Diff lines · s Stage line · u Unstage line",
            PanelKind::Prompt(_) => "Input",
            PanelKind::Confirm(_) => "Confirm operation",
            PanelKind::Text => "Details",
        };
        let mut panel = Panel::new(kind, title);
        if matches!(panel.kind, PanelKind::Content) {
            panel.detail = "Type code to search · Ctrl-R toggle regex · Enter jump · results limited to 2,000, text files ≤ 2 MiB".into();
        }
        if matches!(panel.kind, PanelKind::Files) {
            panel.detail = "Type a few filename characters · Enter reveal file · e Edit text · E External editor · first 2,000 matching files".into();
        }
        self.tools.panel = Some(panel);
        self.reload_tools();
    }

    fn reload_tools(&mut self) {
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        panel.rows.clear();
        panel.selected = 0;
        panel.scroll = 0;
        let kind = panel.kind.clone();
        let query = panel.input.value().to_owned();
        let regex = panel.regex;
        if matches!(kind, PanelKind::Commands | PanelKind::Filters) {
            panel.rows = COMMANDS
                .iter()
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
                    label: (*label).into(),
                    value: RowValue::Command(*c),
                })
                .collect();
            panel.detail = format!(
                "{} selected · filters: status={} extension={} hidden={} · Enter run · Esc close",
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
        let root = self
            .repo_root()
            .unwrap_or_else(|| self.files.tree_root.clone());
        let git = self.services.git;
        let options = self.search_options(matches!(kind, PanelKind::Content), regex);
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(flag) = self.tools.cancel.replace(cancel.clone()) {
            flag.store(true, Ordering::Relaxed);
        }
        let is_search = matches!(kind, PanelKind::Files | PanelKind::Content);
        if !is_search && !matches!(kind, PanelKind::Git(_)) {
            return;
        }
        if let Some(panel) = &mut self.tools.panel {
            panel.message = "Loading…".into();
        }
        self.tools.pending_read = Some(self.services.jobs.spawn(JobPriority::High, Some("tools:read".into()), async move {
            if is_search {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let search_root=root.clone();
                let hits = tokio::task::spawn_blocking(move || forge_fs::workspace::search(&search_root, &query, &options, &cancel)).await??;
                let count = hits.len();
                Ok(ReadReply { rows: hits.into_iter().map(|h| ToolRow { label: format!("{}:{}  {}", h.path.strip_prefix(&root).unwrap_or(&h.path).display(), h.line, h.text), value: RowValue::File(h.path,h.line) }).collect(), detail: format!("{count} results · Enter jump · Ctrl-R toggle regex for contents · Esc close · limit 2,000 / 2 MiB per file") })
            } else if let PanelKind::Git(q) = kind {
                let (items,detail) = git.write.query(&root,q).await?;
                Ok(ReadReply { rows: items.into_iter().map(|item| ToolRow { label: item.label.clone(), value: RowValue::Git(item) }).collect(), detail })
            } else { unreachable!() }
        }));
    }

    fn prompt(&mut self, action: PromptAction, title: &str, value: String, detail: &str) {
        self.open_tools(PanelKind::Prompt(action));
        if let Some(panel) = &mut self.tools.panel {
            panel.title = title.into();
            panel.input = tui_input::Input::new(value);
            panel.detail = detail.into();
        }
    }

    fn confirm(&mut self, operation: Operation, detail: String, after: Option<PanelKind>) {
        self.open_tools(PanelKind::Confirm(operation));
        self.tools.after = after;
        if let Some(panel) = &mut self.tools.panel {
            panel.detail = detail;
            panel.message = "Enter confirm · Esc cancel".into();
        }
    }

    pub(super) fn run_command(&mut self, command: Command) {
        use Command::*;
        let paths = self.operation_paths();
        let current = self.current_path();
        let relative = |p: &Path| {
            p.strip_prefix(&self.files.tree_root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        };
        let parent = current
            .as_ref()
            .map(|p| {
                if p.is_dir() {
                    p.clone()
                } else {
                    p.parent().unwrap_or(p).to_owned()
                }
            })
            .unwrap_or_else(|| self.files.tree_root.clone());
        let prefix = if parent == self.files.tree_root {
            String::new()
        } else {
            format!("{}/", relative(&parent))
        };
        match command {
            FindFile => self.open_tools(PanelKind::Files), FindContent => self.open_tools(PanelKind::Content),
            NewFile => self.prompt(PromptAction::NewFile,"New file",prefix,"Path relative to workspace · existing files are never overwritten"),
            NewDirectory => self.prompt(PromptAction::NewDirectory,"New directory",prefix,"Path relative to workspace · parent must exist"),
            Rename => if let Some(path) = current { let value = relative(&path); self.prompt(PromptAction::Rename(path),"Rename",value,"New path relative to workspace · no overwrite"); },
            Copy | Move if !paths.is_empty() => {
                let n = paths.len(); let action = if matches!(command,Copy) { PromptAction::Copy(paths) } else { PromptAction::Move(paths) };
                self.prompt(action, if matches!(command,Copy) { "Copy items" } else { "Move items" },".".into(),&format!("Destination directory for {n} selected items · no overwrite"));
            }
            Delete if !paths.is_empty() => {
                let detail = format!("Permanently delete {} items and directory contents?\n{}",paths.len(),paths.iter().map(|p| relative(p)).collect::<Vec<_>>().join("\n"));
                self.confirm(Operation::Files(FileOperation::Delete(paths)),detail,None);
            }
            Mark => { self.close_tools(); self.toggle_mark(); }, MarkAll => { self.close_tools(); self.mark_all(); }, ClearMarks => { self.tools.marks.clear(); self.close_tools(); },
            Stage | Unstage => { self.close_tools(); self.batch_stage(matches!(command,Stage)); },
            Lines if self.in_git_sidebar() => {
                if !self.preview_state.preview_loading && self.preview_state.pending_diff.is_none() {
                    if let Some(diff) = self.preview_state.file_diff.clone().filter(|d| self.diff_matches_selection(d)) { self.open_tools(PanelKind::Lines(diff)); }
                    else { self.session.status = "Open a current diff first with d".into(); }
                }
            }
            FileHistory => if let (Some(path),Some(root)) = (current,self.repo_root()) {
                self.tools.pinned = None;
                if !path.is_dir() { if let Ok(path) = path.strip_prefix(root) { self.open_tools(PanelKind::Git(GitQuery::History(Some(forge_model::path_to_rela(path))))); } }
            },
            History => { self.tools.pinned = None; self.open_tools(PanelKind::Git(GitQuery::History(None))); },
            Editor => { self.close_tools(); self.open_editor(current,self.preview_state.preview_scroll+1); },
            EditText => {self.close_tools();if let Some(path)=current{self.open_text_editor(path,self.preview_state.preview_scroll+1);}},
            Filters => self.open_tools(PanelKind::Filters),
            All | Modified | Untracked | Hidden => {
                match command { All => { self.tools.filter.status = 0; self.tools.filter.extension.clear(); }, Modified => self.tools.filter.status = 1, Untracked => self.tools.filter.status = 2, Hidden => self.tools.filter.show_hidden = !self.tools.filter.show_hidden, _=>{} }
                self.rebuild_visible_preserving_selection(); self.close_tools(); self.request_preview_for_selection();
            }
            Extension => self.prompt(PromptAction::Extension,"Filter extension",self.tools.filter.extension.clone(),"Extension without wildcard, e.g. rs or ts · empty clears"),
            Exclusions => self.prompt(PromptAction::Exclusions,"Excluded directory names",if self.tools.filter.excluded.is_empty() { "target,node_modules".into() } else { self.tools.filter.excluded.clone() },"Comma-separated names · .git is always hidden · use a comma alone to clear defaults"),
            Discard => self.discard_selection(),
            Branches => self.open_tools(PanelKind::Git(GitQuery::Branches)), Stashes => self.open_tools(PanelKind::Git(GitQuery::Stashes)), Conflicts => self.open_tools(PanelKind::Git(GitQuery::Conflicts)),
            Amend => self.prompt(PromptAction::Amend,"Amend latest commit",String::new(),"New commit message · includes ALL staged changes · rewrites HEAD · confirmation follows"),
            Sync => self.open_tools(PanelKind::Git(GitQuery::Sync)), Fetch => self.start_tool_operation(Operation::Git(GitAction::Fetch),Some(PanelKind::Git(GitQuery::Sync))),
            Worktrees => self.open_tools(PanelKind::Git(GitQuery::Worktrees)),
            AbortMerge | AbortCherryPick | AbortRevert | ContinueCherryPick | ContinueRevert => {
                let action = match command { AbortMerge=>GitAction::AbortMerge, AbortCherryPick=>GitAction::AbortCherryPick, AbortRevert=>GitAction::AbortRevert, ContinueCherryPick=>GitAction::ContinueCherryPick, _=>GitAction::ContinueRevert };
                self.confirm(Operation::Git(action),"Apply the selected recovery command to this repository? Abort restores the pre-operation state.".into(),Some(PanelKind::Git(GitQuery::Conflicts)));
            }
            _ => { self.session.status = "Select files or directories first".into(); }
        }
    }

    fn submit_prompt(&mut self, action: PromptAction, value: String) {
        use PromptAction::*;
        if matches!(action, Extension | Exclusions) {
            if matches!(action, Extension) {
                self.tools.filter.extension = value.trim().trim_start_matches('.').into();
            } else {
                self.tools.filter.excluded = value;
            }
            self.close_tools();
            self.rebuild_visible_preserving_selection();
            self.request_preview_for_selection();
            return;
        }
        if value.trim().is_empty() {
            if let Some(p) = &mut self.tools.panel {
                p.message = "Value is required".into();
            }
            return;
        }
        let path = PathBuf::from(&value);
        let operation = match action {
            NewFile => Operation::Files(FileOperation::NewFile(path)),
            NewDirectory => Operation::Files(FileOperation::NewDirectory(path)),
            Rename(from) => Operation::Files(FileOperation::Rename {
                path: from,
                destination: path,
            }),
            Copy(paths) => Operation::Files(FileOperation::Copy {
                paths,
                destination: path,
            }),
            Move(paths) => Operation::Files(FileOperation::Move {
                paths,
                destination: path,
            }),
            Branch => Operation::Git(GitAction::CreateBranch(value.clone())),
            RenameBranch(old) => Operation::Git(GitAction::RenameBranch {
                old,
                new: value.clone(),
            }),
            Stash => Operation::Git(GitAction::SaveStash(value.clone())),
            Amend => Operation::Git(GitAction::Amend(value.clone())),
            Worktree => Operation::Git(GitAction::AddWorktree(if path.is_absolute() {
                path
            } else {
                self.files.tree_root.join(path)
            })),
            _ => return,
        };
        let after = match &operation {
            Operation::Git(GitAction::CreateBranch(_) | GitAction::RenameBranch { .. }) => {
                Some(PanelKind::Git(GitQuery::Branches))
            }
            Operation::Git(GitAction::SaveStash(_)) => Some(PanelKind::Git(GitQuery::Stashes)),
            Operation::Git(GitAction::AddWorktree(_)) => Some(PanelKind::Git(GitQuery::Worktrees)),
            _ => None,
        };
        if matches!(
            operation,
            Operation::Git(GitAction::Amend(_) | GitAction::SaveStash(_))
        ) {
            let detail = if matches!(operation, Operation::Git(GitAction::Amend(_))) {
                format!("Rewrite the latest commit with ALL staged changes?\nNew message: {value}\nThis changes the commit ID.")
            } else {
                format!("Save all tracked and untracked changes in a stash?\nMessage: {value}\nSaved edits will be removed from the worktree until the stash is applied.")
            };
            self.confirm(operation, detail, after);
        } else {
            self.start_tool_operation(operation, after);
        }
    }

    pub(super) fn batch_stage(&mut self, stage: bool) {
        if !self.in_git_sidebar() {
            self.session.status = "Use the Git tab (F2) for Stage / Unstage".into();
            return;
        }
        if self.tools.marks.is_empty() {
            self.request_write_stage(stage);
            return;
        }
        let side = if stage {
            DiffSide::Unstaged
        } else {
            DiffSide::Staged
        };
        let selected = self.tools.marks.git_paths(side);
        if selected.is_empty() {
            self.session.status = format!(
                "No {} selection; mark rows on the {} side or use v on the row",
                side.label(),
                if stage { "CHANGES" } else { "STAGED" }
            );
            return;
        }
        let Some(snapshot) = &self.scm.git_status else {
            self.session.status = "Refresh Git status first".into();
            return;
        };
        let source = if stage {
            &snapshot.unstaged
        } else {
            &snapshot.staged
        };
        let paths: Vec<_> = source
            .iter()
            .filter(|e| {
                selected
                    .iter()
                    .any(|p| snapshot.repo_root.join(&e.path).starts_with(p))
            })
            .map(|e| PathBuf::from(&e.path))
            .collect();
        if paths.is_empty() {
            self.session.status = format!(
                "No matching {} changes in the selected items; the other side is not included",
                side.label()
            );
            return;
        }
        self.start_tool_operation(Operation::Stage { paths, stage }, None);
    }

    fn discard_selection(&mut self) {
        if self.session.focus == PaneFocus::Preview {
            if let Some(diff) = self.preview_state.file_diff.clone().filter(|d| {
                d.side == DiffSide::Unstaged
                    && self.diff_matches_selection(d)
                    && self.preview_state.pending_diff.is_none()
                    && !self.preview_state.preview_loading
            }) {
                if let Some(hunk) = self
                    .preview_state
                    .hunk_index
                    .and_then(|i| diff.hunks().get(i).cloned())
                {
                    let detail = format!(
                        "Discard this hunk in {}? Staged changes are preserved.\n\n{}",
                        diff.path,
                        diff.hunk_patch(&hunk)
                    );
                    self.confirm(Operation::DiscardHunk { diff, hunk }, detail, None);
                    return;
                }
            }
            self.session.status = "Select an unstaged hunk first".into();
            return;
        }
        let selected = self.operation_paths();
        let Some(snapshot) = &self.scm.git_status else {
            return;
        };
        let paths: Vec<_> = snapshot
            .unstaged
            .iter()
            .filter(|e| {
                !matches!(
                    e.status,
                    forge_git::GitStatus::Untracked | forge_git::GitStatus::Conflicted
                ) && selected
                    .iter()
                    .any(|p| snapshot.repo_root.join(&e.path).starts_with(p))
            })
            .map(|e| e.path.clone())
            .collect();
        if paths.is_empty() {
            self.session.status =
                "No tracked unstaged changes selected; use Delete for untracked files".into();
            return;
        }
        self.confirm(
            Operation::Git(GitAction::Discard(paths.clone())),
            format!(
                "Discard worktree changes in {} files? Index is preserved.\n{}",
                paths.len(),
                paths.join("\n")
            ),
            None,
        );
    }

    fn start_tool_operation(&mut self, operation: Operation, after: Option<PanelKind>) {
        if self.write_busy() {
            if let Some(p) = &mut self.tools.panel {
                p.message = self.session.status.clone();
            }
            return;
        }
        self.cancel_tool_read();
        let root = self.files.tree_root.clone();
        let repo = self.repo_root().unwrap_or_else(|| root.clone());
        let git = self.services.git;
        let fetch = matches!(operation, Operation::Git(GitAction::Fetch));
        self.tools.after = after;
        self.tools.pending_task = Some(self.services.jobs.spawn(
            JobPriority::High,
            Some("tools:write".into()),
            async move {
                let message = match operation {
                    Operation::Files(op) => {
                        tokio::task::spawn_blocking(move || forge_fs::workspace::operate(&root, op))
                            .await??
                    }
                    Operation::Git(action) => git.write.action(&repo, action).await?,
                    Operation::Stage { paths, stage } => {
                        if stage {
                            git.stage_paths(&repo, &paths).await?;
                        } else {
                            git.unstage_paths(&repo, &paths).await?;
                        }
                        format!(
                            "{} {} files",
                            if stage { "Staged" } else { "Unstaged" },
                            paths.len()
                        )
                    }
                    Operation::Line { diff, index } => {
                        git.write.apply_line(&repo, &diff, index).await?;
                        format!(
                            "{} line {} in {}",
                            if diff.side == DiffSide::Unstaged {
                                "Staged"
                            } else {
                                "Unstaged"
                            },
                            index + 1,
                            diff.path
                        )
                    }
                    Operation::DiscardHunk { diff, hunk } => {
                        git.write.discard_hunk(&repo, &diff, &hunk).await?;
                        format!("Discarded hunk in {}", diff.path)
                    }
                };
                Ok(if fetch {
                    format!("Fetch completed: {message}")
                } else if message.is_empty() {
                    "Operation completed".into()
                } else {
                    message
                })
            },
        ));
        let mut panel = Panel::new(PanelKind::Text, "Working");
        panel.message = "Operation in progress… · Esc closes this view; operation continues".into();
        self.tools.panel = Some(panel);
    }

    pub(super) fn poll_tools(&mut self) {
        if let Some(result) = self
            .tools
            .pending_read
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.tools.pending_read = None;
            if let Some(panel) = &mut self.tools.panel {
                match result {
                    Ok(Ok(reply)) => {
                        panel.rows = reply.rows;
                        panel.detail = reply.detail;
                        panel.message = if panel.rows.is_empty() {
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
        if let Some(result) = self
            .tools
            .pending_task
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.tools.pending_task = None;
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
                        p.detail=format!("{message}\n\nUse Conflicts for conflicts; recovery commands are in : Commands.\nEsc closes this view.");
                    }
                    self.request_status();
                }
                _ => {}
            }
        }
        if let Some(result) = self
            .tools
            .pending_sync
            .as_mut()
            .and_then(JobHandle::try_join)
        {
            self.tools.pending_sync = None;
            if let Ok(Ok(summary)) = result {
                self.tools.sync = summary;
            }
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

    pub(super) fn handle_tools_key(&mut self, key: KeyEvent) {
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        if key.code == KeyCode::Esc {
            self.close_tools();
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.close_tools();
            return;
        }
        if key.code == KeyCode::Char('r')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(panel.kind, PanelKind::Content)
        {
            panel.regex = !panel.regex;
            panel.title = format!(
                "Search contents · {}",
                if panel.regex { "regex" } else { "literal" }
            );
            self.reload_tools();
            return;
        }
        let input_mode = panel.is_input();
        if matches!(key.code, KeyCode::Up | KeyCode::Down)
            || (!input_mode && matches!(key.code, KeyCode::Char('j') | KeyCode::Char('k')))
        {
            let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
                -1
            } else {
                1
            };
            panel.selected = (panel.selected as isize + delta)
                .clamp(0, panel.rows.len().saturating_sub(1) as isize)
                as usize;
            return;
        }
        if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
            if key.code == KeyCode::PageDown {
                panel.scroll = panel.scroll.saturating_add(10).min(panel.scroll_limit);
            } else {
                panel.scroll = panel.scroll.saturating_sub(10);
            }
            return;
        }
        let kind = panel.kind.clone();
        let selected = panel.rows.get(panel.selected).map(|r| r.value.clone());
        if key.code == KeyCode::Enter {
            match kind {
                PanelKind::Prompt(action) => {
                    let value = panel.input.value().to_owned();
                    self.submit_prompt(action, value);
                }
                PanelKind::Confirm(operation) => {
                    let after = self.tools.after.take();
                    self.start_tool_operation(operation, after);
                }
                _ => {
                    if let Some(value) = selected {
                        self.activate_tool_row(kind, value);
                    }
                }
            }
            return;
        }
        if input_mode {
            let old = panel.input.value().to_owned();
            panel.input.handle_event(&crossterm::event::Event::Key(key));
            if panel.input.value() != old && !matches!(kind, PanelKind::Prompt(_)) {
                self.reload_tools();
            }
            return;
        }
        self.tool_action_key(kind, selected, key.code);
    }

    fn activate_tool_row(&mut self, kind: PanelKind, value: RowValue) {
        match value {
            RowValue::Command(c) => self.run_command(c),
            RowValue::File(path, line) => {
                self.close_tools();
                self.reveal_file(path, line);
            }
            RowValue::Line(_) => {}
            RowValue::Git(item) => match kind {
                PanelKind::Git(GitQuery::History(_) | GitQuery::HistoryPage { .. }) => {
                    self.open_tools(PanelKind::Git(GitQuery::Commit(item.id)))
                }
                PanelKind::Git(GitQuery::Commit(oid)) => {
                    if let Some(path) = item.path {
                        self.open_tools(PanelKind::Git(GitQuery::CommitFile { oid, path }));
                    }
                }
                PanelKind::Git(GitQuery::Branches) => self.confirm(
                    Operation::Git(GitAction::SwitchBranch(item.id.clone())),
                    format!("Switch this repository to branch {}?", item.id),
                    Some(PanelKind::Git(GitQuery::Branches)),
                ),
                PanelKind::Git(GitQuery::Stashes) => {
                    self.open_tools(PanelKind::Git(GitQuery::StashDiff(item.id)))
                }
                PanelKind::Git(GitQuery::Conflicts) => {
                    if let Some(path) = item.path {
                        if let Some(root) = self.repo_root() {
                            self.close_tools();
                            self.reveal_file(root.join(path), 1);
                        }
                    }
                }
                PanelKind::Git(GitQuery::Worktrees) => {
                    self.close_tools();
                    self.switch_workspace(PathBuf::from(item.id));
                }
                _ => {}
            },
        }
    }

    fn tool_action_key(&mut self, kind: PanelKind, selected: Option<RowValue>, key: KeyCode) {
        if key == KeyCode::Char(':') {
            self.open_tools(PanelKind::Commands);
            return;
        }
        if matches!(key, KeyCode::Char('[') | KeyCode::Char(']')) {
            let history = match &kind {
                PanelKind::Git(GitQuery::History(path)) => Some((path.clone(), 0)),
                PanelKind::Git(GitQuery::HistoryPage { path, skip }) => Some((path.clone(), *skip)),
                _ => None,
            };
            if let Some((path, skip)) = history {
                self.open_tools(PanelKind::Git(GitQuery::HistoryPage {
                    path,
                    skip: if key == KeyCode::Char(']') {
                        skip + 200
                    } else {
                        skip.saturating_sub(200)
                    },
                }));
                return;
            }
        }
        let kind = match kind {
            PanelKind::Git(GitQuery::HistoryPage { path, .. }) => {
                PanelKind::Git(GitQuery::History(path))
            }
            other => other,
        };
        match (&kind, key) {
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Branch,
                    "Create branch",
                    String::new(),
                    "Creates a branch at HEAD; switch using Enter",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Stash,
                    "Save stash",
                    String::new(),
                    "Stash message · includes untracked files · confirmation follows",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Worktrees), KeyCode::Char('A')) => {
                self.prompt(
                    PromptAction::Worktree,
                    "Create worktree",
                    String::new(),
                    "Path · Git creates a branch named after the directory",
                );
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('F')) => {
                self.run_command(Command::Fetch);
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('p')) => {
                self.close_tools();
                self.request_pull();
                return;
            }
            (PanelKind::Git(GitQuery::Sync), KeyCode::Char('P')) => {
                self.close_tools();
                self.request_push();
                return;
            }
            _ => {}
        }
        let Some(selected) = selected else {
            return;
        };
        if let (PanelKind::Lines(diff), RowValue::Line(index)) = (&kind, &selected) {
            let stage = diff.side == DiffSide::Unstaged;
            if key == KeyCode::Char(if stage { 's' } else { 'u' }) {
                self.start_tool_operation(
                    Operation::Line {
                        diff: diff.clone(),
                        index: *index,
                    },
                    None,
                );
            }
            return;
        }
        let RowValue::Git(item) = selected else {
            return;
        };
        let after = Some(kind.clone());
        match (kind, key) {
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char(' ')) => {
                self.tools.pinned = Some(item.clone());
                if let Some(p) = &mut self.tools.panel {
                    p.message = format!("Pinned {} · select second version, then D", item.label);
                }
            }
            (PanelKind::Git(GitQuery::History(path)), KeyCode::Char('D')) => {
                if let Some(old) = self.tools.pinned.clone() {
                    self.open_tools(PanelKind::Git(GitQuery::Compare {
                        old: old.id,
                        new: item.id,
                        old_path: if path.is_some() { old.path } else { None },
                        new_path: if path.is_some() { item.path } else { None },
                    }));
                } else if let Some(p) = &mut self.tools.panel {
                    p.message = "Pin the first version with Space".into();
                }
            }
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char('C')) => self.confirm(
                Operation::Git(GitAction::CherryPick(item.id.clone())),
                format!(
                    "Cherry-pick {} into this branch? Conflicts may require resolution.",
                    item.label
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::History(_)), KeyCode::Char('R')) => self.confirm(
                Operation::Git(GitAction::Revert(item.id.clone())),
                format!("Create a commit reverting {}?", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('r')) => self.prompt(
                PromptAction::RenameBranch(item.id.clone()),
                "Rename branch",
                item.id,
                "New branch name",
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('X')) => self.confirm(
                Operation::Git(GitAction::DeleteBranch(item.id.clone())),
                format!(
                    "Delete branch {}? Git refuses unmerged or checked-out branches.",
                    item.id
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::Branches), KeyCode::Char('D')) => {
                self.open_tools(PanelKind::Git(GitQuery::BranchDiff(item.id)))
            }
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('a')) => self.confirm(
                Operation::Git(GitAction::ApplyStash(item.id.clone())),
                format!("Apply {}? The stash is kept.", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Stashes), KeyCode::Char('X')) => self.confirm(
                Operation::Git(GitAction::DropStash(item.id.clone())),
                format!("Permanently drop {}?", item.label),
                after,
            ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('o') | KeyCode::Char('t')) => self
                .confirm(
                    Operation::Git(GitAction::Resolve {
                        paths: vec![item.id.clone()],
                        ours: key == KeyCode::Char('o'),
                    }),
                    format!(
                        "Replace {} with {} and stage the resolution?",
                        item.id,
                        if key == KeyCode::Char('o') {
                            "ours"
                        } else {
                            "theirs"
                        }
                    ),
                    after,
                ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('s')) => self.confirm(
                Operation::Git(GitAction::MarkResolved(vec![item.id.clone()])),
                format!(
                    "Stage {} as resolved? Verify manual edits and remove conflict markers first.",
                    item.id
                ),
                after,
            ),
            (PanelKind::Git(GitQuery::Conflicts), KeyCode::Char('e')) => {
                if let Some(root) = self.repo_root() {
                    self.close_tools();
                    self.open_editor(Some(root.join(item.id)), 1);
                }
            }
            (PanelKind::Git(GitQuery::Worktrees), KeyCode::Char('X')) => {
                let path = PathBuf::from(&item.id);
                if self.files.tree_root.starts_with(&path) {
                    if let Some(p) = &mut self.tools.panel {
                        p.message = "Switch away before removing the current worktree".into();
                    }
                } else {
                    self.confirm(
                        Operation::Git(GitAction::RemoveWorktree(path)),
                        format!(
                            "Remove worktree {}? Git refuses dirty or locked worktrees.",
                            item.id
                        ),
                        after,
                    );
                }
            }
            _ => {}
        }
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

    fn reveal_file(&mut self, path: PathBuf, line: usize) {
        self.session.activity = ActivityTab::Files;
        self.session.focus = PaneFocus::List;
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
        let Some((path, _)) = &self.tools.reveal else {
            return;
        };
        if let Some(i) = self
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == *path)
        {
            if self
                .preview_state
                .pending_preview
                .as_ref()
                .is_none_or(|p| p.path != *path)
                && (self.files.selected != i
                    || self
                        .preview_state
                        .preview
                        .as_ref()
                        .is_none_or(|p| p.path != *path))
            {
                self.files.selected = i;
                self.request_preview_for_selection();
            }
        }
    }

    pub(super) fn handle_tools_mouse(&mut self, mouse: MouseEvent) {
        let Some(panel) = &mut self.tools.panel else {
            return;
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if panel.rows.is_empty() {
                    panel.scroll = panel.scroll.saturating_add(3).min(panel.scroll_limit);
                } else {
                    panel.selected = (panel.selected + 1).min(panel.rows.len().saturating_sub(1));
                }
            }
            MouseEventKind::ScrollUp => {
                if panel.rows.is_empty() {
                    panel.scroll = panel.scroll.saturating_sub(3);
                } else {
                    panel.selected = panel.selected.saturating_sub(1);
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if panel
                    .list_area
                    .contains(ratatui::layout::Position::new(mouse.column, mouse.row)) =>
            {
                let index = (mouse.row - panel.list_area.y) as usize
                    + panel
                        .selected
                        .saturating_sub(panel.list_area.height.saturating_sub(1) as usize);
                if index < panel.rows.len() {
                    panel.selected = index;
                }
            }
            _ => {}
        }
    }

    pub(super) fn switch_workspace(&mut self, path: PathBuf) {
        if self.write_busy() {
            return;
        }
        self.tools.marks.clear();
        self.tools.filter = FileFilter::default();
        self.tools.sync.clear();
        self.tools.remote_result.clear();
        self.tools.last_fetch.clear();
        self.files.expanded.clear();
        self.files.tree_root = path;
        self.files.selected = 0;
        self.files.scroll = 0;
        self.scm.git_status = None;
        self.scm.branch.clear();
        self.scm.scm_expanded_staged.clear();
        self.scm.scm_expanded_unstaged.clear();
        self.scm.scm_selected = 0;
        self.session.activity = ActivityTab::Files;
        self.session.focus = PaneFocus::List;
        self.request_list();
        self.request_status();
    }
}

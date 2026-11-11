//! VS Code–style Source Control sidebar (sectioned directory tree).
//!
//! Drawn into the left sidebar when `ActivityTab::Git` is active.
//! Changes are grouped by path segments with expandable folders, and a
//! collapsible HISTORY accordion (lane graph + per-commit file tree) follows
//! them.

use std::collections::{BTreeMap, HashMap, HashSet};

use forge_model::{ChangeEntry, CommitChange, DiffSide, GitStatus, GraphCommit};
use forge_theme::Theme;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::file_list::ensure_selected_visible;
use crate::graph::{
    change_color, flatten_changes, lane_color, ChangeRow, GraphCell, GraphLayout, LANE_WIDTH,
};
use crate::icons::{icon_mode, IconMode, Icons};
use crate::sticky;
use crate::style::{self, rgb};
use crate::tree::fit_compact_label;

/// Which section a change belongs to (unstaged vs staged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StagingColumn {
    #[default]
    Unstaged,
    Staged,
}

impl StagingColumn {
    pub fn side(self) -> DiffSide {
        match self {
            StagingColumn::Unstaged => DiffSide::Unstaged,
            StagingColumn::Staged => DiffSide::Staged,
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            StagingColumn::Unstaged => StagingColumn::Staged,
            StagingColumn::Staged => StagingColumn::Unstaged,
        }
    }
}

/// One row in the SCM sidebar list (section / folder / file / history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScmRow {
    Section {
        title: &'static str,
        count: usize,
        column: StagingColumn,
    },
    /// Expandable directory grouping under CHANGES / STAGED.
    Folder {
        column: StagingColumn,
        /// Repo-relative dir path (`/` separators, no trailing slash).
        path: String,
        /// Last path segment (display name).
        name: String,
        depth: u16,
        expanded: bool,
        /// Number of change files under this folder (recursive).
        file_count: usize,
    },
    Change {
        column: StagingColumn,
        /// Index into the unstaged / staged slice.
        index: usize,
        depth: u16,
    },
    /// Collapsible HISTORY header holding the commit graph.
    HistorySection {
        count: usize,
        expanded: bool,
        loading: bool,
    },
    /// One commit row; `index` selects the commit.
    Commit { index: usize },
    /// Directory grouping inside one commit's changed files.
    CommitFolder {
        /// Index of the commit this folder belongs to.
        commit: usize,
        path: String,
        name: String,
        depth: u16,
        expanded: bool,
        file_count: usize,
    },
    /// One file changed by a commit; `index` selects its [`CommitChange`].
    CommitFile {
        commit: usize,
        index: usize,
        depth: u16,
    },
}

impl ScmRow {
    /// Section a staging row belongs to; `None` for history rows.
    pub fn column(&self) -> Option<StagingColumn> {
        match self {
            ScmRow::Section { column, .. }
            | ScmRow::Folder { column, .. }
            | ScmRow::Change { column, .. } => Some(*column),
            ScmRow::HistorySection { .. }
            | ScmRow::Commit { .. }
            | ScmRow::CommitFolder { .. }
            | ScmRow::CommitFile { .. } => None,
        }
    }

    pub fn is_change(&self) -> bool {
        matches!(self, ScmRow::Change { .. })
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, ScmRow::Folder { .. })
    }

    pub fn is_commit(&self) -> bool {
        matches!(self, ScmRow::Commit { .. })
    }

    pub fn is_commit_file(&self) -> bool {
        matches!(self, ScmRow::CommitFile { .. })
    }

    pub fn depth(&self) -> u16 {
        match self {
            ScmRow::Section { .. } | ScmRow::HistorySection { .. } | ScmRow::Commit { .. } => 0,
            ScmRow::Folder { depth, .. }
            | ScmRow::Change { depth, .. }
            | ScmRow::CommitFolder { depth, .. }
            | ScmRow::CommitFile { depth, .. } => *depth,
        }
    }

    /// Index of the commit for history rows.
    pub fn commit(&self) -> Option<usize> {
        match self {
            ScmRow::Commit { index } => Some(*index),
            ScmRow::CommitFolder { commit, .. } | ScmRow::CommitFile { commit, .. } => {
                Some(*commit)
            }
            _ => None,
        }
    }
}

#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    /// Indices into the ChangeEntry slice for files directly in this dir.
    files: Vec<usize>,
}

impl DirNode {
    fn file_count(&self) -> usize {
        self.files.len() + self.dirs.values().map(DirNode::file_count).sum::<usize>()
    }
}

fn insert_entry(root: &mut DirNode, path: &str, index: usize) {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        root.files.push(index);
        return;
    }
    let mut node = root;
    for seg in &parts[..parts.len() - 1] {
        node = node.dirs.entry((*seg).to_string()).or_default();
    }
    node.files.push(index);
}

fn build_dir_tree(entries: &[ChangeEntry]) -> DirNode {
    let mut root = DirNode::default();
    for (i, e) in entries.iter().enumerate() {
        insert_entry(&mut root, &e.path, i);
    }
    // Keep files in path order (entries are usually sorted; stabilize by index).
    root.files.sort_unstable();
    sort_files_recursive(&mut root);
    root
}

fn sort_files_recursive(node: &mut DirNode) {
    node.files.sort_unstable();
    for child in node.dirs.values_mut() {
        sort_files_recursive(child);
    }
}

fn flatten_dir(
    node: &DirNode,
    prefix: &str,
    depth: u16,
    column: StagingColumn,
    expanded: &HashSet<String>,
    out: &mut Vec<ScmRow>,
) {
    for (name, child) in &node.dirs {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let file_count = child.file_count();
        let is_exp = expanded.contains(&path);
        out.push(ScmRow::Folder {
            column,
            path: path.clone(),
            name: name.clone(),
            depth,
            expanded: is_exp,
            file_count,
        });
        if is_exp {
            flatten_dir(child, &path, depth.saturating_add(1), column, expanded, out);
        }
    }
    for &index in &node.files {
        out.push(ScmRow::Change {
            column,
            index,
            depth,
        });
    }
}

/// Merge visible unbranched directory chains in the Git tree.
/// The final row keeps the deepest real directory path, expansion state,
/// and unchanged change/commit identity. File counts prevent swallowing
/// directories with sibling changes. All descendants get adjusted depth.
pub fn compact_scm_rows(rows: &[ScmRow]) -> Vec<ScmRow> {
    let mut result = Vec::with_capacity(rows.len());
    let mut active: Vec<(u16, u16)> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let depth = rows[i].depth();
        while active.last().is_some_and(|(bottom, _)| depth <= *bottom) {
            active.pop();
        }
        let removed = active.iter().map(|(_, n)| *n as u32).sum::<u32>() as u16;
        let mut last = i;
        let mut label = match &rows[i] {
            ScmRow::Folder { name, .. } | ScmRow::CommitFolder { name, .. } => Some(name.clone()),
            _ => None,
        };
        while let (Some(next), Some(label)) = (rows.get(last + 1), label.as_mut()) {
            let same_chain = match (&rows[last], next) {
                (
                    ScmRow::Folder {
                        column: a,
                        path,
                        expanded: true,
                        file_count: total,
                        depth,
                        ..
                    },
                    ScmRow::Folder {
                        column: b,
                        path: child,
                        file_count: count,
                        depth: next_depth,
                        name,
                        ..
                    },
                ) => {
                    let direct = child
                        .strip_prefix(path.as_str())
                        .is_some_and(|rest| rest.starts_with('/') && !rest[1..].contains('/'));
                    if a == b
                        && total == count
                        && *total > 0
                        && *next_depth == depth.saturating_add(1)
                        && direct
                    {
                        label.push_str(" / ");
                        label.push_str(name);
                        true
                    } else {
                        false
                    }
                }
                (
                    ScmRow::CommitFolder {
                        commit: a,
                        path,
                        expanded: true,
                        file_count: total,
                        depth,
                        ..
                    },
                    ScmRow::CommitFolder {
                        commit: b,
                        path: child,
                        file_count: count,
                        depth: next_depth,
                        name,
                        ..
                    },
                ) => {
                    let direct = child
                        .strip_prefix(path.as_str())
                        .is_some_and(|rest| rest.starts_with('/') && !rest[1..].contains('/'));
                    if a == b
                        && total == count
                        && *total > 0
                        && *next_depth == depth.saturating_add(1)
                        && direct
                    {
                        label.push_str(" / ");
                        label.push_str(name);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if !same_chain {
                break;
            }
            last += 1;
        }
        let mut visible = rows[last].clone();
        let visual_depth = depth.saturating_sub(removed);
        match &mut visible {
            ScmRow::Folder { depth, name, .. } | ScmRow::CommitFolder { depth, name, .. } => {
                *depth = visual_depth;
                if last > i {
                    *name = label.expect("compact folder label");
                }
            }
            ScmRow::Change { depth, .. } | ScmRow::CommitFile { depth, .. } => {
                *depth = visual_depth
            }
            _ => {}
        }
        if last > i {
            active.push((rows[last].depth(), rows[last].depth() - depth));
        }
        result.push(visible);
        i = last + 1;
    }
    result
}

/// Build the SCM list: CHANGES tree + STAGED tree + the HISTORY accordion.
pub fn build_scm_rows(
    unstaged: &[ChangeEntry],
    staged: &[ChangeEntry],
    expanded_unstaged: &HashSet<String>,
    expanded_staged: &HashSet<String>,
    history: &HistoryView<'_>,
) -> Vec<ScmRow> {
    let mut rows = Vec::with_capacity(3 + unstaged.len() + staged.len() + history.commits.len());
    rows.push(ScmRow::Section {
        title: "CHANGES",
        count: unstaged.len(),
        column: StagingColumn::Unstaged,
    });
    let u_tree = build_dir_tree(unstaged);
    flatten_dir(
        &u_tree,
        "",
        0,
        StagingColumn::Unstaged,
        expanded_unstaged,
        &mut rows,
    );
    rows.push(ScmRow::Section {
        title: "STAGED",
        count: staged.len(),
        column: StagingColumn::Staged,
    });
    let s_tree = build_dir_tree(staged);
    flatten_dir(
        &s_tree,
        "",
        0,
        StagingColumn::Staged,
        expanded_staged,
        &mut rows,
    );
    push_history_rows(&mut rows, history);
    rows
}

/// Commit graph data for the HISTORY accordion.
#[derive(Debug)]
pub struct HistoryView<'a> {
    /// Whether the section itself is open.
    pub expanded: bool,
    /// A commit-graph load is in flight.
    pub loading: bool,
    /// Commits in display order (children before parents).
    pub commits: &'a [GraphCommit],
    /// Lane glyphs, one row per commit.
    pub layout: &'a GraphLayout,
    /// Commits whose file list is open.
    pub expanded_commits: &'a HashSet<String>,
    /// Directories opened inside a commit, keyed by [`dir_key`].
    pub expanded_dirs: &'a HashSet<String>,
    /// Changed files per commit, loaded on first expansion.
    pub files: &'a HashMap<String, Vec<CommitChange>>,
}

/// Append the HISTORY section and, when open, each commit plus the file tree of
/// expanded commits.
fn push_history_rows(rows: &mut Vec<ScmRow>, history: &HistoryView<'_>) {
    rows.push(ScmRow::HistorySection {
        count: history.commits.len(),
        expanded: history.expanded,
        loading: history.loading,
    });
    if !history.expanded {
        return;
    }
    for (index, commit) in history.commits.iter().enumerate() {
        rows.push(ScmRow::Commit { index });
        if !history.expanded_commits.contains(&commit.id) {
            continue;
        }
        let Some(changes) = history.files.get(&commit.id) else {
            // Still loading; the commit row shows the hint itself.
            continue;
        };
        for row in flatten_changes(&commit.id, changes, history.expanded_dirs, 1) {
            rows.push(match row {
                ChangeRow::Folder {
                    path,
                    name,
                    depth,
                    expanded,
                    files,
                } => ScmRow::CommitFolder {
                    commit: index,
                    path,
                    name,
                    depth,
                    expanded,
                    file_count: files,
                },
                ChangeRow::File { index: file, depth } => ScmRow::CommitFile {
                    commit: index,
                    index: file,
                    depth,
                },
            });
        }
    }
}

/// Expand every ancestor directory of `rela_path` so the file row is visible.
pub fn expand_ancestors_for_path(expanded: &mut HashSet<String>, rela_path: &str) {
    let parts: Vec<&str> = rela_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return;
    }
    let mut acc = String::new();
    for seg in &parts[..parts.len() - 1] {
        if acc.is_empty() {
            acc.push_str(seg);
        } else {
            acc.push('/');
            acc.push_str(seg);
        }
        expanded.insert(acc.clone());
    }
}

/// Resolve a SCM row to a change entry, if any.
pub fn scm_entry<'a>(
    row: &ScmRow,
    unstaged: &'a [ChangeEntry],
    staged: &'a [ChangeEntry],
) -> Option<&'a ChangeEntry> {
    match row {
        ScmRow::Change {
            column: StagingColumn::Unstaged,
            index,
            ..
        } => unstaged.get(*index),
        ScmRow::Change {
            column: StagingColumn::Staged,
            index,
            ..
        } => staged.get(*index),
        _ => None,
    }
}

/// First selectable change row in a column, or the section header if empty.
pub fn scm_jump_to_column(rows: &[ScmRow], column: StagingColumn) -> usize {
    rows.iter()
        .position(|r| matches!(r, ScmRow::Change { column: c, .. } if *c == column))
        .or_else(|| {
            rows.iter()
                .position(|r| matches!(r, ScmRow::Section { column: c, .. } if *c == column))
        })
        .unwrap_or(0)
}

/// Index of the nearest ancestor folder/section at `depth - 1`.
pub fn find_scm_parent_row(rows: &[ScmRow], index: usize) -> Option<usize> {
    let row = rows.get(index)?;
    let depth = row.depth();
    if depth == 0 {
        // History rows climb back to the HISTORY header.
        if row.commit().is_some() {
            return (0..index)
                .rev()
                .find(|&i| matches!(&rows[i], ScmRow::HistorySection { .. }));
        }
        // Walk up to the section header for this column.
        let col = row.column()?;
        return (0..index)
            .rev()
            .find(|&i| matches!(&rows[i], ScmRow::Section { column: c, .. } if *c == col));
    }
    let target = depth - 1;
    if let Some(commit) = row.commit() {
        return (0..index).rev().find(|&i| {
            rows[i].commit() == Some(commit)
                && rows[i].depth() == target
                && !matches!(rows[i], ScmRow::CommitFile { .. })
        });
    }
    (0..index).rev().find(|&i| {
        matches!(&rows[i], ScmRow::Folder { depth: d, .. } if *d == target)
            || (target == 0 && matches!(&rows[i], ScmRow::Section { .. }))
    })
}

/// Hit regions for the SCM sidebar (occupies the left pane in Git mode).
#[derive(Debug, Clone, Copy, Default)]
pub struct StagingLayout {
    pub area: Rect,
    /// Scrollable list body (below the SOURCE CONTROL chip).
    pub inner: Rect,
    /// Always false for the sidebar path; kept for older call sites.
    pub collapsed: bool,
    // Legacy dual-column rects — empty in sidebar mode.
    pub unstaged: Rect,
    pub unstaged_inner: Rect,
    pub staged: Rect,
    pub staged_inner: Rect,
}

impl StagingLayout {
    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.area.contains(Position { x: column, y: row })
    }

    /// Legacy dual-column hit test — unused in sidebar mode (always `None`).
    pub fn hit_column(&self, _column: u16, _row: u16) -> Option<StagingColumn> {
        None
    }
}

/// Borrowed SCM sidebar view model.
#[derive(Debug)]
pub struct StagingState<'a> {
    pub unstaged: &'a [ChangeEntry],
    pub staged: &'a [ChangeEntry],
    pub expanded_unstaged: &'a HashSet<String>,
    pub expanded_staged: &'a HashSet<String>,
    /// Commit graph accordion below the staging trees.
    pub history: &'a HistoryView<'a>,
    /// Flat index into [`build_scm_rows`].
    pub selected: usize,
    pub scroll: &'a mut usize,
    pub focused: bool,
    pub sticky_scroll: bool,
    pub compact_folders: bool,
    pub follow_selection: bool,
    pub sticky_headers: &'a mut Vec<usize>,
}

/// Draw the VS Code–style SCM sidebar into `area` (full left pane height).
pub fn draw_scm_sidebar(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &mut StagingState<'_>,
) -> StagingLayout {
    if area.width == 0 || area.height == 0 {
        return StagingLayout::default();
    }

    let body = area;

    let raw = build_scm_rows(
        state.unstaged,
        state.staged,
        state.expanded_unstaged,
        state.expanded_staged,
        state.history,
    );
    let rows = if state.compact_folders {
        compact_scm_rows(&raw)
    } else {
        raw
    };
    let count = rows.len();
    let selected = state.selected.min(count.saturating_sub(1));

    style::fill_panel(frame, body, theme);
    let viewport = body.height as usize;
    let headers = if state.sticky_scroll {
        sticky::layout(
            count,
            selected,
            state.scroll,
            viewport,
            |i| find_scm_parent_row(&rows, i),
            state.follow_selection,
        )
    } else {
        if state.follow_selection {
            ensure_selected_visible(selected, state.scroll, viewport, count);
        } else {
            *state.scroll = (*state.scroll).min(count.saturating_sub(viewport));
        }
        Vec::new()
    };
    *state.sticky_headers = headers.clone();
    let start = *state.scroll;
    let end = (start + viewport.saturating_sub(headers.len())).min(count);
    let focused = state.focused;
    let items: Vec<ListItem<'_>> = headers
        .iter()
        .copied()
        .chain(start..end)
        .map(|index| {
            let scm = &rows[index];
            let is_sel = index == selected;
            let pinned = headers.contains(&index);
            match scm {
                ScmRow::Section { title, count, .. } => {
                    section_item(theme, title, *count, is_sel, focused, pinned)
                }
                ScmRow::Folder {
                    name,
                    depth,
                    expanded,
                    file_count,
                    ..
                } => folder_item(
                    theme,
                    name,
                    *depth,
                    *expanded,
                    *file_count,
                    is_sel,
                    focused,
                    body.width,
                    pinned,
                ),
                ScmRow::Change {
                    column,
                    index: ei,
                    depth,
                } => {
                    let entry = match column {
                        StagingColumn::Unstaged => state.unstaged.get(*ei),
                        StagingColumn::Staged => state.staged.get(*ei),
                    };
                    match entry {
                        Some(e) => change_item(theme, e, *depth, is_sel, focused, body.width),
                        None => ListItem::new(Line::from("")),
                    }
                }
                ScmRow::HistorySection {
                    count,
                    expanded,
                    loading,
                } => history_section_item(
                    theme, *count, *expanded, *loading, is_sel, focused, pinned,
                ),
                ScmRow::Commit { index } => {
                    let Some(commit) = state.history.commits.get(*index) else {
                        return ListItem::new(Line::from(""));
                    };
                    let glyphs = state.history.layout.row(*index);
                    let expanded = state.history.expanded_commits.contains(&commit.id);
                    let pending = expanded && !state.history.files.contains_key(&commit.id);
                    commit_item(
                        theme, commit, glyphs, pending, is_sel, focused, body.width, pinned,
                    )
                }
                ScmRow::CommitFolder {
                    commit,
                    name,
                    depth,
                    expanded,
                    file_count,
                    ..
                } => {
                    let visual_depth = history_child_visual_depth(
                        *depth,
                        state.history.layout.row(*commit),
                        body.width,
                    );
                    folder_item(
                        theme,
                        name,
                        visual_depth,
                        *expanded,
                        *file_count,
                        is_sel,
                        focused,
                        body.width,
                        pinned,
                    )
                }
                ScmRow::CommitFile {
                    commit,
                    index: fi,
                    depth,
                } => {
                    let change = state
                        .history
                        .commits
                        .get(*commit)
                        .and_then(|c| state.history.files.get(&c.id))
                        .and_then(|changes| changes.get(*fi));
                    match change {
                        Some(change) => {
                            let visual_depth = history_child_visual_depth(
                                *depth,
                                state.history.layout.row(*commit),
                                body.width,
                            );
                            commit_file_item(
                                theme,
                                change,
                                visual_depth,
                                is_sel,
                                focused,
                                body.width,
                            )
                        }
                        None => ListItem::new(Line::from("")),
                    }
                }
            }
        })
        .collect();

    let list = List::new(items).style(Style::default().bg(rgb(theme.panel_bg)));
    frame.render_widget(list, body);
    sticky::paint(frame, body, theme, &headers, selected);

    StagingLayout {
        area,
        inner: body,
        collapsed: false,
        unstaged: Rect::default(),
        unstaged_inner: Rect::default(),
        staged: Rect::default(),
        staged_inner: Rect::default(),
    }
}

/// Alias — historical name; draws the sidebar SCM list.
pub fn draw_staging(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &mut StagingState<'_>,
) -> StagingLayout {
    draw_scm_sidebar(frame, area, theme, state)
}

/// HISTORY header: chevron, count, and a loading hint.
fn history_section_item(
    theme: &Theme,
    count: usize,
    expanded: bool,
    loading: bool,
    selected: bool,
    focused: bool,
    pinned: bool,
) -> ListItem<'static> {
    let bg = sticky_bg(theme, selected, focused, pinned);
    let title_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.fg
    } else {
        theme.muted
    };
    let icon_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.accent
    } else {
        theme.muted
    };
    let pad = Style::default().bg(rgb(bg));
    let chevron = if expanded {
        Icons::chevron_expanded()
    } else {
        Icons::chevron_collapsed()
    };
    let mut spans = vec![
        Span::styled(" ", pad),
        Span::styled(
            format!("{chevron} "),
            Style::default().fg(rgb(icon_fg)).bg(rgb(bg)),
        ),
        Span::styled(
            "HISTORY",
            Style::default()
                .fg(rgb(title_fg))
                .bg(rgb(bg))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({count})"),
            Style::default().fg(rgb(theme.muted)).bg(rgb(bg)),
        ),
    ];
    if loading {
        spans.push(Span::styled(
            " loading…",
            Style::default().fg(rgb(theme.muted)).bg(rgb(bg)),
        ));
    }
    spans.push(Span::styled(" ", pad));
    ListItem::new(Line::from(spans)).style(Style::default().bg(rgb(bg)))
}

/// Number of Git graph lanes actually drawn beside a commit. History children
/// must use the same visible width, not the full (possibly clipped) graph.
fn visible_commit_lanes(glyphs: &[GraphCell], row_width: u16) -> usize {
    let max_lanes = (row_width as usize).saturating_sub(22).max(LANE_WIDTH) / LANE_WIDTH;
    (glyphs.len() / LANE_WIDTH).min(max_lanes.max(1))
}

/// Commit subtrees are visually nested under the commit's hash, after the
/// graph gutter. Keep the semantic depth unchanged for navigation and sticky
/// ancestor lookup; this offset is for rendering only.
fn history_child_visual_depth(depth: u16, glyphs: &[GraphCell], row_width: u16) -> u16 {
    let extra = visible_commit_lanes(glyphs, row_width).saturating_sub(1);
    depth.saturating_add(extra.min(u16::MAX as usize) as u16)
}

/// One commit row: lane glyphs, short hash, the focused ref badge, subject.
fn commit_item(
    theme: &Theme,
    commit: &GraphCommit,
    glyphs: &[GraphCell],
    pending: bool,
    selected: bool,
    focused: bool,
    row_width: u16,
    pinned: bool,
) -> ListItem<'static> {
    let bg = sticky_bg(theme, selected, focused, pinned);
    let pad = Style::default().bg(rgb(bg));
    let width = row_width as usize;
    let shown = visible_commit_lanes(glyphs, row_width);
    let mut spans = vec![Span::styled(" ", pad)];
    for cell in glyphs.iter().take(shown * LANE_WIDTH) {
        spans.push(Span::styled(
            cell.ch.to_string(),
            Style::default()
                .fg(rgb(lane_color(theme, cell.color)))
                .bg(rgb(bg)),
        ));
    }

    let hash_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };
    spans.push(Span::styled(
        format!("{} ", commit.short),
        Style::default().fg(rgb(hash_fg)).bg(rgb(bg)),
    ));

    let mut used = 1 + shown * LANE_WIDTH + commit.short.width() + 1;
    // Local branches read better than the bare HEAD decoration.
    let badge = commit
        .refs
        .iter()
        .min_by_key(|reference| match reference.kind {
            forge_model::GraphRefKind::LocalBranch => 0,
            forge_model::GraphRefKind::Head => 1,
            forge_model::GraphRefKind::Tag => 2,
            forge_model::GraphRefKind::RemoteBranch => 3,
        });
    if let Some(reference) = badge {
        let label = format!(" {} ", reference.name);
        if width >= 34 && used + label.width() < width {
            let (fg, badge_bg) = match reference.kind {
                forge_model::GraphRefKind::Head => (theme.badge_fg, theme.warning),
                forge_model::GraphRefKind::LocalBranch => (theme.branch_fg, theme.branch_bg),
                forge_model::GraphRefKind::RemoteBranch => (theme.fg, theme.pill_bg),
                forge_model::GraphRefKind::Tag => (theme.badge_fg, theme.git_conflicted),
            };
            spans.push(Span::styled(
                label.clone(),
                Style::default()
                    .fg(rgb(fg))
                    .bg(rgb(badge_bg))
                    .add_modifier(Modifier::BOLD),
            ));
            used += label.width();
        }
    }

    let hint = if pending { " loading…" } else { "" };
    let max_subject = width
        .saturating_sub(used)
        .saturating_sub(hint.width())
        .saturating_sub(1)
        .max(4);
    let subject = truncate_end(&commit.subject, max_subject);
    let subject_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.fg
    };
    let mut subject_style = Style::default().fg(rgb(subject_fg)).bg(rgb(bg));
    if selected
        || commit
            .refs
            .iter()
            .any(|r| matches!(r.kind, forge_model::GraphRefKind::Head))
    {
        subject_style = subject_style.add_modifier(Modifier::BOLD);
    }
    spans.push(Span::styled(subject.clone(), subject_style));
    used += subject.width();
    if !hint.is_empty() {
        spans.push(Span::styled(
            hint,
            Style::default().fg(rgb(theme.muted)).bg(rgb(bg)),
        ));
        used += hint.width();
    }
    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), pad));
    ListItem::new(Line::from(spans)).style(Style::default().bg(rgb(bg)))
}

/// One file changed by a commit (read-only history tree).
fn commit_file_item(
    theme: &Theme,
    change: &CommitChange,
    depth: u16,
    selected: bool,
    focused: bool,
    row_width: u16,
) -> ListItem<'static> {
    let color = change_color(theme, change.kind);
    let bg = row_bg(theme, selected, focused);
    let mut name_style = Style::default().fg(rgb(theme.fg)).bg(rgb(bg));
    if selected && focused {
        name_style = name_style
            .fg(rgb(theme.selection_fg))
            .add_modifier(Modifier::BOLD);
    }
    let gutter_fg = if selected && focused {
        theme.selection_fg
    } else {
        color
    };
    let icon_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };
    let basename = change
        .path
        .rsplit('/')
        .next()
        .unwrap_or(change.path.as_str());
    let icon_tok = Icons::padded(Icons::file_for_name(basename));
    let indent = "  ".repeat(depth as usize);
    let chevron_pad = "  ";
    let left_inset = " ";
    let gutter_tok = format!("{} ", change.kind.badge());
    let prefix_cols = left_inset.width()
        + indent.width()
        + chevron_pad.width()
        + gutter_tok.width()
        + icon_tok.width();
    let max_name = (row_width as usize)
        .saturating_sub(prefix_cols)
        .saturating_sub(1)
        .max(4);
    let label = truncate_end(basename, max_name);
    let used = prefix_cols + label.width();
    let pad_style = Style::default().bg(rgb(bg));
    let mut gutter_style = Style::default().fg(rgb(gutter_fg)).bg(rgb(bg));
    if !(selected && focused) {
        gutter_style = gutter_style.add_modifier(Modifier::BOLD);
    }
    ListItem::new(Line::from(vec![
        Span::styled(left_inset.to_string(), pad_style),
        Span::styled(indent, pad_style),
        Span::styled(chevron_pad.to_string(), pad_style),
        Span::styled(gutter_tok, gutter_style),
        Span::styled(icon_tok, Style::default().fg(rgb(icon_fg)).bg(rgb(bg))),
        Span::styled(label, name_style),
        Span::styled(
            " ".repeat((row_width as usize).saturating_sub(used).max(1)),
            pad_style,
        ),
    ]))
    .style(Style::default().bg(rgb(bg)))
}

fn row_bg(theme: &Theme, selected: bool, focused: bool) -> forge_theme::Rgb {
    if selected {
        if focused {
            theme.selection_bg
        } else {
            theme.pill_bg
        }
    } else {
        theme.panel_bg
    }
}

// A subdued pinned background makes sticky ancestry legible without
// conflating it with the actual selected row or the diff status colors.
fn sticky_bg(theme: &Theme, selected: bool, focused: bool, pinned: bool) -> forge_theme::Rgb {
    if pinned && !selected {
        theme.pill_bg
    } else {
        row_bg(theme, selected, focused)
    }
}

fn section_item(
    theme: &Theme,
    title: &str,
    count: usize,
    selected: bool,
    focused: bool,
    pinned: bool,
) -> ListItem<'static> {
    let bg = sticky_bg(theme, selected, focused, pinned);
    let title_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.fg
    } else {
        theme.muted
    };
    let count_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };
    let icon_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.accent
    } else {
        theme.muted
    };
    let pad = Style::default().bg(rgb(bg));
    let title_style = Style::default()
        .fg(rgb(title_fg))
        .bg(rgb(bg))
        .add_modifier(Modifier::BOLD);
    let icon_tok = Icons::padded(Icons::branch());
    let icon_style = Style::default().fg(rgb(icon_fg)).bg(rgb(bg));
    ListItem::new(Line::from(vec![
        Span::styled(" ", pad),
        Span::styled(icon_tok, icon_style),
        Span::styled(title.to_ascii_uppercase(), title_style),
        Span::styled(
            format!(" ({count})"),
            Style::default().fg(rgb(count_fg)).bg(rgb(bg)),
        ),
        Span::styled(" ", pad),
    ]))
    .style(Style::default().bg(rgb(bg)))
}

fn folder_item(
    theme: &Theme,
    name: &str,
    depth: u16,
    expanded: bool,
    file_count: usize,
    selected: bool,
    focused: bool,
    row_width: u16,
    pinned: bool,
) -> ListItem<'static> {
    let bg = sticky_bg(theme, selected, focused, pinned);
    let indent = "  ".repeat(depth as usize);
    let chevron = if expanded {
        Icons::chevron_expanded()
    } else {
        Icons::chevron_collapsed()
    };
    let icon = Icons::folder(expanded);
    let icon_tok = Icons::padded(icon);
    let chevron_tok = format!("{chevron} ");

    let name_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.accent
    } else {
        // Muted folder names — hierarchy without shouting.
        theme.muted
    };
    let icon_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.accent
    } else {
        theme.muted
    };
    let count_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };

    let mut name_style = Style::default().fg(rgb(name_fg)).bg(rgb(bg));
    if selected || matches!(icon_mode(), IconMode::Ascii) {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let icon_style = Style::default().fg(rgb(icon_fg)).bg(rgb(bg));
    let pad_style = Style::default().bg(rgb(bg));
    let count_style = Style::default().fg(rgb(count_fg)).bg(rgb(bg));

    let label = if matches!(icon_mode(), IconMode::Ascii) {
        format!("{name}/")
    } else {
        name.to_string()
    };
    let count_tok = format!(" ({file_count})");
    let left_inset = " ";
    let prefix_width = left_inset.width() + indent.width() + chevron_tok.width() + icon_tok.width();
    let max_label = (row_width as usize).saturating_sub(prefix_width + count_tok.width() + 1);
    let label = fit_compact_label(&label, max_label);
    let used = prefix_width + label.width() + count_tok.width();
    let pad_len = (row_width as usize).saturating_sub(used);

    ListItem::new(Line::from(vec![
        Span::styled(left_inset.to_string(), pad_style),
        Span::styled(indent, pad_style),
        Span::styled(chevron_tok, icon_style),
        Span::styled(icon_tok, icon_style),
        Span::styled(label, name_style),
        Span::styled(count_tok, count_style),
        Span::styled(" ".repeat(pad_len.max(1)), pad_style),
    ]))
    .style(Style::default().bg(rgb(bg)))
}

fn change_item(
    theme: &Theme,
    entry: &ChangeEntry,
    depth: u16,
    selected: bool,
    focused: bool,
    row_width: u16,
) -> ListItem<'static> {
    let color = match entry.status {
        GitStatus::Modified | GitStatus::Renamed => theme.git_modified,
        GitStatus::Added => theme.git_added,
        GitStatus::Deleted => theme.git_deleted,
        GitStatus::Untracked => theme.git_untracked,
        GitStatus::Conflicted => theme.git_conflicted,
        GitStatus::Ignored => theme.git_ignored,
        GitStatus::Clean => theme.fg,
    };
    let bg = row_bg(theme, selected, focused);
    let gutter = entry.status.gutter();
    let mut name_style = Style::default().fg(rgb(theme.fg)).bg(rgb(bg));
    if selected && focused {
        name_style = name_style
            .fg(rgb(theme.selection_fg))
            .add_modifier(Modifier::BOLD);
    } else if selected {
        name_style = name_style.fg(rgb(theme.fg));
    }
    // Stronger but calm status letters when unselected.
    let gutter_fg = if selected && focused {
        theme.selection_fg
    } else {
        color
    };
    let icon_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };
    let basename = entry.path.rsplit('/').next().unwrap_or(entry.path.as_str());
    let icon = Icons::file_for_name(basename);
    let icon_tok = Icons::padded(icon);
    let indent = "  ".repeat(depth as usize);
    // Align with folder rows: spacer where chevron would be.
    let chevron_pad = "  ";
    let left_inset = " ";
    let gutter_tok = format!("{gutter} ");
    let prefix_cols = left_inset.width()
        + indent.width()
        + chevron_pad.width()
        + gutter_tok.width()
        + icon_tok.width();
    let max_name = (row_width as usize)
        .saturating_sub(prefix_cols)
        .saturating_sub(1)
        .max(4);
    let label = truncate_end(basename, max_name);
    let used = prefix_cols + label.width();
    let pad_len = (row_width as usize).saturating_sub(used);
    let pad_style = Style::default().bg(rgb(bg));
    let mut gutter_style = Style::default().fg(rgb(gutter_fg)).bg(rgb(bg));
    if !(selected && focused) {
        gutter_style = gutter_style.add_modifier(Modifier::BOLD);
    }
    let mut spans = vec![
        Span::styled(left_inset.to_string(), pad_style),
        Span::styled(indent, pad_style),
        Span::styled(chevron_pad.to_string(), pad_style),
        Span::styled(gutter_tok, gutter_style),
        Span::styled(icon_tok, Style::default().fg(rgb(icon_fg)).bg(rgb(bg))),
        Span::styled(label, name_style),
    ];
    spans.push(Span::styled(" ".repeat(pad_len.max(1)), pad_style));
    ListItem::new(Line::from(spans)).style(Style::default().bg(rgb(bg)))
}

/// Truncate from the end with an ellipsis when too wide.
fn truncate_end(name: &str, max: usize) -> String {
    if name.width() <= max {
        return name.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut remaining = max - 1;
    let mut label = String::new();
    for ch in name.chars() {
        let cells = ch.width().unwrap_or(0);
        if cells > remaining {
            break;
        }
        remaining -= cells;
        label.push(ch);
    }
    label.push('…');
    label
}

/// Prefer keeping the filename: `…/crates/foo.rs` when the path is too wide.
#[allow(dead_code)]
fn truncate_left(path: &str, max: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let keep = max - 1;
    let tail: String = chars[chars.len().saturating_sub(keep)..].iter().collect();
    format!("…{tail}")
}

/// Map a click in the SCM list body to a flat row index.
pub fn staging_index_at(
    inner: Rect,
    scroll: usize,
    len: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    scm_index_at(inner, scroll, len, column, row)
}

/// Map a click in the SCM list body to a flat row index.
pub fn scm_index_at(
    inner: Rect,
    scroll: usize,
    len: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    if !inner.contains(Position { x: column, y: row }) {
        return None;
    }
    let offset = (row - inner.y) as usize;
    let index = scroll + offset;
    if index < len {
        Some(index)
    } else {
        None
    }
}

/// Legacy collapsed strip (unused — bottom strip removed). Kept for API.
#[allow(dead_code)]
pub fn draw_git_strip(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    unstaged: usize,
    staged: usize,
    in_repo: bool,
) -> StagingLayout {
    use crate::style::{chrome, chrome_hotkey, pill_spans};

    let dirty = unstaged + staged;
    let mut spans = vec![
        Span::styled(" ", chrome(theme)),
        Span::styled(
            " Source Control ",
            Style::default()
                .fg(rgb(theme.chrome_fg))
                .bg(rgb(theme.chrome_bg)),
        ),
    ];
    if !in_repo {
        spans.push(Span::styled(
            " - not a repository ",
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.chrome_bg)),
        ));
    } else if dirty > 0 {
        spans.push(Span::styled(" ", chrome(theme)));
        spans.extend(pill_spans(
            &format!("U{unstaged}"),
            theme.badge_fg,
            theme.badge_bg,
            theme.chrome_bg,
        ));
        spans.push(Span::styled(" ", chrome(theme)));
        spans.extend(pill_spans(
            &format!("S{staged}"),
            theme.branch_fg,
            theme.branch_bg,
            theme.chrome_bg,
        ));
    } else {
        spans.push(Span::styled(
            " - clean ",
            Style::default()
                .fg(rgb(theme.git_added))
                .bg(rgb(theme.chrome_bg)),
        ));
    }
    spans.push(Span::styled("   ", chrome(theme)));
    spans.push(Span::styled("g", chrome_hotkey(theme)));
    spans.push(Span::styled(" SCM", chrome(theme)));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(chrome(theme)), area);
    StagingLayout {
        area,
        collapsed: true,
        ..StagingLayout::default()
    }
}

/// Centered commit-message modal.
pub fn draw_commit_modal(frame: &mut Frame<'_>, theme: &Theme, input: &str) {
    let area = frame.area();
    let width = area.width.min(64).max(24);
    let height = 5u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(
            Style::default()
                .fg(rgb(theme.border))
                .bg(rgb(theme.menu_bg)),
        )
        .style(
            Style::default()
                .bg(rgb(theme.menu_bg))
                .fg(rgb(theme.menu_fg)),
        )
        .title(Span::styled(
            " Commit ",
            Style::default()
                .fg(rgb(theme.selection_fg))
                .bg(rgb(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(ratatui::layout::Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let prompt = Paragraph::new(Span::styled(
        " message | Enter confirm | Esc cancel",
        Style::default().fg(rgb(theme.muted)).bg(rgb(theme.menu_bg)),
    ));
    frame.render_widget(prompt, rows[0]);

    let caret = format!(" {input}|");
    let input_line = Paragraph::new(Span::styled(
        caret,
        Style::default()
            .fg(rgb(theme.fg))
            .bg(rgb(theme.panel_bg))
            .add_modifier(Modifier::BOLD),
    ))
    .style(Style::default().bg(rgb(theme.panel_bg)));
    frame.render_widget(input_line, rows[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_model::{ChangeEntry, DiffSide, GitStatus};
    use std::collections::HashSet;

    #[test]
    fn history_commit_children_align_after_graph_lanes_in_normal_and_compact_trees() {
        use crate::graph::GraphRow;
        use forge_model::CommitChangeKind;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let oid = "e07f816000000000000000000000000000000000";
        let glyphs = vec![
            GraphCell {
                ch: '│', color: 0
            };
            5 * LANE_WIDTH
        ];
        let mut history = History {
            open: true,
            commits: vec![commit(oid, "fix", &[])],
            layout: GraphLayout {
                rows: vec![GraphRow {
                    cells: glyphs.clone(),
                }],
                lane_count: 5,
            },
            ..History::default()
        };
        history.expanded_commits.insert(oid.into());
        history
            .expanded_dirs
            .insert(crate::graph::dir_key(oid, "agent"));
        history
            .expanded_dirs
            .insert(crate::graph::dir_key(oid, "agent/tests"));
        history.files.insert(
            oid.into(),
            vec![CommitChange {
                path: "agent/tests/sample.rs".into(),
                old_path: None,
                kind: CommitChangeKind::Modified,
            }],
        );

        fn column(cells: &[String], text: &str) -> usize {
            cells
                .windows(text.chars().count())
                .position(|cells| cells.iter().map(String::as_str).collect::<String>() == text)
                .unwrap_or_else(|| panic!("missing {text:?} in rendered row"))
        }

        let empty = HashSet::new();
        for width in [24_u16, 32, 52] {
            let hash_col = 1 + visible_commit_lanes(&glyphs, width) * LANE_WIDTH;
            assert_eq!(
                history_child_visual_depth(1, &glyphs, width),
                1 + visible_commit_lanes(&glyphs, width).saturating_sub(1) as u16
            );
            for compact in [false, true] {
                let view = history.view();
                let mut scroll = 0;
                let mut pinned = Vec::new();
                let mut state = StagingState {
                    unstaged: &[],
                    staged: &[],
                    expanded_unstaged: &empty,
                    expanded_staged: &empty,
                    history: &view,
                    selected: 3,
                    scroll: &mut scroll,
                    focused: true,
                    sticky_scroll: false,
                    compact_folders: compact,
                    follow_selection: false,
                    sticky_headers: &mut pinned,
                };
                let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
                terminal
                    .draw(|frame| {
                        draw_scm_sidebar(frame, frame.area(), &Theme::modern_ide(), &mut state);
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let row = |y: u16| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol().to_owned())
                        .collect::<Vec<_>>()
                };
                let commit_col = column(&row(3), "e07f816");
                assert_eq!(commit_col, hash_col, "commit graph/label layout");
                let root_col = column(&row(4), "agent");
                assert!(
                    root_col > commit_col,
                    "commit subtree must be visually nested below hash (width={width}, compact={compact})"
                );
                if !compact {
                    let nested_col = column(&row(5), "tests");
                    let file_col = column(&row(6), "sample.rs");
                    assert!(nested_col > root_col && file_col > nested_col);
                } else {
                    let file_col = column(&row(5), "sample.rs");
                    assert!(file_col > root_col);
                }
                // Visual padding must never change the tree's semantic depth.
                let semantic_rows = if compact {
                    compact_scm_rows(&build_scm_rows(&[], &[], &empty, &empty, &history.view()))
                } else {
                    build_scm_rows(&[], &[], &empty, &empty, &history.view())
                };
                assert_eq!(find_scm_parent_row(&semantic_rows, 4), Some(3));
            }
        }
    }

    #[test]
    fn sticky_history_titles_stop_at_commit_boundary() {
        let rows = vec![
            ScmRow::HistorySection {
                count: 2,
                expanded: true,
                loading: false,
            },
            ScmRow::Commit { index: 0 },
            ScmRow::CommitFolder {
                commit: 0,
                path: "manager".into(),
                name: "manager".into(),
                depth: 1,
                expanded: true,
                file_count: 1,
            },
            ScmRow::CommitFolder {
                commit: 0,
                path: "manager/server".into(),
                name: "server".into(),
                depth: 2,
                expanded: true,
                file_count: 1,
            },
            ScmRow::CommitFile {
                commit: 0,
                index: 0,
                depth: 3,
            },
            ScmRow::Commit { index: 1 },
            ScmRow::CommitFile {
                commit: 1,
                index: 0,
                depth: 1,
            },
        ];
        let first = sticky::headers(4, 20, |i| find_scm_parent_row(&rows, i));
        assert_eq!(first, vec![0, 1, 2, 3]);
        let second = sticky::headers(6, 20, |i| find_scm_parent_row(&rows, i));
        assert_eq!(second, vec![0, 5]);
        assert_eq!(
            sticky::index_at(Rect::new(2, 3, 30, 10), 4, &first, rows.len(), 5, 4),
            Some(1)
        );
    }

    fn entry(path: &str, side: DiffSide) -> ChangeEntry {
        ChangeEntry {
            path: path.into(),
            status: GitStatus::Modified,
            side,
        }
    }

    /// Owns the borrowed pieces of [`HistoryView`] so tests can build one.
    #[derive(Default)]
    struct History {
        open: bool,
        commits: Vec<GraphCommit>,
        layout: GraphLayout,
        expanded_commits: HashSet<String>,
        expanded_dirs: HashSet<String>,
        files: HashMap<String, Vec<CommitChange>>,
    }

    impl History {
        fn view(&self) -> HistoryView<'_> {
            HistoryView {
                expanded: self.open,
                loading: false,
                commits: &self.commits,
                layout: &self.layout,
                expanded_commits: &self.expanded_commits,
                expanded_dirs: &self.expanded_dirs,
                files: &self.files,
            }
        }
    }

    fn commit(id: &str, subject: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            id: id.into(),
            short: id.chars().take(7).collect(),
            parents: parents.iter().map(|p| (*p).to_string()).collect(),
            refs: Vec::new(),
            author: "forge".into(),
            time: 0,
            subject: subject.into(),
        }
    }

    #[test]
    fn git_sticky_section_and_folder_have_continuous_full_width_background() {
        use crate::sticky;
        use forge_theme::Theme;
        use ratatui::{backend::TestBackend, Terminal};
        let theme = Theme::modern_ide();
        let changes = (0..30)
            .map(|i| entry(&format!("src/file_{i:02}.rs"), DiffSide::Unstaged))
            .collect::<Vec<_>>();
        let expanded = HashSet::from(["src".into()]);
        let empty = HashSet::new();
        let history = History::default();
        let history_view = history.view();
        let mut scroll = 9;
        let mut headers = Vec::new();
        let mut terminal = Terminal::new(TestBackend::new(38, 8)).unwrap();
        terminal
            .draw(|frame| {
                draw_scm_sidebar(
                    frame,
                    Rect::new(0, 0, 38, 8),
                    &theme,
                    &mut StagingState {
                        unstaged: &changes,
                        staged: &[],
                        expanded_unstaged: &expanded,
                        expanded_staged: &empty,
                        history: &history_view,
                        selected: 2,
                        scroll: &mut scroll,
                        focused: true,
                        sticky_scroll: true,
                        compact_folders: false,
                        follow_selection: false,
                        sticky_headers: &mut headers,
                    },
                );
            })
            .unwrap();
        assert_eq!(headers, vec![0, 1]);
        let buffer = terminal.backend().buffer();
        let pinned = sticky::background(&theme);
        let normal = crate::style::rgb(theme.panel_bg);
        for y in 0..2 {
            for x in 0..38 {
                assert_eq!(buffer[(x, y)].bg, pinned, "pinned Git cell ({x},{y})");
            }
            assert_eq!(buffer[(0, y)].symbol(), "▏");
        }
        assert_eq!(buffer[(37, 2)].bg, normal);

        // When disabled, even the same scrolled section has normal styling.
        let mut terminal = Terminal::new(TestBackend::new(38, 8)).unwrap();
        let mut off_scroll = 9;
        terminal
            .draw(|frame| {
                draw_scm_sidebar(
                    frame,
                    Rect::new(0, 0, 38, 8),
                    &theme,
                    &mut StagingState {
                        unstaged: &changes,
                        staged: &[],
                        expanded_unstaged: &expanded,
                        expanded_staged: &empty,
                        history: &history_view,
                        selected: 2,
                        scroll: &mut off_scroll,
                        focused: true,
                        sticky_scroll: false,
                        compact_folders: false,
                        follow_selection: false,
                        sticky_headers: &mut headers,
                    },
                );
            })
            .unwrap();
        assert!(headers.is_empty());
        assert_eq!(terminal.backend().buffer()[(37, 0)].bg, normal);
    }

    #[test]
    fn compact_changes_and_history_stop_at_branches_and_keep_file_identity() {
        let changes = vec![
            entry(
                "manager/server/src/main/java/com/nist/X.java",
                DiffSide::Unstaged,
            ),
            entry("manager/server/src/test/Test.java", DiffSide::Unstaged),
        ];
        let mut open = HashSet::new();
        for p in [
            "manager",
            "manager/server",
            "manager/server/src",
            "manager/server/src/main",
            "manager/server/src/main/java",
            "manager/server/src/main/java/com",
            "manager/server/src/main/java/com/nist",
            "manager/server/src/test",
        ] {
            open.insert(p.into());
        }
        let empty = HashSet::new();
        let history = History::default();
        let raw = build_scm_rows(&changes, &[], &open, &empty, &history.view());
        let compact = compact_scm_rows(&raw);
        assert!(compact.len() < raw.len());
        assert!(compact
            .iter()
            .any(|r| matches!(r, ScmRow::Folder { name, path, depth: 0, .. }
                if name == "manager / server / src" && path == "manager/server/src")));
        assert!(compact
            .iter()
            .any(|r| matches!(r, ScmRow::Folder { name, depth: 1, .. }
                if name == "main / java / com / nist")));
        assert!(compact
            .iter()
            .any(|r| matches!(r, ScmRow::Folder { name, depth: 1, .. } if name == "test")));
        for (i, row) in compact.iter().enumerate() {
            if let ScmRow::Change { index, .. } = row {
                assert_eq!(
                    scm_entry(row, &changes, &[]).unwrap().path,
                    changes[*index].path
                );
                assert!(find_scm_parent_row(&compact, i).is_some());
            }
        }

        let mut history = History {
            open: true,
            commits: vec![
                commit("abcde123", "one", &[]),
                commit("abcde124", "two", &[]),
            ],
            ..History::default()
        };
        history.layout = crate::graph::layout_graph(&history.commits);
        for commit in ["abcde123", "abcde124"] {
            history.expanded_commits.insert(commit.into());
            history.files.insert(
                commit.into(),
                vec![
                    CommitChange {
                        path: "crates/forge-git-cli/src/workbench/tests.rs".into(),
                        old_path: None,
                        kind: forge_model::CommitChangeKind::Modified,
                    },
                    CommitChange {
                        path: "crates/forge-git-cli/src/workbench.rs".into(),
                        old_path: None,
                        kind: forge_model::CommitChangeKind::Modified,
                    },
                ],
            );
            for path in [
                "crates",
                "crates/forge-git-cli",
                "crates/forge-git-cli/src",
                "crates/forge-git-cli/src/workbench",
            ] {
                history
                    .expanded_dirs
                    .insert(crate::graph::dir_key(commit, path));
            }
        }
        let raw = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        let compact = compact_scm_rows(&raw);
        let dirs: Vec<_> = compact
            .iter()
            .filter_map(|r| match r {
                ScmRow::CommitFolder {
                    commit, path, name, ..
                } => Some((*commit, path.as_str(), name.as_str())),
                _ => None,
            })
            .collect();
        // The direct workbench.rs file makes src a branch; never merge the
        // workbench subdirectory into that parent even if its tests folder
        // contains only one changed file.
        assert_eq!(dirs.len(), 4);
        assert_eq!(dirs[0].2, "crates / forge-git-cli / src");
        assert_eq!(dirs[0].1, "crates/forge-git-cli/src");
        assert_eq!(dirs[1].2, "workbench");
        assert_eq!(dirs[1].1, "crates/forge-git-cli/src/workbench");
        assert_eq!(dirs[2].2, dirs[0].2);
        assert_ne!(dirs[0].0, dirs[2].0);
        assert!(compact.iter().any(|r| matches!(
            r,
            ScmRow::CommitFile {
                commit: 0,
                index: 1,
                depth: 2
            }
        )));
    }

    #[test]
    fn scm_rows_tree_groups_by_dir() {
        let u = vec![
            entry("agent/app/a.py", DiffSide::Unstaged),
            entry("agent/app/b.py", DiffSide::Unstaged),
            entry("README.md", DiffSide::Unstaged),
        ];
        let s = vec![entry("crates/x.rs", DiffSide::Staged)];
        let empty = HashSet::new();
        let history = History::default();
        let rows = build_scm_rows(&u, &s, &empty, &empty, &history.view());
        // CHANGES, agent/ (collapsed), README.md, STAGED, crates/ (collapsed)
        assert!(matches!(
            &rows[0],
            ScmRow::Section {
                title: "CHANGES",
                count: 3,
                ..
            }
        ));
        assert!(matches!(
            &rows[1],
            ScmRow::Folder {
                name,
                expanded: false,
                file_count: 2,
                ..
            } if name == "agent"
        ));
        assert!(matches!(
            &rows[2],
            ScmRow::Change {
                index: 2,
                depth: 0,
                ..
            }
        ));
        assert!(matches!(
            &rows[3],
            ScmRow::Section {
                title: "STAGED",
                count: 1,
                ..
            }
        ));
        assert!(matches!(
            &rows[4],
            ScmRow::Folder {
                name,
                file_count: 1,
                ..
            } if name == "crates"
        ));
        assert_eq!(
            scm_entry(&rows[2], &u, &s).map(|e| e.path.as_str()),
            Some("README.md")
        );
    }

    #[test]
    fn scm_rows_expands_nested() {
        let u = vec![
            entry("agent/app/a.py", DiffSide::Unstaged),
            entry("agent/app/api/b.py", DiffSide::Unstaged),
        ];
        let s: Vec<ChangeEntry> = vec![];
        let mut exp = HashSet::new();
        exp.insert("agent".into());
        exp.insert("agent/app".into());
        let empty = HashSet::new();
        let history = History::default();
        let rows = build_scm_rows(&u, &s, &exp, &empty, &history.view());
        let folders: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                ScmRow::Folder {
                    path,
                    depth,
                    expanded,
                    ..
                } => Some((path.as_str(), *depth, *expanded)),
                _ => None,
            })
            .collect();
        assert_eq!(
            folders,
            vec![
                ("agent", 0, true),
                ("agent/app", 1, true),
                ("agent/app/api", 2, false),
            ]
        );
        // Files under agent/app visible; api collapsed so b.py hidden.
        let changes: Vec<_> = rows
            .iter()
            .filter_map(|r| scm_entry(r, &u, &s).map(|e| e.path.as_str()))
            .collect();
        assert_eq!(changes, vec!["agent/app/a.py"]);
    }

    #[test]
    fn expand_ancestors_inserts_parents() {
        let mut exp = HashSet::new();
        expand_ancestors_for_path(&mut exp, "a/b/c/d.py");
        assert!(exp.contains("a"));
        assert!(exp.contains("a/b"));
        assert!(exp.contains("a/b/c"));
        assert!(!exp.contains("a/b/c/d.py"));
    }

    #[test]
    fn scm_jump_and_parent() {
        let u = vec![
            entry("pkg/a.rs", DiffSide::Unstaged),
            entry("pkg/b.rs", DiffSide::Unstaged),
        ];
        let s = vec![entry("z.rs", DiffSide::Staged)];
        let mut exp = HashSet::new();
        exp.insert("pkg".into());
        let empty = HashSet::new();
        let history = History::default();
        let rows = build_scm_rows(&u, &s, &exp, &empty, &history.view());
        assert_eq!(scm_jump_to_column(&rows, StagingColumn::Staged), 5);
        // rows: CHANGES, pkg/, a, b, STAGED, z
        assert_eq!(find_scm_parent_row(&rows, 2), Some(1)); // a -> pkg
        assert_eq!(find_scm_parent_row(&rows, 1), Some(0)); // pkg -> CHANGES
    }

    #[test]
    fn truncate_left_keeps_tail() {
        assert_eq!(
            super::truncate_left("crates/forge-app/src/lib.rs", 20),
            "…orge-app/src/lib.rs"
        );
        assert_eq!(super::truncate_left("short.rs", 20), "short.rs");
        assert_eq!(super::truncate_left("abcdefghij", 4), "…hij");
    }
    #[test]
    fn history_section_lists_commits_and_expands_files() {
        let mut history = History {
            open: true,
            commits: vec![
                commit("aaaaaaa1", "feat: newest", &["bbbbbbb2"]),
                commit("bbbbbbb2", "chore: base", &[]),
            ],
            ..History::default()
        };
        history.layout = crate::graph::layout_graph(&history.commits);
        let empty = HashSet::new();
        let rows = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        // CHANGES, STAGED, HISTORY, commit a, commit b
        assert!(matches!(
            &rows[2],
            ScmRow::HistorySection {
                count: 2,
                expanded: true,
                ..
            }
        ));
        assert!(matches!(&rows[3], ScmRow::Commit { index: 0 }));
        assert!(matches!(&rows[4], ScmRow::Commit { index: 1 }));

        history.expanded_commits.insert("aaaaaaa1".into());
        history.files.insert(
            "aaaaaaa1".into(),
            vec![CommitChange {
                path: "src/app/main.rs".into(),
                old_path: None,
                kind: forge_model::CommitChangeKind::Modified,
            }],
        );
        let rows = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        // HISTORY, commit a, src/, commit b
        assert!(matches!(
            &rows[4],
            ScmRow::CommitFolder { name, depth: 1, expanded: false, .. } if name == "src"
        ));
        assert!(matches!(&rows[5], ScmRow::Commit { index: 1 }));
        assert_eq!(find_scm_parent_row(&rows, 4), Some(3));

        history
            .expanded_dirs
            .insert(crate::graph::dir_key("aaaaaaa1", "src"));
        let rows = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        assert!(matches!(&rows[5], ScmRow::CommitFolder { name, depth: 2, .. } if name == "app"));
        history
            .expanded_dirs
            .insert(crate::graph::dir_key("aaaaaaa1", "src/app"));
        let rows = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        assert!(matches!(
            &rows[6],
            ScmRow::CommitFile {
                index: 0,
                depth: 3,
                ..
            }
        ));
        assert_eq!(find_scm_parent_row(&rows, 6), Some(5));
        assert_eq!(find_scm_parent_row(&rows, 5), Some(4));
        assert_eq!(find_scm_parent_row(&rows, 4), Some(3));
        assert_eq!(find_scm_parent_row(&rows, 3), Some(2));
    }

    #[test]
    fn collapsed_history_hides_commits() {
        let history = History {
            open: false,
            commits: vec![commit("aaaaaaa1", "one", &[])],
            ..History::default()
        };
        let empty = HashSet::new();
        let rows = build_scm_rows(&[], &[], &empty, &empty, &history.view());
        assert_eq!(rows.len(), 3);
        assert!(matches!(
            &rows[2],
            ScmRow::HistorySection {
                expanded: false,
                count: 1,
                ..
            }
        ));
    }
}

//! VS Code–style Source Control sidebar (sectioned directory tree).
//!
//! Drawn into the left sidebar when `ActivityTab::Git` is active.
//! Changes are grouped by path segments with expandable folders.

use std::collections::{BTreeMap, HashSet};

use forge_model::{ChangeEntry, DiffSide, GitStatus};
use forge_theme::Theme;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::file_list::ensure_selected_visible;
use crate::icons::{icon_mode, IconMode, Icons};
use crate::style::{self, rgb};

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

/// One row in the SCM sidebar list (section / folder / file).
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
}

impl ScmRow {
    pub fn column(&self) -> StagingColumn {
        match self {
            ScmRow::Section { column, .. }
            | ScmRow::Folder { column, .. }
            | ScmRow::Change { column, .. } => *column,
        }
    }

    pub fn is_change(&self) -> bool {
        matches!(self, ScmRow::Change { .. })
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, ScmRow::Folder { .. })
    }

    pub fn depth(&self) -> u16 {
        match self {
            ScmRow::Section { .. } => 0,
            ScmRow::Folder { depth, .. } | ScmRow::Change { depth, .. } => *depth,
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

/// Build the SCM list: CHANGES tree + STAGED tree (expandable folders).
pub fn build_scm_rows(
    unstaged: &[ChangeEntry],
    staged: &[ChangeEntry],
    expanded_unstaged: &HashSet<String>,
    expanded_staged: &HashSet<String>,
) -> Vec<ScmRow> {
    let mut rows = Vec::with_capacity(2 + unstaged.len() + staged.len());
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
    rows
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
        ScmRow::Section { .. } | ScmRow::Folder { .. } => None,
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
    let depth = rows.get(index)?.depth();
    if depth == 0 {
        // Walk up to the section header for this column.
        let col = rows.get(index)?.column();
        return (0..index)
            .rev()
            .find(|&i| matches!(&rows[i], ScmRow::Section { column: c, .. } if *c == col));
    }
    let target = depth - 1;
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
    /// Flat index into [`build_scm_rows`].
    pub selected: usize,
    pub scroll: &'a mut usize,
    pub focused: bool,
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

    let rows = build_scm_rows(
        state.unstaged,
        state.staged,
        state.expanded_unstaged,
        state.expanded_staged,
    );
    let count = rows.len();
    let selected = state.selected.min(count.saturating_sub(1));

    style::fill_panel(frame, body, theme);
    let viewport = body.height as usize;
    ensure_selected_visible(selected, state.scroll, viewport, count);

    let start = *state.scroll;
    let end = (start + viewport).min(count);
    let focused = state.focused;
    let items: Vec<ListItem<'_>> = rows[start..end]
        .iter()
        .enumerate()
        .map(|(row, scm)| {
            let index = start + row;
            let is_sel = index == selected;
            match scm {
                ScmRow::Section { title, count, .. } => {
                    section_item(theme, title, *count, is_sel, focused)
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
                        Some(e) => {
                            change_item(
                                theme,
                                e,
                                *depth,
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

fn section_item(
    theme: &Theme,
    title: &str,
    count: usize,
    selected: bool,
    focused: bool,
) -> ListItem<'static> {
    let bg = row_bg(theme, selected, focused);
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
) -> ListItem<'static> {
    let bg = row_bg(theme, selected, focused);
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
    let used = left_inset.width()
        + indent.width()
        + chevron_tok.width()
        + icon_tok.width()
        + label.width()
        + count_tok.width();
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

    fn entry(path: &str, side: DiffSide) -> ChangeEntry {
        ChangeEntry {
            path: path.into(),
            status: GitStatus::Modified,
            side,
        }
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
        let rows = build_scm_rows(&u, &s, &empty, &empty);
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
        let rows = build_scm_rows(&u, &s, &exp, &empty);
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
        let rows = build_scm_rows(&u, &s, &exp, &empty);
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
}

//! VS Code–style Source Control sidebar (single-column, sectioned list).
//!
//! Drawn into the left sidebar when `ActivityTab::Git` is active — not a
//! bottom strip. Dual-column / collapsed-strip helpers remain as unused
//! compatibility shims.

use forge_model::{ChangeEntry, DiffSide, GitStatus};
use forge_theme::Theme;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::file_list::ensure_selected_visible;
use crate::icons::Icons;
use crate::style::{self, rgb};

/// Which section a change belongs to (unstaged vs staged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// One row in the flattened SCM sidebar list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmRow {
    Section {
        title: &'static str,
        count: usize,
        column: StagingColumn,
    },
    Change {
        column: StagingColumn,
        /// Index into the unstaged / staged slice.
        index: usize,
    },
}

impl ScmRow {
    pub fn column(self) -> StagingColumn {
        match self {
            ScmRow::Section { column, .. } | ScmRow::Change { column, .. } => column,
        }
    }

    pub fn is_change(self) -> bool {
        matches!(self, ScmRow::Change { .. })
    }
}

/// Build the flat SCM list: Changes section + rows, then Staged section + rows.
pub fn build_scm_rows(unstaged: &[ChangeEntry], staged: &[ChangeEntry]) -> Vec<ScmRow> {
    let mut rows = Vec::with_capacity(2 + unstaged.len() + staged.len());
    rows.push(ScmRow::Section {
        title: "CHANGES",
        count: unstaged.len(),
        column: StagingColumn::Unstaged,
    });
    for index in 0..unstaged.len() {
        rows.push(ScmRow::Change {
            column: StagingColumn::Unstaged,
            index,
        });
    }
    rows.push(ScmRow::Section {
        title: "STAGED",
        count: staged.len(),
        column: StagingColumn::Staged,
    });
    for index in 0..staged.len() {
        rows.push(ScmRow::Change {
            column: StagingColumn::Staged,
            index,
        });
    }
    rows
}

/// Resolve a flat SCM row to a change entry, if any.
pub fn scm_entry<'a>(
    row: ScmRow,
    unstaged: &'a [ChangeEntry],
    staged: &'a [ChangeEntry],
) -> Option<&'a ChangeEntry> {
    match row {
        ScmRow::Change {
            column: StagingColumn::Unstaged,
            index,
        } => unstaged.get(index),
        ScmRow::Change {
            column: StagingColumn::Staged,
            index,
        } => staged.get(index),
        ScmRow::Section { .. } => None,
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
        self.area.contains(Position {
            x: column,
            y: row,
        })
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

    let dirty = state.unstaged.len() + state.staged.len();
    let title = if dirty > 0 {
        format!("GIT · {dirty}")
    } else {
        "SOURCE CONTROL".into()
    };
    let body = style::draw_section_header(frame, area, theme, &title, state.focused);

    let rows = build_scm_rows(state.unstaged, state.staged);
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
            match *scm {
                ScmRow::Section {
                    title, count, ..
                } => section_item(theme, title, count, is_sel, focused),
                ScmRow::Change { column, index: ei } => {
                    let entry = match column {
                        StagingColumn::Unstaged => state.unstaged.get(ei),
                        StagingColumn::Staged => state.staged.get(ei),
                    };
                    match entry {
                        Some(e) => change_item(theme, e, is_sel, focused, body.width),
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

fn section_item(
    theme: &Theme,
    title: &str,
    count: usize,
    selected: bool,
    focused: bool,
) -> ListItem<'static> {
    let row_bg = if selected {
        if focused {
            theme.selection_bg
        } else {
            theme.pill_bg
        }
    } else {
        theme.panel_bg
    };
    let title_fg = if selected && focused {
        theme.selection_fg
    } else if selected {
        theme.fg
    } else {
        theme.muted
    };
    // Count stays muted even when selected (VS Code–like).
    let count_fg = if selected && focused {
        theme.selection_fg
    } else {
        theme.muted
    };
    let pad = Style::default().bg(rgb(row_bg));
    // Stronger section headers: always uppercase + bold when idle muted.
    let title_style = Style::default()
        .fg(rgb(title_fg))
        .bg(rgb(row_bg))
        .add_modifier(Modifier::BOLD);
    ListItem::new(Line::from(vec![
        Span::styled(" ", pad),
        Span::styled(title.to_ascii_uppercase(), title_style),
        Span::styled(
            format!(" ({count})"),
            Style::default().fg(rgb(count_fg)).bg(rgb(row_bg)),
        ),
        Span::styled(" ", pad),
    ]))
}

fn change_item(
    theme: &Theme,
    entry: &ChangeEntry,
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
    let row_bg = if selected {
        if focused {
            theme.selection_bg
        } else {
            theme.pill_bg
        }
    } else {
        theme.panel_bg
    };
    let gutter = entry.status.gutter();
    let mut name_style = Style::default().fg(rgb(theme.fg)).bg(rgb(row_bg));
    if selected && focused {
        name_style = name_style
            .fg(rgb(theme.selection_fg))
            .add_modifier(Modifier::BOLD);
    } else if selected {
        name_style = name_style.fg(rgb(theme.fg));
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
    let basename = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(entry.path.as_str());
    let icon = Icons::file_for_name(basename);
    let icon_tok = Icons::padded(icon);
    // " " + gutter + " " + icon_tok  (gutter is 1 char + space in format)
    let prefix_cols = 1 + 2 + icon_tok.chars().count();
    let max_path = (row_width as usize).saturating_sub(prefix_cols).saturating_sub(1);
    let label = truncate_left(&entry.path, max_path.max(4));
    let used = prefix_cols + label.chars().count();
    let pad_len = (row_width as usize).saturating_sub(used);
    let pad_style = Style::default().bg(rgb(row_bg));
    // Status letter keeps git_* color when unselected; bold for contrast.
    let mut gutter_style = Style::default().fg(rgb(gutter_fg)).bg(rgb(row_bg));
    if !(selected && focused) {
        gutter_style = gutter_style.add_modifier(Modifier::BOLD);
    }
    ListItem::new(Line::from(vec![
        Span::styled(" ", pad_style),
        Span::styled(format!("{gutter} "), gutter_style),
        Span::styled(icon_tok, Style::default().fg(rgb(icon_fg)).bg(rgb(row_bg))),
        Span::styled(label, name_style),
        Span::styled(" ".repeat(pad_len.max(1)), pad_style),
    ]))
}

/// Prefer keeping the filename: `…/crates/foo.rs` when the path is too wide.
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
    if !inner.contains(Position {
        x: column,
        y: row,
    }) {
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
        .border_style(Style::default().fg(rgb(theme.border)).bg(rgb(theme.menu_bg)))
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

    let rows =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
    let prompt = Paragraph::new(Span::styled(
        " message | Enter confirm | Esc cancel",
        Style::default()
            .fg(rgb(theme.muted))
            .bg(rgb(theme.menu_bg)),
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
    use super::{build_scm_rows, scm_entry, scm_jump_to_column, ScmRow, StagingColumn};
    use forge_model::{ChangeEntry, DiffSide, GitStatus};

    fn entry(path: &str, side: DiffSide) -> ChangeEntry {
        ChangeEntry {
            path: path.into(),
            status: GitStatus::Modified,
            side,
        }
    }

    #[test]
    fn scm_rows_order() {
        let u = vec![entry("a.rs", DiffSide::Unstaged)];
        let s = vec![entry("b.rs", DiffSide::Staged)];
        let rows = build_scm_rows(&u, &s);
        assert_eq!(rows.len(), 4);
        assert!(matches!(
            rows[0],
            ScmRow::Section {
                title: "CHANGES",
                count: 1,
                ..
            }
        ));
        assert!(matches!(
            rows[1],
            ScmRow::Change {
                column: StagingColumn::Unstaged,
                index: 0
            }
        ));
        assert!(matches!(
            rows[2],
            ScmRow::Section {
                title: "STAGED",
                count: 1,
                ..
            }
        ));
        assert_eq!(
            scm_entry(rows[1], &u, &s).map(|e| e.path.as_str()),
            Some("a.rs")
        );
        assert_eq!(scm_jump_to_column(&rows, StagingColumn::Staged), 3);
    }

    #[test]
    fn truncate_left_keeps_tail() {
        assert_eq!(super::truncate_left("crates/forge-app/src/lib.rs", 20), "…orge-app/src/lib.rs");
        assert_eq!(super::truncate_left("short.rs", 20), "short.rs");
        assert_eq!(super::truncate_left("abcdefghij", 4), "…hij");
    }
}

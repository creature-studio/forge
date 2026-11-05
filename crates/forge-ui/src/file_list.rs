//! Directory listing widget with virtual scrolling (Modern Minimal TUI).

use forge_model::{EntryKind, GitStatus, StatusSnapshot};
use forge_theme::Theme;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::icons::{icon_mode, IconMode, Icons};
use crate::style::{self, rgb};
use crate::tree::TreeRow;

/// Borrowed view model for the file list pane.
#[derive(Debug)]
pub struct FileListState<'a> {
    pub rows: &'a [TreeRow],
    pub selected: usize,
    /// First visible row index; adjusted during draw so `selected` stays in view.
    pub scroll: &'a mut usize,
    /// Current git status snapshot (paths relative to repo root).
    pub git: Option<&'a StatusSnapshot>,
    /// Whether the list pane currently has keyboard focus.
    pub focused: bool,
}

/// Keep `selected` inside the visible window `[scroll, scroll + height)`.
pub fn ensure_selected_visible(selected: usize, scroll: &mut usize, height: usize, len: usize) {
    if len == 0 || height == 0 {
        *scroll = 0;
        return;
    }
    let max_scroll = len.saturating_sub(height);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + height {
        *scroll = selected + 1 - height;
    }
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
}

/// Draw the file list into a borderless pane (no section title chip).
/// Returns the content rect for hit-testing.
pub fn draw_file_list(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    state: &mut FileListState<'_>,
) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    style::fill_panel(frame, area, theme);

    let viewport = area.height as usize;
    ensure_selected_visible(state.selected, state.scroll, viewport, state.rows.len());

    let start = *state.scroll;
    let end = (start + viewport).min(state.rows.len());
    let row_width = area.width;
    let focused = state.focused;
    let items: Vec<ListItem<'_>> = state.rows[start..end]
        .iter()
        .enumerate()
        .map(|(row, tree_row)| {
            let index = start + row;
            list_item(
                theme,
                index,
                tree_row,
                state.selected,
                state.git,
                row_width,
                focused,
            )
        })
        .collect();

    let list = List::new(items).style(Style::default().bg(rgb(theme.panel_bg)));
    frame.render_widget(list, area);
    area
}

/// Map a terminal cell to a list entry index, accounting for scroll + content rect.
pub fn list_index_at(
    list_inner: Rect,
    scroll: usize,
    len: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    if !list_inner.contains(Position { x: column, y: row }) {
        return None;
    }
    let offset = (row - list_inner.y) as usize;
    let index = scroll + offset;
    if index < len {
        Some(index)
    } else {
        None
    }
}

fn list_item(
    theme: &Theme,
    index: usize,
    row: &TreeRow,
    selected: usize,
    git: Option<&StatusSnapshot>,
    row_width: u16,
    focused: bool,
) -> ListItem<'static> {
    let entry = &row.entry;
    let git_status = git
        .map(|snap| snap.status_for(&entry.path))
        .unwrap_or(GitStatus::Clean);
    let is_sel = index == selected;
    let is_dir = matches!(entry.kind, EntryKind::Directory);

    // Keep the selection neutral; use the small leading mark for focus.
    let row_bg = if is_sel {
        if focused {
            theme.selection_bg
        } else {
            theme.pill_bg
        }
    } else {
        theme.panel_bg
    };

    // [indent][chevron| ][ ][icon][ ][name]
    let indent = "  ".repeat(row.depth as usize);
    let chevron = if is_dir {
        if row.expanded {
            Icons::chevron_expanded()
        } else {
            Icons::chevron_collapsed()
        }
    } else {
        " "
    };
    let icon = if is_dir {
        Icons::folder(row.expanded)
    } else {
        Icons::file_for_name(&entry.name)
    };
    // Nerd folder icon already signals directory — skip trailing `/`.
    // Ascii mode keeps `name/` for clarity when the icon is just `/`.
    let name = if is_dir && matches!(icon_mode(), IconMode::Ascii) {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    };

    // Reserve semantic color for changed files, keeping directory names quiet.
    let name_fg = if is_sel {
        if focused {
            theme.selection_fg
        } else {
            theme.accent
        }
    } else if !matches!(git_status, GitStatus::Clean | GitStatus::Ignored) {
        git_color(theme, git_status)
    } else {
        theme.fg
    };

    let icon_fg = if is_sel {
        if focused {
            theme.selection_fg
        } else {
            theme.accent
        }
    } else {
        theme.muted
    };

    let mut name_style = Style::default().fg(rgb(name_fg)).bg(rgb(row_bg));
    if is_sel {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let icon_style = Style::default().fg(rgb(icon_fg)).bg(rgb(row_bg));
    let pad_style = Style::default().bg(rgb(row_bg));

    let left_inset = if is_sel { "›" } else { " " };
    let chevron_tok = format!("{chevron} ");
    let icon_tok = Icons::padded(icon);
    let prefix_width = left_inset.width() + indent.width() + chevron_tok.width() + icon_tok.width();
    let changed = !matches!(git_status, GitStatus::Clean | GitStatus::Ignored);
    let suffix_width = if changed { 3 } else { 1 };
    let name = fit_name(
        &name,
        (row_width as usize).saturating_sub(prefix_width + suffix_width),
    );
    let trailing = " ".repeat(
        (row_width as usize).saturating_sub(prefix_width + name.width() + suffix_width - 1),
    );
    let marker_style = Style::default()
        .fg(rgb(if focused { theme.accent } else { theme.muted }))
        .bg(rgb(row_bg));

    ListItem::new(Line::from(vec![
        Span::styled(left_inset.to_string(), marker_style),
        Span::styled(indent, pad_style),
        Span::styled(chevron_tok, icon_style),
        Span::styled(icon_tok, icon_style),
        Span::styled(name, name_style),
        Span::styled(trailing, pad_style),
        Span::styled(
            if changed {
                format!("{} ", git_status.gutter())
            } else {
                String::new()
            },
            Style::default()
                .fg(rgb(git_color(theme, git_status)))
                .bg(rgb(row_bg)),
        ),
    ]))
    .style(Style::default().bg(rgb(row_bg)))
}

fn fit_name(name: &str, width: usize) -> String {
    if name.width() <= width {
        return name.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut remaining = width - 1;
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

fn git_color(theme: &Theme, git: GitStatus) -> forge_theme::Rgb {
    match git {
        GitStatus::Modified => theme.git_modified,
        GitStatus::Added => theme.git_added,
        GitStatus::Deleted => theme.git_deleted,
        GitStatus::Renamed => theme.git_renamed,
        GitStatus::Untracked => theme.git_untracked,
        GitStatus::Conflicted => theme.git_conflicted,
        GitStatus::Ignored => theme.git_ignored,
        GitStatus::Clean => theme.fg,
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_selected_visible, fit_name, list_index_at};
    use ratatui::layout::Rect;

    #[test]
    fn long_names_fit_terminal_cells_without_cutting_wide_characters() {
        assert_eq!(fit_name("中文文件.rs", 7), "中文文…");
        assert_eq!(fit_name("中文.rs", 7), "中文.rs");
        assert_eq!(fit_name("中文.rs", 1), "…");
        assert_eq!(fit_name("中文.rs", 0), "");
    }

    #[test]
    fn scrolls_down_when_selected_past_viewport() {
        let mut scroll = 0;
        ensure_selected_visible(15, &mut scroll, 10, 50);
        assert_eq!(scroll, 6);
    }

    #[test]
    fn scrolls_up_when_selected_above_viewport() {
        let mut scroll = 20;
        ensure_selected_visible(5, &mut scroll, 10, 50);
        assert_eq!(scroll, 5);
    }

    #[test]
    fn clamps_scroll_when_list_shrinks() {
        let mut scroll = 40;
        ensure_selected_visible(0, &mut scroll, 10, 12);
        assert_eq!(scroll, 0);
    }

    #[test]
    fn empty_list_resets_scroll() {
        let mut scroll = 3;
        ensure_selected_visible(0, &mut scroll, 10, 0);
        assert_eq!(scroll, 0);
    }

    #[test]
    fn hit_test_accounts_for_scroll_and_inner() {
        let inner = Rect {
            x: 1,
            y: 2,
            width: 20,
            height: 10,
        };
        assert_eq!(list_index_at(inner, 5, 40, 5, 2), Some(5));
        assert_eq!(list_index_at(inner, 5, 40, 5, 4), Some(7));
        assert_eq!(list_index_at(inner, 5, 40, 0, 2), None);
        assert_eq!(list_index_at(inner, 5, 6, 5, 4), None);
    }
}

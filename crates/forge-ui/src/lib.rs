//! Terminal UI widgets and layout helpers (Ratatui).
//!
//! **Modern Minimal TUI**: weak borders, clear focus, compact density,
//! keyboard-first. Layout follows Helix/Yazi - not a GUI IDE clone.

#![forbid(unsafe_code)]

mod context_menu;
mod file_list;
mod tree;
mod icons;
mod preview_pane;
mod staging;
mod style;

use forge_theme::Theme;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub use context_menu::{
    diff_items, draw_context_menu, file_list_items, staging_items, ContextAction, ContextMenu,
    ContextMenuItem, ContextTarget,
};
pub use file_list::{
    draw_file_list, ensure_selected_visible, list_index_at, FileListState,
};
pub use tree::{find_parent_row, flatten_tree, TreeRow};
pub use preview_pane::{draw_preview, diff_render_hunk_start, preview_line_count, scroll_to_hunk, PreviewView, TextLang};
pub use staging::{
    build_scm_rows, draw_commit_modal, draw_git_strip, draw_scm_sidebar, draw_staging, scm_entry,
    scm_index_at, scm_jump_to_column, staging_index_at, ScmRow, StagingColumn, StagingLayout,
    StagingState,
};
pub use style::{color_mode, set_color_mode};
pub use icons::{icon_mode, init_from_env as init_icons_from_env, set_icon_mode, IconMode, Icons};
pub use forge_theme::ColorMode;

use style::{chrome, chrome_hotkey, fg_bg, rgb};

/// Legacy constant — bottom staging strip removed (VS Code sidebar layout).
#[allow(dead_code)]
pub const STAGING_EXPANDED_HEIGHT: u16 = 10;

/// Which pane currently receives keyboard focus / mouse-wheel fallback.
///
/// `List` = left sidebar (Files tree **or** Git SCM, depending on [`ActivityTab`]).
/// `Preview` = right pane. `Staging` is deprecated (mapped to List in app logic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    #[default]
    List,
    Preview,
    /// Deprecated: prefer [`PaneFocus::List`] + [`ActivityTab::Git`].
    Staging,
}

/// Left-sidebar mode: file tree vs Source Control (VS Code–style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityTab {
    #[default]
    Files,
    Search,
    Git,
}

/// Hit regions recorded on the last draw (screen coords).
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneLayout {
    pub list: Rect,
    pub list_inner: Rect,
    pub preview: Rect,
    pub preview_inner: Rect,
    pub staging: StagingLayout,
    pub header: Rect,
    pub footer: Rect,
    pub hotkeys: Rect,
    /// Always empty in Modern Minimal (no activity strip).
    pub activity: Rect,
    pub window: Rect,
    /// False when terminal width &lt; 80 (single-pane Tab switch).
    pub dual_pane: bool,
}

impl PaneLayout {
    pub fn contains_list(&self, column: u16, row: u16) -> bool {
        self.list.contains(Position {
            x: column,
            y: row,
        })
    }

    pub fn contains_preview(&self, column: u16, row: u16) -> bool {
        self.preview.contains(Position {
            x: column,
            y: row,
        })
    }

    pub fn contains_staging(&self, column: u16, row: u16) -> bool {
        self.staging.contains(column, row)
    }

    pub fn contains_activity(&self, column: u16, row: u16) -> bool {
        self.activity.contains(Position {
            x: column,
            y: row,
        })
    }
}

/// Full-frame view model.
#[derive(Debug)]
pub struct ShellState<'a> {
    pub cwd: &'a str,
    pub git_label: &'a str,
    pub branch: &'a str,
    pub status: &'a str,
    pub list: FileListState<'a>,
    pub preview_title: &'a str,
    pub preview: PreviewView<'a>,
    pub preview_scroll: &'a mut usize,
    pub staging: StagingState<'a>,
    pub focus: PaneFocus,
    pub commit_input: Option<&'a str>,
    pub context_menu: Option<&'a mut ContextMenu>,
    pub in_repo: bool,
    pub activity: ActivityTab,
}

/// Root UI shell.
#[derive(Debug)]
pub struct UiShell {
    pub theme: Theme,
}

impl UiShell {
    pub fn new(theme: Theme) -> Self {
        Self { theme }
    }

    /// Draw VS Code–style chrome: Header(2) / Main(sidebar|preview) / Footer(2).
    /// No bottom staging strip — Git reuses the left sidebar via [`ActivityTab`].
    pub fn draw(&self, frame: &mut Frame<'_>, state: &mut ShellState<'_>) -> PaneLayout {
        let theme = &self.theme;
        let desktop = frame.area();
        style::fill_desktop(frame, desktop, theme);

        // Header(2) | Main(Min 8) | Footer(2)
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(desktop);

        let header = rows[0];
        let main = rows[1];
        let footer = rows[2];

        let dirty = state.staging.unstaged.len() + state.staging.staged.len();
        draw_header(
            frame,
            header,
            theme,
            state.cwd,
            state.branch,
            dirty,
            state.in_repo,
        );

        let dual_pane = desktop.width >= 80;
        let sidebar_focus = matches!(state.focus, PaneFocus::List | PaneFocus::Staging);
        let (list_area, preview_area) = draw_main_with_sep(
            frame,
            main,
            dual_pane,
            state.focus,
            state.activity,
            theme,
        );

        let git_sidebar = matches!(state.activity, ActivityTab::Git);
        let (list_inner, staging_layout) = if list_area.width > 0 && list_area.height > 0 {
            if git_sidebar {
                state.staging.focused = sidebar_focus;
                let layout = draw_scm_sidebar(frame, list_area, theme, &mut state.staging);
                (layout.inner, layout)
            } else {
                state.list.focused = sidebar_focus;
                let inner = draw_file_list(frame, list_area, theme, &mut state.list);
                (
                    inner,
                    StagingLayout {
                        collapsed: true,
                        ..StagingLayout::default()
                    },
                )
            }
        } else {
            (
                Rect::default(),
                StagingLayout {
                    collapsed: true,
                    ..StagingLayout::default()
                },
            )
        };

        let preview_inner = if preview_area.width > 0 && preview_area.height > 0 {
            draw_preview(
                frame,
                preview_area,
                theme,
                state.preview_title,
                state.preview,
                state.preview_scroll,
                matches!(state.focus, PaneFocus::Preview),
            )
        } else {
            Rect::default()
        };

        // Git status left chip uses basename only (side lives in the preview header).
        let selected_display;
        let selected_name = if git_sidebar {
            let rows = build_scm_rows(state.staging.unstaged, state.staging.staged);
            selected_display = rows
                .get(state.staging.selected)
                .and_then(|r| {
                    let entry = scm_entry(*r, state.staging.unstaged, state.staging.staged)?;
                    let base = entry
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(entry.path.as_str());
                    Some(base.to_string())
                })
                .unwrap_or_default();
            selected_display.as_str()
        } else {
            selected_display = state
                .list
                .rows
                .get(state.list.selected)
                .map(|r| r.entry.name.clone())
                .unwrap_or_default();
            selected_display.as_str()
        };
        let mode_label = match (state.focus, state.activity) {
            (PaneFocus::Preview, _) => "VIEW",
            (_, ActivityTab::Git) => "GIT",
            _ => "FILES",
        };
        let is_diff_preview = matches!(state.preview, PreviewView::Diff { .. });
        let cursor_line = match state.focus {
            // Diff pane has no meaningful caret — omit Ln/Col (keep UTF-8 · DIFF).
            PaneFocus::Preview if is_diff_preview => String::new(),
            PaneFocus::Preview => {
                format!("Ln {}, Col 1", state.preview_scroll.saturating_add(1))
            }
            PaneFocus::List | PaneFocus::Staging if git_sidebar => {
                format!("Ln {}, Col 1", state.staging.selected.saturating_add(1))
            }
            PaneFocus::List | PaneFocus::Staging => {
                if state.list.rows.is_empty() {
                    "Ln 0, Col 0".into()
                } else {
                    format!("Ln {}, Col 1", state.list.selected.saturating_add(1))
                }
            }
        };
        let file_ty = match state.preview {
            PreviewView::Diff { .. } => "DIFF".to_string(),
            _ => file_type_label(selected_name, state.preview_title),
        };

        let footer_chunks =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(footer);
        draw_status_line(
            frame,
            footer_chunks[0],
            theme,
            mode_label,
            selected_name,
            &cursor_line,
            &file_ty,
            state.status,
        );
        draw_hotkey_legend(
            frame,
            footer_chunks[1],
            theme,
            state.focus,
            state.activity,
        );

        if let Some(input) = state.commit_input {
            draw_commit_modal(frame, theme, input);
        }
        if let Some(menu) = state.context_menu.as_mut() {
            draw_context_menu(frame, theme, menu);
        }

        PaneLayout {
            list: list_area,
            list_inner,
            preview: preview_area,
            preview_inner,
            staging: staging_layout,
            header,
            footer: footer_chunks[0],
            hotkeys: footer_chunks[1],
            activity: Rect::default(),
            window: desktop,
            dual_pane,
        }
    }
}

impl Default for UiShell {
    fn default() -> Self {
        Self::new(Theme::modern_ide())
    }
}

fn files_pane_width(total_cols: u16, git: bool) -> u16 {
    let base = if total_cols >= 120 {
        36
    } else {
        // 80..119 → 24..=30
        let t = total_cols.saturating_sub(80); // 0..39
        24 + (t * 6) / 39
    };
    if git {
        // SCM paths are longer — give the sidebar a few extra columns.
        (base + 4).min(42)
    } else {
        base
    }
}

fn draw_main_with_sep(
    frame: &mut Frame<'_>,
    main: Rect,
    dual_pane: bool,
    focus: PaneFocus,
    activity: ActivityTab,
    theme: &Theme,
) -> (Rect, Rect) {
    if main.width == 0 || main.height == 0 {
        return (Rect::default(), Rect::default());
    }

    if !dual_pane {
        return match focus {
            PaneFocus::Preview => (Rect::default(), main),
            // Staging focus in narrow mode: keep FILES visible in main.
            _ => (main, Rect::default()),
        };
    }

    let git = matches!(activity, ActivityTab::Git);
    let files_w = files_pane_width(main.width, git).min(main.width.saturating_sub(2));
    let cols = Layout::horizontal([
        Constraint::Length(files_w),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(main);

    style::draw_pane_separator(frame, cols[1], theme);
    (cols[0], cols[2])
}

fn draw_header(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    cwd: &str,
    branch: &str,
    dirty: usize,
    in_repo: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

    // Line 0: forge + path (no colorful pills)
    let short = truncate_path(cwd, area.width.saturating_sub(8) as usize);
    let top = Line::from(vec![
        Span::styled(
            " forge",
            Style::default()
                .fg(rgb(theme.accent))
                .bg(rgb(theme.header_bg))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  ",
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.header_bg)),
        ),
        Span::styled(
            short,
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.header_bg)),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(top).style(fg_bg(theme.fg, theme.header_bg)),
        rows[0],
    );

    // Line 1: Edit-ish `git:⎇ branch · N modified` — dirty phrase uses warning fg
    let branch_icon = crate::icons::Icons::branch();
    let branch_prefix = if branch_icon.is_empty() {
        String::new()
    } else {
        format!("{branch_icon} ")
    };
    let header_bg = Style::default().bg(rgb(theme.header_bg));
    let muted = header_bg.fg(rgb(theme.muted));
    let fg = header_bg.fg(rgb(theme.fg));
    let warn = header_bg.fg(rgb(theme.warning));

    let accent = header_bg.fg(rgb(theme.accent));
    let git_line = if in_repo && !branch.is_empty() {
        if dirty > 0 {
            Line::from(vec![
                Span::styled(" git:", muted),
                Span::styled(branch_prefix.clone(), accent),
                Span::styled(branch.to_string(), fg),
                Span::styled(format!(" · {dirty} modified"), warn),
            ])
        } else {
            Line::from(vec![
                Span::styled(" git:", muted),
                Span::styled(branch_prefix.clone(), accent),
                Span::styled(format!("{branch} · clean"), muted),
            ])
        }
    } else if in_repo {
        Line::from(vec![
            Span::styled(" git:", muted),
            Span::styled(branch_prefix, accent),
            Span::styled("· clean", muted),
        ])
    } else {
        Line::from(Span::styled(" no git", muted))
    };
    frame.render_widget(
        Paragraph::new(git_line).style(fg_bg(theme.muted, theme.header_bg)),
        rows[1],
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_status_line(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    mode: &str,
    selected: &str,
    cursor: &str,
    file_ty: &str,
    status: &str,
) {
    // Status only — never duplicate hotkeys here.
    let left = if selected.is_empty() {
        format!(" {mode} ")
    } else {
        format!(" {mode} · {selected} ")
    };
    let meta = if cursor.is_empty() {
        format!(" UTF-8 · {file_ty} ")
    } else {
        format!(" {cursor} · UTF-8 · {file_ty} ")
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(
        left.clone(),
        Style::default()
            .fg(rgb(theme.chrome_fg))
            .bg(rgb(theme.chrome_bg)),
    ));

    let left_len = left.chars().count();
    let meta_len = meta.chars().count();
    let pad = (area.width as usize)
        .saturating_sub(left_len)
        .saturating_sub(meta_len);

    if pad > 0 {
        let middle = if status.is_empty() {
            " ".repeat(pad)
        } else {
            let max = pad.saturating_sub(2);
            let t: String = status.chars().take(max).collect();
            let mut s = format!(" {t}");
            while s.chars().count() < pad {
                s.push(' ');
            }
            s
        };
        spans.push(Span::styled(
            middle,
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.chrome_bg)),
        ));
    }

    spans.push(Span::styled(
        meta,
        Style::default()
            .fg(rgb(theme.muted))
            .bg(rgb(theme.chrome_bg)),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(chrome(theme)), area);
}

fn draw_hotkey_legend(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    focus: PaneFocus,
    activity: ActivityTab,
) {
    let git = matches!(activity, ActivityTab::Git);
    let keys: &[(&str, &str)] = match focus {
        PaneFocus::List | PaneFocus::Staging if git => &[
            ("j/k", "Move"),
            ("Enter", "Diff"),
            ("s/u", "Stage"),
            ("c", "Commit"),
            ("[/]", "Sect"),
            ("g", "Files"),
            ("Tab", "Pane"),
            ("q", "Quit"),
        ],
        PaneFocus::List | PaneFocus::Staging => &[
            ("j/k", "Move"),
            ("Enter", "Open"),
            ("g", "Git"),
            ("Tab", "Pane"),
            ("q", "Quit"),
        ],
        PaneFocus::Preview if git => &[
            ("j/k", "Scroll"),
            ("n/N", "Hunk"),
            ("s/u", "Hunk"),
            ("Esc", "SCM"),
            ("Tab", "Pane"),
            ("q", "Quit"),
        ],
        PaneFocus::Preview => &[
            ("j/k", "Scroll"),
            ("Tab", "Pane"),
            ("Esc", "Files"),
            ("q", "Quit"),
        ],
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" ", chrome(theme)));
    for (i, (key, label)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", chrome(theme)));
        }
        spans.push(Span::styled((*key).to_string(), chrome_hotkey(theme)));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default()
                .fg(rgb(theme.chrome_fg))
                .bg(rgb(theme.chrome_bg)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(chrome(theme)), area);
}

fn file_type_label(name: &str, preview_title: &str) -> String {
    let src = if name.is_empty() { preview_title } else { name };
    if src.is_empty() {
        return "-".into();
    }
    if let Some(ext) = src.rsplit('.').next() {
        if ext != src && !ext.is_empty() && ext.len() <= 8 {
            return ext.to_ascii_uppercase();
        }
    }
    if src.ends_with('/') {
        return "DIR".into();
    }
    "FILE".into()
}

fn truncate_path(path: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = chars[chars.len().saturating_sub(keep)..].iter().collect();
    format!("...{tail}")
}

/// Public helper used by draw - keep split + sep together for call sites that need both.
pub fn layout_main_panes(
    frame: &mut Frame<'_>,
    main: Rect,
    dual_pane: bool,
    focus: PaneFocus,
    theme: &Theme,
) -> (Rect, Rect) {
    draw_main_with_sep(frame, main, dual_pane, focus, ActivityTab::Files, theme)
}

#[cfg(test)]
mod tests {
    use super::files_pane_width;

    #[test]
    fn files_width_wide() {
        assert_eq!(files_pane_width(180, false), 36);
        assert_eq!(files_pane_width(120, false), 36);
        assert_eq!(files_pane_width(180, true), 40);
        assert_eq!(files_pane_width(120, true), 40);
    }

    #[test]
    fn files_width_medium() {
        assert_eq!(files_pane_width(80, false), 24);
        assert_eq!(files_pane_width(119, false), 30);
        assert_eq!(files_pane_width(80, true), 28);
        // base 36+4 capped at 42 for wide; medium 30+4=34
        assert_eq!(files_pane_width(119, true), 34);
    }

    #[test]
    fn files_width_git_capped() {
        // If base were ever high, git bump still caps at 42.
        assert_eq!(files_pane_width(200, true), 40); // 36+4
    }
}

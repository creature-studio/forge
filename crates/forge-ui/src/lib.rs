//! Terminal UI widgets and layout helpers (Ratatui).
//!
//! Native content renderers and view models hosted by ratatui-kit components.

#![forbid(unsafe_code)]

mod context_menu;
mod file_list;
mod icons;
mod preview_pane;
mod staging;
mod style;
mod tree;

use ratatui::layout::{Position, Rect};

pub use context_menu::{
    diff_items, draw_context_menu, file_list_items, staging_items, ContextAction, ContextMenu,
    ContextMenuItem, ContextTarget,
};
pub use file_list::{draw_file_list, ensure_selected_visible, list_index_at, FileListState};
pub use forge_theme::ColorMode;
pub use icons::{icon_mode, init_from_env as init_icons_from_env, set_icon_mode, IconMode, Icons};
pub use preview_pane::{
    diff_render_hunk_start, draw_preview, preview_line_count, scroll_to_hunk, PreviewView,
    SplitRowsCache, TextLang,
};
pub use staging::{
    build_scm_rows, draw_commit_modal, draw_git_strip, draw_scm_sidebar, draw_staging,
    expand_ancestors_for_path, find_scm_parent_row, scm_entry, scm_index_at, scm_jump_to_column,
    staging_index_at, ScmRow, StagingColumn, StagingLayout, StagingState,
};
pub use style::rgb as to_color;
pub use style::{color_mode, set_color_mode};
pub use tree::{find_parent_row, flatten_tree, TreeRow};

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
    /// Visible Files/Git tab strip.
    pub activity: Rect,
    pub files_tab: Rect,
    pub git_tab: Rect,
    pub window: Rect,
    /// False when terminal width &lt; 80 (single-pane Tab switch).
    pub dual_pane: bool,
}

impl PaneLayout {
    pub fn contains_list(&self, column: u16, row: u16) -> bool {
        self.list.contains(Position { x: column, y: row })
    }

    pub fn contains_preview(&self, column: u16, row: u16) -> bool {
        self.preview.contains(Position { x: column, y: row })
    }

    pub fn contains_staging(&self, column: u16, row: u16) -> bool {
        self.staging.contains(column, row)
    }

    pub fn contains_activity(&self, column: u16, row: u16) -> bool {
        self.activity.contains(Position { x: column, y: row })
    }
}

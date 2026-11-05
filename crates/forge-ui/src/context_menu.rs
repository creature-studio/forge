//! Floating right-click context menu.

use std::path::PathBuf;

use forge_theme::Theme;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::staging::StagingColumn;
use crate::style::rgb;

/// Action ids emitted when the user confirms a menu item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextAction {
    Open,
    Preview,
    Stage,
    Unstage,
    StageFolder,
    UnstageFolder,
    Diff,
    Commit,
    CopyPath,
    Push,
    Pull,
    StageHunk,
    UnstageHunk,
    NewFile,
    NewDirectory,
    Rename,
    Copy,
    Move,
    DeleteFile,
    DeleteDirectory,
    EditText,
    ExternalEditor,
}

impl ContextAction {
    pub fn label(self) -> &'static str {
        match self {
            ContextAction::Open => "Open",
            ContextAction::Preview => "Preview",
            ContextAction::Stage => "Stage file",
            ContextAction::Unstage => "Unstage file",
            ContextAction::StageFolder => "Stage folder",
            ContextAction::UnstageFolder => "Unstage folder",
            ContextAction::Diff => "Diff",
            ContextAction::Commit => "Commit staged...",
            ContextAction::CopyPath => "Copy path",
            ContextAction::Push => "Push repository",
            ContextAction::Pull => "Pull repository",
            ContextAction::StageHunk => "Stage hunk",
            ContextAction::UnstageHunk => "Unstage hunk",
            ContextAction::NewFile => "New file…",
            ContextAction::NewDirectory => "New folder…",
            ContextAction::Rename => "Rename…",
            ContextAction::Copy => "Copy to folder…",
            ContextAction::Move => "Move to folder…",
            ContextAction::DeleteFile => "Delete file…",
            ContextAction::DeleteDirectory => "Delete folder…",
            ContextAction::EditText => "Edit text",
            ContextAction::ExternalEditor => "Open in $EDITOR",
        }
    }
}

/// Where the menu was opened from (app uses this to resolve the target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextTarget {
    FileList { path: PathBuf },
    FileRoot { path: PathBuf },
    Staging { column: StagingColumn, index: usize },
    StagingFolder { column: StagingColumn, path: String },
    Diff,
}

/// One row in the floating menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMenuItem {
    pub action: ContextAction,
    pub enabled: bool,
}

/// Live floating menu state.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub items: Vec<ContextMenuItem>,
    pub selected: usize,
    pub scroll: usize,
    pub anchor_x: u16,
    pub anchor_y: u16,
    pub target: ContextTarget,
    /// Screen rect of the last draw (for hit-testing).
    pub area: Rect,
}

impl ContextMenu {
    pub fn new(
        items: Vec<ContextMenuItem>,
        anchor_x: u16,
        anchor_y: u16,
        target: ContextTarget,
    ) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let selected = items.iter().position(|i| i.enabled).unwrap_or(0);
        Some(Self {
            items,
            selected,
            scroll: 0,
            anchor_x,
            anchor_y,
            target,
            area: Rect::default(),
        })
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let mut next = self.selected as isize;
        for _ in 0..len {
            next = (next + delta).rem_euclid(len);
            if self.items[next as usize].enabled {
                self.selected = next as usize;
                return;
            }
        }
    }

    pub fn current_action(&self) -> Option<ContextAction> {
        self.items
            .get(self.selected)
            .and_then(|i| if i.enabled { Some(i.action) } else { None })
    }

    pub fn action_at_row(&self, column: u16, row: u16) -> Option<(usize, ContextAction)> {
        if !self.area.contains(Position { x: column, y: row }) {
            return None;
        }
        // Top/bottom borders: first content row is y + 1.
        if row <= self.area.y
            || row
                >= self
                    .area
                    .y
                    .saturating_add(self.area.height.saturating_sub(1))
        {
            return None;
        }
        let idx = (row - self.area.y - 1) as usize + self.scroll;
        self.items.get(idx).map(|item| (idx, item.action))
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.area.contains(Position { x: column, y: row })
    }

    /// Update selection from mouse hover when over a row.
    pub fn hover_at(&mut self, column: u16, row: u16) {
        if let Some((idx, _)) = self.action_at_row(column, row) {
            if self.items.get(idx).is_some_and(|i| i.enabled) {
                self.selected = idx;
            }
        }
    }
}

/// Build file-list context items (only relevant / enabled ones).
pub fn file_list_items(is_dir: bool) -> Vec<ContextMenuItem> {
    let mut items = Vec::new();
    if is_dir {
        items.push(ContextMenuItem {
            action: ContextAction::Open,
            enabled: true,
        });
    } else {
        items.push(ContextMenuItem {
            action: ContextAction::EditText,
            enabled: true,
        });
        items.push(ContextMenuItem {
            action: ContextAction::ExternalEditor,
            enabled: true,
        });
        items.push(ContextMenuItem {
            action: ContextAction::Preview,
            enabled: true,
        });
    }
    for action in [
        ContextAction::NewFile,
        ContextAction::NewDirectory,
        ContextAction::Rename,
        ContextAction::Copy,
        ContextAction::Move,
        if is_dir {
            ContextAction::DeleteDirectory
        } else {
            ContextAction::DeleteFile
        },
    ] {
        items.push(ContextMenuItem {
            action,
            enabled: true,
        });
    }
    items.push(ContextMenuItem {
        action: ContextAction::CopyPath,
        enabled: true,
    });
    items
}

/// Build staging-panel context items.
pub fn staging_items(
    column: StagingColumn,
    has_staged: bool,
    in_repo: bool,
) -> Vec<ContextMenuItem> {
    let mut items = Vec::new();
    match column {
        StagingColumn::Unstaged => {
            items.push(ContextMenuItem {
                action: ContextAction::Stage,
                enabled: true,
            });
        }
        StagingColumn::Staged => {
            items.push(ContextMenuItem {
                action: ContextAction::Unstage,
                enabled: true,
            });
        }
    }
    items.push(ContextMenuItem {
        action: ContextAction::Diff,
        enabled: true,
    });
    items.push(ContextMenuItem {
        action: ContextAction::Commit,
        enabled: has_staged,
    });
    items.push(ContextMenuItem {
        action: ContextAction::Pull,
        enabled: in_repo,
    });
    items.push(ContextMenuItem {
        action: ContextAction::Push,
        enabled: in_repo,
    });
    items.push(ContextMenuItem {
        action: ContextAction::CopyPath,
        enabled: true,
    });
    items
}

/// Diff-pane context items for hunk ops.
pub fn diff_items(side_unstaged: bool, has_hunk: bool) -> Vec<ContextMenuItem> {
    let mut items = Vec::new();
    if side_unstaged {
        items.push(ContextMenuItem {
            action: ContextAction::StageHunk,
            enabled: has_hunk,
        });
    } else {
        items.push(ContextMenuItem {
            action: ContextAction::UnstageHunk,
            enabled: has_hunk,
        });
    }
    items.push(ContextMenuItem {
        action: ContextAction::CopyPath,
        enabled: true,
    });
    items
}

/// Draw a compact floating menu clipped to the frame. Updates `menu.area`.
pub fn draw_context_menu(frame: &mut Frame<'_>, theme: &Theme, menu: &mut ContextMenu) {
    let frame_area = frame.area();
    let label_w = menu
        .items
        .iter()
        .map(|i| i.action.label().chars().count() + 4)
        .max()
        .unwrap_or(12)
        .clamp(12, 30) as u16;
    let width = (label_w + 2).min(frame_area.width); // left/right border
    let height = (menu.items.len() as u16)
        .saturating_add(2)
        .min(frame_area.height.max(1));

    let mut x = menu.anchor_x.min(
        frame_area
            .x
            .saturating_add(frame_area.width.saturating_sub(1)),
    );
    let mut y = menu.anchor_y.min(
        frame_area
            .y
            .saturating_add(frame_area.height.saturating_sub(1)),
    );
    if x + width > frame_area.x + frame_area.width {
        x = (frame_area.x + frame_area.width).saturating_sub(width);
    }
    if y + height > frame_area.y + frame_area.height {
        y = (frame_area.y + frame_area.height).saturating_sub(height);
    }

    let area = Rect {
        x,
        y,
        width,
        height,
    };
    menu.area = area;

    frame.render_widget(Clear, area);

    // Edit File menu: dark surface, white text, solid cyan selected row.
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
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(menu.items.len());
    if menu.selected < menu.scroll {
        menu.scroll = menu.selected;
    }
    if menu.selected >= menu.scroll + inner.height as usize {
        menu.scroll = menu
            .selected
            .saturating_sub(inner.height.saturating_sub(1) as usize);
    }
    for (i, item) in menu
        .items
        .iter()
        .enumerate()
        .skip(menu.scroll)
        .take(inner.height as usize)
    {
        // Edit File menu: solid cyan selected row, no `>` gutter.
        let label = format!(" {} ", item.action.label());
        let style = if !item.enabled {
            Style::default().fg(rgb(theme.muted)).bg(rgb(theme.menu_bg))
        } else if i == menu.selected {
            Style::default()
                .fg(rgb(theme.selection_fg))
                .bg(rgb(theme.selection_bg))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(rgb(theme.menu_fg))
                .bg(rgb(theme.menu_bg))
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

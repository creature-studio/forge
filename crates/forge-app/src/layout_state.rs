//! Workbench layout geometry and resizing, independent of domain state.
use super::{splitter::SplitterState, PaneFocus, PaneLayout};

pub(super) struct LayoutState {
    sidebar_visible: bool,
    sidebar_width: u16,
    terminal_height: Option<u16>,
    pub(super) splitter: SplitterState,
    pub(super) geometry: PaneLayout,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            sidebar_width: 30,
            terminal_height: None,
            splitter: SplitterState::default(),
            geometry: PaneLayout::default(),
        }
    }
}

pub(super) struct PanePlan {
    pub(super) dual: bool,
    pub(super) sidebar_width: u16,
    pub(super) terminal_height: u16,
    pub(super) show_list: bool,
    pub(super) show_preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SidebarTransition {
    Unchanged,
    Revealed,
    Hidden,
}

impl LayoutState {
    pub(super) fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }
    #[cfg(test)]
    pub(super) fn sidebar_width(&self) -> u16 {
        self.sidebar_width
    }
    #[cfg(test)]
    pub(super) fn terminal_height(&self) -> Option<u16> {
        self.terminal_height
    }
    pub(super) fn set_sidebar_visible(&mut self, visible: bool) -> SidebarTransition {
        let changed = self.sidebar_visible != visible;
        self.sidebar_visible = visible;
        match (changed, visible) {
            (true, true) => SidebarTransition::Revealed,
            (true, false) => SidebarTransition::Hidden,
            (false, _) => SidebarTransition::Unchanged,
        }
    }

    pub(super) fn plan(
        &self,
        width: u16,
        height: u16,
        focus: PaneFocus,
        terminal_open: bool,
        terminal_maximized: bool,
        activity_width: u16,
    ) -> PanePlan {
        let dual = width >= 80;
        let sidebar_width = if dual {
            self.sidebar_width
                .clamp(20, width.saturating_sub(29).max(20))
        } else {
            width.saturating_sub(activity_width)
        };
        PanePlan {
            dual,
            sidebar_width,
            terminal_height: if terminal_open && !terminal_maximized {
                terminal_panel_height(height, self.terminal_height)
            } else {
                0
            },
            show_list: self.sidebar_visible && (dual || focus != PaneFocus::Preview),
            show_preview: !self.sidebar_visible || dual || focus == PaneFocus::Preview,
        }
    }

    fn sidebar_max(&self) -> u16 {
        self.geometry
            .window
            .width
            .saturating_sub(self.geometry.activity.width + 25)
            .max(20)
    }

    pub(super) fn adjust_sidebar(&mut self, delta: i16) {
        if delta < 0 {
            self.sidebar_width = self
                .sidebar_width
                .saturating_sub(delta.unsigned_abs())
                .max(20);
        } else {
            self.sidebar_width = self
                .sidebar_width
                .saturating_add(delta as u16)
                .min(self.sidebar_max());
        }
    }

    pub(super) fn resize_sidebar_at(&mut self, column: u16) {
        let desired = column.saturating_sub(self.geometry.activity.right());
        self.sidebar_width = desired.clamp(20, self.sidebar_max());
    }

    pub(super) fn resize_terminal_at(&mut self, row: u16) {
        let height = self.geometry.window.height;
        if height < 14 {
            return;
        }
        let requested = self.geometry.footer.y.saturating_sub(row.saturating_add(1));
        self.terminal_height = Some(terminal_panel_height(height, Some(requested)));
    }
}

/// Terminal body height in rows; reserve Header, Footer and a dedicated
/// splitter row, plus at least four rows of workspace above the splitter.
pub(super) fn terminal_panel_height(window_height: u16, requested: Option<u16>) -> u16 {
    if window_height < 14 {
        return 0;
    }
    let max = window_height.saturating_sub(1 + 3 + 1 + 4);
    requested
        .unwrap_or((window_height / 3).clamp(5, 16))
        .clamp(5, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn pane_plan_uses_only_geometry_and_focus_inputs() {
        let layout = LayoutState::default();
        let wide = layout.plan(100, 40, PaneFocus::List, true, false, 5);
        assert!(wide.dual && wide.show_list && wide.show_preview);
        assert!(wide.terminal_height > 0);
        let narrow = layout.plan(60, 40, PaneFocus::Preview, true, false, 5);
        assert!(!narrow.dual && !narrow.show_list && narrow.show_preview);
        let maximized = layout.plan(100, 40, PaneFocus::List, true, true, 5);
        assert_eq!(maximized.terminal_height, 0);
        assert_eq!(layout.sidebar_width(), 30);
    }

    #[test]
    fn independent_splitters_do_not_change_each_others_preferences() {
        let mut layout = LayoutState::default();
        layout.geometry.window = Rect::new(0, 0, 120, 43);
        layout.geometry.activity = Rect::new(0, 0, 5, 43);
        layout.geometry.footer = Rect::new(0, 40, 120, 3);
        layout.resize_sidebar_at(60);
        assert_eq!(layout.sidebar_width(), 55);
        assert_eq!(layout.terminal_height(), None);
        layout.resize_terminal_at(20);
        assert_eq!(layout.terminal_height(), Some(19));
        assert_eq!(layout.sidebar_width(), 55);
        layout.set_sidebar_visible(false);
        assert_eq!(layout.sidebar_width(), 55);
        assert_eq!(layout.terminal_height(), Some(19));
        layout.adjust_sidebar(-2);
        assert_eq!(layout.sidebar_width(), 53);
        assert_eq!(layout.terminal_height(), Some(19));
    }
}

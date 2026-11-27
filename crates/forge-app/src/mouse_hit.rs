//! Read-only hit testing against geometry recorded by the last frame.
use super::{ActivityAction, PaneLayout, ScrollbarTarget};
use ratatui::layout::{Position, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneHit {
    List,
    Preview,
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeaderHit {
    Sync,
    OpenFile,
    MarkdownPreview,
    MarkdownCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FindHit {
    Input,
    Close,
    Next,
    Previous,
    MatchCase,
    WholeWord,
    Regex,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalHit {
    Body,
    Tabs,
    Outside,
}

/// The visible PTY's body wins over the tab strip if regions overlap.
/// A hidden PTY owns no pointer region, even if its old geometry survives.
pub(super) fn terminal_at(point: Position, body: Option<Rect>, tabs: Rect) -> TerminalHit {
    match body {
        Some(area) if area.contains(point) => TerminalHit::Body,
        Some(_) if tabs.contains(point) => TerminalHit::Tabs,
        _ => TerminalHit::Outside,
    }
}

/// Borrow existing frame geometry; no allocations or separate hit map.
pub(super) struct HitGeometry<'a> {
    layout: &'a PaneLayout,
}

impl<'a> HitGeometry<'a> {
    pub(super) fn new(layout: &'a PaneLayout) -> Self {
        Self { layout }
    }

    pub(super) fn pane(&self, point: Position) -> PaneHit {
        if self.layout.list.contains(point) {
            PaneHit::List
        } else if self.layout.preview.contains(point) {
            PaneHit::Preview
        } else {
            PaneHit::Outside
        }
    }

    pub(super) fn activity(&self, point: Position) -> Option<ActivityAction> {
        [
            (self.layout.files_tab, ActivityAction::Files),
            (self.layout.git_tab, ActivityAction::Git),
            (self.layout.search_tab, ActivityAction::Search),
            (self.layout.terminal_activity, ActivityAction::Terminal),
            (self.layout.settings_tab, ActivityAction::Settings),
            (self.layout.sidebar_toggle, ActivityAction::Collapse),
        ]
        .into_iter()
        .find_map(|(rect, action)| rect.contains(point).then_some(action))
    }

    pub(super) fn workspace(&self, point: Position) -> bool {
        self.pane(point) != PaneHit::Outside
            || self.layout.activity.contains(point)
            || self.layout.files_tab.contains(point)
            || self.layout.git_tab.contains(point)
            || self.layout.sidebar_toggle.contains(point)
    }

    pub(super) fn header(&self, point: Position) -> Option<HeaderHit> {
        [
            (self.layout.git_sync_status, HeaderHit::Sync),
            (self.layout.open_file_tab, HeaderHit::OpenFile),
            (self.layout.markdown_preview_tab, HeaderHit::MarkdownPreview),
            (self.layout.markdown_code_tab, HeaderHit::MarkdownCode),
        ]
        .into_iter()
        .find_map(|(rect, action)| rect.contains(point).then_some(action))
    }

    pub(super) fn scrollbar_candidates(
        &self,
        point: Position,
    ) -> impl Iterator<Item = ScrollbarTarget> {
        [
            (
                self.layout.preview_horizontal_scrollbar,
                ScrollbarTarget::PreviewHorizontal,
            ),
            (self.layout.preview_scrollbar, ScrollbarTarget::Preview),
            (self.layout.sidebar_scrollbar, ScrollbarTarget::Sidebar),
        ]
        .into_iter()
        .filter_map(move |(rect, target)| rect.contains(point).then_some(target))
    }

    pub(super) fn find(&self, point: Position) -> Option<FindHit> {
        let rect = self.layout.file_find_rect;
        if !rect.contains(point) {
            return None;
        }
        let x = point.x - rect.x;
        let y = point.y - rect.y;
        Some(match y {
            1 => FindHit::Input,
            2 if rect.width >= 34 && x >= rect.width - 3 => FindHit::Close,
            2 if rect.width >= 34 && x >= rect.width - 9 => FindHit::Next,
            2 if rect.width >= 34 && x >= rect.width - 16 => FindHit::Previous,
            2 if (2..6).contains(&x) => FindHit::MatchCase,
            2 if (7..11).contains(&x) => FindHit::WholeWord,
            2 if (12..17).contains(&x) => FindHit::Regex,
            _ => FindHit::Background,
        })
    }

    pub(super) fn overview(&self, point: Position) -> Option<(usize, usize)> {
        let rect = self.layout.overview_ruler;
        rect.contains(point)
            .then(|| ((point.y - rect.y) as usize, rect.height as usize))
    }
}

/// A borrowed projection, including pinned sticky rows, not the domain tree.
pub(super) struct ListHitGeometry<'a> {
    pub area: Rect,
    pub scroll: usize,
    pub sticky: &'a [usize],
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowHit {
    pub index: usize,
    pub pinned: bool,
}

impl ListHitGeometry<'_> {
    pub(super) fn row(&self, point: Position) -> Option<RowHit> {
        let index = forge_ui::sticky_index_at(
            self.area,
            self.scroll,
            self.sticky,
            self.len,
            point.x,
            point.y,
        )?;
        Some(RowHit {
            index,
            pinned: (point.y.saturating_sub(self.area.y) as usize) < self.sticky.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sticky_rows_map_to_original_indices_without_domain_data() {
        let view = ListHitGeometry {
            area: Rect::new(5, 2, 20, 8),
            scroll: 12,
            sticky: &[0, 4],
            len: 16,
        };
        assert_eq!(
            view.row(Position::new(6, 2)),
            Some(RowHit {
                index: 0,
                pinned: true
            })
        );
        assert_eq!(
            view.row(Position::new(6, 3)),
            Some(RowHit {
                index: 4,
                pinned: true
            })
        );
        assert_eq!(
            view.row(Position::new(6, 4)),
            Some(RowHit {
                index: 12,
                pinned: false
            })
        );
        assert_eq!(
            view.row(Position::new(6, 7)),
            Some(RowHit {
                index: 15,
                pinned: false
            })
        );
        assert_eq!(view.row(Position::new(6, 8)), None);
        assert_eq!(view.row(Position::new(25, 3)), None);
    }

    #[test]
    fn terminal_hit_respects_visibility_and_body_precedence() {
        let body = Rect::new(30, 20, 70, 10);
        let tabs = Rect::new(30, 19, 70, 2);
        assert_eq!(
            terminal_at(Position::new(50, 19), Some(body), tabs),
            TerminalHit::Tabs
        );
        assert_eq!(
            terminal_at(Position::new(50, 20), Some(body), tabs),
            TerminalHit::Body
        );
        assert_eq!(
            terminal_at(Position::new(50, 20), None, tabs),
            TerminalHit::Outside
        );
        assert_eq!(
            terminal_at(Position::new(5, 20), Some(body), tabs),
            TerminalHit::Outside
        );
    }

    #[test]
    fn header_and_activity_use_painted_rectangles() {
        let layout = PaneLayout {
            git_sync_status: Rect::new(4, 30, 8, 1),
            open_file_tab: Rect::new(35, 1, 12, 1),
            markdown_preview_tab: Rect::new(50, 1, 7, 1),
            markdown_code_tab: Rect::new(57, 1, 5, 1),
            files_tab: Rect::new(0, 2, 4, 1),
            git_tab: Rect::new(0, 3, 4, 1),
            ..Default::default()
        };
        let hit = HitGeometry::new(&layout);
        assert_eq!(hit.header(Position::new(4, 30)), Some(HeaderHit::Sync));
        assert_eq!(hit.header(Position::new(46, 1)), Some(HeaderHit::OpenFile));
        assert_eq!(
            hit.header(Position::new(56, 1)),
            Some(HeaderHit::MarkdownPreview)
        );
        assert_eq!(
            hit.header(Position::new(57, 1)),
            Some(HeaderHit::MarkdownCode)
        );
        assert_eq!(hit.header(Position::new(62, 1)), None);
        assert_eq!(hit.activity(Position::new(2, 3)), Some(ActivityAction::Git));
        assert_eq!(hit.activity(Position::new(4, 3)), None);
    }

    #[test]
    fn same_point_resolves_against_current_frame_only() {
        let mut layout = PaneLayout {
            list: Rect::new(5, 2, 30, 20),
            preview: Rect::new(35, 2, 50, 20),
            ..Default::default()
        };
        let point = Position::new(39, 10);
        assert_eq!(HitGeometry::new(&layout).pane(point), PaneHit::Preview);
        layout.list.width = 45;
        layout.preview.x = 50;
        assert_eq!(HitGeometry::new(&layout).pane(point), PaneHit::List);
        layout.list = Rect::default();
        assert_eq!(HitGeometry::new(&layout).pane(point), PaneHit::Outside);
    }

    #[test]
    fn scrollbar_candidates_are_geometry_only_and_ordered() {
        let layout = PaneLayout {
            preview_horizontal_scrollbar: Rect::new(20, 12, 20, 1),
            preview_scrollbar: Rect::new(39, 3, 1, 10),
            sidebar_scrollbar: Rect::new(18, 3, 1, 10),
            ..Default::default()
        };
        let hit = HitGeometry::new(&layout);
        assert_eq!(
            hit.scrollbar_candidates(Position::new(39, 12))
                .collect::<Vec<_>>(),
            vec![ScrollbarTarget::PreviewHorizontal, ScrollbarTarget::Preview]
        );
        assert!(hit
            .scrollbar_candidates(Position::new(25, 5))
            .next()
            .is_none());
        assert_eq!(
            hit.scrollbar_candidates(Position::new(18, 5)).next(),
            Some(ScrollbarTarget::Sidebar)
        );
    }

    #[test]
    fn find_targets_respect_width_and_background() {
        for width in [24, 34, 60] {
            let layout = PaneLayout {
                file_find_rect: Rect::new(10, 4, width, 4),
                ..Default::default()
            };
            let hit = HitGeometry::new(&layout);
            assert_eq!(hit.find(Position::new(12, 5)), Some(FindHit::Input));
            assert_eq!(hit.find(Position::new(12, 6)), Some(FindHit::MatchCase));
            assert_eq!(hit.find(Position::new(17, 6)), Some(FindHit::WholeWord));
            assert_eq!(hit.find(Position::new(22, 6)), Some(FindHit::Regex));
            assert_eq!(
                hit.find(Position::new(10 + width - 1, 6)),
                Some(if width >= 34 {
                    FindHit::Close
                } else {
                    FindHit::Background
                })
            );
            assert_eq!(hit.find(Position::new(9, 5)), None);
        }
    }
}

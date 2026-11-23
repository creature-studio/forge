//! Shared vertical and horizontal scrollbars for file trees, Git lists and previews.
//!
//! Geometry, painting and pointer-to-scroll mapping belong together: a caller
//! should never need to reproduce the thumb formula for hit testing.
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Style},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerticalScrollbar {
    pub area: Rect,
    pub first: usize,
    pub total: usize,
}

/// Single-row horizontal scrollbar for unwrapped file and Git Diff previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HorizontalScrollbar {
    pub area: Rect,
    pub first: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarDrag {
    grab: usize,
}

/// Shared thumb geometry keeps both orientations and pointer hit testing in sync.
fn thumb_geometry(extent: usize, first: usize, total: usize) -> Option<(usize, usize, usize)> {
    if extent < 3 || total <= extent {
        return None;
    }
    let max = total - extent;
    let thumb = ((extent * extent + total / 2) / total).clamp(1, extent);
    let travel = extent - thumb;
    let position = (first.min(max) * travel + max / 2) / max;
    Some((thumb, position, max))
}

fn offset_at(
    pointer: u16,
    start: u16,
    extent: usize,
    thumb: usize,
    max: usize,
    drag: ScrollbarDrag,
) -> usize {
    let travel = extent - thumb;
    if travel == 0 {
        return 0;
    }
    let axis_position = pointer.saturating_sub(start) as usize;
    let thumb_position = axis_position.saturating_sub(drag.grab).min(travel);
    (thumb_position * max + travel / 2) / travel
}

impl VerticalScrollbar {
    pub const fn new(area: Rect, first: usize, total: usize) -> Self {
        Self { area, first, total }
    }

    /// (thumb height, thumb top relative to area, maximum scroll offset).
    pub fn geometry(self) -> Option<(usize, usize, usize)> {
        (self.area.width > 0)
            .then_some(())
            .and_then(|_| thumb_geometry(self.area.height as usize, self.first, self.total))
    }

    pub fn contains(self, column: u16, row: u16) -> bool {
        self.geometry().is_some() && self.area.contains(Position::new(column, row))
    }

    /// Start dragging from the thumb, or center the thumb on a track click.
    pub fn begin_drag(self, column: u16, row: u16) -> Option<ScrollbarDrag> {
        if !self.contains(column, row) {
            return None;
        }
        let (thumb, top, _) = self.geometry()?;
        let y = (row - self.area.y) as usize;
        Some(ScrollbarDrag {
            grab: if (top..top + thumb).contains(&y) {
                y - top
            } else {
                thumb / 2
            },
        })
    }

    /// Coordinates are intentionally clamped; mouse capture can extend far
    /// outside the 1-cell lane, including into another pane or the header.
    pub fn scroll_at(self, row: u16, drag: ScrollbarDrag) -> Option<usize> {
        let (thumb, _, max) = self.geometry()?;
        Some(offset_at(
            row,
            self.area.y,
            self.area.height as usize,
            thumb,
            max,
            drag,
        ))
    }

    pub fn draw(self, frame: &mut Frame<'_>, background: Color, track: Color, accent: Color) {
        let Some((thumb, top, _)) = self.geometry() else {
            return;
        };
        let x = self.area.right() - 1;
        for y in 0..self.area.height as usize {
            let active = (top..top + thumb).contains(&y);
            frame.buffer_mut()[(x, self.area.y + y as u16)]
                .set_symbol(if active { "┃" } else { "│" })
                .set_style(
                    Style::default()
                        .bg(background)
                        .fg(if active { accent } else { track }),
                );
        }
    }
}

impl HorizontalScrollbar {
    pub const fn new(area: Rect, first: usize, total: usize) -> Self {
        Self { area, first, total }
    }

    /// (thumb width, thumb left relative to area, maximum horizontal offset).
    pub fn geometry(self) -> Option<(usize, usize, usize)> {
        (self.area.height > 0 && self.area.width >= 6)
            .then_some(())
            .and_then(|_| thumb_geometry(self.area.width as usize, self.first, self.total))
    }

    pub fn contains(self, column: u16, row: u16) -> bool {
        self.geometry().is_some() && self.area.contains(Position::new(column, row))
    }

    /// Grabbing a thumb preserves the pointer's offset within it; track clicks
    /// center the thumb. The same capture continues outside the horizontal lane.
    pub fn begin_drag(self, column: u16, row: u16) -> Option<ScrollbarDrag> {
        if !self.contains(column, row) {
            return None;
        }
        let (thumb, left, _) = self.geometry()?;
        let x = (column - self.area.x) as usize;
        Some(ScrollbarDrag {
            grab: if (left..left + thumb).contains(&x) {
                x - left
            } else {
                thumb / 2
            },
        })
    }

    pub fn scroll_at(self, column: u16, drag: ScrollbarDrag) -> Option<usize> {
        let (thumb, _, max) = self.geometry()?;
        Some(offset_at(
            column,
            self.area.x,
            self.area.width as usize,
            thumb,
            max,
            drag,
        ))
    }

    pub fn draw(self, frame: &mut Frame<'_>, background: Color, track: Color, accent: Color) {
        let Some((thumb, left, _)) = self.geometry() else {
            return;
        };
        for x in 0..self.area.width as usize {
            let active = (left..left + thumb).contains(&x);
            frame.buffer_mut()[(self.area.x + x as u16, self.area.y)]
                .set_symbol(if active { "━" } else { "─" })
                .set_style(
                    Style::default()
                        .bg(background)
                        .fg(if active { accent } else { track }),
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bar(first: usize) -> VerticalScrollbar {
        VerticalScrollbar::new(Rect::new(99, 3, 1, 20), first, 200)
    }

    #[test]
    fn geometry_and_hit_testing_are_consistent() {
        assert_eq!(bar(0).geometry(), Some((2, 0, 180)));
        assert_eq!(bar(180).geometry(), Some((2, 18, 180)));
        assert_eq!(bar(999).geometry(), Some((2, 18, 180)));
        assert_eq!(
            VerticalScrollbar::new(Rect::new(0, 0, 1, 10), 0, 10).geometry(),
            None
        );
        assert!(bar(0).begin_drag(99, 3).is_some());
        assert!(bar(0).begin_drag(90, 3).is_none());
    }

    #[test]
    fn horizontal_scrollbar_thumb_track_and_drag_share_geometry() {
        let bar = |first| HorizontalScrollbar::new(Rect::new(5, 7, 40, 1), first, 200);
        assert_eq!(bar(0).geometry(), Some((8, 0, 160)));
        assert_eq!(bar(160).geometry(), Some((8, 32, 160)));
        assert_eq!(bar(500).geometry(), Some((8, 32, 160)));
        assert!(bar(0).contains(5, 7));
        assert!(!bar(0).contains(5, 8));
        assert!(!bar(0).contains(4, 7));
        let grab = bar(0).begin_drag(7, 7).expect("thumb starts here");
        assert_eq!(bar(0).scroll_at(7, grab), Some(0));
        assert_eq!(bar(0).scroll_at(999, grab), Some(160));
        assert_eq!(bar(160).scroll_at(0, grab), Some(0));
        let track_click = bar(0).begin_drag(44, 7).expect("track is clickable");
        assert_eq!(bar(0).scroll_at(44, track_click), Some(160));
        assert!(
            HorizontalScrollbar::new(Rect::new(0, 0, 40, 1), 0, 40)
                .geometry()
                .is_none(),
            "no thumb when content fits"
        );
        assert!(
            HorizontalScrollbar::new(Rect::new(0, 0, 5, 1), 0, 80)
                .geometry()
                .is_none(),
            "short lanes stay non-interactive"
        );
    }

    #[test]
    fn horizontal_scrollbar_paint_matches_drag_geometry() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(50, 2)).unwrap();
        let bar = HorizontalScrollbar::new(Rect::new(5, 0, 40, 1), 160, 200);
        term.draw(|f| bar.draw(f, Color::Black, Color::Gray, Color::Cyan))
            .unwrap();
        let line: String = (5..45)
            .map(|x| term.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert_eq!(line.chars().filter(|&c| c == '━').count(), 8);
        assert_eq!(line.chars().filter(|&c| c == '─').count(), 32);
        assert!(line.ends_with("━━━━━━━━"), "{line}");
    }

    #[test]
    fn drag_keeps_working_arbitrarily_far_outside_lane() {
        let drag = bar(0).begin_drag(99, 3).unwrap();
        assert_eq!(bar(0).scroll_at(500, drag), Some(180));
        assert_eq!(bar(180).scroll_at(0, drag), Some(0));
        let track_click = bar(0).begin_drag(99, 22).unwrap();
        assert_eq!(bar(0).scroll_at(22, track_click), Some(180));
    }
}

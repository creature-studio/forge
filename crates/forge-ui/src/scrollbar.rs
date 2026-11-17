//! Shared one-column vertical scrollbar for file trees, Git lists and previews.
//!
//! Geometry, painting and pointer-to-scroll mapping belong together: a caller
//! should never need to reproduce the thumb formula for hit testing.
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerticalScrollbar {
    pub area: Rect,
    pub first: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarDrag {
    grab: usize,
}

impl VerticalScrollbar {
    pub const fn new(area: Rect, first: usize, total: usize) -> Self {
        Self { area, first, total }
    }

    /// (thumb height, thumb top relative to area, maximum scroll offset).
    pub fn geometry(self) -> Option<(usize, usize, usize)> {
        let height = self.area.height as usize;
        if self.area.width == 0 || height < 3 || self.total <= height {
            return None;
        }
        let max = self.total - height;
        let thumb = ((height * height + self.total / 2) / self.total).clamp(1, height);
        let travel = height - thumb;
        let top = (self.first.min(max) * travel + max / 2) / max;
        Some((thumb, top, max))
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
        let travel = self.area.height as usize - thumb;
        if travel == 0 {
            return Some(0);
        }
        let y = row.saturating_sub(self.area.y) as usize;
        let top = y.saturating_sub(drag.grab).min(travel);
        Some((top * max + travel / 2) / travel)
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
    fn drag_keeps_working_arbitrarily_far_outside_lane() {
        let drag = bar(0).begin_drag(99, 3).unwrap();
        assert_eq!(bar(0).scroll_at(500, drag), Some(180));
        assert_eq!(bar(180).scroll_at(0, drag), Some(0));
        let track_click = bar(0).begin_drag(99, 22).unwrap();
        assert_eq!(bar(0).scroll_at(22, track_click), Some(180));
    }
}

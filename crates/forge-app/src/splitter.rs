//! Shared, mouse-capturing one-cell panel splitter.
//! The component renders both orientations and owns only the pointer gesture;
//! the workspace decides what changing a pane's size actually means.
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use forge_ui::PaneLayout;
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Style},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gesture {
    None,
    Started,
    Moved(Axis, u16),
    Released(Axis, u16),
}

#[derive(Debug, Default)]
pub(super) struct SplitterState {
    active: Option<Axis>,
    hovered: Option<Axis>,
}

impl SplitterState {
    fn hit(layout: &PaneLayout, sidebar_visible: bool, point: Position) -> Option<Axis> {
        if sidebar_visible && layout.sidebar_divider.contains(point) {
            Some(Axis::Vertical)
        } else if layout.terminal_divider.contains(point) {
            Some(Axis::Horizontal)
        } else {
            None
        }
    }

    pub(super) fn hover(
        &mut self,
        layout: &PaneLayout,
        sidebar_visible: bool,
        point: Position,
        blocked: bool,
    ) {
        self.hovered = if blocked {
            None
        } else {
            Self::hit(layout, sidebar_visible, point)
        };
    }

    /// Called before the child terminal, scrollbars and workspace click router.
    /// Once pressed, the divider captures all left-button drag/release events,
    /// even when the pointer leaves its original rectangle.
    pub(super) fn mouse(
        &mut self,
        event: MouseEvent,
        layout: &PaneLayout,
        sidebar_visible: bool,
        blocked: bool,
    ) -> Gesture {
        let point = Position::new(event.column, event.row);
        if let Some(axis) = self.active {
            let coordinate = match axis {
                Axis::Vertical => event.column,
                Axis::Horizontal => event.row,
            };
            return match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => Gesture::Moved(axis, coordinate),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.active = None;
                    self.hover(layout, sidebar_visible, point, blocked);
                    Gesture::Released(axis, coordinate)
                }
                // A drag already owns the pointer. Never allow its other
                // events to click the workspace or get injected into a PTY.
                _ => Gesture::Started,
            };
        }
        if !blocked && matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(axis) = Self::hit(layout, sidebar_visible, point) {
                self.active = Some(axis);
                self.hovered = Some(axis);
                return Gesture::Started;
            }
        }
        Gesture::None
    }

    pub(super) fn highlighted(&self, axis: Axis) -> bool {
        self.active == Some(axis) || self.hovered == Some(axis)
    }

    #[cfg(test)]
    pub(super) fn dragging(&self, axis: Axis) -> bool {
        self.active == Some(axis)
    }
}

/// One-cell visual splitter, shared by vertical and horizontal panels.
/// Hit testing and pointer ownership are handled by `SplitterState`.
pub(super) struct Splitter;

impl Splitter {
    pub(super) fn draw(
        frame: &mut Frame<'_>,
        area: Rect,
        axis: Axis,
        highlighted: bool,
        border: Color,
        accent: Color,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let style = Style::default().fg(if highlighted { accent } else { border });
        let buffer = frame.buffer_mut();
        match axis {
            Axis::Vertical => {
                let mid = area.height / 2;
                for offset in 0..area.height {
                    let symbol = if area.height >= 3 && offset.abs_diff(mid) <= 1 {
                        "┃"
                    } else {
                        "│"
                    };
                    buffer.set_string(area.x, area.y + offset, symbol, style);
                }
            }
            Axis::Horizontal => {
                for offset in 0..area.width {
                    buffer.set_string(area.x + offset, area.y, "─", style);
                }
                const GRIP: &str = " ━━━ ";
                const GRIP_WIDTH: u16 = 5;
                if area.width >= GRIP_WIDTH {
                    buffer.set_string(area.x + (area.width - GRIP_WIDTH) / 2, area.y, GRIP, style);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
    use ratatui::{backend::TestBackend, Terminal};

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn splitter_captures_vertical_and_horizontal_drags_without_leaking() {
        let layout = PaneLayout {
            sidebar_divider: Rect::new(32, 1, 1, 20),
            terminal_divider: Rect::new(0, 21, 90, 1),
            ..Default::default()
        };
        let mut splitter = SplitterState::default();
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        let at = |state: &mut SplitterState, kind, x, y, blocked| {
            state.mouse(mouse(kind, x, y), &layout, true, blocked)
        };
        assert_eq!(
            at(&mut splitter, down, 32, 5, true),
            Gesture::None,
            "a modal blocks new splitters"
        );
        assert_eq!(at(&mut splitter, down, 32, 5, false), Gesture::Started);
        assert!(splitter.dragging(Axis::Vertical));
        assert_eq!(
            at(&mut splitter, drag, 1, 30, false),
            Gesture::Moved(Axis::Vertical, 1)
        );
        assert_eq!(
            at(&mut splitter, up, 68, 4, true),
            Gesture::Released(Axis::Vertical, 68)
        );
        assert!(!splitter.dragging(Axis::Vertical));
        assert_eq!(at(&mut splitter, down, 55, 21, false), Gesture::Started);
        assert_eq!(
            at(&mut splitter, drag, 32, 5, false),
            Gesture::Moved(Axis::Horizontal, 5)
        );
        assert_eq!(
            at(&mut splitter, up, 32, 5, false),
            Gesture::Released(Axis::Horizontal, 5)
        );
        assert!(!splitter.dragging(Axis::Horizontal));
        splitter.hover(&layout, true, Position::new(32, 10), false);
        assert!(splitter.highlighted(Axis::Vertical));
        splitter.hover(&layout, true, Position::new(32, 10), true);
        assert!(!splitter.highlighted(Axis::Vertical));
        splitter.hover(&layout, false, Position::new(32, 10), false);
        assert!(
            !splitter.highlighted(Axis::Vertical),
            "a collapsed Sidebar has no divider"
        );
    }

    #[test]
    fn both_splitter_orientations_draw_grips_and_highlight() {
        let mut terminal = Terminal::new(TestBackend::new(25, 15)).unwrap();
        terminal
            .draw(|frame| {
                Splitter::draw(
                    frame,
                    Rect::new(10, 1, 1, 13),
                    Axis::Vertical,
                    false,
                    Color::DarkGray,
                    Color::Cyan,
                );
                Splitter::draw(
                    frame,
                    Rect::new(0, 14, 25, 1),
                    Axis::Horizontal,
                    true,
                    Color::DarkGray,
                    Color::Cyan,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(10, 1)].symbol(), "│");
        assert_eq!(buffer[(10, 7)].symbol(), "┃");
        assert_eq!(buffer[(10, 13)].symbol(), "│");
        assert_eq!(buffer[(10, 1)].fg, Color::DarkGray);
        assert_eq!(buffer[(0, 14)].symbol(), "─");
        assert_eq!(buffer[(12, 14)].symbol(), "━");
        assert_eq!(buffer[(12, 14)].fg, Color::Cyan);
    }
}

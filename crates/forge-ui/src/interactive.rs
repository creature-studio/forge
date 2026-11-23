//! Reusable pointer-aware controls: one geometry source for hover and clicks.
//! The view registers each control while painting it; the event handler
//! resolves clicks against precisely the same rectangles.
use ratatui::{
    layout::{Position, Rect},
    style::Style,
    widgets::Paragraph,
    Frame,
};

pub struct HoverTargets<Action> {
    pointer: Option<Position>,
    regions: Vec<(Rect, Action)>,
}

impl<Action> Default for HoverTargets<Action> {
    fn default() -> Self {
        Self {
            pointer: None,
            regions: Vec::new(),
        }
    }
}

impl<Action: Copy> HoverTargets<Action> {
    /// Start a frame without losing the last known mouse position.
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.pointer = Some(Position::new(column, row));
    }

    pub fn clear_hover(&mut self) {
        self.pointer = None;
    }

    pub fn is_hovered(&self, region: Rect) -> bool {
        self.pointer
            .is_some_and(|position| region.contains(position))
    }

    /// Region and action under the most recent pointer position.
    pub fn hovered(&self) -> Option<(Rect, Action)> {
        let point = self.pointer?;
        self.regions
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .copied()
    }

    pub fn action_at(&self, column: u16, row: u16) -> Option<Action> {
        let position = Position::new(column, row);
        self.regions
            .iter()
            .find(|(region, _)| region.contains(position))
            .map(|(_, action)| *action)
    }

    /// Render a clickable text control and register exactly its painted area.
    pub fn draw_text(
        &mut self,
        frame: &mut Frame<'_>,
        rect: Rect,
        action: Action,
        label: &str,
        normal: Style,
        hovered: Style,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let style = if self.is_hovered(rect) {
            hovered
        } else {
            normal
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
        self.regions.push((rect, action));
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Rect, Action)> {
        self.regions.iter()
    }
}

/// A themeable, clickable text link. The painted glyphs and hit target use
/// precisely the same rectangle; the owning view decides where navigation goes.
/// Unlike a web link, this does not control the host terminal mouse cursor.
pub struct Link<'a, Action> {
    label: &'a str,
    action: Action,
    normal: Style,
    hovered: Style,
}

impl<'a, Action: Copy> Link<'a, Action> {
    pub fn new(label: &'a str, action: Action, normal: Style, hovered: Style) -> Self {
        Self {
            label,
            action,
            normal,
            hovered,
        }
    }

    pub fn draw(self, frame: &mut Frame<'_>, rect: Rect, targets: &mut HoverTargets<Action>) {
        targets.draw_text(
            frame,
            rect,
            self.action,
            self.label,
            self.normal,
            self.hovered,
        );
    }
}

impl<'a, Action> IntoIterator for &'a HoverTargets<Action> {
    type Item = &'a (Rect, Action);
    type IntoIter = std::slice::Iter<'a, (Rect, Action)>;
    fn into_iter(self) -> Self::IntoIter {
        self.regions.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        backend::TestBackend,
        style::{Color, Modifier},
        Terminal,
    };

    #[test]
    fn link_uses_a_single_clickable_label_with_hover_style() {
        let mut links = HoverTargets::<usize>::default();
        let mut terminal = Terminal::new(TestBackend::new(30, 2)).unwrap();
        let normal = Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED);
        let hover = Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED);
        let rect = Rect::new(3, 1, 8, 1);
        links.hover_at(4, 1);
        terminal
            .draw(|frame| Link::new("file.rs", 9, normal, hover).draw(frame, rect, &mut links))
            .unwrap();
        assert_eq!(links.action_at(3, 1), Some(9));
        assert_eq!(links.action_at(11, 1), None);
        assert_eq!(terminal.backend().buffer()[(4, 1)].bg, Color::Blue);
        links.hover_at(15, 1);
        links.clear();
        terminal
            .draw(|frame| Link::new("file.rs", 9, normal, hover).draw(frame, rect, &mut links))
            .unwrap();
        assert_ne!(terminal.backend().buffer()[(4, 1)].bg, Color::Blue);
    }

    #[test]
    fn hover_and_click_share_the_same_region() {
        let mut targets = HoverTargets::<u8>::default();
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        let rect = Rect::new(2, 0, 6, 1);
        let normal = Style::default().fg(Color::White).bg(Color::Black);
        let hovered = Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD);
        targets.hover_at(3, 0);
        terminal
            .draw(|frame| targets.draw_text(frame, rect, 7, " Click", normal, hovered))
            .unwrap();
        assert_eq!(targets.action_at(3, 0), Some(7));
        assert_eq!(targets.action_at(8, 0), None);
        assert_eq!(terminal.backend().buffer()[(3, 0)].bg, Color::White);

        targets.hover_at(9, 0);
        targets.clear();
        terminal
            .draw(|frame| targets.draw_text(frame, rect, 7, " Click", normal, hovered))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(3, 0)].bg, Color::Black);
        targets.clear_hover();
        assert!(!targets.is_hovered(rect));
    }
}

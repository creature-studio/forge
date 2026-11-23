//! Small, non-interactive pointer hints for TUI controls.
//! The owning view provides hit regions and the tooltip is painted last so
//! it overlays content without grabbing focus or changing mouse routing.
use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Placement preference; the tooltip flips or clamps when near an edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TooltipPlacement {
    #[default]
    Vertical,
    Right,
}

/// One-line tooltip with optional shortcut, anchored to an existing hitbox.
/// No timers or independent event handling; the pointer owner drives visibility.
pub struct Tooltip<'a> {
    label: &'a str,
    shortcut: Option<&'a str>,
    placement: TooltipPlacement,
}

impl<'a> Tooltip<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            shortcut: None,
            placement: TooltipPlacement::Vertical,
        }
    }

    pub fn shortcut(mut self, shortcut: Option<&'a str>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn placement(mut self, placement: TooltipPlacement) -> Self {
        self.placement = placement;
        self
    }

    fn content(&self) -> String {
        match self.shortcut {
            Some(key) if !key.is_empty() => format!("{}  ·  {}", self.label, key),
            _ => self.label.to_owned(),
        }
    }

    /// Draw strictly inside `bounds`; return the painted rectangle for tests.
    /// Tiny terminals suppress the hint instead of occluding the entire pane.
    pub fn draw(
        &self,
        frame: &mut Frame<'_>,
        anchor: Rect,
        bounds: Rect,
        text_style: Style,
        border_style: Style,
    ) -> Option<Rect> {
        let bounds = bounds.intersection(frame.area());
        if bounds.width < 10 || bounds.height < 4 || anchor.width == 0 {
            return None;
        }
        let max_text_width = usize::from(bounds.width.saturating_sub(4));
        let text = truncate_display(&self.content(), max_text_width);
        let width = (UnicodeWidthStr::width(text.as_str()) + 4) as u16;
        let height = 3;
        let (x, y) = match self.placement {
            TooltipPlacement::Vertical => {
                let left = anchor
                    .x
                    .saturating_add(anchor.width / 2)
                    .saturating_sub(width / 2);
                let x = left.clamp(bounds.x, bounds.right().saturating_sub(width));
                let y = if anchor.bottom().saturating_add(height) <= bounds.bottom() {
                    anchor.bottom()
                } else if anchor.y >= bounds.y.saturating_add(height) {
                    anchor.y - height
                } else {
                    return None;
                };
                (x, y)
            }
            TooltipPlacement::Right => {
                let x = if anchor.right().saturating_add(width) <= bounds.right() {
                    anchor.right()
                } else if anchor.x >= bounds.x.saturating_add(width) {
                    anchor.x - width
                } else {
                    bounds.right().saturating_sub(width)
                };
                let top = anchor.y.saturating_sub(1);
                let y = top.clamp(bounds.y, bounds.bottom().saturating_sub(height));
                (x, y)
            }
        };
        let rect = Rect::new(x, y, width, height);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(format!(" {} ", text))
                .style(text_style)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .style(text_style),
                ),
            rect,
        );
        Some(rect)
    }
}

fn truncate_display(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let available = width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let size = ch.width().unwrap_or(0);
        if used + size > available {
            break;
        }
        output.push(ch);
        used += size;
    }
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    #[test]
    fn tooltip_uses_shortcut_and_stays_inside_narrow_bounds() {
        let mut terminal = Terminal::new(TestBackend::new(45, 12)).unwrap();
        let mut rect = None;
        terminal
            .draw(|frame| {
                rect = Tooltip::new("Create a new terminal")
                    .shortcut(Some("Shift+F8"))
                    .draw(
                        frame,
                        Rect::new(42, 2, 3, 1),
                        Rect::new(0, 2, 45, 8),
                        Style::default().fg(Color::White).bg(Color::Black),
                        Style::default().fg(Color::Yellow),
                    );
            })
            .unwrap();
        let rect = rect.unwrap();
        assert!(rect.right() <= 45 && rect.bottom() <= 10);
        let row = (rect.x..rect.right())
            .map(|x| terminal.backend().buffer()[(x, rect.y + 1)].symbol())
            .collect::<String>();
        assert!(row.contains("Create a new terminal"));
        assert!(row.contains("Shift+F8"));
    }

    #[test]
    fn tooltip_truncates_unicode_without_overflow() {
        let mut terminal = Terminal::new(TestBackend::new(17, 6)).unwrap();
        terminal
            .draw(|frame| {
                let rect = Tooltip::new("终端 Terminal with an extremely long label")
                    .draw(
                        frame,
                        Rect::new(14, 0, 2, 1),
                        Rect::new(0, 0, 17, 6),
                        Style::default(),
                        Style::default(),
                    )
                    .unwrap();
                assert!(rect.right() <= 17);
            })
            .unwrap();
    }

    #[test]
    fn side_tooltip_appears_to_right_and_stays_inside_viewport_at_bottom() {
        let mut terminal = Terminal::new(TestBackend::new(32, 12)).unwrap();
        let mut rect = None;
        terminal
            .draw(|frame| {
                rect = Tooltip::new("Settings")
                    .shortcut(Some("F9"))
                    .placement(TooltipPlacement::Right)
                    .draw(
                        frame,
                        Rect::new(1, 10, 3, 1),
                        frame.area(),
                        Style::default(),
                        Style::default(),
                    );
            })
            .unwrap();
        let rect = rect.unwrap();
        assert_eq!(rect.x, 4);
        assert!(rect.bottom() <= 12);
        let row = (rect.x..rect.right())
            .map(|x| terminal.backend().buffer()[(x, rect.y + 1)].symbol())
            .collect::<String>();
        assert!(row.contains("Settings"));
        assert!(row.contains("F9"));
    }

    #[test]
    fn side_tooltip_flips_left_when_near_right_edge() {
        let mut terminal = Terminal::new(TestBackend::new(35, 10)).unwrap();
        let mut rect = None;
        terminal
            .draw(|frame| {
                rect = Tooltip::new("Open selected file")
                    .placement(TooltipPlacement::Right)
                    .draw(
                        frame,
                        Rect::new(30, 2, 4, 1),
                        frame.area(),
                        Style::default(),
                        Style::default(),
                    );
            })
            .unwrap();
        let rect = rect.unwrap();
        assert!(rect.right() <= 30);
    }

    #[test]
    fn tiny_terminal_does_not_draw_tooltip() {
        let mut terminal = Terminal::new(TestBackend::new(8, 3)).unwrap();
        terminal
            .draw(|frame| {
                assert!(Tooltip::new("Close")
                    .draw(
                        frame,
                        Rect::new(1, 0, 1, 1),
                        frame.area(),
                        Style::default(),
                        Style::default()
                    )
                    .is_none());
            })
            .unwrap();
    }
}

//! Native TUI prompt/confirmation layout: one border, no fake web buttons.
//! The bottom border labels are real mouse targets using existing Enter/Esc.
use super::*;
use ratatui::widgets::Wrap;
use unicode_width::UnicodeWidthChar;

fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect::new(x, y, width, height)
}

/// Clip a one-line editor by terminal cell width without splitting UTF-8.
fn visible_input(value: &str, cursor: usize, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }
    let left = cursor.saturating_sub(width.saturating_sub(1));
    let mut cells = 0usize;
    let mut shown = 0usize;
    let mut text = String::new();
    for c in value.chars() {
        let len = UnicodeWidthChar::width(c).unwrap_or(0);
        let end = cells + len;
        if end <= left {
            cells = end;
            continue;
        }
        if shown + len > width {
            break;
        }
        text.push(if c.is_control() { ' ' } else { c });
        shown += len;
        cells = end;
    }
    (
        text,
        cursor.saturating_sub(left).min(width.saturating_sub(1)),
    )
}

fn footer_actions(frame: &mut Frame<'_>, area: Rect, label: &str, p: Palette) -> (Rect, Rect) {
    if area.width < 4 || area.height < 4 {
        return (Rect::default(), Rect::default());
    }
    let y = area.bottom() - 1;
    let cancel = " Esc cancel ";
    let submit = format!(" Enter {} ", label.to_ascii_lowercase());
    let gap = 4u16;
    let total = cancel.len() as u16 + submit.len() as u16 + gap;
    if area.width.saturating_sub(2) < total {
        // Narrow terminals still retain a keyboard path; don't draw truncated
        // click targets or overwrite the corners.
        return (Rect::default(), Rect::default());
    }
    let cancel_rect = rect(area.x + 2, y, cancel.len() as u16, 1);
    let submit_rect = rect(
        area.right() - 2 - submit.len() as u16,
        y,
        submit.len() as u16,
        1,
    );
    let neutral = Style::default().fg(p.fg_dim).bg(p.bg);
    let hotkey = Style::default()
        .fg(p.accent)
        .bg(p.bg)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", neutral),
            Span::styled("Esc", hotkey),
            Span::styled(" cancel ", neutral),
        ])),
        cancel_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", neutral),
            Span::styled("Enter", hotkey),
            Span::styled(format!(" {} ", label.to_ascii_lowercase()), neutral),
        ])),
        submit_rect,
    );
    (cancel_rect, submit_rect)
}

/// The prompt owns keyboard focus throughout editing. Mouse click regions
/// always dispatch to the same Enter/Esc handling as keyboard actions.
pub(crate) fn draw_form(
    frame: &mut Frame<'_>,
    area: Rect,
    panel: &mut tools::Panel,
    p: Palette,
    mode: FormMode,
) {
    panel.area = area;
    panel.list_area = Rect::default();
    panel.form_cancel = Rect::default();
    panel.form_submit = Rect::default();
    if area.width < 2 || area.height < 2 {
        return;
    }

    let inner = draw_shell(frame, area, &panel.title, "", p);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let (cancel, submit) = footer_actions(frame, area, mode.action_name(&panel.kind), p);
    panel.form_cancel = cancel;
    panel.form_submit = submit;

    let pad = u16::from(inner.width >= 8);
    let x = inner.x + pad;
    let width = inner.width.saturating_sub(pad * 2);
    let mut y = inner.y + u16::from(inner.height >= 4);
    // Leave one visual breathing row above the bottom-border actions.
    let bottom = inner.bottom().saturating_sub(u16::from(inner.height >= 3));

    if mode == FormMode::Prompt {
        if y < bottom {
            frame.render_widget(
                Paragraph::new(mode.field_name(&panel.kind)).style(Style::default().fg(p.fg_dim)),
                rect(x, y, width, 1),
            );
            y += 1;
        }
        if y < bottom {
            let input_width = width.saturating_sub(2) as usize;
            let (shown, cursor) = visible_input(
                panel.input.value(),
                panel.input.visual_cursor(),
                input_width,
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "› ",
                        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(shown, Style::default().fg(p.fg)),
                ])),
                rect(x, y, width, 1),
            );
            if input_width > 0 {
                frame.set_cursor_position((x + 2 + cursor as u16, y));
            }
            y += 1;
        }
    }
    let message_rows = u16::from(!panel.message.is_empty() && bottom > y);
    let detail_height = bottom.saturating_sub(y).saturating_sub(message_rows);
    panel.scroll_limit = wrapped_line_count(&panel.detail, width.max(1) as usize)
        .saturating_sub(detail_height as usize);
    panel.scroll = panel.scroll.min(panel.scroll_limit);
    if detail_height > 0 {
        frame.render_widget(
            Paragraph::new(panel.detail.as_str())
                .style(Style::default().fg(if mode == FormMode::Confirm {
                    p.fg
                } else {
                    p.fg_dim
                }))
                .wrap(Wrap { trim: false })
                .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0)),
            rect(x, y, width, detail_height),
        );
    }
    if message_rows > 0 {
        frame.render_widget(
            Paragraph::new(format!("! {}", panel.message)).style(Style::default().fg(p.error)),
            rect(x, bottom - 1, width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_viewport_handles_utf8_and_long_paths_without_overflow() {
        let (text, cursor) = visible_input("路径/名字.rs", 8, 5);
        assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= 5);
        assert!(cursor < 5);
        let (text, cursor) = visible_input("abcdefghijklmnopqrstuvwxyz", 26, 8);
        assert_eq!(text, "tuvwxyz");
        assert_eq!(cursor, 7);
        assert_eq!(visible_input("hi", 2, 0), (String::new(), 0));
    }
}

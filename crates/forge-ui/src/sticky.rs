//! Sticky scroll for flat tree lists. Indices always refer to the original
//! rows, so decorations cannot change Git operation or file-selection targets.

use forge_theme::{ColorMode, Rgb, Theme};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::style;

/// Match VS Code's small sticky region: up to seven lines or 40% of the view.
pub const MAX_STICKY_ROWS: usize = 7;

/// A dedicated pinned-area fill. The old pill background was too close to the
/// normal panel and could quantize to exactly the same terminal color.
///
/// Blend toward the theme accent rather than hard-coding a different palette
/// for each Forge theme; 16-color terminals need explicit dark gray contrast.
pub fn background(theme: &Theme) -> Color {
    if matches!(style::color_mode(), ColorMode::Ansi16) {
        return Color::DarkGray;
    }
    fn blend(base: u8, accent: u8) -> u8 {
        ((base as u16 * 68 + accent as u16 * 32) / 100) as u8
    }
    let base = theme.panel_bg;
    let accent = theme.accent;
    style::rgb(Rgb::new(
        blend(base.r, accent.r),
        blend(base.g, accent.g),
        blend(base.b, accent.b),
    ))
}

/// Apply the same full-width background to Files and Git/History sticky rows.
///
/// Styling every buffer cell after the tree widgets render ensures that long
/// names, empty trailing space, and the various Git section headers all share
/// one continuous pinned surface, even if their inner spans use different bg.
pub fn paint(frame: &mut Frame<'_>, area: Rect, theme: &Theme, pinned: &[usize], selected: usize) {
    if area.width == 0 {
        return;
    }
    let background = background(theme);
    for (line, index) in pinned.iter().enumerate().take(area.height as usize) {
        let y = area.y + line as u16;
        frame.buffer_mut().set_style(
            Rect::new(area.x, y, area.width, 1),
            Style::default().bg(background),
        );
        // Keep the selected-row chevron visible; otherwise provide a thin
        // accent-colored rail that indicates these lines are pinned, not
        // selected or ordinary nested directories.
        if *index != selected {
            frame.render_widget(
                Paragraph::new("▏")
                    .style(Style::default().fg(style::rgb(theme.accent)).bg(background)),
                Rect::new(area.x, y, 1, 1),
            );
        }
    }
}

pub fn headers(
    start: usize,
    height: usize,
    mut parent: impl FnMut(usize) -> Option<usize>,
) -> Vec<usize> {
    let cap = MAX_STICKY_ROWS
        .min(height.saturating_mul(2) / 5)
        .min(height.saturating_sub(1));
    if cap == 0 || start == 0 {
        return Vec::new();
    }
    let mut ancestors = Vec::new();
    let mut cursor = start;
    while let Some(p) = parent(cursor) {
        if p >= cursor {
            break;
        }
        ancestors.push(p);
        cursor = p;
        if ancestors.len() >= 128 {
            break;
        }
    }
    ancestors.reverse();
    if ancestors.len() > cap {
        ancestors.drain(..ancestors.len() - cap);
    }
    ancestors
}

/// Last content-row anchor that still fills the viewport with content and
/// sticky ancestors. A simple len-height bound would hide the last few
/// nested files behind pinned headings.
pub fn max_scroll(
    len: usize,
    height: usize,
    parent: impl Fn(usize) -> Option<usize> + Copy,
) -> usize {
    if len <= height || height == 0 {
        return 0;
    }
    let base = len - height;
    let cap = MAX_STICKY_ROWS.min(height.saturating_mul(2) / 5);
    for shift in (0..=cap).rev() {
        let start = (base + shift).min(len - 1);
        if len - start + headers(start, height, parent).len() >= height {
            return start;
        }
    }
    base
}

/// Adjust only the virtual scroll anchor; never alter tree row identities.
pub fn layout(
    len: usize,
    selected: usize,
    scroll: &mut usize,
    height: usize,
    parent: impl Fn(usize) -> Option<usize> + Copy,
    follow_selection: bool,
) -> Vec<usize> {
    if len == 0 || height == 0 {
        *scroll = 0;
        return Vec::new();
    }
    if !follow_selection {
        *scroll = (*scroll).min(max_scroll(len, height, parent));
        return headers(*scroll, height, parent);
    }
    *scroll = (*scroll).min(len - 1);
    for _ in 0..MAX_STICKY_ROWS + 8 {
        if selected < *scroll {
            *scroll = selected;
        }
        let pinned = headers(*scroll, height, parent);
        let content_height = height.saturating_sub(pinned.len()).max(1);
        if selected >= *scroll + content_height {
            *scroll = selected + 1 - content_height;
        } else {
            return pinned;
        }
    }
    headers(*scroll, height, parent)
}

/// Map terminal coordinates to original indices, including pinned ancestors.
pub fn index_at(
    rect: Rect,
    scroll: usize,
    pinned: &[usize],
    len: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    if !rect.contains(Position { x: column, y: row }) {
        return None;
    }
    let offset = (row - rect.y) as usize;
    let index = if offset < pinned.len() {
        pinned[offset]
    } else {
        scroll.checked_add(offset - pinned.len())?
    };
    (index < len).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_ancestors_are_bounded_and_restore_on_top() {
        let parent = |i: usize| (i > 0).then(|| i - 1);
        assert!(headers(0, 20, parent).is_empty());
        assert_eq!(headers(3, 20, parent), vec![0, 1, 2]);
        assert_eq!(headers(12, 10, parent), vec![8, 9, 10, 11]);
        assert_eq!(headers(12, 4, parent), vec![11]);
    }

    #[test]
    fn manual_scroll_leaves_selection_unchanged_and_keeps_last_file_visible() {
        // 0 root, 1 folder, 2..20 children; several rows remain sticky.
        let parent = |i: usize| if (2..=20).contains(&i) { Some(1) } else { None };
        let mut scroll = 8;
        let pinned = layout(21, 0, &mut scroll, 7, parent, false);
        assert_eq!(scroll, 8, "scroll is independent of selected root file");
        assert_eq!(pinned, vec![1]);
        let end = max_scroll(21, 7, parent);
        assert_eq!(end, 15); // 1 pinned + 6 content rows through index 20
        let mut scroll = 200;
        let pinned = layout(21, 0, &mut scroll, 7, parent, false);
        assert_eq!(scroll, end);
        assert_eq!(pinned, vec![1]);
        assert_eq!(scroll + 7 - pinned.len(), 21);
        // Keyboard navigation explicitly restores selection-follow behavior.
        let pinned = layout(21, 0, &mut scroll, 7, parent, true);
        assert_eq!(scroll, 0);
        assert!(pinned.is_empty());
    }

    #[test]
    fn selection_and_clicks_stay_on_original_rows() {
        let parent = |i: usize| (i > 0 && i < 6).then(|| i - 1);
        let mut scroll = 4;
        let pinned = layout(20, 9, &mut scroll, 5, parent, true);
        assert!(9 >= scroll && 9 < scroll + 5 - pinned.len());
        let rect = Rect::new(1, 2, 30, 5);
        assert_eq!(
            index_at(rect, scroll, &pinned, 20, 5, 2),
            pinned.first().copied().or(Some(scroll))
        );
        assert_eq!(
            index_at(rect, scroll, &pinned, 20, 5, 2 + pinned.len() as u16),
            Some(scroll)
        );
        assert_eq!(index_at(rect, scroll, &pinned, 20, 5, 30), None);
    }
}

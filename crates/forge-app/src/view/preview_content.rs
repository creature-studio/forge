//! Preview content mapping and theme bridging.
use super::*;

pub(super) fn preview_content(model: &App) -> (String, PreviewView<'_>) {
    model.preview_state.content(
        model.files.root(),
        model.markdown_available(),
        if model.preview_state.needs_empty_diff_hint() {
            model.empty_diff_hint()
        } else {
            ""
        },
    )
}

/// Adapt semantic kit colors for the existing syntax/diff renderers. Reset
/// colors use neutral fallbacks because the legacy token type stores RGB only.
pub(crate) fn bridge_theme(p: Palette) -> forge_theme::Theme {
    use forge_theme::{vga, Rgb};
    fn rgb(c: Color, fallback: Rgb) -> Rgb {
        match c {
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
            Color::Black => vga::BLACK,
            Color::White => vga::WHITE,
            Color::Gray => vga::LIGHT_GRAY,
            Color::Cyan => vga::CYAN,
            Color::DarkGray => vga::DARK_GRAY,
            Color::Green => vga::GREEN,
            Color::Red => vga::RED,
            Color::Yellow => vga::BROWN,
            Color::Blue => vga::BLUE,
            _ => fallback,
        }
    }
    let mut t = forge_theme::Theme::modern_ide();
    t.name = "forge-terminal".into();
    t.bg = rgb(p.bg, Rgb::new(0, 0, 0));
    t.panel_bg = t.bg;
    t.fg = rgb(p.fg, Rgb::new(229, 229, 229));
    t.muted = rgb(p.fg_dim, t.muted);
    t.accent = rgb(p.accent, t.accent);
    t.border = rgb(p.border, t.muted);
    t.border_focus = rgb(p.border_active, t.accent);
    t.selection_bg = rgb(p.selection, t.accent);
    t.selection_fg = rgb(p.on_accent, t.bg);
    t.pill_bg = t.bg;
    t.menu_bg = t.bg;
    t.menu_fg = t.fg;
    t.git_added = rgb(p.success, t.git_added);
    t.git_deleted = rgb(p.error, t.git_deleted);
    t.git_modified = rgb(p.warning, t.git_modified);
    t.git_conflicted = t.git_deleted;
    t.git_untracked = t.git_added;
    t.git_ignored = t.muted;
    t.git_renamed = rgb(p.info, t.git_renamed);
    t.directory = rgb(p.info, t.directory);
    t.diff_add = t.git_added;
    t.diff_del = t.git_deleted;
    t
}

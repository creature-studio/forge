//! Preview content mapping and theme bridging.
use super::*;

pub(super) fn preview_content(model: &App) -> (String, PreviewView<'_>) {
    if model.preview_state.preview_loading {
        return ("Loading".into(), PreviewView::Loading);
    }
    if matches!(model.preview_state.preview_mode, PreviewMode::Diff) {
        return match &model.preview_state.file_diff {
            Some(diff) => {
                let hunks = diff.hunks();
                let hunk = model.preview_state.hunk_index.and_then(|i| hunks.get(i));
                (
                    format!(
                        "{} · {}{}",
                        model.preview_state.diff_label,
                        diff.path,
                        model
                            .preview_state
                            .hunk_index
                            .map(|i| format!(" · hunk {}/{}", i + 1, hunks.len()))
                            .unwrap_or_default()
                    ),
                    PreviewView::Diff {
                        path: &diff.path,
                        side: &model.preview_state.diff_label,
                        lines: &diff.lines,
                        hunk_start: hunk.map(|h| h.start),
                        hunk_end: hunk.map(|h| h.end),
                        hunk_idx: model.preview_state.hunk_index.map(|i| (i, hunks.len())),
                    },
                )
            }
            None => (
                String::new(),
                PreviewView::Hint {
                    text: model.empty_diff_hint(),
                },
            ),
        };
    }
    let Some(preview) = &model.preview_state.preview else {
        return (String::new(), PreviewView::Idle);
    };
    let title = preview
        .path
        .strip_prefix(&model.files.tree_root)
        .unwrap_or(&preview.path)
        .display()
        .to_string();
    let content = match &preview.content {
        PreviewContent::Empty => PreviewView::Idle,
        PreviewContent::Directory {
            name,
            children,
            truncated,
        } => PreviewView::Directory {
            name,
            children,
            truncated: *truncated,
        },
        PreviewContent::Binary { notice, .. } => PreviewView::Binary { notice },
        PreviewContent::Error { message } => PreviewView::Error { message },
        PreviewContent::Text { .. }
            if model.markdown_available() && !model.preview_state.markdown_code =>
        {
            PreviewView::Markdown {
                preview: &model.preview_state.markdown,
            }
        }
        PreviewContent::Text {
            lines,
            truncated,
            bytes_read,
        } => PreviewView::Text {
            lines,
            truncated: *truncated,
            bytes_read: *bytes_read,
            language: TextLang::from_name(
                preview
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            ),
        },
    };
    (title, content)
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

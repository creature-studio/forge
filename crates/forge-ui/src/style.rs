//! Theme → Ratatui style helpers + modern dark IDE chrome.
//!
//! Default emission is **Ansi256** so charcoal greys from `Theme::modern_ide`
//! stay visible. Named Ansi16 / TrueColor when forced.

use std::sync::atomic::{AtomicU8, Ordering};

use forge_theme::{match_vga, nearest_vga, to_ansi256, ColorMode, Rgb, Theme, VgaNamed};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Clear, Paragraph};
use ratatui::Frame;

const MODE_TRUECOLOR: u8 = 0;
const MODE_ANSI256: u8 = 1;
const MODE_ANSI16: u8 = 2;

static COLOR_MODE: AtomicU8 = AtomicU8::new(MODE_ANSI256);

/// Install the color emission mode used by every `rgb()` call.
pub fn set_color_mode(mode: ColorMode) {
    let v = match mode {
        ColorMode::TrueColor => MODE_TRUECOLOR,
        ColorMode::Ansi256 => MODE_ANSI256,
        ColorMode::Ansi16 => MODE_ANSI16,
    };
    COLOR_MODE.store(v, Ordering::Relaxed);
}

/// Current color emission mode.
pub fn color_mode() -> ColorMode {
    match COLOR_MODE.load(Ordering::Relaxed) {
        MODE_TRUECOLOR => ColorMode::TrueColor,
        MODE_ANSI256 => ColorMode::Ansi256,
        _ => ColorMode::Ansi16,
    }
}

fn vga_to_ratatui(n: VgaNamed) -> Color {
    match n {
        VgaNamed::Black => Color::Black,
        VgaNamed::Blue => Color::Blue,
        VgaNamed::Green => Color::Green,
        VgaNamed::Cyan => Color::Cyan,
        VgaNamed::Red => Color::Red,
        VgaNamed::Magenta => Color::Magenta,
        VgaNamed::Brown => Color::Yellow,
        VgaNamed::LightGray => Color::Gray,
        VgaNamed::DarkGray => Color::DarkGray,
        VgaNamed::LightBlue => Color::LightBlue,
        VgaNamed::LightGreen => Color::LightGreen,
        VgaNamed::LightCyan => Color::LightCyan,
        VgaNamed::LightRed => Color::LightRed,
        VgaNamed::LightMagenta => Color::LightMagenta,
        VgaNamed::Yellow => Color::LightYellow,
        VgaNamed::White => Color::White,
    }
}

/// Convert a theme RGB into a Ratatui `Color` for the active [`ColorMode`].
#[inline]
pub fn rgb(c: Rgb) -> Color {
    match color_mode() {
        ColorMode::Ansi16 => vga_to_ratatui(nearest_vga(c)),
        ColorMode::Ansi256 => {
            if let Some(named) = match_vga(c) {
                vga_to_ratatui(named)
            } else {
                Color::Indexed(to_ansi256(c))
            }
        }
        ColorMode::TrueColor => {
            if let Some(named) = match_vga(c) {
                vga_to_ratatui(named)
            } else {
                Color::Rgb(c.r, c.g, c.b)
            }
        }
    }
}

#[inline]
pub fn fg_bg(fg: Rgb, bg: Rgb) -> Style {
    Style::default().fg(rgb(fg)).bg(rgb(bg))
}

#[allow(dead_code)]
pub fn fg(_theme: &Theme, c: Rgb) -> Style {
    Style::default().fg(rgb(c))
}

#[allow(dead_code)]
pub fn base(theme: &Theme) -> Style {
    fg_bg(theme.fg, theme.panel_bg)
}

pub fn muted(theme: &Theme) -> Style {
    fg_bg(theme.muted, theme.panel_bg)
}

#[allow(dead_code)]
pub fn accent(theme: &Theme) -> Style {
    fg_bg(theme.accent, theme.panel_bg)
}

#[allow(dead_code)]
pub fn border(theme: &Theme, focused: bool) -> Style {
    if focused {
        fg_bg(theme.border_focus, theme.panel_bg)
    } else {
        fg_bg(theme.border, theme.panel_bg)
    }
}

#[allow(dead_code)]
pub fn selection(theme: &Theme) -> Style {
    fg_bg(theme.selection_fg, theme.selection_bg)
}

#[allow(dead_code)]
pub fn title(theme: &Theme, focused: bool) -> Style {
    if focused {
        fg_bg(theme.title_active, theme.header_bg).add_modifier(Modifier::BOLD)
    } else {
        fg_bg(theme.title_inactive, theme.header_bg)
    }
}

pub fn chrome(theme: &Theme) -> Style {
    fg_bg(theme.chrome_fg, theme.chrome_bg)
}

pub fn chrome_hotkey(theme: &Theme) -> Style {
    fg_bg(theme.chrome_hotkey, theme.chrome_bg).add_modifier(Modifier::BOLD)
}

/// Fill the whole frame with charcoal desktop (theme bg).
pub fn fill_desktop(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(Block::default().style(fg_bg(theme.fg, theme.bg)), area);
}

/// Legacy name kept for callers; now charcoal, not blue.
#[allow(dead_code)]
pub fn fill_desktop_blue(frame: &mut Frame<'_>, area: Rect) {
    // Hard charcoal black - named Black so Ansi16 never drops it.
    let style = Style::default().fg(Color::White).bg(Color::Black);
    frame.render_widget(Block::default().style(style), area);
    let _ = area;
}

pub fn fill_panel(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    frame.render_widget(
        Block::default().style(fg_bg(theme.fg, theme.panel_bg)),
        area,
    );
}

pub fn draw_pane_separator(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let style = fg_bg(theme.border, theme.bg);
    let mut lines = Vec::with_capacity(area.height as usize);
    for _ in 0..area.height {
        lines.push(Line::from(Span::styled("│", style)));
    }
    frame.render_widget(Paragraph::new(lines).style(style), area);
}

/// Soft section header — Edit-style cyan title chip when focused, no box borders.
pub fn draw_section_header(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    focused: bool,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let header = chunks[0];
    let body = chunks[1];

    // Active: solid cyan title block (accent bg + dark fg). Inactive: muted on panel.
    let chip = format!(" {title_text} ");
    let rest = header.width as usize;
    if focused {
        let chip_w = chip.chars().count().min(rest);
        let pad = rest.saturating_sub(chip_w);
        let chip_style = fg_bg(theme.selection_fg, theme.selection_bg).add_modifier(Modifier::BOLD);
        let pad_style = fg_bg(theme.title_inactive, theme.panel_bg);
        let mut spans = vec![Span::styled(chip.chars().take(chip_w).collect::<String>(), chip_style)];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), pad_style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(fg_bg(theme.title_inactive, theme.panel_bg)),
            header,
        );
    } else {
        let style = fg_bg(theme.title_inactive, theme.panel_bg);
        let label = format!(" {title_text}");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{label:<width$}", width = rest),
                style,
            )))
            .style(style),
            header,
        );
    }
    fill_panel(frame, body, theme);
    body
}

/// Docked pane header - alias used by older call sites.
#[allow(dead_code)]
pub fn draw_docked_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    focused: bool,
) -> Rect {
    draw_section_header(frame, area, theme, title_text, focused)
}

#[allow(dead_code)]
pub fn draw_tv_window(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    focused: bool,
) -> Rect {
    draw_section_header(frame, area, theme, title_text, focused)
}

#[allow(dead_code)]
pub fn draw_pane_header(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title_text: &str,
    focused: bool,
) -> Rect {
    draw_section_header(frame, area, theme, title_text, focused)
}

/// ASCII-safe pill: `[text]` with fg/bg (no powerline / rounded glyphs).
pub fn pill_spans(text: &str, fg: Rgb, bg: Rgb, outer_bg: Rgb) -> Vec<Span<'static>> {
    let _ = outer_bg;
    vec![
        Span::styled(
            "[".to_string(),
            Style::default().fg(rgb(fg)).bg(rgb(bg)),
        ),
        Span::styled(
            text.to_string(),
            Style::default()
                .fg(rgb(fg))
                .bg(rgb(bg))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "]".to_string(),
            Style::default().fg(rgb(fg)).bg(rgb(bg)),
        ),
    ]
}

/// Soft key legend chip: ` key ` on pill_bg.
#[allow(dead_code)]
pub fn key_chip(label: &str, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(" ".to_string(), chrome(theme)),
        Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(rgb(theme.chrome_fg))
                .bg(rgb(theme.pill_bg)),
        ),
    ]
}

/// Draw a plain single-line border block (ASCII-safe).
#[allow(dead_code)]
pub fn soft_block(theme: &Theme, focused: bool) -> Block<'static> {
    let border_c = if focused {
        theme.border_focus
    } else {
        theme.border
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(fg_bg(border_c, theme.panel_bg))
        .style(fg_bg(theme.fg, theme.panel_bg))
}

#[allow(dead_code)]
pub fn draw_chrome_line(frame: &mut Frame<'_>, area: Rect, theme: &Theme, line: Line<'static>) {
    let para = Paragraph::new(line).style(chrome(theme));
    frame.render_widget(para, area);
}

#[allow(dead_code)]
pub fn split_header_body(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    (chunks[0], chunks[1])
}

/// No-op floating insets - modern IDE is edge-to-edge.
#[allow(dead_code)]
pub fn floating_work_area(full: Rect) -> Rect {
    full
}

#[allow(dead_code)]
pub fn draw_window_shadow(_frame: &mut Frame<'_>, _window: Rect, _theme: &Theme, _desktop: Rect) {}

/// Legacy floating window - now just fills charcoal + returns full area.
#[allow(dead_code)]
pub fn draw_main_window(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    _app_name: &str,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    frame.render_widget(Clear, area);
    fill_desktop(frame, area, theme);
    area
}

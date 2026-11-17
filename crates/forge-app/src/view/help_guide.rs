//! Help content, guide formatting and compact label helpers.
use super::*;

#[cfg(test)]
pub(super) fn wrap_help(width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    let mut lines = Vec::new();
    for source in interaction::HELP_TEXT.lines() {
        let mut line = String::new();
        for word in source.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(Line::from(std::mem::take(&mut line)));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(Line::from(line));
    }
    lines
}

pub(super) fn guide_section(out: &mut Vec<Line<'static>>, title: &str, p: Palette) {
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled("  ━  ", Style::default().fg(p.info)),
        Span::styled(title.to_owned(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]));
    out.push(Line::from(""));
}

pub(super) fn guide_step(out: &mut Vec<Line<'static>>, number: &str, title: &str, keys: &str, description: &str, p: Palette) {
    out.push(Line::from(vec![
        Span::styled(format!("  {number}  "), Style::default().bg(p.info).fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {title}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]));
    out.push(Line::from(vec![
        Span::styled(format!("       {keys}"), Style::default().fg(p.info)),
        Span::styled(format!("   ·   {description}"), Style::default().fg(p.fg_dim)),
    ]));
    out.push(Line::from(""));
}

pub(super) fn quick_guide(p: Palette, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  F O R G E", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("   /   FILES + GIT WORKSPACE", Style::default().fg(p.info)),
        ]),
        Line::styled("  Browse code, review changes, and commit from your terminal.", Style::default().fg(p.fg_dim)),
    ];
    if width < 60 {
        out.clear();
        out.push(Line::styled("  FORGE  /  GETTING STARTED", Style::default().fg(p.info).add_modifier(Modifier::BOLD)));
        guide_section(&mut out, "01   QUICK START", p);
        for (step, title, hint) in [
            ("1", "Browse", "F1 · j/k · Enter"),
            ("2", "Review Git", "F2 · Tab · n/N"),
            ("3", "Stage", "s · u"),
            ("4", "Commit", "c · Enter"),
        ] {
            out.push(Line::from(vec![Span::styled(format!("  {step}  "), Style::default().fg(p.info)),
                Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]));
            out.push(Line::styled(format!("     {hint}"), Style::default().fg(p.fg_dim)));
            out.push(Line::from(""));
        }
        guide_section(&mut out, "02   LEARN MORE", p);
        out.push(Line::styled("  F3 Markdown · F6 Blame · F7 Peek", Style::default().fg(p.fg_dim)));
        out.push(Line::styled("  F4 Shortcuts · : Commands", Style::default().fg(p.fg_dim)));
        out.push(Line::styled("  Tab → Full Reference", Style::default().fg(p.info)));
        return out;
    }
    guide_section(&mut out, "01   QUICK START", p);
    guide_step(&mut out, "1", "Browse files", "F1 · j/k · Enter · Ctrl-P", "Navigate, open, find files", p);
    guide_step(&mut out, "2", "Review changes", "F2 · Tab · n/N", "Select a file and inspect its diff", p);
    guide_step(&mut out, "3", "Stage / unstage", "s · u", "Stage the selected file or hunk", p);
    guide_step(&mut out, "4", "Commit", "c · Enter", "Review staged changes and commit", p);
    guide_section(&mut out, "02   ESSENTIAL SHORTCUTS", p);
    for (name, keys, description) in [
        ("FILES", "Ctrl-F · Ctrl-E", "Find inside a file · Edit"),
        ("PREVIEW", "F3 · F6 · F7", "Markdown · Blame · Change Peek"),
        ("GIT", "s/u · n/N · c", "Stage · Hunks · Commit"),
        ("TOOLS", "F4 · : · ,", "Shortcuts · Commands · Settings"),
    ] {
        out.push(Line::from(vec![
            Span::styled(format!("  {name:<10}"), Style::default().fg(p.info).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{keys:<22}"), Style::default().fg(Color::White)),
            Span::styled(description.to_owned(), Style::default().fg(p.fg_dim)),
        ]));
    }
    guide_section(&mut out, "03   COMMON WORKFLOWS", p);
    for (task, route) in [
        ("Review a file", "Files → Select → Preview"),
        ("Stage a change", "F2 → Changes → s"),
        ("Inspect history", "Git → History → Enter"),
        ("Explain a line", "Click source line → F6"),
    ] {
        out.push(Line::from(vec![
            Span::styled(format!("  {task:<20}"), Style::default().fg(Color::White)),
            Span::styled(route.to_owned(), Style::default().fg(p.fg_dim)),
        ]));
    }
    out.push(Line::from(""));
    out.push(Line::styled("  Tab → Full Reference for all commands and safety rules", Style::default().fg(p.info)));
    out
}

pub(super) fn terminal_guide(p: Palette, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  TERMINAL", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("   /   TABS + KEYBOARD + MOUSE", Style::default().fg(p.info)),
        ]),
        Line::styled("  Each tab is an independent local Shell.", Style::default().fg(p.fg_dim)),
    ];
    if width < 52 {
        guide_section(&mut out, "KEYBOARD", p);
        for line in [
            "  Alt+1..8     Go directly to Tab",
            "  F12         Next Tab",
            "  Shift+F12   Previous Tab",
            "  Shift+F8    New Tab",
            "  Ctrl+Shift+W Close Tab",
            "  F8 / F11    Hide / Maximize",
            "  F4          This help",
        ] {
            out.push(Line::styled(line, Style::default().fg(p.fg)));
        }
    } else {
        guide_section(&mut out, "01   TERMINAL TAB NAVIGATION", p);
        guide_step(&mut out, "1", "Switch tabs", "Alt+1..8 · F12 · Shift+F12", "Direct / next / previous", p);
        guide_step(&mut out, "2", "Create and close", "Shift+F8 · Ctrl+Shift+W", "Independent local Shells", p);
        guide_step(&mut out, "3", "Layout and focus", "F8 · F11 · Ctrl+Shift+Tab", "Hide / maximize / back to Forge", p);
        guide_step(&mut out, "4", "Scroll history", "Shift+PageUp/PageDown", "Return with Ctrl+Shift+End", p);
    }
    out.push(Line::styled("  F4 opens this guide; Esc closes it and returns to the Shell.", Style::default().fg(p.info)));
    guide_section(&mut out, "02   MOUSE", p);
    out.push(Line::styled("  Click tab = switch     × / middle-click = close", Style::default().fg(p.fg)));
    out.push(Line::styled("  + new     ‹ / › switch     □ maximize     ? help     − hide", Style::default().fg(p.fg_dim)));
    guide_section(&mut out, "03   CLIPBOARD", p);
    out.push(Line::styled("  Drag select to copy · Ctrl+Shift+C / V", Style::default().fg(p.fg)));
    out.push(Line::styled("  Shift+mouse selects text when child TUI tracks the mouse", Style::default().fg(p.fg_dim)));
    out.push(Line::from(""));
    out.push(Line::styled("  Tab → Full Reference · Esc → Close", Style::default().fg(p.info)));
    out
}

pub(super) fn reference_guide(width: usize, p: Palette) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    let mut out = Vec::new();
    for source in interaction::HELP_TEXT.lines() {
        if source.is_empty() { out.push(Line::from("")); continue; }
        // Recognize sections before wrapping: a short continuation must
        // never turn into a fake heading on narrower terminals.
        if !source.contains(':') && source.len() < 37 {
            out.push(Line::from(vec![
                Span::styled("  ━  ", Style::default().fg(p.info)),
                Span::styled(source.to_owned(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]));
            continue;
        }
        let mut words = String::new();
        let mut segments = Vec::new();
        for word in source.split_whitespace() {
            if !words.is_empty() && words.width() + word.width() + 1 > width.max(1) {
                segments.push(std::mem::take(&mut words));
            }
            if !words.is_empty() { words.push(' '); }
            words.push_str(word);
        }
        if !words.is_empty() { segments.push(words); }
        for (i, text) in segments.into_iter().enumerate() {
            let prefix = (i == 0).then(|| text.split_once(':')).flatten()
                .filter(|(key, _)| !key.is_empty() && key.len() <= 18);
            if let Some((key, rest)) = prefix {
                out.push(Line::from(vec![
                    Span::styled(format!("{key}:"), Style::default().fg(p.info).add_modifier(Modifier::BOLD)),
                    Span::styled(rest.to_owned(), Style::default().fg(p.fg_dim)),
                ]));
            } else { out.push(Line::styled(text, Style::default().fg(p.fg_dim))); }
        }
    }
    out
}

pub(super) fn draw_help_guide(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    frame.render_widget(Clear, area);
    model.session.help_area = area;
    model.session.help_close_rect = Rect::default();
    model.session.help_overview_rect = Rect::default();
    model.session.help_reference_rect = Rect::default();
    if area.width < 2 || area.height < 2 { return; }
    frame.render_widget(Block::default().borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.bg)), area);
    let inset = u16::from(area.width >= 68) * 2;
    let content = Rect::new(area.x.saturating_add(1 + inset), area.y + 1,
        area.width.saturating_sub(2 + inset * 2).min(106), area.height - 2);
    if content.width < 1 || content.height < 1 { return; }
    let overview = !model.session.help_reference;
    let tab = if overview { "   [1 Overview]  2 Reference" } else { "   1 Overview  [2 Reference]" };
    let terminal_help = model.terminal.as_ref().is_some_and(|term| term.visible);
    // The close hit target is independent of terminal text and remains
    // accessible even when the help modal owns pointer input.
    let close_text = " × Close ";
    let close_width = unicode_width::UnicodeWidthStr::width(close_text) as u16;
    let close_x = area.right().saturating_sub(close_width + 1);
    if close_x >= content.x.saturating_add(32) {
        let close_rect = Rect::new(close_x,content.y,close_width,1);
        model.session.help_close_rect = close_rect;
        frame.render_widget(Paragraph::new(close_text).style(
            Style::default().fg(p.info).bg(p.bg)),close_rect);
    }
    // Header navigation is clickable as well as keyboard-operated.
    let tab_x = content.x.saturating_add(
        if terminal_help && overview { "FORGE  /  TERMINAL GUIDE".len() as u16 }
        else if overview { "FORGE  /  QUICK START".len() as u16 }
        else { "FORGE  /  REFERENCE".len() as u16 }
    );
    model.session.help_overview_rect = Rect::new(tab_x,content.y,16.min(content.width),1);
    model.session.help_reference_rect = Rect::new(tab_x.saturating_add(16),content.y,15.min(content.width.saturating_sub(16)),1);
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("FORGE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(if overview && terminal_help { "  /  TERMINAL GUIDE" }
            else if overview { "  /  QUICK START" } else { "  /  REFERENCE" },
            Style::default().fg(p.info)),
        Span::styled(tab, Style::default().fg(p.fg_dim)),
    ])).style(Style::default().bg(p.bg)), Rect::new(content.x, content.y, content.width, 1));
    if model.session.help_close_rect.width > 0 {
        frame.render_widget(Paragraph::new(close_text).style(
            Style::default().fg(p.info).bg(p.bg)),model.session.help_close_rect);
    }
    if content.height < 4 { return; }
    let body = Rect::new(content.x, content.y + 2, content.width, content.height - 4);
    let lines = if overview && terminal_help { terminal_guide(p, body.width as usize) }
        else if overview { quick_guide(p, body.width as usize) }
        else { reference_guide(body.width as usize, p) };
    model.session.help_scroll_limit = lines.len().saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    model.session.help_scroll = model.session.help_scroll.min(model.session.help_scroll_limit);
    frame.render_widget(Paragraph::new(lines).scroll((model.session.help_scroll, 0))
        .style(Style::default().bg(p.bg)), body);
    let progress = if model.session.help_scroll_limit == 0 { 100 }
        else { usize::from(model.session.help_scroll) * 100 / usize::from(model.session.help_scroll_limit) };
    frame.render_widget(Paragraph::new(format!("Tab  Switch page    j/k  Scroll    Esc  Close     {progress}%"))
        .style(Style::default().fg(p.fg_dim).bg(p.bg)),
        Rect::new(content.x, content.bottom().saturating_sub(1), content.width, 1));
}

/// Fit both ends independently so a long path or status cannot hide metadata.
pub(super) fn draw_balanced_line(
    frame: &mut Frame<'_>,
    area: Rect,
    left: Line<'_>,
    right: Line<'_>,
    p: Palette,
) {
    let inner = Rect::new(
        area.x + area.width.min(2),
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );
    let right_width = right.width().min(inner.width as usize / 2) as u16;
    let gap = u16::from(right_width > 0) * 2;
    let left_width = inner.width.saturating_sub(right_width + gap);
    frame.render_widget(
        Paragraph::new(left).style(Style::default().fg(p.fg)),
        Rect::new(inner.x, inner.y, left_width, inner.height),
    );
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect::new(
            inner.right().saturating_sub(right_width),
            inner.y,
            right_width,
            inner.height,
        ),
    );
}

pub(super) fn fit_label(text: &str, width: usize, keep_end: bool) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut remaining = width - 1;
    let chars: Vec<_> = if keep_end {
        text.chars().rev().collect()
    } else {
        text.chars().collect()
    };
    let mut kept = Vec::new();
    for ch in chars {
        let cells = ch.width().unwrap_or(0);
        if cells > remaining {
            break;
        }
        remaining -= cells;
        kept.push(ch);
    }
    if keep_end {
        format!("…{}", kept.into_iter().rev().collect::<String>())
    } else {
        format!("{}…", kept.into_iter().collect::<String>())
    }
}

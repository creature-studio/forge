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
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("FORGE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(if overview { "  /  QUICK START" } else { "  /  REFERENCE" }, Style::default().fg(p.info)),
        Span::styled(tab, Style::default().fg(p.fg_dim)),
    ])).style(Style::default().bg(p.bg)), Rect::new(content.x, content.y, content.width, 1));
    if content.height < 4 { return; }
    let body = Rect::new(content.x, content.y + 2, content.width, content.height - 4);
    let lines = if overview { quick_guide(p, body.width as usize) } else { reference_guide(body.width as usize, p) };
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

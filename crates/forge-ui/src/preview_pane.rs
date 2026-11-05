//! Right-hand preview pane (file text or side-by-side diff) — Modern Minimal.

use forge_model::{DiffLine, DiffLineKind};
use forge_theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::style::{self, fg_bg, rgb};

/// Lightweight language hint for plain-text preview coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextLang {
    #[default]
    Plain,
    Rust,
    Markdown,
}

impl TextLang {
    /// Infer from a file name or path segment (extension only).
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".rs") {
            Self::Rust
        } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
            Self::Markdown
        } else {
            Self::Plain
        }
    }
}

/// Borrowed view for the preview pane (keeps forge-ui free of forge-fs).
#[derive(Debug, Clone, Copy)]
pub enum PreviewView<'a> {
    Loading,
    Idle,
    /// Custom idle/hint message (e.g. Git mode with nothing selected).
    Hint {
        text: &'a str,
    },
    Directory {
        name: &'a str,
        children: &'a [(bool, String)],
        truncated: bool,
    },
    Binary {
        notice: &'a str,
    },
    Error {
        message: &'a str,
    },
    Text {
        lines: &'a [String],
        truncated: bool,
        bytes_read: usize,
        language: TextLang,
    },
    Diff {
        path: &'a str,
        side: &'a str,
        lines: &'a [DiffLine],
        hunk_start: Option<usize>,
        hunk_end: Option<usize>,
        hunk_idx: Option<(usize, usize)>,
    },
}

/// Cached side-by-side rows for the current unified diff.
///
/// Rebuilt only when the underlying [`DiffLine`] buffer identity changes, so
/// pure scroll / hunk highlight updates reuse the structural split.
#[derive(Debug, Default, Clone)]
pub struct SplitRowsCache {
    lines_ptr: usize,
    lines_len: usize,
    rows: Vec<SplitRow>,
}

impl SplitRowsCache {
    pub fn clear(&mut self) {
        self.lines_ptr = 0;
        self.lines_len = 0;
        self.rows.clear();
    }

    fn rows_for(&mut self, lines: &[DiffLine]) -> &[SplitRow] {
        let ptr = lines.as_ptr() as usize;
        let len = lines.len();
        if self.lines_ptr != ptr || self.lines_len != len {
            self.lines_ptr = ptr;
            self.lines_len = len;
            self.rows = build_split_rows(lines);
        }
        &self.rows
    }

    /// Ensure cache is warm; returns split-row count (0 if `lines` empty).
    pub fn ensure(&mut self, lines: &[DiffLine]) -> usize {
        self.rows_for(lines).len()
    }
}

/// Draw the preview pane into `area` (no nested box borders).
pub fn draw_preview(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    view: PreviewView<'_>,
    scroll: &mut usize,
    split_cache: &mut SplitRowsCache,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }

    let is_diff = matches!(view, PreviewView::Diff { .. });
    let inner = area;
    style::fill_panel(frame, inner, theme);

    let viewport = inner.height as usize;
    let total = preview_line_count(view, split_cache);
    let max_scroll = total.saturating_sub(viewport);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    let start = *scroll;
    let end = (start + viewport).min(total);
    // Virtualize: only allocate Line/Span for the visible window.
    let visible = build_visible_lines(theme, view, inner.width, start, end, split_cache);

    let style = Style::default().bg(rgb(theme.panel_bg)).fg(rgb(theme.fg));
    // Diff rows are pre-fitted to width; wrapping would break the split columns.
    let para = if is_diff {
        Paragraph::new(visible).style(style)
    } else {
        Paragraph::new(visible)
            .style(style)
            .wrap(Wrap { trim: false })
    };
    frame.render_widget(para, inner);
    inner
}

/// Build only the `[start, end)` content lines (viewport virtualization).
fn build_visible_lines(
    theme: &Theme,
    view: PreviewView<'_>,
    width: u16,
    start: usize,
    end: usize,
    split_cache: &mut SplitRowsCache,
) -> Vec<Line<'static>> {
    if end <= start {
        return Vec::new();
    }
    match view {
        PreviewView::Loading => slice_static(
            start,
            end,
            &[Line::from(Span::styled(
                "  loading...",
                style::muted(theme),
            ))],
        ),
        PreviewView::Idle => slice_static(
            start,
            end,
            &[Line::from(Span::styled(
                "  select a file to preview",
                style::muted(theme),
            ))],
        ),
        PreviewView::Hint { text } => slice_static(
            start,
            end,
            &[Line::from(Span::styled(
                format!("  {text}"),
                style::muted(theme),
            ))],
        ),
        PreviewView::Directory {
            name,
            children,
            truncated,
        } => {
            let count = children.len();
            let count_label = if count == 0 {
                "  (empty)".to_string()
            } else if truncated {
                format!("  {count}+ items")
            } else if count == 1 {
                "  1 item".to_string()
            } else {
                format!("  {count} items")
            };
            let all = [
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {name}/"),
                    Style::default()
                        .fg(rgb(theme.directory))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(count_label, style::muted(theme))),
                Line::from(""),
                Line::from(Span::styled("  Enter / l to open", style::muted(theme))),
            ];
            slice_static(start, end, &all)
        }
        PreviewView::Binary { notice } => {
            let all = [
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {notice}"),
                    Style::default().fg(rgb(theme.warning)),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  binary preview not shown",
                    style::muted(theme),
                )),
            ];
            slice_static(start, end, &all)
        }
        PreviewView::Error { message } => {
            let all = [
                Line::from(""),
                Line::from(Span::styled(
                    format!("  error: {message}"),
                    Style::default().fg(rgb(theme.danger)),
                )),
            ];
            slice_static(start, end, &all)
        }
        PreviewView::Text {
            lines,
            truncated,
            bytes_read,
            language,
        } => {
            let total = text_preview_line_count(lines, truncated);
            let end = end.min(total);
            let start = start.min(end);
            let mut out = Vec::with_capacity(end.saturating_sub(start));
            for idx in start..end {
                if lines.is_empty() {
                    // single placeholder at 0; footer only if truncated (rare for empty)
                    if idx == 0 {
                        out.push(Line::from(Span::styled(
                            "  (empty file)",
                            style::muted(theme),
                        )));
                    } else if truncated && idx == total.saturating_sub(1) {
                        out.push(Line::from(Span::styled(
                            format!("  ... truncated - {bytes_read} bytes shown"),
                            style::muted(theme),
                        )));
                    } else if truncated && idx == total.saturating_sub(2) {
                        out.push(Line::from(""));
                    }
                    continue;
                }
                let body_len = lines.len();
                if idx < body_len {
                    out.push(text_line_numbered(theme, idx + 1, &lines[idx], language));
                } else if truncated {
                    let footer_i = idx - body_len;
                    if footer_i == 0 {
                        out.push(Line::from(""));
                    } else {
                        out.push(Line::from(Span::styled(
                            format!("  ... truncated - {bytes_read} bytes shown"),
                            style::muted(theme),
                        )));
                    }
                }
            }
            out
        }
        PreviewView::Diff {
            lines,
            hunk_start,
            hunk_end,
            ..
        } => {
            if lines.is_empty() {
                return slice_static(
                    start,
                    end,
                    &[Line::from(Span::styled("  (no diff)", style::muted(theme)))],
                );
            }
            let rows = split_cache.rows_for(lines);
            let end = end.min(rows.len());
            let start = start.min(end);
            rows[start..end]
                .iter()
                .map(|row| {
                    let in_hunk = match (hunk_start, hunk_end) {
                        (Some(s), Some(e)) => row.src_indices.iter().any(|&i| i >= s && i < e),
                        _ => false,
                    };
                    render_split_row(theme, row, width, in_hunk)
                })
                .collect()
        }
    }
}

fn slice_static(start: usize, end: usize, all: &[Line<'static>]) -> Vec<Line<'static>> {
    let end = end.min(all.len());
    let start = start.min(end);
    all[start..end].to_vec()
}

fn text_preview_line_count(lines: &[String], truncated: bool) -> usize {
    let base = if lines.is_empty() { 1 } else { lines.len() };
    if truncated {
        base + 2
    } else {
        base
    }
}

/// Test/helper: build the full line list (not used on the hot scroll path).
#[cfg(test)]
fn build_lines(
    theme: &Theme,
    view: PreviewView<'_>,
    width: u16,
    split_cache: &mut SplitRowsCache,
) -> Vec<Line<'static>> {
    let total = preview_line_count(view, split_cache);
    build_visible_lines(theme, view, width, 0, total, split_cache)
}

/// Split a path-like preview title into muted dirname + basename.
/// Basename-only titles return `(None, name)`.
#[cfg(test)]
fn split_preview_path(title: &str) -> (Option<String>, String) {
    let title = title.trim();
    if title.is_empty() {
        return (None, "-".to_string());
    }
    let norm = title.replace('\\', "/");
    match norm.rsplit_once('/') {
        Some((dir, base)) if !base.is_empty() => {
            let prefix = if dir.is_empty() {
                "/".to_string()
            } else {
                format!("{dir}/")
            };
            (Some(prefix), base.to_string())
        }
        _ => (None, title.to_string()),
    }
}

/// Line-number gutter: slightly brighter than chrome muted + visible rule.
fn gutter_num_fg(theme: &Theme) -> forge_theme::Rgb {
    // Lift muted ~1/3 of the way toward fg so numbers stay secondary but readable.
    forge_theme::Rgb::new(
        ((2 * u16::from(theme.muted.r) + u16::from(theme.fg.r)) / 3) as u8,
        ((2 * u16::from(theme.muted.g) + u16::from(theme.fg.g)) / 3) as u8,
        ((2 * u16::from(theme.muted.b) + u16::from(theme.fg.b)) / 3) as u8,
    )
}

fn text_line_numbered(theme: &Theme, n: usize, raw: &str, language: TextLang) -> Line<'static> {
    let num = Span::styled(
        format!("{n:>4}"),
        Style::default()
            .fg(rgb(gutter_num_fg(theme)))
            .bg(rgb(theme.panel_bg)),
    );
    // Muted rule — theme.border is too close to panel_bg to read as a gutter.
    let sep = Span::styled(
        "│",
        Style::default()
            .fg(rgb(theme.muted))
            .bg(rgb(theme.panel_bg)),
    );
    let body = match language {
        TextLang::Rust => rust_line(theme, raw),
        TextLang::Markdown => markdown_line(theme, raw),
        TextLang::Plain => Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.fg, theme.panel_bg),
        )),
    };
    let mut spans = vec![num, sep];
    spans.extend(body.spans);
    Line::from(spans)
}

fn markdown_line(theme: &Theme, raw: &str) -> Line<'static> {
    let trimmed = raw.trim_start();

    if trimmed.starts_with("# ") {
        return Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.accent, theme.panel_bg).add_modifier(Modifier::BOLD),
        ));
    }
    if trimmed.starts_with("## ") {
        return Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.directory, theme.panel_bg).add_modifier(Modifier::BOLD),
        ));
    }
    if trimmed.starts_with("### ") || trimmed.starts_with("#### ") {
        return Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.fg, theme.panel_bg).add_modifier(Modifier::BOLD),
        ));
    }
    if trimmed.starts_with("```") || trimmed.starts_with("---") || trimmed.starts_with("***") {
        return Line::from(Span::styled(raw.to_string(), style::muted(theme)));
    }
    if trimmed.starts_with('|') {
        return Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.muted, theme.panel_bg),
        ));
    }
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        let bullet_end = raw.len() - trimmed.len() + 2;
        let (head, rest) = raw.split_at(bullet_end.min(raw.len()));
        return Line::from(vec![
            Span::styled(head.to_string(), fg_bg(theme.accent, theme.panel_bg)),
            Span::styled(rest.to_string(), fg_bg(theme.fg, theme.panel_bg)),
        ]);
    }

    Line::from(Span::styled(
        raw.to_string(),
        fg_bg(theme.fg, theme.panel_bg),
    ))
}

const RUST_KEYWORDS: &[&str] = &[
    "use", "as", "pub", "struct", "enum", "fn", "let", "mut", "const", "static", "if", "else",
    "match", "for", "while", "loop", "return", "impl", "trait", "type", "where", "self", "Self",
    "crate", "mod", "async", "await", "move", "ref", "in", "of", "true", "false",
];

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn rust_line(theme: &Theme, raw: &str) -> Line<'static> {
    let trimmed = raw.trim_start();
    if trimmed.starts_with("//") || trimmed.contains("/*") || trimmed.contains("*/") {
        return Line::from(Span::styled(raw.to_string(), style::muted(theme)));
    }

    // Edit-like magenta keywords — accent cyan is too close to UI chrome / easy to miss in code.
    let keyword_fg = forge_theme::Rgb::new(0xc6, 0x78, 0xdd);
    let keyword = fg_bg(keyword_fg, theme.panel_bg).add_modifier(Modifier::BOLD);
    let string_st = fg_bg(theme.warning, theme.panel_bg);
    let normal = fg_bg(theme.fg, theme.panel_bg);

    let chars: Vec<char> = raw.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut i = 0usize;
    let mut buf = String::new();

    let flush = |buf: &mut String, style: Style, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };

    while i < chars.len() {
        let c = chars[i];

        // Double-quoted string
        if c == '"' {
            flush(&mut buf, normal, &mut spans);
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            spans.push(Span::styled(s, string_st));
            continue;
        }

        // Char literal or lifetime
        if c == '\'' {
            let next = chars.get(i + 1).copied();
            // Lifetime / label: 'ident — alpha after quote, not a closed char literal
            let looks_like_lifetime = matches!(next, Some(n) if n.is_ascii_alphabetic() || n == '_')
                && !matches!((next, chars.get(i + 2).copied()), (Some(_), Some('\'')));
            if looks_like_lifetime {
                // keep as normal text (lifetime)
                buf.push(c);
                i += 1;
                continue;
            }
            flush(&mut buf, normal, &mut spans);
            let start = i;
            i += 1;
            if i < chars.len() && chars[i] == '\\' {
                i += 2; // escape + char
            } else if i < chars.len() {
                i += 1; // single char
            }
            if i < chars.len() && chars[i] == '\'' {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            spans.push(Span::styled(s, string_st));
            continue;
        }

        // Identifier / keyword
        if c.is_ascii_alphabetic() || c == '_' {
            flush(&mut buf, normal, &mut spans);
            let start = i;
            i += 1;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if RUST_KEYWORDS.contains(&word.as_str()) {
                spans.push(Span::styled(word, keyword));
            } else {
                spans.push(Span::styled(word, normal));
            }
            continue;
        }

        buf.push(c);
        i += 1;
    }
    flush(&mut buf, normal, &mut spans);

    if spans.is_empty() {
        Line::from(Span::styled(raw.to_string(), normal))
    } else {
        Line::from(spans)
    }
}

/// One rendered row in the side-by-side diff body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SplitRow {
    kind: SplitRowKind,
    /// Original unified [`DiffLine`] indices contributing to this row.
    src_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SplitRowKind {
    /// Optional one-line file note (`new file` / `deleted file`).
    Note(String),
    /// Hunk header (`@@ … @@`), full width.
    Header(String),
    /// Old (left) | new (right). `None` = empty counterpart cell.
    Pair {
        old: Option<String>,
        new: Option<String>,
        old_del: bool,
        new_add: bool,
    },
}

fn strip_diff_mark(text: &str) -> &str {
    let b = text.as_bytes();
    if b.is_empty() {
        return text;
    }
    match b[0] {
        b'+' | b'-' | b' ' => &text[1..],
        _ => text,
    }
}

fn is_hunk_header(line: &DiffLine) -> bool {
    matches!(line.kind, DiffLineKind::Header) && line.text.starts_with("@@")
}

fn is_file_meta(line: &DiffLine) -> bool {
    matches!(line.kind, DiffLineKind::Meta) && line.text.starts_with("diff ")
}

/// Transform unified diff lines into side-by-side rows (drops raw git chrome).
fn build_split_rows(lines: &[DiffLine]) -> Vec<SplitRow> {
    let mut rows: Vec<SplitRow> = Vec::new();

    // Optional single muted note from meta (keep minimal).
    let mut note: Option<(usize, String)> = None;
    for (idx, line) in lines.iter().enumerate() {
        if !matches!(line.kind, DiffLineKind::Meta) {
            continue;
        }
        let lower = line.text.to_ascii_lowercase();
        if lower.contains("new file") {
            note = Some((idx, "new file".into()));
            break;
        }
        if lower.contains("deleted file") {
            note = Some((idx, "deleted file".into()));
            break;
        }
    }
    if let Some((idx, text)) = note {
        rows.push(SplitRow {
            kind: SplitRowKind::Note(text),
            src_indices: vec![idx],
        });
    }

    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        if matches!(line.kind, DiffLineKind::Meta) {
            i += 1;
            continue;
        }
        if is_hunk_header(line) {
            rows.push(SplitRow {
                kind: SplitRowKind::Header(line.text.clone()),
                src_indices: vec![i],
            });
            i += 1;
            while i < lines.len() {
                if is_hunk_header(&lines[i]) || is_file_meta(&lines[i]) {
                    break;
                }
                if matches!(lines[i].kind, DiffLineKind::Meta) {
                    i += 1;
                    continue;
                }
                match lines[i].kind {
                    DiffLineKind::Context => {
                        let t = strip_diff_mark(&lines[i].text).to_string();
                        rows.push(SplitRow {
                            kind: SplitRowKind::Pair {
                                old: Some(t.clone()),
                                new: Some(t),
                                old_del: false,
                                new_add: false,
                            },
                            src_indices: vec![i],
                        });
                        i += 1;
                    }
                    DiffLineKind::Delete | DiffLineKind::Add => {
                        let mut dels: Vec<usize> = Vec::new();
                        let mut adds: Vec<usize> = Vec::new();
                        while i < lines.len() && matches!(lines[i].kind, DiffLineKind::Delete) {
                            dels.push(i);
                            i += 1;
                        }
                        while i < lines.len() && matches!(lines[i].kind, DiffLineKind::Add) {
                            adds.push(i);
                            i += 1;
                        }
                        let n = dels.len().max(adds.len());
                        for k in 0..n {
                            let mut src = Vec::new();
                            let old = dels.get(k).map(|&di| {
                                src.push(di);
                                strip_diff_mark(&lines[di].text).to_string()
                            });
                            let new = adds.get(k).map(|&ai| {
                                src.push(ai);
                                strip_diff_mark(&lines[ai].text).to_string()
                            });
                            rows.push(SplitRow {
                                kind: SplitRowKind::Pair {
                                    old,
                                    new,
                                    old_del: dels.get(k).is_some(),
                                    new_add: adds.get(k).is_some(),
                                },
                                src_indices: src,
                            });
                        }
                    }
                    DiffLineKind::Header | DiffLineKind::Meta => break,
                }
            }
            continue;
        }
        // Orphan non-meta lines outside a hunk — skip quietly.
        i += 1;
    }

    rows
}

/// Pad/truncate `s` to exactly `width` *terminal columns* (CJK = 2).
fn fit_cell(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cw == 0 {
            // Keep combining marks attached to the previous visible char.
            if !out.is_empty() {
                out.push(ch);
            }
            continue;
        }
        if w + cw > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    while w < width {
        out.push(' ');
        w += 1;
    }
    debug_assert_eq!(UnicodeWidthStr::width(out.as_str()), width);
    out
}

/// accent(1) + left + │ + right == `total`. Returns `(left_w, right_w)`.
fn split_half_widths(total: usize) -> Option<(usize, usize)> {
    if total < 5 {
        return None;
    }
    let rest = total - 1; // after accent
    let left_w = rest / 2;
    let right_w = rest.saturating_sub(left_w + 1);
    Some((left_w, right_w))
}

fn render_split_row(
    theme: &Theme,
    row: &SplitRow,
    width: u16,
    in_active_hunk: bool,
) -> Line<'static> {
    let w = width as usize;
    let base_bg = if in_active_hunk {
        theme.pill_bg
    } else {
        theme.panel_bg
    };
    let accent_ch = if in_active_hunk { "▎" } else { " " };
    let accent_style = Style::default()
        .fg(rgb(if in_active_hunk {
            theme.accent
        } else {
            theme.panel_bg
        }))
        .bg(rgb(base_bg));

    let sep_style = Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg));
    let Some((left_w, right_w)) = split_half_widths(w) else {
        return Line::from(Span::styled(
            fit_cell("", w),
            Style::default().bg(rgb(base_bg)),
        ));
    };

    match &row.kind {
        // Note/Header share the same │ column as Pair rows so the rule stays straight.
        SplitRowKind::Note(text) => {
            let left_txt = fit_cell(&format!(" {text}"), left_w);
            let right_txt = fit_cell("", right_w);
            let muted = Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg));
            Line::from(vec![
                Span::styled(accent_ch.to_string(), accent_style),
                Span::styled(left_txt, muted),
                Span::styled("│".to_string(), sep_style),
                Span::styled(right_txt, muted),
            ])
        }
        SplitRowKind::Header(text) => {
            let left_txt = fit_cell(&format!(" {text}"), left_w);
            let right_txt = fit_cell("", right_w);
            let hdr = Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(base_bg))
                .add_modifier(Modifier::BOLD);
            let muted = Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg));
            Line::from(vec![
                Span::styled(accent_ch.to_string(), accent_style),
                Span::styled(left_txt, hdr),
                Span::styled("│".to_string(), sep_style),
                Span::styled(right_txt, muted),
            ])
        }
        SplitRowKind::Pair {
            old,
            new,
            old_del,
            new_add,
        } => {
            let left_txt = fit_cell(old.as_deref().unwrap_or(""), left_w);
            let right_txt = fit_cell(new.as_deref().unwrap_or(""), right_w);

            let left_style = if *old_del {
                Style::default()
                    .fg(rgb(theme.diff_del))
                    .bg(rgb(theme.diff_del_bg))
            } else if old.is_some() {
                Style::default().fg(rgb(theme.fg)).bg(rgb(base_bg))
            } else {
                Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg))
            };
            let right_style = if *new_add {
                Style::default()
                    .fg(rgb(theme.diff_add))
                    .bg(rgb(theme.diff_add_bg))
            } else if new.is_some() {
                Style::default().fg(rgb(theme.fg)).bg(rgb(base_bg))
            } else {
                Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg))
            };

            Line::from(vec![
                Span::styled(accent_ch.to_string(), accent_style),
                Span::styled(left_txt, left_style),
                Span::styled("│".to_string(), sep_style),
                Span::styled(right_txt, right_style),
            ])
        }
    }
}

/// Rendered row index of a unified hunk start (for n/N scroll), after meta collapse.
pub fn diff_render_hunk_start(
    lines: &[DiffLine],
    hunk_start: usize,
    split_cache: &mut SplitRowsCache,
) -> usize {
    split_cache
        .rows_for(lines)
        .iter()
        .position(|r| r.src_indices.contains(&hunk_start))
        .unwrap_or(0)
}

/// How many content lines a preview view would render (before scrolling).
pub fn preview_line_count(view: PreviewView<'_>, split_cache: &mut SplitRowsCache) -> usize {
    match view {
        PreviewView::Loading | PreviewView::Idle | PreviewView::Hint { .. } => 1,
        PreviewView::Error { .. } => 2,
        PreviewView::Binary { .. } => 4,
        PreviewView::Directory { .. } => {
            // blank + name + count + blank + Enter hint
            5
        }
        PreviewView::Text {
            lines, truncated, ..
        } => text_preview_line_count(lines, truncated),
        PreviewView::Diff { lines, .. } => {
            if lines.is_empty() {
                1
            } else {
                split_cache.ensure(lines).max(1)
            }
        }
    }
}

/// Prefer scrolling so the active hunk header stays near the top third.
pub fn scroll_to_hunk(
    scroll: &mut usize,
    viewport: usize,
    total_lines: usize,
    rendered_hunk_start: usize,
) {
    if viewport == 0 || total_lines == 0 {
        *scroll = 0;
        return;
    }
    let target = rendered_hunk_start.saturating_sub(viewport / 3);
    let max_scroll = total_lines.saturating_sub(viewport);
    *scroll = target.min(max_scroll);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_theme::Theme;

    #[test]
    fn rust_keywords_use_magenta_not_fg() {
        let theme = Theme::modern_ide();
        let line = rust_line(&theme, "use clap::Parser;");
        let styles: Vec<_> = line.spans.iter().map(|s| s.style.fg).collect();
        let magenta = style::rgb(forge_theme::Rgb::new(0xc6, 0x78, 0xdd));
        let fg = style::rgb(theme.fg);
        assert!(
            styles.iter().any(|c| *c == Some(magenta)),
            "expected magenta keyword span, got {:?}",
            styles
        );
        // first span should be the keyword "use"
        assert_eq!(line.spans[0].content.as_ref(), "use");
        assert_eq!(line.spans[0].style.fg, Some(magenta));
        assert_ne!(magenta, fg);
    }

    #[test]
    fn split_preview_path_dirname_and_basename() {
        assert_eq!(
            split_preview_path("crates/forge/src/main.rs"),
            (Some("crates/forge/src/".into()), "main.rs".into())
        );
        assert_eq!(split_preview_path("main.rs"), (None, "main.rs".into()));
        assert_eq!(split_preview_path(""), (None, "-".into()));
        assert_eq!(
            split_preview_path("/abs/file.rs"),
            (Some("/abs/".into()), "file.rs".into())
        );
    }

    #[test]
    fn gutter_uses_brighter_muted_and_rule() {
        let theme = Theme::modern_ide();
        let line = text_line_numbered(&theme, 7, "fn main() {}", TextLang::Plain);
        assert!(line.spans.len() >= 2);
        assert_eq!(line.spans[0].content.as_ref(), "   7");
        assert_eq!(line.spans[1].content.as_ref(), "│");
        let muted = style::rgb(theme.muted);
        let bright = style::rgb(gutter_num_fg(&theme));
        assert_ne!(bright, muted);
        assert_eq!(line.spans[0].style.fg, Some(bright));
        assert_eq!(line.spans[1].style.fg, Some(muted));
    }

    #[test]
    fn split_rows_skip_meta_and_pair_changes() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "diff --git a/.gitignore b/.gitignore".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "new file mode 100644".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "index 0000000..9227398".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "--- /dev/null".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "+++ b/.gitignore".into(),
            },
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -0,0 +1,3 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+/target/".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+**/*.rs.bk".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+.DS_Store".into(),
            },
        ];
        let rows = build_split_rows(&lines);
        assert!(matches!(rows[0].kind, SplitRowKind::Note(ref s) if s == "new file"));
        assert!(matches!(rows[1].kind, SplitRowKind::Header(ref s) if s.starts_with("@@")));
        assert_eq!(rows.len(), 5); // note + header + 3 adds
        match &rows[2].kind {
            SplitRowKind::Pair {
                old,
                new,
                old_del,
                new_add,
            } => {
                assert!(old.is_none());
                assert_eq!(new.as_deref(), Some("/target/"));
                assert!(!*old_del);
                assert!(*new_add);
            }
            other => panic!("expected pair, got {other:?}"),
        }
        assert_eq!(
            diff_render_hunk_start(&lines, 5, &mut SplitRowsCache::default()),
            1
        );
    }

    #[test]
    fn split_rows_pair_delete_with_add() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -1,2 +1,2 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                text: " keep".into(),
            },
            DiffLine {
                kind: DiffLineKind::Delete,
                text: "-old".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+new".into(),
            },
        ];
        let rows = build_split_rows(&lines);
        assert_eq!(rows.len(), 3);
        match &rows[2].kind {
            SplitRowKind::Pair {
                old,
                new,
                old_del,
                new_add,
            } => {
                assert_eq!(old.as_deref(), Some("old"));
                assert_eq!(new.as_deref(), Some("new"));
                assert!(*old_del && *new_add);
            }
            other => panic!("expected paired change, got {other:?}"),
        }
    }

    #[test]
    fn split_render_has_no_cyan_marker_prefix() {
        let theme = Theme::modern_ide();
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -1 +1 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+x".into(),
            },
        ];
        let view = PreviewView::Diff {
            path: "f",
            side: "unstaged",
            lines: &lines,
            hunk_start: Some(0),
            hunk_end: Some(2),
            hunk_idx: Some((0, 1)),
        };
        let mut cache = SplitRowsCache::default();
        let rendered = build_lines(&theme, view, 40, &mut cache);
        let joined: String = rendered
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!joined.contains("> +"), "no unified > marker: {joined}");
        assert!(joined.contains('│'), "split separator present");
        // Separator should use muted (readable on charcoal), not cyan accent.
        let sep_fg = rendered
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "│")
            .and_then(|s| s.style.fg);
        assert_eq!(
            sep_fg,
            Some(crate::style::rgb(theme.muted)),
            "split │ should use muted fg"
        );
        assert_eq!(preview_line_count(view, &mut cache), rendered.len());
    }

    #[test]
    fn split_change_row_keeps_del_add_backgrounds() {
        let theme = Theme::modern_ide();
        let row = SplitRow {
            kind: SplitRowKind::Pair {
                old: Some("gone".into()),
                new: Some("here".into()),
                old_del: true,
                new_add: true,
            },
            src_indices: vec![0, 1],
        };
        let line = render_split_row(&theme, &row, 40, false);
        let spans: Vec<_> = line.spans.iter().collect();
        assert!(spans.len() >= 4);
        assert_eq!(spans[2].content.as_ref(), "│");
        assert_eq!(
            spans[1].style.bg,
            Some(crate::style::rgb(theme.diff_del_bg))
        );
        assert_eq!(
            spans[3].style.bg,
            Some(crate::style::rgb(theme.diff_add_bg))
        );
        assert_eq!(spans[2].style.fg, Some(crate::style::rgb(theme.muted)));
        assert_ne!(
            spans[2].style.fg,
            Some(crate::style::rgb(theme.border_focus))
        );
    }

    #[test]
    fn split_separator_column_stable_with_cjk() {
        use unicode_width::UnicodeWidthStr;
        let theme = Theme::modern_ide();
        let width = 48u16;
        let rows = [
            SplitRow {
                kind: SplitRowKind::Header("@@ -1,3 +1,3 @@".into()),
                src_indices: vec![0],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("- 近黑背景，冷白/青 accent".into()),
                    new: Some("- 近黑背景，冷白/青 accent".into()),
                    old_del: false,
                    new_add: false,
                },
                src_indices: vec![1],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("冷白/青 accent: 选中为浅抬升而非刺眼反色".into()),
                    new: Some("冷白/青 accent: 选中为浅抬升，左右对比 diff 更直观".into()),
                    old_del: true,
                    new_add: true,
                },
                src_indices: vec![2, 3],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("ascii context line".into()),
                    new: Some("ascii context line".into()),
                    old_del: false,
                    new_add: false,
                },
                src_indices: vec![4],
            },
        ];
        let rendered: Vec<_> = rows
            .iter()
            .map(|r| render_split_row(&theme, r, width, true))
            .collect();
        let left_widths: Vec<_> = rendered
            .iter()
            .map(|l| {
                assert_eq!(l.spans[2].content.as_ref(), "│");
                UnicodeWidthStr::width(l.spans[1].content.as_ref())
            })
            .collect();
        assert!(
            left_widths.windows(2).all(|w| w[0] == w[1]),
            "left cell display width must match across rows: {left_widths:?}"
        );
        let (left_w, _) = split_half_widths(width as usize).unwrap();
        assert!(left_widths.iter().all(|&w| w == left_w));
    }

    #[test]
    fn fit_cell_counts_cjk_as_two_columns() {
        use unicode_width::UnicodeWidthStr;
        let s = fit_cell("中文ab", 8);
        assert_eq!(UnicodeWidthStr::width(s.as_str()), 8);
        assert!(s.starts_with("中文ab"));
        let tight = fit_cell("中文", 3); // 中=2, only one CJK fits + pad
        assert_eq!(UnicodeWidthStr::width(tight.as_str()), 3);
        assert!(tight.starts_with('中'));
    }

    #[test]
    fn split_rows_zip_uneven_delete_add_runs() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -1,3 +1,2 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Delete,
                text: "-a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Delete,
                text: "-b".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+c".into(),
            },
        ];
        let rows = build_split_rows(&lines);
        assert_eq!(rows.len(), 3); // header + 2 paired/padded rows
        match &rows[1].kind {
            SplitRowKind::Pair {
                old,
                new,
                old_del,
                new_add,
            } => {
                assert_eq!(old.as_deref(), Some("a"));
                assert_eq!(new.as_deref(), Some("c"));
                assert!(*old_del && *new_add);
            }
            other => panic!("expected first zip pair, got {other:?}"),
        }
        match &rows[2].kind {
            SplitRowKind::Pair {
                old,
                new,
                old_del,
                new_add,
            } => {
                assert_eq!(old.as_deref(), Some("b"));
                assert!(new.is_none());
                assert!(*old_del && !*new_add);
            }
            other => panic!("expected unpaired delete, got {other:?}"),
        }
        assert_eq!(
            diff_render_hunk_start(&lines, 0, &mut SplitRowsCache::default()),
            0
        );
    }
    #[test]
    fn split_cache_reuses_rows_across_lookups() {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Header,
                text: "@@ -1 +1 @@".into(),
            },
            DiffLine {
                kind: DiffLineKind::Add,
                text: "+x".into(),
            },
        ];
        let mut cache = SplitRowsCache::default();
        let n1 = preview_line_count(
            PreviewView::Diff {
                path: "f",
                side: "unstaged",
                lines: &lines,
                hunk_start: None,
                hunk_end: None,
                hunk_idx: None,
            },
            &mut cache,
        );
        let ptr1 = cache.rows.as_ptr();
        let n2 = preview_line_count(
            PreviewView::Diff {
                path: "f",
                side: "unstaged",
                lines: &lines,
                hunk_start: Some(0),
                hunk_end: Some(2),
                hunk_idx: Some((0, 1)),
            },
            &mut cache,
        );
        let ptr2 = cache.rows.as_ptr();
        assert_eq!(n1, n2);
        assert_eq!(
            ptr1, ptr2,
            "scroll/hunk highlight must not rebuild split rows"
        );
        assert_eq!(diff_render_hunk_start(&lines, 0, &mut cache), 0);
        assert_eq!(cache.rows.as_ptr(), ptr2);
    }

    #[test]
    fn visible_text_window_matches_full_slice() {
        let theme = Theme::modern_ide();
        let lines = (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>();
        let view = PreviewView::Text {
            lines: &lines,
            truncated: false,
            bytes_read: 100,
            language: TextLang::Plain,
        };
        let mut cache = SplitRowsCache::default();
        let full = build_lines(&theme, view, 40, &mut cache);
        let window = build_visible_lines(&theme, view, 40, 5, 15, &mut cache);
        assert_eq!(window.len(), 10);
        for (a, b) in window.iter().zip(full[5..15].iter()) {
            assert_eq!(a.spans[0].content.as_ref(), b.spans[0].content.as_ref());
        }
    }

    #[test]
    fn scroll_to_hunk_clamps_and_targets_third() {
        let mut scroll = 0;
        scroll_to_hunk(&mut scroll, 30, 100, 40);
        // 40 - 30/3 = 30
        assert_eq!(scroll, 30);
        scroll_to_hunk(&mut scroll, 30, 100, 95);
        assert_eq!(scroll, 70); // max = 100-30
        scroll_to_hunk(&mut scroll, 0, 100, 10);
        assert_eq!(scroll, 0);
    }
}

//! Right-hand preview pane (file text or side-by-side diff) — Modern Minimal.

use forge_model::{DiffLine, DiffLineKind};
use forge_theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
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

/// Draw the preview pane into `area` (no nested box borders).
pub fn draw_preview(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    title: &str,
    view: PreviewView<'_>,
    scroll: &mut usize,
    focused: bool,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }

    let is_diff = matches!(view, PreviewView::Diff { .. });
    // Diff: title + thin rule + body. File/text: title + body.
    let (title_area, inner) = if is_diff {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(area);
        (chunks[0], chunks[2])
    } else {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
        (chunks[0], chunks[1])
    };

    // Diff: muted side label + cyan chip on basename (+ hunk note).
    // Text/file: muted dirname prefix + cyan basename chip when path-like.
    let title_line = match view {
        PreviewView::Diff {
            path,
            side,
            hunk_idx,
            ..
        } => {
            let basename = path.rsplit('/').next().unwrap_or(path);
            let hunk_note = match hunk_idx {
                Some((i, n)) if n > 0 => {
                    let unit = if n == 1 { "hunk" } else { "hunks" };
                    format!("  {}/{} {unit}", i + 1, n)
                }
                _ => String::new(),
            };
            // One header line: muted side + cyan basename chip + optional hunk note.
            Line::from(vec![
                Span::styled(
                    format!(" {side} "),
                    Style::default()
                        .fg(rgb(theme.muted))
                        .bg(rgb(theme.panel_bg)),
                ),
                Span::styled(
                    format!(" {basename} "),
                    Style::default()
                        .fg(rgb(theme.selection_fg))
                        .bg(rgb(theme.selection_bg))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    hunk_note,
                    Style::default()
                        .fg(rgb(theme.muted))
                        .bg(rgb(theme.panel_bg)),
                ),
            ])
        }
        _ => file_preview_title_line(theme, title, focused),
    };
    frame.render_widget(
        Paragraph::new(title_line).style(Style::default().bg(rgb(theme.panel_bg))),
        title_area,
    );

    if is_diff {
        // Thin muted rule under the title — anchors short diffs without filler.
        let rule_area = Rect {
            x: area.x,
            y: title_area.y.saturating_add(1),
            width: area.width,
            height: 1,
        };
        let rule = "─".repeat(rule_area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                rule,
                Style::default()
                    .fg(rgb(theme.muted))
                    .bg(rgb(theme.panel_bg)),
            )))
            .style(Style::default().bg(rgb(theme.panel_bg))),
            rule_area,
        );
    }

    style::fill_panel(frame, inner, theme);

    let lines = build_lines(theme, view, inner.width);

    let viewport = inner.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(viewport);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    let start = *scroll;
    let end = (start + viewport).min(total);
    let visible = lines[start..end].to_vec();

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

fn build_lines(theme: &Theme, view: PreviewView<'_>, width: u16) -> Vec<Line<'static>> {
    match view {
        PreviewView::Loading => vec![Line::from(Span::styled(
            "  loading...",
            style::muted(theme),
        ))],
        PreviewView::Idle => vec![Line::from(Span::styled(
            "  select a file to preview",
            style::muted(theme),
        ))],
        PreviewView::Hint { text } => vec![Line::from(Span::styled(
            format!("  {text}"),
            style::muted(theme),
        ))],
        PreviewView::Directory {
            name,
            children,
            truncated,
        } => {
            // Minimal: name + child count (tree already lists children).
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
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {name}/"),
                    Style::default()
                        .fg(rgb(theme.directory))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(count_label, style::muted(theme))),
                Line::from(""),
                Line::from(Span::styled(
                    "  Enter / l to open",
                    style::muted(theme),
                )),
            ]
        }
        PreviewView::Binary { notice } => vec![
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
        ],
        PreviewView::Error { message } => vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  error: {message}"),
                Style::default().fg(rgb(theme.danger)),
            )),
        ],
        PreviewView::Text {
            lines,
            truncated,
            bytes_read,
            language,
        } => {
            let mut out: Vec<Line<'static>> = if lines.is_empty() {
                vec![Line::from(Span::styled(
                    "  (empty file)",
                    style::muted(theme),
                ))]
            } else {
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| text_line_numbered(theme, i + 1, l, language))
                    .collect()
            };
            if truncated {
                out.push(Line::from(""));
                out.push(Line::from(Span::styled(
                    format!("  ... truncated - {bytes_read} bytes shown"),
                    style::muted(theme),
                )));
            }
            out
        }
        PreviewView::Diff {
            lines,
            hunk_start,
            hunk_end,
            ..
        } => {
            // Side-by-side rows; raw git meta is dropped in build_split_rows.
            if lines.is_empty() {
                vec![Line::from(Span::styled(
                    "  (no diff)",
                    style::muted(theme),
                ))]
            } else {
                let rows = build_split_rows(lines);
                rows.iter()
                    .map(|row| {
                        let in_hunk = match (hunk_start, hunk_end) {
                            (Some(s), Some(e)) => {
                                row.src_indices.iter().any(|&i| i >= s && i < e)
                            }
                            _ => false,
                        };
                        render_split_row(theme, row, width, in_hunk)
                    })
                    .collect()
            }
        }
    }
}

/// Split a path-like preview title into muted dirname + basename.
/// Basename-only titles return `(None, name)`.
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

fn file_preview_title_line(theme: &Theme, title: &str, focused: bool) -> Line<'static> {
    let (prefix, base) = split_preview_path(title);
    let has_prefix = prefix.is_some();
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let Some(dir) = prefix {
        spans.push(Span::styled(
            format!(" {dir}"),
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.panel_bg)),
        ));
    }
    if focused {
        spans.push(Span::styled(
            format!(" {base} "),
            Style::default()
                .fg(rgb(theme.selection_fg))
                .bg(rgb(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        ));
    } else if has_prefix {
        spans.push(Span::styled(
            base,
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.panel_bg)),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {base}"),
            Style::default()
                .fg(rgb(theme.muted))
                .bg(rgb(theme.panel_bg)),
        ));
    }
    Line::from(spans)
}

/// Line-number gutter: slightly brighter than chrome muted + visible rule.
fn gutter_num_fg(theme: &Theme) -> forge_theme::Rgb {
    // Lift muted ~1/3 of the way toward fg so numbers stay secondary but readable.
    forge_theme::Rgb::new(
        theme.muted.r + (theme.fg.r - theme.muted.r) / 3,
        theme.muted.g + (theme.fg.g - theme.muted.g) / 3,
        theme.muted.b + (theme.fg.b - theme.muted.b) / 3,
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
                && !matches!(
                    (next, chars.get(i + 2).copied()),
                    (Some(_), Some('\''))
                );
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

fn fit_cell(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        if w + 1 > width {
            break;
        }
        out.push(ch);
        w += 1;
    }
    while w < width {
        out.push(' ');
        w += 1;
    }
    out
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

    match &row.kind {
        SplitRowKind::Note(text) => {
            let body = if w <= 1 {
                String::new()
            } else {
                fit_cell(&format!(" {text}"), w.saturating_sub(1))
            };
            Line::from(vec![
                Span::styled(accent_ch.to_string(), accent_style),
                Span::styled(
                    body,
                    Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg)),
                ),
            ])
        }
        SplitRowKind::Header(text) => {
            let body = if w <= 1 {
                String::new()
            } else {
                fit_cell(&format!(" {text}"), w.saturating_sub(1))
            };
            Line::from(vec![
                Span::styled(accent_ch.to_string(), accent_style),
                Span::styled(
                    body,
                    Style::default()
                        .fg(rgb(theme.muted))
                        .bg(rgb(base_bg))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        }
        SplitRowKind::Pair {
            old,
            new,
            old_del,
            new_add,
        } => {
            if w < 5 {
                return Line::from(Span::styled(
                    fit_cell("", w),
                    Style::default().bg(rgb(base_bg)),
                ));
            }
            // accent(1) + left + │ + right
            let rest = w - 1;
            let left_w = rest / 2;
            let right_w = rest.saturating_sub(left_w + 1);
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
            let sep_style = Style::default().fg(rgb(theme.border)).bg(rgb(base_bg));

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
pub fn diff_render_hunk_start(lines: &[DiffLine], hunk_start: usize) -> usize {
    let rows = build_split_rows(lines);
    rows.iter()
        .position(|r| r.src_indices.contains(&hunk_start))
        .unwrap_or(0)
}

/// How many content lines a preview view would render (before scrolling).
pub fn preview_line_count(view: PreviewView<'_>) -> usize {
    match view {
        PreviewView::Loading | PreviewView::Idle | PreviewView::Hint { .. } => 1,
        PreviewView::Error { .. } => 2,
        PreviewView::Binary { .. } => 4,
        PreviewView::Directory { .. } => {
            // blank + name + count + blank + Enter hint
            5
        }
        PreviewView::Text {
            lines,
            truncated,
            ..
        } => {
            let base = if lines.is_empty() { 1 } else { lines.len() };
            if truncated {
                base + 2
            } else {
                base
            }
        }
        PreviewView::Diff { lines, .. } => {
            if lines.is_empty() {
                1
            } else {
                build_split_rows(lines).len().max(1)
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
        assert!(styles.iter().any(|c| *c == Some(magenta)), "expected magenta keyword span, got {:?}", styles);
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
        assert_eq!(diff_render_hunk_start(&lines, 5), 1);
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
        let rendered = build_lines(&theme, view, 40);
        let joined: String = rendered
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!joined.contains("> +"), "no unified > marker: {joined}");
        assert!(joined.contains('│'), "split separator present");
        assert_eq!(preview_line_count(view), rendered.len());
    }
}

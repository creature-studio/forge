//! Right-hand preview pane (file text or side-by-side diff) — Modern Minimal.

use forge_model::{DiffLine, DiffLineKind};
use forge_theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::markdown::wrap_line;
use crate::style::{self, fg_bg, rgb};
use crate::MarkdownPreview;

/// Lightweight language hint for plain-text preview coloring.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TextLang {
    #[default]
    Plain,
    Rust,
    Markdown,
    /// A minimal Syntect grammar (built-in or user-installed plugin).
    Syntax(String),
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
            // Dotless special filenames are not file extensions.
            let basename = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
            let extension = if basename == "dockerfile" || basename.starts_with("dockerfile.") {
                "dockerfile"
            } else {
                lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
            };
            if !extension.is_empty() && crate::syntax::supports_extension(extension) {
                Self::Syntax(extension.to_string())
            } else {
                Self::Plain
            }
        }
    }
}

/// Borrowed view for the preview pane (keeps forge-ui free of forge-fs).
#[derive(Debug, Clone)]
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
    Markdown {
        preview: &'a MarkdownPreview,
    },
    /// Pre-wrapped, source-numbered code rows. Every entry occupies one
    /// terminal row, so rendering must never invoke ratatui's implicit Wrap.
    TextRows {
        rows: &'a [Line<'static>],
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

/// Width-safe, source-aware layout for the Code and plain-text preview.
///
/// A source line may occupy several terminal rows. Counting rendered rows here
/// avoids the old bug where a long line swallowed following lines but the
/// scrollbar and preview selection still counted one source line per row.
/// The cache is invalidated when content changes or the pane width/theme does.
#[derive(Debug, Default)]
pub struct TextRowsCache {
    rows: Vec<Line<'static>>,
    source_starts: Vec<usize>,
    width: u16,
    theme: Option<Theme>,
    lang: TextLang,
    truncated: bool,
    bytes_read: usize,
    source_count: usize,
    source_rows: usize,
}

impl TextRowsCache {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn ensure(
        &mut self,
        theme: &Theme,
        lines: &[String],
        truncated: bool,
        bytes_read: usize,
        language: TextLang,
        width: u16,
    ) {
        if self.width == width
            && self.theme.as_ref() == Some(theme)
            && self.lang == language
            && self.truncated == truncated
            && self.bytes_read == bytes_read
            && self.source_count == lines.len()
        {
            return;
        }
        self.rows.clear();
        self.source_starts.clear();
        let width = width.max(1);
        self.rows.reserve(lines.len());
        self.source_starts.reserve(lines.len());
        match &language {
            TextLang::Syntax(extension) => {
                crate::syntax::highlight_lines(theme, extension, lines, |index, body| {
                    self.source_starts.push(self.rows.len());
                    let source = numbered_line_with_body(theme, index + 1, body);
                    self.rows.extend(wrap_line(&source, width));
                });
            }
            _ => {
                for (index, line) in lines.iter().enumerate() {
                    self.source_starts.push(self.rows.len());
                    let source = text_line_numbered(theme, index + 1, line, &language);
                    self.rows.extend(wrap_line(&source, width));
                }
            }
        }
        self.source_rows = self.rows.len();
        if lines.is_empty() {
            self.rows.push(Line::from(Span::styled(
                "  (empty file)",
                style::muted(theme),
            )));
        }
        if truncated {
            self.rows.push(Line::from(""));
            let footer = Line::from(Span::styled(
                format!("  ↓ Scroll for more · {bytes_read} B"),
                style::muted(theme),
            ));
            self.rows.extend(wrap_line(&footer, width));
        }
        self.width = width;
        self.theme = Some(theme.clone());
        self.lang = language;
        self.truncated = truncated;
        self.bytes_read = bytes_read;
        self.source_count = lines.len();
    }

    pub fn rows(&self) -> &[Line<'static>] {
        &self.rows
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn line_count(&self) -> usize {
        self.rows.len()
    }

    /// 1-based source line -> first rendered row (0-based).
    pub fn source_to_row(&self, line: usize) -> usize {
        self.source_starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    /// 0-based rendered row -> 1-based source line for editor navigation.
    pub fn row_to_source(&self, row: usize) -> usize {
        self.source_starts
            .partition_point(|start| *start <= row)
            .max(1)
    }

    /// Attribution must ignore empty-file and truncation notice rows.
    pub fn source_line_at(&self, row: usize) -> Option<usize> {
        (row < self.source_rows).then(|| self.row_to_source(row))
    }
}

/// Cached side-by-side rows for the current unified diff.
///
/// Rebuilt only when the underlying [`DiffLine`] buffer identity changes, so
/// pure scroll / hunk highlight updates reuse the structural split.
#[derive(Debug, Default, Clone)]
pub struct SplitRowsCache {
    lines_ptr: usize,
    lines_len: usize,
    full_new_lines: Option<Vec<String>>,
    rows: Vec<SplitRow>,
}

impl SplitRowsCache {
    pub fn clear(&mut self) {
        self.lines_ptr = 0;
        self.lines_len = 0;
        self.full_new_lines = None;
        self.rows.clear();
    }

    /// Complete file data remains separate from the actionable patch.
    pub fn set_full_new_lines(&mut self, lines: Option<&[String]>) {
        self.clear();
        self.full_new_lines = lines.map(|s| s.to_vec());
    }

    fn rows_for(&mut self, lines: &[DiffLine]) -> &[SplitRow] {
        let ptr = lines.as_ptr() as usize;
        let len = lines.len();
        if self.lines_ptr != ptr || self.lines_len != len {
            self.lines_ptr = ptr;
            self.lines_len = len;
            let partial = build_split_rows(lines);
            self.rows = self.full_new_lines.as_deref()
                .and_then(|full| build_complete_split_rows(partial.clone(), full))
                .unwrap_or(partial);
            annotate_split_line_numbers(&mut self.rows);
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

    // Callers without a persistent cache must still get the exact same safe
    // layout as the App's cached path. Never let raw Code/Text through the
    // old Paragraph::wrap route: it may spill terminal-wide CJK glyphs.
    if let PreviewView::Text {
        lines,
        truncated,
        bytes_read,
        language,
    } = view
    {
        let mut safe = TextRowsCache::default();
        safe.ensure(theme, lines, truncated, bytes_read, language, area.width);
        return draw_preview(
            frame,
            area,
            theme,
            PreviewView::TextRows { rows: safe.rows() },
            scroll,
            split_cache,
        );
    }

    let is_diff = matches!(view, PreviewView::Diff { .. });
    let inner = area;
    style::fill_panel(frame, inner, theme);

    let viewport = inner.height as usize;
    let total = preview_line_count(view.clone(), split_cache);
    let max_scroll = total.saturating_sub(viewport);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    let start = *scroll;
    let end = (start + viewport).min(total);
    if let PreviewView::Markdown { preview } = view {
        preview.draw(frame, inner, start);
        return inner;
    }
    // Virtualize: only allocate Line/Span for the visible window.
    let visible = build_visible_lines(theme, view.clone(), inner.width, start, end, split_cache);

    let style = Style::default().bg(rgb(theme.panel_bg)).fg(rgb(theme.fg));
    // Diff rows are pre-fitted to width; wrapping would break the split columns.
    let para = if is_diff || matches!(view, PreviewView::TextRows { .. }) {
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
        PreviewView::Markdown { .. } => Vec::new(),
        PreviewView::TextRows { rows } => rows[start.min(rows.len())..end.min(rows.len())].to_vec(),
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
                            format!("  ↓ Scroll for more · {bytes_read} B"),
                            style::muted(theme),
                        )));
                    } else if truncated && idx == total.saturating_sub(2) {
                        out.push(Line::from(""));
                    }
                    continue;
                }
                let body_len = lines.len();
                if idx < body_len {
                    out.push(text_line_numbered(theme, idx + 1, &lines[idx], &language));
                } else if truncated {
                    let footer_i = idx - body_len;
                    if footer_i == 0 {
                        out.push(Line::from(""));
                    } else {
                        out.push(Line::from(Span::styled(
                            format!("  ↓ Scroll for more · {bytes_read} B"),
                            style::muted(theme),
                        )));
                    }
                }
            }
            out
        }
        PreviewView::Diff {
            path,
            lines,
            hunk_start,
            hunk_end,
            ..
        } => {
            let rows = split_cache.rows_for(lines);
            if rows.is_empty() {
                return slice_static(
                    start,
                    end,
                    &[Line::from(Span::styled("  (no diff)", style::muted(theme)))],
                );
            }
            let end = end.min(rows.len());
            let start = start.min(end);
            let (left_w, right_w) = split_half_widths(width as usize).unwrap_or((0, 0));
            let visible = &rows[start..end];
            // Parse each side independently, once per visible window. Source and
            // viewport remain bounded; never tokenize the entire Git history.
            let left_text: Vec<String> = visible.iter().map(|row| match &row.kind {
                SplitRowKind::Pair { old, .. } => fit_cell(old.as_deref().unwrap_or(""), left_w.saturating_sub(6)),
                _ => String::new(),
            }).collect();
            let right_text: Vec<String> = visible.iter().map(|row| match &row.kind {
                SplitRowKind::Pair { new, .. } => fit_cell(new.as_deref().unwrap_or(""), right_w.saturating_sub(6)),
                _ => String::new(),
            }).collect();
            let left_syntax = highlight_code_lines(theme, path, &left_text);
            let right_syntax = highlight_code_lines(theme, path, &right_text);
            visible.iter().enumerate().map(|(index, row)| {
                let in_hunk = match (hunk_start, hunk_end) {
                    (Some(s), Some(e)) => row.src_indices.iter().any(|&i| i >= s && i < e),
                    _ => false,
                };
                render_split_row(theme, row, width, in_hunk,
                    Some((&left_syntax[index], &right_syntax[index])))
            }).collect()
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
    let total = preview_line_count(view.clone(), split_cache);
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

fn numbered_line_with_body(theme: &Theme, n: usize, body: Line<'static>) -> Line<'static> {
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
    let mut spans = vec![num, sep];
    spans.extend(body.spans);
    Line::from(spans)
}

/// Shared syntax palette for preview, editor, and Diff. No source mutation.
/// Parsing is bounded by the caller; keep one parser state per supplied batch.
pub fn highlight_code_lines(theme: &Theme, path: &str, lines: &[String]) -> Vec<Line<'static>> {
    let language = TextLang::from_name(path);
    match language {
        TextLang::Syntax(extension) => {
            let mut out = Vec::with_capacity(lines.len());
            crate::syntax::highlight_lines(theme, &extension, lines, |_, line| out.push(line));
            out
        }
        TextLang::Rust | TextLang::Markdown => lines.iter().map(|s| match language {
            TextLang::Rust => rust_line(theme, s),
            TextLang::Markdown => markdown_line(theme, s),
            _ => unreachable!(),
        }).collect(),
        TextLang::Plain => lines.iter().map(|line| Line::from(Span::styled(
            line.clone(), fg_bg(theme.fg, theme.panel_bg),
        ))).collect(),
    }
}

fn text_line_numbered(theme: &Theme, n: usize, raw: &str, language: &TextLang) -> Line<'static> {
    let body = match language {
        TextLang::Rust => rust_line(theme, raw),
        TextLang::Markdown => markdown_line(theme, raw),
        TextLang::Syntax(_) | TextLang::Plain => Line::from(Span::styled(
            raw.to_string(),
            fg_bg(theme.fg, theme.panel_bg),
        )),
    };
    numbered_line_with_body(theme, n, body)
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
                    // A trailing escape must not advance beyond the source slice.
                    i = (i + 2).min(chars.len());
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
                i = (i + 2).min(chars.len()); // escape + char, possibly truncated
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
    /// Actual source line numbers on each side, if this row contains code.
    old_num: Option<usize>,
    new_num: Option<usize>,
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

/// Infer both gutter line numbers from the original patch headers, while
/// retaining native, independent numbering for inserted/deleted lines.
fn annotate_split_line_numbers(rows: &mut [SplitRow]) {
    fn start(token: &str) -> Option<usize> {
        let range = token.get(1..)?;
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        let number = start.parse::<usize>().ok()?;
        Some(if count == "0" { number.saturating_add(1) } else { number })
    }
    let (mut old, mut new) = (1usize, 1usize);
    for row in rows {
        match &row.kind {
            SplitRowKind::Header(header) => {
                let mut old_token = None;
                let mut new_token = None;
                for token in header.split_whitespace() {
                    if token.starts_with('-') { old_token = Some(token); }
                    if token.starts_with('+') { new_token = Some(token); }
                }
                if let Some(n) = old_token.and_then(start) { old = n; }
                if let Some(n) = new_token.and_then(start) { new = n; }
            }
            SplitRowKind::Pair { old: left, new: right, .. } => {
                if left.is_some() { row.old_num = Some(old); old = old.saturating_add(1); }
                if right.is_some() { row.new_num = Some(new); new = new.saturating_add(1); }
            }
            SplitRowKind::Note(_) => {}
        }
    }
}

/// Reconstruct unmodified code omitted by `git diff --unified=3`.
/// Hunk source indices remain bound to the original patch, so stage/unstage
/// still operates on the exact, separate original hunks.
fn build_complete_split_rows(rows: Vec<SplitRow>, full: &[String]) -> Option<Vec<SplitRow>> {
    let mut result = Vec::with_capacity(full.len().saturating_add(rows.len()));
    let mut next_line = 0usize;
    for row in rows {
        match &row.kind {
            SplitRowKind::Header(text) => {
                let plus = text.split_whitespace().find(|word| word.starts_with('+'))?;
                let range = plus.trim_start_matches('+');
                let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                let start = start.parse::<usize>().ok()?;
                let count = count.parse::<usize>().ok()?;
                // For deletion-only hunks the zero-length new range points
                // after the preceding new-side line.
                let target = if count == 0 { start } else { start.saturating_sub(1) };
                if target < next_line || target > full.len() { return None; }
                for source in &full[next_line..target] {
                    result.push(SplitRow {
                        kind: SplitRowKind::Pair {
                            old: Some(source.clone()), new: Some(source.clone()),
                            old_del: false, new_add: false,
                        },
                        old_num: None, new_num: None, src_indices: Vec::new(),
                    });
                }
                next_line = target;
                result.push(row);
            }
            SplitRowKind::Pair { new, .. } => {
                if let Some(new) = new {
                    // Never silently display a misaligned file: patch and blob
                    // might have changed between the two independent reads.
                    if full.get(next_line)? != new { return None; }
                    next_line += 1;
                }
                result.push(row);
            }
            SplitRowKind::Note(_) => result.push(row),
        }
    }
    for source in &full[next_line..] {
        result.push(SplitRow {
            kind: SplitRowKind::Pair {
                old: Some(source.clone()), new: Some(source.clone()),
                old_del: false, new_add: false,
            },
            old_num: None, new_num: None, src_indices: Vec::new(),
        });
    }
    Some(result)
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
            old_num: None, new_num: None, src_indices: vec![idx],
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
                old_num: None, new_num: None, src_indices: vec![i],
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
                            old_num: None, new_num: None, src_indices: vec![i],
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
                                old_num: None, new_num: None, src_indices: src,
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

/// Preserve syntax colors while hard-clipping to the assigned terminal cells.
/// Adding line numbers must never cause code to spill into the other pane.
fn fit_styled_cell(line: &Line<'_>, width: usize, bg: ratatui::style::Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    let mut clipped = false;
    for span in &line.spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + cw > width { clipped = true; break; }
            text.push(ch);
            used += cw;
        }
        if !text.is_empty() { out.push(Span::styled(text, span.style.bg(bg))); }
        if clipped { break; }
    }
    if used < width {
        out.push(Span::styled(" ".repeat(width - used), Style::default().bg(bg)));
    }
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
    syntax: Option<(&Line<'_>, &Line<'_>)>,
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
            let old_gutter = left_w.min(6);
            let new_gutter = right_w.min(6);
            let left_txt = fit_cell(old.as_deref().unwrap_or(""), left_w - old_gutter);
            let right_txt = fit_cell(new.as_deref().unwrap_or(""), right_w - new_gutter);
            let number_text = |number: Option<usize>, width: usize| -> String {
                if let Some(number) = number {
                    fit_cell(&format!("{:>5} ", number), width)
                } else {
                    " ".repeat(width)
                }
            };
            let numbers_style = Style::default().fg(rgb(theme.muted)).bg(rgb(base_bg));

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

            let mut spans = vec![Span::styled(accent_ch.to_string(), accent_style)];
            spans.push(Span::styled(number_text(row.old_num, old_gutter), numbers_style));
            if let Some((left, right)) = syntax {
                let left_bg = if *old_del { theme.diff_del_bg } else { base_bg };
                let right_bg = if *new_add { theme.diff_add_bg } else { base_bg };
                spans.extend(fit_styled_cell(left, left_w - old_gutter, rgb(left_bg)));
                spans.push(Span::styled("│".to_string(), sep_style));
                spans.push(Span::styled(number_text(row.new_num, new_gutter), numbers_style));
                spans.extend(fit_styled_cell(right, right_w - new_gutter, rgb(right_bg)));
            } else {
                spans.push(Span::styled(left_txt, left_style));
                spans.push(Span::styled("│".to_string(), sep_style));
                spans.push(Span::styled(number_text(row.new_num, new_gutter), numbers_style));
                spans.push(Span::styled(right_txt, right_style));
            }
            Line::from(spans)
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
        PreviewView::Markdown { preview } => preview.line_count(),
        PreviewView::TextRows { rows } => rows.len(),
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
        PreviewView::Diff { lines, .. } => split_cache.ensure(lines).max(1),
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
    fn code_preview_pre_wraps_long_cjk_rows_and_scrolls_by_visual_row() {
        use ratatui::{backend::TestBackend, buffer::CellWidth, Terminal};

        let theme = Theme::modern_ide();
        let lines = vec![
            "说明 ".to_string() + &("文档 README Code 模式包含中文与very_long_token".repeat(18)),
            "const flag = true; // 中文".into(),
            "这是一个很长的中文字符串".repeat(20),
        ];
        for width in [96, 40, 17, 12] {
            let mut cache = TextRowsCache::default();
            cache.ensure(&theme, &lines, false, 500, TextLang::Markdown, width);
            assert!(
                cache.line_count() > lines.len(),
                "code wrapping missing at width {width}"
            );
            let starts = (1..=lines.len())
                .map(|i| cache.source_to_row(i))
                .collect::<Vec<_>>();
            assert_eq!(starts[0], 0);
            assert!(starts[1] > starts[0] && starts[2] > starts[1]);
            assert_eq!(cache.row_to_source(starts[1]), 2);
            assert_eq!(cache.row_to_source(starts[1] - 1), 1);

            for row in cache.rows() {
                assert!(
                    row.width() <= width as usize,
                    "text row exceeds preview width {width}: {row:?}"
                );
            }

            let total_width = width + 33;
            let mut terminal = Terminal::new(TestBackend::new(total_width, 12)).unwrap();
            let area = Rect::new(33, 0, width, 12);
            let mut diff_cache = SplitRowsCache::default();
            for &scroll_at in &[0, starts[1], cache.line_count().saturating_sub(12)] {
                terminal
                    .draw(|frame| {
                        let mut scroll = scroll_at;
                        let rendered = draw_preview(
                            frame,
                            area,
                            &theme,
                            PreviewView::TextRows { rows: cache.rows() },
                            &mut scroll,
                            &mut diff_cache,
                        );
                        assert_eq!(rendered, area);
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                for y in 0..12u16 {
                    // The entire Files area must remain untouched, even when
                    // the last preview cell would be a two-column CJK glyph.
                    for x in 0..33u16 {
                        assert_eq!(
                            buffer[(x, y)].symbol(),
                            " ",
                            "Code text leaked into Files at ({x},{y}) / width {width}"
                        );
                    }
                    let last = buffer[(total_width - 1, y)].symbol();
                    assert!(last.cell_width() < 2,
                        "Code emitted a wide glyph in terminal's last column at width {width}: {last:?}");
                }
            }
        }
    }

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
        let line = text_line_numbered(&theme, 7, "fn main() {}", &TextLang::Plain);
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
        let rendered = build_lines(&theme, view.clone(), 40, &mut cache);
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
            old_num: None, new_num: None, src_indices: vec![0, 1],
        };
        let line = render_split_row(&theme, &row, 40, false, None);
        let spans: Vec<_> = line.spans.iter().collect();
        assert!(spans.len() >= 6);
        assert_eq!(spans[3].content.as_ref(), "│");
        assert_eq!(
            spans[2].style.bg,
            Some(crate::style::rgb(theme.diff_del_bg))
        );
        assert_eq!(
            spans[5].style.bg,
            Some(crate::style::rgb(theme.diff_add_bg))
        );
        assert_eq!(spans[3].style.fg, Some(crate::style::rgb(theme.muted)));
        assert_ne!(
            spans[3].style.fg,
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
                old_num: None, new_num: None, src_indices: vec![0],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("- 近黑背景，冷白/青 accent".into()),
                    new: Some("- 近黑背景，冷白/青 accent".into()),
                    old_del: false,
                    new_add: false,
                },
                old_num: None, new_num: None, src_indices: vec![1],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("冷白/青 accent: 选中为浅抬升而非刺眼反色".into()),
                    new: Some("冷白/青 accent: 选中为浅抬升，左右对比 diff 更直观".into()),
                    old_del: true,
                    new_add: true,
                },
                old_num: None, new_num: None, src_indices: vec![2, 3],
            },
            SplitRow {
                kind: SplitRowKind::Pair {
                    old: Some("ascii context line".into()),
                    new: Some("ascii context line".into()),
                    old_del: false,
                    new_add: false,
                },
                old_num: None, new_num: None, src_indices: vec![4],
            },
        ];
        let rendered: Vec<_> = rows
            .iter()
            .map(|r| render_split_row(&theme, r, width, true, None))
            .collect();
        let left_widths: Vec<_> = rendered
            .iter()
            .map(|l| {
                let sep = l.spans.iter().position(|s| s.content.as_ref() == "│").unwrap();
                l.spans[1..sep].iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum::<usize>()
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
    fn additional_languages_and_special_filenames_are_detected() {
        for name in [
            "main.go",
            "main.c",
            "include.hpp",
            "worker.cpp",
            "Demo.java",
            "view.cs",
            "page.html",
            "component.vue",
            "component.svelte",
            "style.css",
            "Cargo.toml",
            "Dockerfile",
            "Dockerfile.dev",
            "module.lua",
            "index.php",
            "Gemfile.rb",
            "App.kt",
            "Main.swift",
            "install.ps1",
            "layout.xml",
        ] {
            assert!(
                matches!(TextLang::from_name(name), TextLang::Syntax(_)),
                "missing syntax for {name}"
            );
        }
        assert!(matches!(TextLang::from_name("source.rs"), TextLang::Rust));
        assert!(matches!(
            TextLang::from_name("readme.md"),
            TextLang::Markdown
        ));
        assert!(matches!(
            TextLang::from_name("misc.unknown"),
            TextLang::Plain
        ));
    }

    #[test]
    fn rust_highlighter_never_overruns_trailing_backslash() {
        let theme = Theme::modern_ide();
        for source in ["\"abc\\", "'\\", "\"", "'", "\"escaped \\\" then end\\", "\"Unicode中文\\"] {
            let rendered = rust_line(&theme, source);
            let joined: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
            assert_eq!(joined, source, "failed source: {source:?}");
        }
    }

    #[test]
    fn full_diff_preserves_all_unmodified_lines_between_distant_hunks() {
        let diff = vec![
            DiffLine { kind: DiffLineKind::Header, text: "@@ -2 +2 @@".into() },
            DiffLine { kind: DiffLineKind::Delete, text: "-two".into() },
            DiffLine { kind: DiffLineKind::Add, text: "+TWO".into() },
            DiffLine { kind: DiffLineKind::Header, text: "@@ -5 +5 @@".into() },
            DiffLine { kind: DiffLineKind::Delete, text: "-five".into() },
            DiffLine { kind: DiffLineKind::Add, text: "+FIVE".into() },
        ];
        let full = ["one", "TWO", "three", "four", "FIVE", "six"]
            .into_iter().map(str::to_owned).collect::<Vec<_>>();
        let mut cache = SplitRowsCache::default();
        cache.set_full_new_lines(Some(&full));
        let count = cache.ensure(&diff);
        let new_code = cache.rows_for(&diff).iter().filter_map(|row| match &row.kind {
            SplitRowKind::Pair { new, .. } => new.clone(),
            _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(new_code, ["one", "TWO", "three", "four", "FIVE", "six"]);
        assert_eq!(count, 8); // Six source rows plus two hunk headings.
        assert_eq!(diff_render_hunk_start(&diff, 0, &mut cache), 1);
        assert_eq!(diff_render_hunk_start(&diff, 3, &mut cache), 5);
        let partial = build_split_rows(&diff);
        assert!(partial.len() < count);
    }

    #[test]
    fn full_diff_can_scroll_to_source_line_1062_outside_patch_context() {
        let theme = Theme::modern_ide();
        let mut full = (1..=1400).map(|i| format!("source line {i}")).collect::<Vec<_>>();
        full[1099] = "updated source line 1100".into();
        let diff = vec![
            DiffLine { kind: DiffLineKind::Header, text: "@@ -1100 +1100 @@".into() },
            DiffLine { kind: DiffLineKind::Delete, text: "-source line 1100".into() },
            DiffLine { kind: DiffLineKind::Add, text: "+updated source line 1100".into() },
        ];
        let mut cache = SplitRowsCache::default();
        cache.set_full_new_lines(Some(&full));
        assert_eq!(cache.ensure(&diff), 1401); // one hunk heading + all source lines
        // The original unified patch has only three lines, but the full
        // display remains navigable at line 1062 without loading new hunks.
        let view = PreviewView::Diff {
            path: "example.rs", side: "unstaged", lines: &diff,
            hunk_start: None, hunk_end: None, hunk_idx: None,
        };
        let rendered = build_visible_lines(&theme, view, 116, 1061, 1062, &mut cache);
        let text = rendered[0].spans.iter().map(|span| span.content.as_ref()).collect::<String>();
        assert!(text.contains("source line 1062"), "{text}");
        assert_eq!(cache.rows_for(&diff)[1061].new_num, Some(1062));
        assert_eq!(diff_render_hunk_start(&diff, 0, &mut cache), 1099);
    }

    #[test]
    fn full_diff_mismatch_reverts_to_original_patch_rows() {
        let diff = vec![
            DiffLine { kind: DiffLineKind::Header, text: "@@ -2 +2 @@".into() },
            DiffLine { kind: DiffLineKind::Delete, text: "-two".into() },
            DiffLine { kind: DiffLineKind::Add, text: "+TWO".into() },
        ];
        let full = vec!["one".into(), "changed again".into(), "three".into()];
        let mut cache = SplitRowsCache::default();
        cache.set_full_new_lines(Some(&full));
        assert_eq!(cache.ensure(&diff), build_split_rows(&diff).len());
    }

    #[test]
    fn shared_highlight_is_reusable_across_code_views() {
        let theme = Theme::modern_ide();
        let code = ["const answer = 42; // shared".to_owned()];
        let colored = highlight_code_lines(&theme, "example.ts", &code);
        assert!(colored[0].spans.len() > 1);
        assert_eq!(colored[0].spans.iter().map(|span| span.content.as_ref()).collect::<String>(), code[0]);
    }

    #[test]
    fn diff_syntax_preserves_background_and_column_width() {
        let theme = Theme::modern_ide();
        let width = 80;
        let (left_w, right_w) = split_half_widths(width).unwrap();
        let old = fit_cell("const old = 1", left_w);
        let new = fit_cell("const next = 2", right_w);
        let old_lines = highlight_code_lines(&theme, "x.ts", &[old.clone()]);
        let new_lines = highlight_code_lines(&theme, "x.ts", &[new.clone()]);
        let row = SplitRow { kind: SplitRowKind::Pair {
            old: Some("const old = 1".into()), new: Some("const next = 2".into()),
            old_del: true, new_add: true,
        }, old_num: None, new_num: None, src_indices: vec![0] };
        let rendered = render_split_row(&theme, &row, width as u16, false,
            Some((&old_lines[0], &new_lines[0])));
        assert_eq!(rendered.width(), width);
        assert!(rendered.spans.iter().any(|span| span.style.bg == Some(rgb(theme.diff_del_bg))));
        assert!(rendered.spans.iter().any(|span| span.style.bg == Some(rgb(theme.diff_add_bg))));
        assert!(rendered.spans.len() > 4);
    }

    #[test]
    fn script_extensions_are_detected_without_changing_rust_markdown() {
        for name in [
            "demo.py",
            "run.sh",
            "app.js",
            "widget.jsx",
            "main.ts",
            "view.tsx",
            "settings.json",
            "config.yaml",
            "build.yml",
            "query.sql",
        ] {
            assert!(
                matches!(TextLang::from_name(name), TextLang::Syntax(_)),
                "{name}"
            );
        }
        assert_eq!(TextLang::from_name("lib.rs"), TextLang::Rust);
        assert_eq!(TextLang::from_name("README.md"), TextLang::Markdown);
        assert_eq!(TextLang::from_name("notes.txt"), TextLang::Plain);
    }

    #[test]
    fn script_preview_highlights_keywords_without_changing_source_text() {
        let theme = Theme::modern_ide();
        let samples = [
            ("py", "def greet(): print(\"hello\")"),
            ("sh", "export VAR=\"hello\" # note"),
            ("js", "const a = \"hello\"; // note"),
            ("ts", "interface User { name: string }"),
            ("json", "{\"active\": true, \"count\": 2}"),
            ("yaml", "enabled: true # note"),
            ("sql", "SELECT name FROM users WHERE id = 12"),
        ];
        for (extension, code) in samples {
            let mut cache = TextRowsCache::default();
            let input = vec![code.to_owned()];
            cache.ensure(
                &theme,
                &input,
                false,
                code.len(),
                TextLang::Syntax(extension.to_owned()),
                180,
            );
            let displayed = &cache.rows()[0].spans[2..];
            assert_eq!(
                displayed
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
                code,
                "lost source bytes for {extension}"
            );
            let distinct_colors: std::collections::HashSet<_> =
                displayed.iter().map(|span| span.style.fg).collect();
            assert!(
                distinct_colors.len() > 1,
                "{extension} not highlighted: {:?}",
                displayed
            );
        }
    }

    #[test]
    fn script_preview_cache_preserves_source_line_mapping() {
        let theme = Theme::modern_ide();
        let lines: Vec<_> = (0..400)
            .map(|index| format!("const value_{index} = \"test\";"))
            .collect();
        let mut cache = TextRowsCache::default();
        cache.ensure(
            &theme,
            &lines,
            true,
            65_536,
            TextLang::Syntax("js".into()),
            36,
        );
        let first_count = cache.line_count();
        assert!(first_count >= 400);
        assert_eq!(cache.row_to_source(cache.source_to_row(400)), 400);
        cache.ensure(
            &theme,
            &lines,
            true,
            65_536,
            TextLang::Syntax("js".into()),
            36,
        );
        assert_eq!(cache.line_count(), first_count);
        assert_eq!(cache.source_to_row(1), 0);
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
        let full = build_lines(&theme, view.clone(), 40, &mut cache);
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

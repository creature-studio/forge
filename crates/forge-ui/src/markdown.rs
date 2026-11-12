//! Cached Markdown layout in terminal rows, including wrapped lines.

use forge_theme::Theme;
use ratatui::{
    buffer::CellWidth,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, StyledGrapheme},
    widgets::Paragraph,
    Frame,
};
use tui_markdown::{Options, StyleSheet};

use crate::style::rgb;

#[derive(Debug, Default)]
pub struct MarkdownPreview {
    lines: Vec<Line<'static>>,
    row_starts: Vec<usize>,
    width: u16,
    theme: Option<Theme>,
}

impl MarkdownPreview {
    /// The caller clears this cache whenever the loaded file content changes.
    pub fn ensure(
        &mut self,
        lines: &[String],
        truncated: bool,
        bytes_read: usize,
        width: u16,
        theme: &Theme,
    ) {
        let width = width.max(1);
        if self.width == width && self.theme.as_ref() == Some(theme) {
            return;
        }
        let source = lines.join("\n");
        let options = Options::new(MarkdownStyles(theme.clone())).table_width(width);
        let mut lines = tui_markdown::from_str_with_options(&source, &options)
            .lines
            .into_iter()
            .map(|line| Line {
                spans: line
                    .spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect(),
                style: line.style,
                alignment: line.alignment,
            })
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(Line::from("  (empty file)"));
        }
        if truncated {
            lines.push(Line::default());
            lines.push(Line::styled(
                format!("  ↓ Scroll for more · {bytes_read} B"),
                Style::default().fg(rgb(theme.muted)),
            ));
        }
        // Pre-wrap here: a wrapped row occupies exactly one terminal row, so the
        // scroll math stays in rows and `draw` never asks Paragraph to wrap.
        self.lines = lines
            .iter()
            .flat_map(|line| wrap_line(line, width))
            .collect();
        self.row_starts = (0..=self.lines.len()).collect();
        self.width = width;
        self.theme = Some(theme.clone());
    }

    pub fn line_count(&self) -> usize {
        self.row_starts.last().copied().unwrap_or(0)
    }

    pub(crate) fn draw(&self, frame: &mut Frame<'_>, area: Rect, scroll: usize) {
        if self.lines.is_empty() {
            return;
        }
        let first = self.row_starts.partition_point(|&row| row <= scroll) - 1;
        let end_row = scroll.saturating_add(area.height as usize);
        let end = self.row_starts.partition_point(|&row| row < end_row);
        let visible = self.lines[first..end.min(self.lines.len())].to_vec();
        let offset = (scroll - self.row_starts[first]).min(u16::MAX as usize) as u16;
        // Every line is already pre-wrapped to the area width. Rendering without
        // `Wrap` means an over-wide row is truncated instead of spilling a
        // double-width glyph into the terminal's next screen row.
        frame.render_widget(Paragraph::new(visible).scroll((offset, 0)), area);
    }
}

/// Reflow one styled line into rows no wider than `width`.
///
/// This mirrors `Paragraph` + `Wrap { trim: false }`, with one correction in the
/// overflow check: the width of the *incoming* grapheme must be counted. Ratatui
/// 0.30.1 checks `line + word >= width` before appending the grapheme, so a
/// two-column CJK glyph whose prefix ends at `width - 1` is emitted in the last
/// column of a `width + 1` row. Terminals then wrap that glyph to column 0 of the
/// next screen row, outside the preview area (e.g. over the file tree).
pub fn wrap_line(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let mut rows: Vec<Vec<StyledGrapheme<'_>>> = Vec::new();
    let mut pending_line: Vec<StyledGrapheme<'_>> = Vec::new();
    let mut pending_word: Vec<StyledGrapheme<'_>> = Vec::new();
    let mut pending_whitespace = std::collections::VecDeque::new();
    let mut line_width = 0u16;
    let mut word_width = 0u16;
    let mut whitespace_width = 0u16;
    let mut non_whitespace_previous = false;

    for grapheme in line.styled_graphemes(Style::default()) {
        let is_whitespace = grapheme.is_whitespace();
        let symbol_width = grapheme.symbol.cell_width();
        if symbol_width > width {
            continue;
        }

        let word_found = non_whitespace_previous && is_whitespace;
        // `trim: false`: when only pending text is present, flush it before a
        // symbol that would not fit (long words and indentation break here).
        let untrimmed_overflow =
            pending_line.is_empty() && word_width + whitespace_width + symbol_width > width;
        if word_found || untrimmed_overflow {
            pending_line.extend(pending_whitespace.drain(..));
            line_width += whitespace_width;
            pending_line.append(&mut pending_word);
            line_width += word_width;
            pending_whitespace.clear();
            whitespace_width = 0;
            word_width = 0;
        }

        let line_full = line_width >= width;
        // Count the incoming grapheme, otherwise a wide glyph can overflow by one column.
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width + symbol_width > width;
        if line_full || pending_word_overflow {
            let mut remaining_width = width.saturating_sub(line_width);
            rows.push(std::mem::take(&mut pending_line));
            line_width = 0;
            while let Some(grapheme) = pending_whitespace.front() {
                let grapheme_width = grapheme.symbol.cell_width();
                if grapheme_width > remaining_width {
                    break;
                }
                whitespace_width -= grapheme_width;
                remaining_width -= grapheme_width;
                pending_whitespace.pop_front();
            }
            if is_whitespace && pending_whitespace.is_empty() {
                continue;
            }
        }

        if is_whitespace {
            whitespace_width += symbol_width;
            pending_whitespace.push_back(grapheme);
        } else {
            word_width += symbol_width;
            pending_word.push(grapheme);
        }
        non_whitespace_previous = !is_whitespace;
    }

    pending_line.extend(pending_whitespace.drain(..));
    pending_line.append(&mut pending_word);
    if !pending_line.is_empty() {
        rows.push(pending_line);
    } else if rows.is_empty() {
        rows.push(Vec::new());
    }

    rows.into_iter()
        .map(|graphemes| owned_line(line.alignment, graphemes))
        .collect()
}

fn owned_line(alignment: Option<Alignment>, graphemes: Vec<StyledGrapheme<'_>>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for grapheme in graphemes {
        if let Some(span) = spans.last_mut() {
            if span.style == grapheme.style {
                span.content.to_mut().push_str(grapheme.symbol);
                continue;
            }
        }
        spans.push(Span::styled(grapheme.symbol.to_string(), grapheme.style));
    }
    Line {
        spans,
        style: Style::default(),
        alignment,
    }
}

#[derive(Clone)]
struct MarkdownStyles(Theme);

impl StyleSheet for MarkdownStyles {
    fn heading(&self, _level: u8) -> Style {
        Style::default()
            .fg(rgb(self.0.accent))
            .add_modifier(Modifier::BOLD)
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn code(&self) -> Style {
        Style::default().fg(rgb(self.0.warning))
    }

    fn link(&self) -> Style {
        Style::default()
            .fg(rgb(self.0.accent))
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::default().fg(rgb(self.0.muted))
    }

    fn list_marker(&self) -> Style {
        Style::default().fg(rgb(self.0.accent))
    }

    fn table_border(&self) -> Style {
        Style::default().fg(rgb(self.0.muted))
    }

    fn table_header(&self) -> Style {
        self.heading(1)
    }

    fn table_cell(&self) -> Style {
        Style::default().fg(rgb(self.0.fg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        backend::TestBackend,
        buffer::{Buffer, CellWidth},
        widgets::{Widget, Wrap},
        Terminal,
    };

    fn preview(source: &str, width: u16) -> MarkdownPreview {
        let mut preview = MarkdownPreview::default();
        let lines = source.lines().map(String::from).collect::<Vec<_>>();
        preview.ensure(&lines, false, source.len(), width, &Theme::default());
        preview
    }

    /// A terminal wraps when a double-width glyph is emitted in the last column,
    /// which moves it to column 0 of the next screen row (over the file tree).
    fn assert_no_wide_glyph_in_last_column(
        preview: &MarkdownPreview,
        width: u16,
        height: u16,
        label: &str,
    ) {
        assert!(width > 1);
        for scroll in 0..=preview.line_count() {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| preview.draw(frame, frame.area(), scroll))
                .unwrap();
            let buffer = terminal.backend().buffer();
            for y in 0..height {
                let symbol = buffer[(width - 1, y)].symbol();
                assert!(
                    symbol.cell_width() < 2,
                    "{label}: scroll {scroll} row {y} ends with wide glyph {symbol:?} in the last column"
                );
            }
        }
    }

    #[test]
    fn readme_cjk_paragraph_never_ends_a_row_with_a_wide_glyph() {
        // README.md first paragraph at 129x47: sidebar 32 + divider 1 leaves 96 columns.
        let source =
            "终端 Git + 文件管理工具，使用 ratatui-kit 组件与响应式状态，gix 读取 Git 数据、git CLI 执行写入。";
        let preview = preview(source, 96);
        assert_no_wide_glyph_in_last_column(&preview, 96, 6, "readme paragraph");
    }

    #[test]
    fn styled_cjk_markdown_never_ends_a_row_with_a_wide_glyph() {
        let source = "# 中文标题\n\n第一段文字很长，用于检查中文换行。second paragraph with **bold words** and a long tail.\n\n- 最后的条目\n\n| 名称 | Description |\n| --- | --- |\n| 中文 | A longer table cell that must wrap |\n\n```rust\nfn main() { println!(\"中文\"); }\n```";
        for width in [96, 40, 17, 12] {
            let preview = preview(source, width);
            assert_no_wide_glyph_in_last_column(&preview, width, 8, &format!("width {width}"));
        }
    }

    fn plain(preview: &MarkdownPreview) -> String {
        preview
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn formats_blocks_and_inline_styles_without_source_markers() {
        let preview = preview("# 标题\n\n**bold** and *italic* with `code`\n\n- item\n\n> quote\n\n```rust\nfn main() {}\n```\n\n[link](https://example.test)", 80);
        let plain = plain(&preview);
        assert!(
            plain.contains("标题") && plain.contains("fn main() {}"),
            "{plain}"
        );
        assert!(
            plain.contains("item") && plain.contains("quote") && plain.contains("link"),
            "{plain}"
        );
        assert!(
            !plain.contains("# ") && !plain.contains("**") && !plain.contains("```"),
            "{plain}"
        );
        let spans = preview
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .collect::<Vec<_>>();
        assert!(
            spans
                .iter()
                .any(|span| span.content == "bold"
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(spans
            .iter()
            .any(|span| span.content == "italic"
                && span.style.add_modifier.contains(Modifier::ITALIC)));
    }

    #[test]
    fn scrolling_wrapped_cjk_rows_matches_full_paragraph() {
        let preview = preview("# 中文标题\n\n第一段文字很长，用于检查中文换行。second paragraph with **bold words** and a long tail.\n\n- 最后的条目", 12);
        // Wrapped rows are pre-computed, so every stored line is one screen row.
        assert_eq!(preview.line_count(), preview.lines.len());
        assert!(
            preview.line_count() > 5,
            "the CJK paragraph must wrap at width 12"
        );
        let full = Paragraph::new(preview.lines.clone()).wrap(Wrap { trim: false });
        for scroll in 0..preview.line_count() {
            let mut actual = Terminal::new(TestBackend::new(12, 3)).unwrap();
            actual.draw(|f| preview.draw(f, f.area(), scroll)).unwrap();
            let mut expected = Terminal::new(TestBackend::new(12, 3)).unwrap();
            expected
                .draw(|f| f.render_widget(full.clone().scroll((scroll as u16, 0)), f.area()))
                .unwrap();
            assert_eq!(
                actual.backend().buffer(),
                expected.backend().buffer(),
                "scroll {scroll}"
            );
        }
    }

    #[test]
    fn reflows_tables_and_wrapped_row_count_when_resized() {
        let source =
            "| 名称 | Description |\n| --- | --- |\n| 中文 | A longer table cell that must wrap |";
        let lines = source.lines().map(String::from).collect::<Vec<_>>();
        let mut preview = preview(source, 60);
        let wide = preview.line_count();
        preview.ensure(&lines, false, source.len(), 24, &Theme::default());
        assert!(preview.line_count() > wide);
        assert!(preview.lines.iter().all(|line| line.width() <= 24));
        assert!(!plain(&preview).contains("| --- |"));
    }

    #[test]
    fn wrapped_rows_fit_the_width_and_preserve_non_whitespace_text() {
        let cases = [
            "终端 Git + 文件管理工具，使用 ratatui-kit 组件与响应式状态，gix 读取 Git 数据、git CLI 执行写入。",
            "コンピュ ータ上で文字を扱う場合、 典型的には文 字による 通信を行 う場合にその両端点では、",
            "AAA AAA AAAAA AA AAAAAA",
            "                 must wrap! 中文",
            "e\u{301} combining and emoji 🚀 mixed 中文 text",
            "",
        ];
        let strip_whitespace =
            |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        for text in cases {
            for width in 2..=40 {
                let rows = wrap_line(&Line::from(text), width);
                for row in &rows {
                    assert!(
                        row.width() <= width as usize,
                        "{text:?} at {width}: row {:?} is {} columns",
                        Line::to_string(row),
                        row.width()
                    );
                }
                let joined = rows.iter().map(Line::to_string).collect::<String>();
                assert_eq!(
                    strip_whitespace(&joined),
                    strip_whitespace(text),
                    "{text:?} lost content at width {width}"
                );
            }
        }
    }

    #[test]
    fn wrapping_matches_paragraph_reflow_where_ratatui_is_safe() {
        let cases = [
            "AAA AAA AAAAA AA AAAAAA",
            "abcd efghij klmnopabcd efgh ijklmnopabcdefg hijkl mnopab c d e f g h i j k l m n o",
            "コンピュータ上で文字を扱う場合、典型的には文字による通信を行う場合にその両端点では、",
            "               4 Indent",
            "trailing spaces are kept                    ",
        ];
        for text in cases {
            for width in [10u16, 12, 20, 40] {
                let original = Line::from(text);
                let rows = wrap_line(&original, width);
                let row_count = Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .line_count(width);
                let height = rows.len().max(row_count) as u16;
                let area = Rect::new(0, 0, width, height);

                let mut actual = Buffer::empty(area);
                Paragraph::new(rows).render(area, &mut actual);
                let mut expected = Buffer::empty(area);
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .render(area, &mut expected);
                assert_eq!(
                    actual, expected,
                    "{text:?} at width {width} diverged from Paragraph reflow"
                );
            }
        }
    }

    #[test]
    fn empty_and_truncated_files_keep_notices() {
        let empty = preview("", 40);
        assert!(plain(&empty).contains("(empty file)"));
        let mut truncated = MarkdownPreview::default();
        truncated.ensure(&["# Heading".into()], true, 65536, 40, &Theme::default());
        assert!(plain(&truncated).contains("Scroll for more · 65536 B"));
    }
}

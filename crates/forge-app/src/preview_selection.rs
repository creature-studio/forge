//! Mouse selection for the rendered right-hand preview (including Markdown
//! tables, wrapped CJK, code and diff). Screen-cell coordinates are retained
//! separately from file/source positions: copy means exactly what was shown.
use forge_theme::{Rgb, Theme};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Style},
    Frame,
};
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;

/// Accent tint distinguishes a drag selection from Markdown table shading.
fn highlight_bg(theme: &Theme) -> Color {
    fn blend(base: u8, accent: u8) -> u8 {
        ((base as u16 * 55 + accent as u16 * 45) / 100) as u8
    }
    forge_ui::to_color(Rgb::new(
        blend(theme.selection_bg.r, theme.accent.r),
        blend(theme.selection_bg.g, theme.accent.g),
        blend(theme.selection_bg.b, theme.accent.b),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Point {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
struct CellText {
    text: String,
    continuation: bool,
}

#[derive(Default, Debug)]
pub(super) struct PreviewSelection {
    anchor: Option<Point>,
    head: Option<Point>,
    dragging: bool,
    rows: BTreeMap<usize, Vec<CellText>>,
    width: u16,
    identity: String,
    numbered: bool,
}

/// File/Code previews use a line-number gutter. Ignore its visual cells
/// when copying or highlighting code, including during multi-line selection.
/// Wrapped continuation rows have no gutter and retain their full text.
fn gutter_width(cells: &[CellText], numbered: bool) -> usize {
    if !numbered {
        return 0;
    }
    for (i, cell) in cells.iter().take(12).enumerate() {
        if cell.continuation {
            return 0;
        }
        if cell.text == "│" {
            return if i > 0 { i + 1 } else { 0 };
        }
        if cell.text != " " && !(cell.text.len() == 1 && cell.text.as_bytes()[0].is_ascii_digit()) {
            return 0;
        }
    }
    0
}

impl PreviewSelection {
    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
        self.rows.clear();
    }

    pub fn active(&self) -> bool {
        self.anchor.is_some() && self.head.is_some()
    }

    pub fn begin(&mut self, area: Rect, scroll: usize, x: u16, y: u16) {
        if area.width == 0 || area.height == 0 || !area.contains(Position::new(x, y)) {
            return;
        }
        let point = Point {
            row: scroll + (y - area.y) as usize,
            col: (x - area.x) as usize,
        };
        self.anchor = Some(point);
        self.head = None;
        self.dragging = true;
    }

    pub fn move_to(&mut self, area: Rect, scroll: usize, x: u16, y: u16, release: bool) {
        if !self.dragging {
            return;
        }
        if area.width == 0 || area.height == 0 {
            self.clear();
            return;
        }
        let point = Point {
            row: scroll + (y.clamp(area.y, area.bottom() - 1) - area.y) as usize,
            col: (x.clamp(area.x, area.right() - 1) - area.x) as usize,
        };
        self.head = self.anchor.filter(|anchor| *anchor != point).map(|_| point);
        if release {
            self.dragging = false;
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn observe(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        scroll: usize,
        theme: &Theme,
        identity: &str,
        numbered: bool,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.width != area.width || self.identity != identity || self.numbered != numbered {
            self.clear();
            self.width = area.width;
            self.identity = identity.into();
            self.numbered = numbered;
        }
        // Snapshot before coloring: preserve the actual display glyphs, not
        // Markdown syntax, with wide glyph continuations explicitly marked.
        for dy in 0..area.height {
            let mut cells = Vec::with_capacity(area.width as usize);
            let mut trailing = 0usize;
            for dx in 0..area.width {
                let symbol = frame
                    .buffer_mut()
                    .cell((area.x + dx, area.y + dy))
                    .map_or(" ", |cell| cell.symbol())
                    .to_string();
                let continuation = trailing > 0;
                if continuation {
                    trailing -= 1;
                } else {
                    trailing = symbol.width().saturating_sub(1);
                }
                cells.push(CellText {
                    text: symbol,
                    continuation,
                });
            }
            self.rows.insert(scroll + dy as usize, cells);
        }
        // Bound storage if users browse huge logs while the preview stays open.
        while self.rows.len() > 4096 {
            if let Some(&row) = self.rows.keys().next() {
                self.rows.remove(&row);
            }
        }
        let Some((start, end)) = self.range() else {
            return;
        };
        let fg = forge_ui::to_color(theme.selection_fg);
        let bg = highlight_bg(theme);
        for dy in 0..area.height as usize {
            let abs = scroll + dy;
            if abs < start.row || abs > end.row {
                continue;
            }
            let from = if abs == start.row { start.col } else { 0 };
            let to = if abs == end.row {
                end.col + 1
            } else {
                area.width as usize
            };
            let from = from.min(area.width as usize).max(
                self.rows
                    .get(&abs)
                    .map_or(0, |cells| gutter_width(cells, self.numbered)),
            );
            let to = to.min(area.width as usize);
            if from < to {
                frame.buffer_mut().set_style(
                    Rect::new(
                        area.x + from as u16,
                        area.y + dy as u16,
                        (to - from) as u16,
                        1,
                    ),
                    Style::default().fg(fg).bg(bg),
                );
            }
        }
    }

    fn range(&self) -> Option<(Point, Point)> {
        let (Some(a), Some(b)) = (self.anchor, self.head) else {
            return None;
        };
        Some((a.min(b), a.max(b)))
    }

    /// Join visual rows with LF. Preserve meaningful leading spaces and table
    /// separators; discard padding to the right of selected text. Selecting a
    /// continuation cell of a wide glyph includes that glyph exactly once.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.range()?;
        let mut lines = Vec::with_capacity(end.row.saturating_sub(start.row).min(4096) + 1);
        for row in start.row..=end.row {
            let cells = self.rows.get(&row)?;
            let mut from = if row == start.row { start.col } else { 0 }.min(cells.len());
            let to = (if row == end.row {
                end.col.saturating_add(1)
            } else {
                cells.len()
            })
            .min(cells.len());
            if from > 0 && from < cells.len() && cells[from].continuation {
                from -= 1;
            }
            from = from.max(gutter_width(cells, self.numbered));
            let mut value = String::new();
            for cell in cells.iter().take(to).skip(from) {
                if !cell.continuation {
                    value.push_str(&cell.text);
                }
            }
            lines.push(value.trim_end_matches(' ').to_string());
        }
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_visual_text_handles_wide_cjk_table_and_partial_ranges() {
        let mut selection = PreviewSelection::default();
        selection.rows.insert(
            4,
            vec![
                CellText {
                    text: "中".into(),
                    continuation: false,
                },
                CellText {
                    text: " ".into(),
                    continuation: true,
                },
                CellText {
                    text: "文".into(),
                    continuation: false,
                },
                CellText {
                    text: " ".into(),
                    continuation: true,
                },
                CellText {
                    text: "│".into(),
                    continuation: false,
                },
                CellText {
                    text: " ".into(),
                    continuation: false,
                },
            ],
        );
        selection.rows.insert(
            5,
            vec![
                CellText {
                    text: "A".into(),
                    continuation: false,
                },
                CellText {
                    text: " ".into(),
                    continuation: false,
                },
                CellText {
                    text: "B".into(),
                    continuation: false,
                },
            ],
        );
        selection.anchor = Some(Point { row: 4, col: 1 });
        selection.head = Some(Point { row: 5, col: 2 });
        assert_eq!(selection.selected_text().as_deref(), Some("中文│\nA B"));
    }

    #[test]
    fn rendered_markdown_table_and_chinese_copy_exactly_as_visible() {
        use forge_ui::{MarkdownPreview, PreviewView, SplitRowsCache};
        use ratatui::{backend::TestBackend, Terminal};
        let theme = Theme::modern_ide();
        let mut md = MarkdownPreview::default();
        let content = vec![
            "# 中文说明".to_string(),
            "".into(),
            "| 参数 | 描述 |".into(),
            "| --- | --- |".into(),
            "| userId | 用户 ID |".into(),
        ];
        md.ensure(&content, false, content.join("\n").len(), 36, &theme);
        let mut terminal = Terminal::new(TestBackend::new(36, 12)).unwrap();
        let mut selection = PreviewSelection::default();
        let area = Rect::new(0, 0, 36, 12);
        let mut cache = SplitRowsCache::default();
        terminal
            .draw(|frame| {
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::Markdown { preview: &md },
                    &mut 0,
                    &mut cache,
                );
                selection.observe(frame, area, 0, &theme, "md", false);
            })
            .unwrap();
        let row_text = |y: u16| -> String {
            (0..36)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect()
        };
        let header = (0..12)
            .find(|y| row_text(*y).contains("参 数"))
            .unwrap_or_else(|| {
                panic!(
                    "Markdown rendered rows: {:?}",
                    (0..12).map(row_text).collect::<Vec<_>>()
                )
            });
        let value = (header..12)
            .find(|y| row_text(*y).contains("userId"))
            .unwrap();
        selection.begin(area, 0, 1, header);
        selection.move_to(area, 0, 25, value, true);
        terminal
            .draw(|frame| {
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::Markdown { preview: &md },
                    &mut 0,
                    &mut cache,
                );
                selection.observe(frame, area, 0, &theme, "md", false);
            })
            .unwrap();
        let copied = selection.selected_text().unwrap();
        assert!(copied.contains("参数"), "{copied}");
        assert!(copied.contains("userId"), "{copied}");
        assert!(copied.contains("用户 ID"), "{copied}");
        assert!(
            !copied.contains("| --- |"),
            "copy should be visually rendered Markdown"
        );
        assert_eq!(
            terminal.backend().buffer()[(10, header)].bg,
            highlight_bg(&theme)
        );
        // A multi-row selection spans to the end of the first line and
        // begins at column 0 on the last line. Only these two edge cells
        // are outside the user's selection.
        assert_ne!(
            terminal.backend().buffer()[(0, header)].bg,
            highlight_bg(&theme),
            "selection must not paint left of its first row",
        );
        assert_ne!(
            terminal.backend().buffer()[(35, value)].bg,
            highlight_bg(&theme),
            "selection must not paint right of its final row",
        );
    }

    #[test]
    fn markdown_code_and_plain_text_copy_original_visible_syntax() {
        use forge_ui::{PreviewView, SplitRowsCache, TextLang};
        use ratatui::{backend::TestBackend, Terminal};
        let theme = Theme::modern_ide();
        let lines = vec![
            "# 标题".into(),
            "const userId = 123;".into(),
            "  console.log(userId);".into(),
        ];
        let mut cache = SplitRowsCache::default();
        let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
        let area = Rect::new(0, 0, 50, 8);
        let mut selection = PreviewSelection::default();
        terminal
            .draw(|frame| {
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::Text {
                        lines: &lines,
                        truncated: false,
                        bytes_read: 50,
                        language: TextLang::Markdown,
                    },
                    &mut 0,
                    &mut cache,
                );
                selection.observe(frame, area, 0, &theme, "README.md:code", true);
            })
            .unwrap();
        // Source Markdown Code mode retains #, whereas Preview renders a
        // styled heading without the Markdown marker.
        let line = (0..50)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect::<String>();
        let hash = line
            .chars()
            .position(|ch| ch == '#')
            .expect("Code mode lost raw Markdown syntax") as u16;
        selection.begin(area, 0, hash, 0);
        selection.move_to(area, 0, 35, 2, true);
        terminal
            .draw(|frame| {
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::Text {
                        lines: &lines,
                        truncated: false,
                        bytes_read: 50,
                        language: TextLang::Markdown,
                    },
                    &mut 0,
                    &mut cache,
                );
                selection.observe(frame, area, 0, &theme, "README.md:code", true);
            })
            .unwrap();
        let selected = selection.selected_text().unwrap();
        assert!(selected.contains("# 标题"), "{selected}");
        assert!(selected.contains("const userId = 123;"), "{selected}");
        assert!(selected.contains("console.log(userId);"), "{selected}");
        assert!(
            !selected.contains("2│"),
            "copy must not include code gutter: {selected}"
        );
        assert!(
            !selected.contains("3│"),
            "copy must not include code gutter: {selected}"
        );
        assert!(
            selected.contains("\n  console.log(userId);"),
            "source indentation was lost: {selected}"
        );
        assert!(
            !selected.contains("0x1b"),
            "copy must not include ANSI color codes"
        );
        // Switching tabs must clear old visual coordinates.
        terminal
            .draw(|frame| {
                forge_ui::draw_preview(
                    frame,
                    area,
                    &theme,
                    PreviewView::Text {
                        lines: &lines,
                        truncated: false,
                        bytes_read: 50,
                        language: TextLang::Plain,
                    },
                    &mut 0,
                    &mut cache,
                );
                selection.observe(frame, area, 0, &theme, "notes.txt", true);
            })
            .unwrap();
        assert!(
            !selection.active(),
            "old file selection leaked to new content"
        );
    }

    #[test]
    fn single_click_and_missing_rows_do_not_copy_arbitrary_data() {
        let mut s = PreviewSelection::default();
        s.anchor = Some(Point { row: 2, col: 0 });
        assert!(s.selected_text().is_none());
        s.head = Some(Point { row: 10, col: 3 });
        assert!(s.selected_text().is_none());
        s.clear();
        assert!(!s.active());
    }
}

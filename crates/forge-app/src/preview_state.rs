//! Preview content and cache transitions; no App or service access.
use super::*;
use forge_theme::Theme;
use forge_ui::{draw_preview, PreviewView};
use ratatui::{layout::Rect, Frame};

pub(super) struct PreviewFrame {
    pub(super) inner: Rect,
    pub(super) scroll: usize,
    pub(super) horizontal_bar: Option<(Rect, usize)>,
}

/// Geometry-derived data. Reflow and scrolling cannot replace the document.
#[derive(Default)]
pub(super) struct PreviewPresentation {
    pub(super) preview_scroll: usize,
    pub(super) preview_scroll_x: usize,
    pub(super) markdown: MarkdownPreview,
    pub(super) selection: PreviewSelection,
    pub(super) text_rows: TextRowsCache,
    split_cache: SplitRowsCache,
    diff_row_count: usize,
}

#[derive(Default)]
pub(super) struct PreviewNavigation {
    pub(super) pending_source_line: Option<usize>,
    pub(super) pending_overview_line: Option<usize>,
    pub(super) pending_find_byte: Option<usize>,
    // Source line and wrapped-row displacement, independent of viewport width.
    refresh_anchor: Option<(usize, usize)>,
}

pub(super) struct PreviewState {
    document_kind: EntryKind,
    pub(super) presentation: PreviewPresentation,
    pub(super) navigation: PreviewNavigation,
    pub(super) find: file_find::FileFindState,
    pub(super) markdown_code: bool,
    pub(super) commit_link_focus: Option<usize>,
    pub(super) pending_preview: Option<PendingPreview>,
    pub(super) pending_page: Option<PendingPreviewPage>,
    pub(super) pending_diff: Option<PendingDiff>,
    pub(super) preview: Option<Preview>,
    pub(super) preview_loading: bool,
    pub(super) preview_mode: PreviewMode,
    /// Which kind of diff `file_diff` holds.
    pub(super) diff_source: DiffSource,
    /// Header chip for the loaded diff (`unstaged`, `staged`, or a short oid).
    pub(super) diff_label: String,
    pub(super) file_diff: Option<FileDiff>,
    pub(super) hunk_index: Option<usize>,
}

impl PreviewPresentation {
    fn source_anchor(&self) -> Option<(usize, usize)> {
        let line = self.text_rows.source_line_at(self.preview_scroll)?;
        let range = self.text_rows.source_row_range(line)?;
        Some((line, self.preview_scroll.saturating_sub(range.start)))
    }

    fn restore_source_anchor(&mut self, (line, offset): (usize, usize)) {
        if let Some(range) = self.text_rows.source_row_range(line) {
            self.preview_scroll = range
                .start
                .saturating_add(offset)
                .min(range.end.saturating_sub(1));
        }
    }
}

impl PreviewState {
    pub(super) fn new() -> Self {
        Self {
            document_kind: EntryKind::File,
            presentation: PreviewPresentation::default(),
            navigation: PreviewNavigation::default(),
            find: file_find::FileFindState::default(),
            markdown_code: false,
            commit_link_focus: None,
            pending_preview: None,
            pending_page: None,
            pending_diff: None,
            preview: None,
            preview_loading: false,
            preview_mode: PreviewMode::File,
            diff_source: DiffSource::Working(DiffSide::Unstaged),
            diff_label: String::new(),
            file_diff: None,
            hunk_index: None,
        }
    }

    pub(super) fn needs_empty_diff_hint(&self) -> bool {
        !self.preview_loading
            && matches!(self.preview_mode, PreviewMode::Diff)
            && self.file_diff.is_none()
    }

    pub(super) fn content(
        &self,
        root: &Path,
        markdown_available: bool,
        empty_diff_hint: &'static str,
    ) -> (String, PreviewView<'_>) {
        if self.preview_loading {
            return ("Loading".into(), PreviewView::Loading);
        }
        if matches!(self.preview_mode, PreviewMode::Diff) {
            return match &self.file_diff {
                Some(diff) => {
                    let hunks = diff.hunks();
                    let hunk = self.hunk_index.and_then(|i| hunks.get(i));
                    (
                        format!(
                            "{} · {}{}",
                            self.diff_label,
                            diff.path,
                            self.hunk_index
                                .map(|i| format!(" · hunk {}/{}", i + 1, hunks.len()))
                                .unwrap_or_default()
                        ),
                        PreviewView::Diff {
                            path: &diff.path,
                            side: &self.diff_label,
                            lines: &diff.lines,
                            hunk_start: hunk.map(|h| h.start),
                            hunk_end: hunk.map(|h| h.end),
                            hunk_idx: self.hunk_index.map(|i| (i, hunks.len())),
                        },
                    )
                }
                None => (
                    String::new(),
                    PreviewView::Hint {
                        text: empty_diff_hint,
                    },
                ),
            };
        }
        let Some(preview) = &self.preview else {
            return (String::new(), PreviewView::Idle);
        };
        let title = preview
            .path
            .strip_prefix(root)
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
            PreviewContent::Text { .. } if markdown_available && !self.markdown_code => {
                PreviewView::Markdown {
                    preview: &self.presentation.markdown,
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

    pub(super) fn prepare_source_rows(&mut self, theme: &Theme, width: u16, wrap: bool) {
        let Some(Preview {
            path,
            content:
                PreviewContent::Text {
                    lines,
                    truncated,
                    bytes_read,
                },
        }) = &self.preview
        else {
            return;
        };
        let preserve = self.navigation.pending_source_line.is_none()
            && !(self.markdown_available(false) && !self.markdown_code)
            && (self.presentation.text_rows.width() != width
                || self.presentation.text_rows.wrap_enabled() != wrap);
        let anchor = if self.navigation.pending_source_line.is_some() {
            self.navigation.refresh_anchor = None;
            None
        } else if let Some(anchor) = self.navigation.refresh_anchor.take() {
            Some(anchor)
        } else if preserve {
            self.presentation.source_anchor()
        } else {
            None
        };
        self.presentation.text_rows.ensure_wrapping(
            theme,
            lines,
            *truncated,
            *bytes_read,
            TextLang::from_name(path.file_name().and_then(|s| s.to_str()).unwrap_or("")),
            width,
            wrap,
        );
        if let Some(anchor) = anchor {
            self.presentation.restore_source_anchor(anchor);
        }
    }

    pub(super) fn markdown_available(&self, git_sidebar: bool) -> bool {
        !git_sidebar
            && !self.preview_loading
            && matches!(self.preview_mode, PreviewMode::File)
            && self.preview.as_ref().is_some_and(|preview| {
                matches!(preview.content, PreviewContent::Text { .. })
                    && TextLang::from_name(&preview.path.to_string_lossy()) == TextLang::Markdown
            })
    }

    // Rendering owns only presentation caches, viewport offsets and selection cells.
    // No App, Git service, scheduler, or workspace mutation is available here.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_frame(
        &mut self,
        frame: &mut Frame<'_>,
        mut area: Rect,
        theme: &Theme,
        root: &Path,
        markdown_available: bool,
        empty_diff_hint: &'static str,
        wrap: bool,
    ) -> PreviewFrame {
        if markdown_available && !self.markdown_code {
            if let Some(Preview {
                content:
                    PreviewContent::Text {
                        lines,
                        truncated,
                        bytes_read,
                    },
                ..
            }) = &self.preview
            {
                self.presentation.markdown.ensure(
                    lines,
                    *truncated,
                    *bytes_read,
                    area.width,
                    theme,
                );
            }
        }
        if !self.preview_loading
            && matches!(self.preview_mode, PreviewMode::File)
            && (!markdown_available || self.markdown_code)
        {
            self.prepare_source_rows(theme, area.width, wrap);
        }
        let mut scroll = self.presentation.preview_scroll;
        // Detach caches before borrowing the preview. Code view uses
        // the same safe, pre-wrapped row boundaries as Markdown, so
        // a two-cell CJK glyph can never spill into the Files pane.
        let mut cache = std::mem::take(&mut self.presentation.split_cache);
        let text_cache = std::mem::take(&mut self.presentation.text_rows);
        let pending_line = self.navigation.pending_source_line.take();
        let pending_find_byte = self.navigation.pending_find_byte.take();
        let (_, content) = self.content(root, markdown_available, empty_diff_hint);
        let numbered = matches!(content, PreviewView::Text { .. });
        let mut horizontal_x = self.presentation.preview_scroll_x;
        let mut horizontal_bar = None;
        let inner = match content {
            PreviewView::Text { .. } => {
                if !wrap && area.height > 2 && area.width > 7 {
                    let visible_cols = area.width.saturating_sub(5) as usize;
                    let max_x = text_cache.max_code_columns().saturating_sub(visible_cols);
                    horizontal_x = horizontal_x.min(max_x);
                    if max_x > 0 {
                        horizontal_bar = Some((
                            Rect::new(
                                area.x + 5,
                                area.bottom() - 1,
                                area.width.saturating_sub(6),
                                1,
                            ),
                            max_x,
                        ));
                        area.height -= 1;
                    }
                } else {
                    horizontal_x = 0;
                }
                if let Some(line) = pending_line {
                    scroll = pending_find_byte.map_or_else(
                        || text_cache.source_to_row(line),
                        |byte| text_cache.source_byte_to_row(line, byte),
                    );
                }
                draw_preview(
                    frame,
                    area,
                    theme,
                    if wrap {
                        PreviewView::TextRows {
                            rows: text_cache.rows(),
                        }
                    } else {
                        PreviewView::TextRowsScrolled {
                            rows: text_cache.rows(),
                            horizontal_offset: horizontal_x,
                        }
                    },
                    &mut scroll,
                    &mut cache,
                )
            }
            PreviewView::Diff {
                path,
                lines,
                hunk_start,
                hunk_end,
                ..
            } if !wrap => {
                let half_content = (area.width as usize / 2).saturating_sub(7);
                let max_x = cache.max_code_columns(lines).saturating_sub(half_content);
                horizontal_x = horizontal_x.min(max_x);
                if max_x > 0 && area.height > 2 {
                    horizontal_bar = Some((
                        Rect::new(area.x, area.bottom() - 1, area.width.saturating_sub(1), 1),
                        max_x,
                    ));
                    area.height -= 1;
                }
                draw_preview(
                    frame,
                    area,
                    theme,
                    PreviewView::DiffScrolled {
                        path,
                        lines,
                        hunk_start,
                        hunk_end,
                        horizontal_offset: horizontal_x,
                    },
                    &mut scroll,
                    &mut cache,
                )
            }
            other => draw_preview(frame, area, theme, other, &mut scroll, &mut cache),
        };
        self.presentation.preview_scroll_x = horizontal_x;
        self.presentation.split_cache = cache;
        self.presentation.text_rows = text_cache;
        self.presentation.preview_scroll = scroll;
        // Snapshot rendered cells before overlaying the selection.
        // This preserves Markdown table borders and wrapped CJK.
        let identity = if let Some(diff) = &self.file_diff {
            format!("diff:{}:{:?}", diff.path, self.diff_source)
        } else if let Some(preview) = &self.preview {
            format!("file:{}:{}", preview.path.display(), self.markdown_code)
        } else {
            "empty".to_owned()
        };
        self.presentation
            .selection
            .observe(frame, inner, scroll, theme, &identity, numbered);
        PreviewFrame {
            inner,
            scroll,
            horizontal_bar,
        }
    }

    pub(super) fn document_kind(&self) -> EntryKind {
        self.document_kind
    }
    pub(super) fn set_document_kind(&mut self, kind: EntryKind) {
        self.document_kind = kind;
    }

    /// The caller takes the currently owned pending job. Its result must
    /// identify that same path, regardless of the sidebar's filter/selection.
    pub(super) fn accepts_requested_file_result(
        &self,
        requested: &Path,
        returned: Option<&Path>,
    ) -> bool {
        matches!(self.preview_mode, PreviewMode::File)
            && returned.is_none_or(|path| path == requested)
    }

    /// Returns whether the caller should refresh the existing in-file search.
    pub(super) fn install_file_preview(&mut self, path: &Path, preview: Preview) -> bool {
        let same_file = self.preview.as_ref().is_some_and(|old| old.path == path);
        self.navigation.refresh_anchor = if same_file
            && !self.preview_loading
            && (!self.markdown_available(false) || self.markdown_code)
            && matches!(preview.content, PreviewContent::Text { .. })
        {
            self.navigation
                .refresh_anchor
                .or_else(|| self.presentation.source_anchor())
        } else {
            None
        };
        self.presentation.selection.clear();
        self.presentation.markdown = MarkdownPreview::default();
        self.presentation.text_rows.clear();
        self.navigation.pending_source_line = None;
        self.preview = Some(preview);
        self.preview_loading = false;
        if !same_file {
            self.presentation.preview_scroll = 0;
        }
        same_file && self.find.open && !self.find.query.is_empty()
    }

    pub(super) fn install_file_error(&mut self, path: PathBuf, message: String) {
        self.presentation.selection.clear();
        self.presentation.text_rows.clear();
        self.navigation.pending_source_line = None;
        self.navigation.refresh_anchor = None;
        self.preview = Some(Preview::error(path, message));
        self.preview_loading = false;
        self.presentation.preview_scroll = 0;
    }

    pub(super) fn reveal_source_line(&mut self, line: usize, markdown_available: bool) {
        if markdown_available {
            self.markdown_code = true;
        }
        self.presentation.preview_scroll = line.saturating_sub(1);
        self.navigation.pending_source_line = Some(line);
        self.navigation.refresh_anchor = None;
    }

    pub(super) fn clear_diff_view(&mut self) {
        self.presentation.selection.clear();
        self.file_diff = None;
        self.hunk_index = None;
        self.presentation.split_cache.clear();
        self.presentation.diff_row_count = 0;
    }

    pub(super) fn diff_row_count(&self) -> usize {
        self.presentation.diff_row_count
    }

    #[cfg(test)]
    pub(super) fn set_test_diff_row_count(&mut self, rows: usize) {
        self.presentation.diff_row_count = rows;
    }

    pub(super) fn max_horizontal_offset(&mut self, width: u16) -> usize {
        if matches!(self.preview_mode, PreviewMode::Diff) {
            let visible = (width as usize / 2).saturating_sub(7);
            self.file_diff.as_ref().map_or(0, |diff| {
                self.presentation
                    .split_cache
                    .max_code_columns(&diff.lines)
                    .saturating_sub(visible)
            })
        } else {
            self.presentation
                .text_rows
                .max_code_columns()
                .saturating_sub(width.saturating_sub(5) as usize)
        }
    }

    pub(super) fn ensure_hunk_visible(&mut self, viewport: usize) {
        let Some(idx) = self.hunk_index else {
            return;
        };
        let Some(diff) = self.file_diff.take() else {
            return;
        };
        let hunks = diff.hunks();
        let Some(hunk) = hunks.get(idx).cloned() else {
            self.file_diff = Some(diff);
            return;
        };
        // Diff body starts at line 0 (title chip is outside the scroll area).
        // Map unified hunk index → split-row index (meta lines collapsed).
        let rendered_start =
            diff_render_hunk_start(&diff.lines, hunk.start, &mut self.presentation.split_cache);
        let total = if diff.lines.is_empty() {
            1
        } else {
            self.presentation.split_cache.ensure(&diff.lines).max(1)
        };
        self.presentation.diff_row_count = total;
        scroll_to_hunk(
            &mut self.presentation.preview_scroll,
            viewport,
            total,
            rendered_start,
        );
        self.file_diff = Some(diff);
    }

    pub(super) fn set_file_diff(&mut self, diff: FileDiff) {
        self.presentation.selection.clear();
        self.presentation
            .split_cache
            .set_full_new_lines(diff.full_new_lines.as_deref());
        let count = if diff.lines.is_empty() && diff.full_new_lines.is_none() {
            1
        } else {
            self.presentation.split_cache.ensure(&diff.lines).max(1)
        };
        self.presentation.diff_row_count = count;
        self.file_diff = Some(diff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_line_is_preserved_across_width_and_wrap_changes() {
        let mut state = PreviewState::new();
        state.preview = Some(Preview {
            path: PathBuf::from("test.txt"),
            content: PreviewContent::Text {
                lines: (0..30)
                    .map(|i| format!("{i}: {}", "word ".repeat(20)))
                    .collect(),
                truncated: false,
                bytes_read: 3100,
            },
        });
        let theme = Theme::default();
        state.prepare_source_rows(&theme, 80, true);
        state.presentation.preview_scroll = state.presentation.text_rows.source_to_row(12);
        let original = state.presentation.preview_scroll;
        state.prepare_source_rows(&theme, 24, true);
        assert_ne!(state.presentation.preview_scroll, original);
        assert_eq!(
            state
                .presentation
                .text_rows
                .source_line_at(state.presentation.preview_scroll),
            Some(12)
        );
        state.prepare_source_rows(&theme, 24, false);
        assert_eq!(state.presentation.preview_scroll, 11);
        assert_eq!(
            state
                .presentation
                .text_rows
                .source_line_at(state.presentation.preview_scroll),
            Some(12)
        );
        assert_eq!(
            state.preview.as_ref().unwrap().path,
            PathBuf::from("test.txt")
        );
        state.reveal_source_line(5, false);
        state.prepare_source_rows(&theme, 80, true);
        assert_eq!(state.navigation.pending_source_line, Some(5));
    }

    #[test]
    fn refreshed_wrapped_document_restores_source_anchor_after_resize() {
        let mut state = PreviewState::new();
        let path = PathBuf::from("long.txt");
        let make = |word: &str| Preview {
            path: path.clone(),
            content: PreviewContent::Text {
                lines: (0..60)
                    .map(|i| format!("{i} {}", word.repeat(30)))
                    .collect(),
                truncated: false,
                bytes_read: 9000,
            },
        };
        let theme = Theme::default();
        state.install_file_preview(&path, make("aaaa "));
        state.prepare_source_rows(&theme, 40, true);
        state.presentation.preview_scroll = state.presentation.text_rows.source_to_row(22) + 1;
        assert_eq!(state.presentation.source_anchor(), Some((22, 1)));
        state.install_file_preview(&path, make("bbbbbbbb "));
        state.prepare_source_rows(&theme, 24, true);
        assert_eq!(state.presentation.source_anchor(), Some((22, 1)));
        state.prepare_source_rows(&theme, 100, false);
        assert_eq!(state.presentation.source_anchor(), Some((22, 0)));
        assert_eq!(state.preview.as_ref().unwrap().path, path);
    }

    #[test]
    fn refresh_preserves_scroll_but_new_document_resets_it_without_app() {
        let mut preview = PreviewState::new();
        let a = PathBuf::from("a.txt");
        let b = PathBuf::from("b.txt");
        preview.install_file_preview(&a, Preview::error(a.clone(), "a"));
        preview.presentation.preview_scroll = 20;
        preview.find.open = true;
        preview.find.query = "word".into();
        assert!(preview.install_file_preview(&a, Preview::error(a.clone(), "a changed")));
        assert_eq!(preview.presentation.preview_scroll, 20);
        assert!(!preview.install_file_preview(&b, Preview::error(b.clone(), "b")));
        assert_eq!(preview.presentation.preview_scroll, 0);
        preview.reveal_source_line(8, true);
        assert_eq!(preview.presentation.preview_scroll, 7);
        assert!(preview.markdown_code);
        assert_eq!(preview.navigation.pending_source_line, Some(8));
    }

    #[test]
    fn unrelated_file_and_diff_mode_reject_late_results() {
        let mut preview = PreviewState::new();
        let a = PathBuf::from("a.txt");
        let b = PathBuf::from("b.txt");
        assert!(preview.accepts_requested_file_result(&a, Some(&a)));
        assert!(!preview.accepts_requested_file_result(&a, Some(&b)));
        preview.preview_mode = PreviewMode::Diff;
        assert!(!preview.accepts_requested_file_result(&a, Some(&a)));
    }
}

//! Reusable responsive sizing for workspace dialogs.
//! The modal's semantic content determines its size; no list should occupy
//! 90% of the screen simply because it shares the Tool panel renderer.
use super::*;

mod form;
pub(super) use form::draw_form;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DialogSize {
    pub width: u16,
    pub height: u16,
}

impl DialogSize {
    /// Reserve a small surrounding margin where possible without hiding
    /// focusable controls on narrow or short terminals.
    fn bounded(screen: (u16, u16), preferred: (u16, u16)) -> Self {
        let (sw, sh) = screen;
        let w = preferred.0.min(sw.saturating_sub(4).max(sw.min(8))).max(1);
        let h = preferred.1.min(sh.saturating_sub(4).max(sh.min(6))).max(1);
        Self {
            width: w,
            height: h,
        }
    }

    /// Popups with a short set of choices or a brief confirmation use their
    /// content height. Exploratory views (search, history, diff) stay spacious.
    pub(super) fn tools(panel: &tools::Panel, screen: (u16, u16)) -> Self {
        use tools::PanelKind;
        let row_count = panel.rows.len().min(18) as u16;
        let body_lines = panel.detail.lines().count().min(8) as u16;
        let input = u16::from(panel.is_input()) * 2;
        let compact = (row_count + input + 6 + u16::from(body_lines > 2)).clamp(8, 24);
        let (w, h) = match &panel.kind {
            PanelKind::Content => (
                screen.0.saturating_mul(92) / 100,
                screen.1.saturating_mul(88) / 100,
            ),
            PanelKind::Git(
                forge_git::GitQuery::History(_)
                | forge_git::GitQuery::HistoryPage { .. }
                | forge_git::GitQuery::Commit(_)
                | forge_git::GitQuery::StashDiff(_),
            )
            | PanelKind::Lines(_) => (
                screen.0.saturating_mul(86) / 100,
                screen.1.saturating_mul(84) / 100,
            ),
            PanelKind::Text => return result_size(panel, screen),
            PanelKind::Settings => (82, 21),
            PanelKind::Files => (90, (row_count + input + 6).clamp(9, 22)),
            PanelKind::Commands => (86, (row_count.min(8) + input + 5).clamp(8, 17)),
            PanelKind::Confirm(_) | PanelKind::Prompt(_) => {
                return form_size(
                    FormMode::from_panel(&panel.kind).expect("form panel"),
                    screen,
                    &panel.detail,
                    !panel.message.is_empty(),
                )
            }
            PanelKind::Workspaces => (90, compact),
            PanelKind::Git(_) | PanelKind::Filters => (86, compact),
        };
        Self::bounded(screen, (w, h))
    }

    pub(super) fn commit(screen: (u16, u16)) -> Self {
        Self::bounded(screen, (92, 20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_dialogs_fit_content_and_never_take_the_full_screen() {
        let branch = tools::Panel::new(
            tools::PanelKind::Git(forge_git::GitQuery::Branches),
            "Branches",
        );
        let empty = DialogSize::tools(&branch, (130, 38));
        assert!(empty.height <= 10, "{empty:?}");
        assert!(empty.width < 130);
        let commands = tools::Panel::new(tools::PanelKind::Commands, "Commands");
        assert!(DialogSize::tools(&commands, (130, 38)).height <= 9);
        let search = tools::Panel::new(tools::PanelKind::Content, "Contents");
        assert!(DialogSize::tools(&search, (130, 38)).height > 20);
        let narrow = DialogSize::tools(&branch, (30, 9));
        assert!(narrow.width <= 30 && narrow.height <= 9);
        let mut completed = tools::Panel::new(tools::PanelKind::Text, "Completed");
        completed.detail = "Created a file".into();
        assert!(DialogSize::tools(&completed, (130, 38)).height <= 9);
        completed.detail = "Detailed diagnostics\n".repeat(45);
        assert!(DialogSize::tools(&completed, (130, 38)).height > 20);
    }

    #[test]
    fn completed_result_uses_message_for_size_and_renders_like_native_form() {
        let mut panel = tools::Panel::new(tools::PanelKind::Text, "Completed");
        panel.message = "Renamed to /home/fxh/tools/forge/wwww223".into();
        let size = DialogSize::tools(&panel, (118, 36));
        assert_eq!(size.height, 5);
        assert!(
            size.width < 70,
            "result must fit its text, not fill preview: {size:?}"
        );
        let area = Rect::new(
            (118 - size.width) / 2,
            (36 - size.height) / 2,
            size.width,
            size.height,
        );
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(118, 36)).unwrap();
        term.draw(|frame| draw_result(frame, area, &mut panel, Palette::default()))
            .unwrap();
        let b = term.backend().buffer();
        assert_eq!(b[(area.x, area.y)].symbol(), "╭");
        assert_eq!(b[(area.right() - 1, area.y)].symbol(), "╮");
        assert_eq!(b[(area.x, area.bottom() - 1)].symbol(), "╰");
        assert_eq!(b[(area.right() - 1, area.bottom() - 1)].symbol(), "╯");
        let screen: String = b.content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("Completed"), "{screen}");
        assert!(
            screen.contains("Renamed to /home/fxh/tools/forge/wwww223"),
            "{screen}"
        );
        assert!(screen.contains("Esc close"), "{screen}");
        assert_eq!(panel.scroll_limit, 0);
        assert_eq!(panel.list_area, Rect::default());
    }

    #[test]
    fn long_result_and_error_are_scrollable_and_fit_narrow_terminals() {
        let mut panel = tools::Panel::new(tools::PanelKind::Text, "Operation failed");
        panel.detail = (1..=45)
            .map(|i| format!("Error diagnostic {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        for (w, h) in [(130, 38), (52, 16), (27, 9)] {
            let size = DialogSize::tools(&panel, (w, h));
            assert!(size.width <= w && size.height <= h);
            let area = Rect::new(
                (w - size.width) / 2,
                (h - size.height) / 2,
                size.width,
                size.height,
            );
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            term.draw(|f| draw_result(f, area, &mut panel, Palette::default()))
                .unwrap();
            let screen: String = term
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(screen.contains("Operation failed"), "{screen}");
            assert!(panel.scroll_limit > 0);
            panel.scroll = usize::MAX;
            term.draw(|f| draw_result(f, area, &mut panel, Palette::default()))
                .unwrap();
            assert_eq!(panel.scroll, panel.scroll_limit);
        }
    }
}

/// Identical shell for tool lists, settings and interactive form dialogs.
pub(super) fn draw_shell(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    hint: &str,
    p: Palette,
) -> Rect {
    frame.render_widget(Clear, area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(p.border))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().fg(p.fg).bg(p.bg));
    if !hint.is_empty() {
        // Keep the Esc key visually consistent with native form actions.
        let footer = if let Some(rest) = hint.strip_prefix(" Esc ") {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "Esc",
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {rest}"), Style::default().fg(p.fg_dim)),
            ])
        } else {
            Line::from(Span::styled(hint.to_owned(), Style::default().fg(p.fg_dim)))
        };
        block = block.title_bottom(footer);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// Result panels may contain a long report in detail, or a concise
/// success notification in message. Both display and sizing use this source.
fn result_content(panel: &tools::Panel) -> &str {
    if panel.detail.trim().is_empty() {
        &panel.message
    } else {
        &panel.detail
    }
}

fn result_size(panel: &tools::Panel, screen: (u16, u16)) -> DialogSize {
    use unicode_width::UnicodeWidthStr;
    let content = result_content(panel);
    let longest = content
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    let preferred_width = longest.saturating_add(6).clamp(38, 78) as u16;
    let width = DialogSize::bounded(screen, (preferred_width, 5)).width;
    let line_count = wrapped_line_count(content, width.saturating_sub(4).max(1) as usize);
    // Two vertical breathing rows, two borders, then the visible message.
    let preferred_height = (line_count.saturating_add(4) as u16).clamp(5, 24);
    DialogSize::bounded(screen, (width, preferred_height))
}

/// Native result popup: no fake list area, no filler height, same chrome as
/// Rename/Confirm. Long content scrolls without interfering with Esc.
pub(super) fn draw_result(frame: &mut Frame<'_>, area: Rect, panel: &mut tools::Panel, p: Palette) {
    panel.area = area;
    panel.list_area = Rect::default();
    panel.form_cancel = Rect::default();
    panel.form_submit = Rect::default();
    let inner = draw_shell(frame, area, &panel.title, " Esc close ", p);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let pad_x = u16::from(inner.width > 4);
    let pad_y = u16::from(inner.height > 2);
    let text_area = Rect::new(
        inner.x + pad_x,
        inner.y + pad_y,
        inner.width.saturating_sub(pad_x * 2),
        inner.height.saturating_sub(pad_y * 2),
    );
    if text_area.width == 0 || text_area.height == 0 {
        return;
    }
    let content = result_content(panel).to_owned();
    panel.scroll_limit = wrapped_line_count(&content, text_area.width as usize)
        .saturating_sub(text_area.height as usize);
    panel.scroll = panel.scroll.min(panel.scroll_limit);
    let fg = if panel.title == "Operation failed" {
        p.error
    } else {
        p.fg
    };
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(fg).bg(p.bg))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0)),
        text_area,
    );
}

/// A prompt is exclusively an input, and a confirmation exclusively a review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormMode {
    Prompt,
    Confirm,
}

impl FormMode {
    pub(super) fn from_panel(kind: &tools::PanelKind) -> Option<Self> {
        match kind {
            tools::PanelKind::Prompt(_) => Some(Self::Prompt),
            tools::PanelKind::Confirm(_) => Some(Self::Confirm),
            _ => None,
        }
    }

    pub(super) fn action_name(self, kind: &tools::PanelKind) -> &'static str {
        use tools::PromptAction;
        match (self, kind) {
            (
                Self::Confirm,
                tools::PanelKind::Confirm(tools::Operation::Files(
                    forge_fs::workspace::FileOperation::Delete(_),
                )),
            ) => "Delete",
            (Self::Confirm, _) => "Confirm",
            (
                Self::Prompt,
                tools::PanelKind::Prompt(PromptAction::Rename(_) | PromptAction::RenameBranch(_)),
            ) => "Rename",
            (
                Self::Prompt,
                tools::PanelKind::Prompt(
                    PromptAction::NewFile
                    | PromptAction::NewDirectory
                    | PromptAction::Branch
                    | PromptAction::Worktree,
                ),
            ) => "Create",
            (Self::Prompt, tools::PanelKind::Prompt(PromptAction::OpenWorkspace)) => "Open",
            (Self::Prompt, tools::PanelKind::Prompt(PromptAction::Copy(_))) => "Copy",
            (Self::Prompt, tools::PanelKind::Prompt(PromptAction::Move(_))) => "Move",
            (
                Self::Prompt,
                tools::PanelKind::Prompt(PromptAction::Stash | PromptAction::StashUnstaged),
            ) => "Save",
            (Self::Prompt, tools::PanelKind::Prompt(PromptAction::Amend)) => "Review",
            _ => "Apply",
        }
    }

    pub(super) fn field_name(self, kind: &tools::PanelKind) -> &'static str {
        use tools::PromptAction;
        match (self, kind) {
            (
                Self::Prompt,
                tools::PanelKind::Prompt(
                    PromptAction::Rename(_) | PromptAction::Copy(_) | PromptAction::Move(_),
                ),
            ) => "New path",
            (Self::Prompt, tools::PanelKind::Prompt(PromptAction::RenameBranch(_))) => {
                "Branch name"
            }
            (
                Self::Prompt,
                tools::PanelKind::Prompt(
                    PromptAction::NewFile
                    | PromptAction::NewDirectory
                    | PromptAction::OpenWorkspace
                    | PromptAction::Worktree,
                ),
            ) => "Path",
            (
                Self::Prompt,
                tools::PanelKind::Prompt(
                    PromptAction::Amend | PromptAction::Stash | PromptAction::StashUnstaged,
                ),
            ) => "Message",
            (
                Self::Prompt,
                tools::PanelKind::Prompt(PromptAction::Extension | PromptAction::Exclusions),
            ) => "Filter",
            _ => "Name",
        }
    }
}

fn wrapped_line_count(text: &str, width: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    text.lines()
        .map(|line| {
            let mut count = 1;
            let mut column = 0;
            for c in line.chars() {
                let advance = UnicodeWidthChar::width(c).unwrap_or(0).min(width);
                if column + advance > width {
                    count += 1;
                    column = 0;
                }
                column += advance;
            }
            count
        })
        .sum::<usize>()
        .max(1)
}

/// A native terminal dialog should fit the message rather than forcing
/// button-sized empty space around it. Leave one blank line above the
/// footer; long messages grow vertically and remain scrollable when clipped.
pub(super) fn form_size(
    mode: FormMode,
    screen: (u16, u16),
    detail: &str,
    has_error: bool,
) -> DialogSize {
    use unicode_width::UnicodeWidthStr;
    let widest = detail
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    let preferred_width = (widest.saturating_add(8)).clamp(44, 64) as u16;
    let width = preferred_width.min(screen.0.saturating_sub(4).max(screen.0.min(8)));
    let body_width = width.saturating_sub(6).max(1) as usize;
    let detail_rows = wrapped_line_count(detail, body_width).min(12) as u16;
    let base = if mode == FormMode::Prompt { 7 } else { 5 };
    DialogSize::bounded(
        screen,
        (
            width,
            (base + detail_rows.saturating_sub(1) + u16::from(has_error)).min(22),
        ),
    )
}

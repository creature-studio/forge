//! VS Code-style Activity Bar and collapsible Sidebar heading.
//! Hit regions and hover styling share the reusable HoverTargets component.
use super::*;
use forge_ui::{ActivityIcon, Icons};

const BAR_WIDTH: u16 = 5;

pub(super) fn draw_activity_bar(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) {
    model.activity_hits.clear();
    model.session.layout.activity = area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    // This lane owns the full rectangle, not only the icon cells. A styled
    // borderless Block changes colors but does not erase existing glyphs;
    // clear first so no stale text can survive in the empty rail (including
    // when a modal is drawn over the workspace).
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(Style::default().bg(p.bg)), area);
    // Fixed one-cell icons, evenly centered with a slim active indicator.
    let entries = [
        (
            ActivityAction::Files,
            ActivityIcon::Files,
            1,
            model.session.activity == ActivityTab::Files && model.session.sidebar_visible,
        ),
        (
            ActivityAction::Git,
            ActivityIcon::Git,
            3,
            model.session.activity == ActivityTab::Git && model.session.sidebar_visible,
        ),
        (ActivityAction::Search, ActivityIcon::Search, 5, false),
        (
            ActivityAction::Terminal,
            ActivityIcon::Terminal,
            7,
            model.terminal.as_ref().is_some_and(|t| t.visible),
        ),
    ];
    let settings_y = area.bottom().saturating_sub(2);
    for (action, icon, offset, active) in entries {
        let y = area.y.saturating_add(offset);
        if y >= settings_y {
            continue;
        }
        draw_button(frame, area, model, p, action, icon, y, active);
    }
    if settings_y >= area.y && area.height > 1 {
        draw_button(
            frame,
            area,
            model,
            p,
            ActivityAction::Settings,
            ActivityIcon::Settings,
            settings_y,
            false,
        );
    }
}

fn draw_button(
    frame: &mut Frame<'_>,
    bar: Rect,
    model: &mut App,
    p: Palette,
    action: ActivityAction,
    icon: ActivityIcon,
    y: u16,
    active: bool,
) {
    let rect = Rect::new(
        bar.x.saturating_add(1),
        y,
        bar.width.saturating_sub(2).min(3),
        1,
    );
    if active && bar.width > 0 {
        frame.buffer_mut()[(bar.x, y)]
            .set_symbol("┃")
            .set_style(Style::default().fg(p.accent).bg(p.bg));
    }
    let label = format!(" {} ", Icons::activity(icon));
    model.activity_hits.draw_text(
        frame,
        rect,
        action,
        &label,
        Style::default()
            .fg(if active { p.accent } else { p.fg_dim })
            .bg(if active { p.selection } else { p.bg })
            .add_modifier(if active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        Style::default()
            .fg(if active { p.accent } else { p.fg })
            .bg(p.selection)
            .add_modifier(Modifier::BOLD),
    );
    match action {
        ActivityAction::Files => model.session.layout.files_tab = rect,
        ActivityAction::Git => model.session.layout.git_tab = rect,
        ActivityAction::Search => model.session.layout.search_tab = rect,
        ActivityAction::Terminal => model.session.layout.terminal_activity = rect,
        ActivityAction::Settings => model.session.layout.settings_tab = rect,
        ActivityAction::Collapse => {}
    }
}

pub(super) fn draw_sidebar_heading(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &mut App,
    p: Palette,
) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let title = if model.in_git_sidebar() {
        " SOURCE CONTROL"
    } else {
        " EXPLORER"
    };
    let heading = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(title).style(
            Style::default()
                .fg(p.fg_dim)
                .bg(p.bg)
                .add_modifier(Modifier::BOLD),
        ),
        heading,
    );
    if area.width >= 5 {
        let toggle = Rect::new(area.right() - 3, area.y, 3, 1);
        model.session.layout.sidebar_toggle = toggle;
        model.activity_hits.draw_text(
            frame,
            toggle,
            ActivityAction::Collapse,
            " ‹ ",
            Style::default().fg(p.fg_dim).bg(p.bg),
            Style::default().fg(p.fg).bg(p.selection),
        );
    }
    Rect::new(area.x, area.y + 1, area.width, area.height - 1)
}

pub(super) fn activity_bar_width() -> u16 {
    BAR_WIDTH
}

/// Shared overlay for Activity Bar icons and the Git Diff header link.
/// Render from the Footer pane (after the workspace) so hints are not
/// overwritten by the Sidebar/Preview/Terminal which paint after the bar.
pub(super) fn draw_workspace_tooltip(frame: &mut Frame<'_>, model: &App, p: Palette) {
    if model.session.help_open
        || model.keys.open
        || model.editing.active()
        || model.tools.panel.is_some()
        || model.session.commit_open
        || model.session.context_menu.is_some()
        || model.blame.popup.is_some()
    {
        return;
    }
    let bounds = frame.area();
    let style = Style::default().fg(p.fg).bg(p.bg);
    let border = Style::default().fg(p.accent);
    if let Some((anchor, action)) = model.activity_hits.hovered() {
        let (text, shortcut) = match action {
            ActivityAction::Files => ("Explorer", model.keys.label("F1")),
            ActivityAction::Git => ("Source Control", model.keys.label("F2")),
            ActivityAction::Search => ("Search contents", "Ctrl+Shift+F".into()),
            ActivityAction::Terminal => ("Toggle terminal", "F8".into()),
            ActivityAction::Settings => ("Settings", model.keys.label("F9")),
            ActivityAction::Collapse => ("Collapse sidebar", "Ctrl+B".into()),
        };
        forge_ui::Tooltip::new(text)
            .shortcut(Some(&shortcut))
            .placement(forge_ui::TooltipPlacement::Right)
            .draw(frame, anchor, bounds, style, border);
    } else if model.session.layout.open_file_tab.width > 0
        && model
            .activity_hits
            .is_hovered(model.session.layout.open_file_tab)
    {
        // The Preview header already owns the clickable rectangle. The
        // Activity hover tracker also remembers the global pointer position;
        // do not create a second set of click coordinates for this button.
        let shortcut = model.keys.label("o");
        forge_ui::Tooltip::new("Open file in Explorer")
            .shortcut(Some(&shortcut))
            .draw(
                frame,
                model.session.layout.open_file_tab,
                bounds,
                style,
                border,
            );
    }
}

#[cfg(test)]
mod tooltip_tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
    };

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let area = terminal.backend().size().expect("backend size");
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn paint(app: &mut App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 26)).unwrap();
        terminal
            .draw(|frame| {
                let p = workspace_palette();
                draw_activity_bar(frame, Rect::new(0, 1, 5, 22), app, p);
                draw_sidebar_heading(frame, Rect::new(5, 1, 28, 22), app, p);
                draw_workspace_tooltip(frame, app, p);
            })
            .unwrap();
        screen_text(&terminal)
    }

    #[tokio::test]
    async fn activity_bar_erases_stale_glyphs_across_its_entire_lane() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        let area = Rect::new(0, 1, 5, 22);
        let p = workspace_palette();
        terminal.draw(|frame| {
            // Simulate prior content in this lane before the activity bar
            // paints. A borderless Block alone does NOT clear these symbols.
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    frame.buffer_mut()[(x, y)].set_symbol("3");
                }
            }
            draw_activity_bar(frame, area, &mut app, p);
        }).unwrap();
        let buffer = terminal.backend().buffer();
        for y in 10..21 {
            for x in 0..5 {
                assert_eq!(buffer[(x, y)].symbol(), " ",
                    "stale glyph at ({x},{y}) in empty activity-bar lane");
                assert_eq!(buffer[(x, y)].bg, p.bg);
            }
        }
        assert_ne!(buffer[(2, 2)].symbol(), " ", "active icon remains visible");
    }

    #[tokio::test]
    async fn activity_tooltips_follow_mouse_without_changing_activity() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        paint(&mut app, 90);
        let before = app.session.activity;
        for (action, text, shortcut) in [
            (ActivityAction::Files, "Explorer", "F1"),
            (ActivityAction::Git, "Source Control", "F2"),
            (ActivityAction::Search, "Search contents", "Ctrl+Shift+F"),
            (ActivityAction::Terminal, "Toggle terminal", "F8"),
            (ActivityAction::Settings, "Settings", "F9"),
            (ActivityAction::Collapse, "Collapse sidebar", "Ctrl+B"),
        ] {
            let rect = app
                .activity_hits
                .iter()
                .find(|(_, item)| *item == action)
                .expect("control painted")
                .0;
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            });
            let painted = paint(&mut app, 90);
            assert!(painted.contains(text), "{action:?}: {painted}");
            assert!(painted.contains(shortcut), "{action:?}: {painted}");
            assert_eq!(app.session.activity, before, "hover cannot switch panels");
        }
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 45,
            row: 14,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!paint(&mut app, 90).contains("Collapse sidebar"));
        assert!(!paint(&mut app, 90).contains("Source Control  ·"));
    }

    #[tokio::test]
    async fn git_header_tooltip_uses_existing_click_region_and_obeys_modals() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        paint(&mut app, 90);
        app.session.layout.open_file_tab = Rect::new(71, 1, 14, 1);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 74,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        terminal
            .draw(|frame| draw_workspace_tooltip(frame, &app, workspace_palette()))
            .unwrap();
        let painted = screen_text(&terminal);
        assert!(painted.contains("Open file in Explorer"), "{painted}");
        assert!(painted.contains(" ·  o"), "{painted}");
        app.session.help_open = true;
        terminal
            .draw(|frame| draw_workspace_tooltip(frame, &app, workspace_palette()))
            .unwrap();
        assert!(!screen_text(&terminal).contains("Open file in Explorer"));
    }
}

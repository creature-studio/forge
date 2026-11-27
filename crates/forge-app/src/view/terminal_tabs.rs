//! Mouse-accessible terminal tabs; the header belongs to Forge, not the PTY.
use super::*;
use crate::embedded_terminal::MouseRouting;
use ratatui::{style::Style, widgets::Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct Label {
    id: u64,
    name: String,
    ended: bool,
    history: usize,
}
fn shorten(name: &str, width: usize) -> String {
    let (mut out, mut n) = (String::new(), 0);
    for ch in name.chars() {
        let w = ch.width().unwrap_or(0);
        if n + w > width {
            break;
        }
        n += w;
        out.push(ch);
    }
    out
}
pub(super) fn draw_tab_strip(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &mut App,
    palette: Palette,
) -> Rect {
    model.terminal_tab_hits.clear();
    model.terminal_tab_bar = Rect::default();
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    let bar = Rect::new(area.x, area.y, area.width, 1);
    model.terminal_tab_bar = bar;
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(palette.bg)),
        bar,
    );
    let current = model.terminals.active.as_ref().map(|t| t.id);
    let mut tabs: Vec<_> = model
        .terminals
        .inactive()
        .iter()
        .chain(model.terminals.active.iter())
        .map(|t| Label {
            id: t.id,
            name: t.draw_tab_title(),
            ended: t.ended.load(std::sync::atomic::Ordering::Acquire),
            history: if Some(t.id) == current {
                t.scrollback_position()
            } else {
                0
            },
        })
        .collect();
    tabs.sort_by_key(|t| t.id);
    // At narrow widths, give the active tab priority over secondary buttons.
    let mouse_mode_label = if model
        .terminals
        .active
        .as_ref()
        .is_some_and(|t| t.mouse_routing == MouseRouting::Native)
    {
        " APP "
    } else {
        " SEL "
    };
    let controls: Vec<_> = if area.width < 24 {
        vec![
            (" + ", TerminalTabAction::New),
            (" ▾ ", TerminalTabAction::Profiles),
        ]
    } else if area.width < 42 {
        vec![
            (" + ", TerminalTabAction::New),
            (" ▾ ", TerminalTabAction::Profiles),
            (" ‹ ", TerminalTabAction::Previous),
            (" › ", TerminalTabAction::Next),
        ]
    } else {
        vec![
            (" + ", TerminalTabAction::New),
            (" ▾ ", TerminalTabAction::Profiles),
            (" ‹ ", TerminalTabAction::Previous),
            (" › ", TerminalTabAction::Next),
            (" □ ", TerminalTabAction::Maximize),
            (mouse_mode_label, TerminalTabAction::ToggleMouseRouting),
            (" ? ", TerminalTabAction::Help),
            (" − ", TerminalTabAction::Hide),
        ]
    };
    let control_width: u16 = controls.iter().map(|(s, _)| s.width() as u16).sum();
    let available = area.width.saturating_sub(control_width);
    let tab_width = |tab: &Label, n: usize| {
        let title = shorten(&tab.name, 10);
        let label = format!(" {} {}{} ", n + 1, title, if tab.ended { "!" } else { "" });
        (label.width() + 3).min(21).min(available as usize) as u16
    };
    let current_idx = tabs.iter().position(|t| Some(t.id) == current).unwrap_or(0);
    let mut first = 0;
    while first < current_idx
        && tabs[first..=current_idx]
            .iter()
            .enumerate()
            .map(|(i, t)| tab_width(t, first + i) as usize)
            .sum::<usize>()
            > available as usize
    {
        first += 1;
    }
    let mut x = area.x;
    let limit = area.x.saturating_add(available);
    for (i, tab) in tabs.iter().enumerate().skip(first) {
        let width = tab_width(tab, i);
        if x.saturating_add(width) > limit {
            break;
        }
        let active = Some(tab.id) == current;
        let fg = if active {
            palette.accent
        } else {
            palette.fg_dim
        };
        let bg = if active {
            palette.selection
        } else {
            palette.bg
        };
        let label = format!(
            " {} {}{}{} ",
            i + 1,
            shorten(&tab.name, 10),
            if tab.ended { "!" } else { "" },
            if tab.history > 0 { " ↑" } else { "" }
        );
        let body = Rect::new(x, bar.y, width.saturating_sub(3), 1);
        model.terminal_tab_hits.draw_text(
            frame,
            body,
            TerminalTabAction::Activate(tab.id),
            &label,
            Style::default().fg(fg).bg(bg),
            Style::default()
                .fg(if active {
                    palette.on_accent
                } else {
                    palette.fg
                })
                .bg(if active {
                    palette.accent
                } else {
                    palette.selection
                }),
        );
        let close = Rect::new(x + width - 3, bar.y, 3, 1);
        model.terminal_tab_hits.draw_text(
            frame,
            close,
            TerminalTabAction::Close(tab.id),
            " × ",
            Style::default()
                .fg(if active { palette.fg } else { palette.fg_dim })
                .bg(bg),
            Style::default().fg(palette.on_accent).bg(palette.accent),
        );
        x += width;
    }
    let mut right = area.right().saturating_sub(control_width);
    // Maximized Terminal has no workspace footer. Keep an always-visible
    // keyboard hint in the spare strip between tabs and action buttons.
    if model
        .terminals
        .active
        .as_ref()
        .is_some_and(|term| term.maximized)
    {
        let hint = " Alt+1-8 tabs   Shift+F8 new   F12 next   F4 help ";
        let room = right.saturating_sub(x);
        let text = if room as usize >= hint.width() {
            hint
        } else if room >= 22 {
            " Alt+1-8 tabs · F4 help "
        } else if room >= 11 {
            " F4 help "
        } else {
            ""
        };
        if !text.is_empty() {
            frame.render_widget(
                Paragraph::new(text).style(Style::default().fg(palette.fg_dim).bg(palette.bg)),
                Rect::new(x, bar.y, room, 1),
            );
        }
    }
    for (label, action) in controls {
        let width = label.width() as u16;
        if right >= area.x && right.saturating_add(width) <= area.right() {
            let rect = Rect::new(right, bar.y, width, 1);
            model.terminal_tab_hits.draw_text(
                frame,
                rect,
                action,
                label,
                Style::default()
                    .fg(if action == TerminalTabAction::New {
                        palette.accent
                    } else {
                        palette.fg_dim
                    })
                    .bg(palette.bg),
                Style::default().fg(palette.fg).bg(palette.selection),
            );
        }
        right = right.saturating_add(width);
    }
    Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    )
}

/// Non-interactive feedback, including when the workspace footer is hidden.
pub(super) fn draw_copy_notice(frame: &mut Frame<'_>, area: Rect, model: &App, palette: Palette) {
    let Some(notice) = model.session.terminal_copy_notice.as_ref() else {
        return;
    };
    if model.terminal_copy_notice_expired() || area.width < 8 || area.height == 0 {
        return;
    }
    let label = format!(" {} ", notice.message);
    let width = (label.width() as u16).min(area.width);
    let rect = Rect::new(area.right() - width, area.y, width, 1);
    let fg = if notice.error {
        ratatui::style::Color::Red
    } else {
        palette.accent
    };
    frame.render_widget(
        Paragraph::new(shorten(&label, width as usize))
            .style(Style::default().fg(fg).bg(palette.bg)),
        rect,
    );
}

/// Draw after the PTY so the hint sits above terminal output, not inside it.
/// Reuse HoverTargets' painted hitbox rather than a second set of coordinates.
pub(super) fn draw_tab_tooltip(frame: &mut Frame<'_>, area: Rect, model: &App, palette: Palette) {
    let Some((anchor, action)) = model.terminal_tab_hits.hovered() else {
        return;
    };
    if !model
        .terminals
        .active
        .as_ref()
        .is_some_and(|t| t.is_visible())
        || model.keys.open
        || model.session.help_open
        || model.editing.active()
        || model.tools.panel.is_some()
        || model.session.commit_open
        || model.blame.popup.is_some()
        || model.session.context_menu.is_some()
    {
        return;
    }
    let (description, shortcut) = tab_action_hint(model, action);
    forge_ui::Tooltip::new(&description)
        .shortcut(shortcut.as_deref())
        .draw(
            frame,
            anchor,
            area,
            Style::default().fg(palette.fg).bg(palette.bg),
            Style::default().fg(palette.accent),
        );
}

fn tab_action_hint(model: &App, action: TerminalTabAction) -> (String, Option<String>) {
    let mut tabs: Vec<_> = model
        .terminals
        .inactive()
        .iter()
        .chain(model.terminals.active.iter())
        .collect();
    tabs.sort_by_key(|tab| tab.id);
    match action {
        TerminalTabAction::Activate(id) => {
            let position = tabs.iter().position(|tab| tab.id == id);
            let label = tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.display_title())
                .unwrap_or_else(|| "Terminal".into());
            let number = position.map(|p| p + 1).unwrap_or(1);
            (
                format!("Terminal {number}: {label}"),
                (number <= 8).then(|| format!("Alt+{number}")),
            )
        }
        TerminalTabAction::Close(id) => {
            let label = tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.display_title())
                .unwrap_or_else(|| "Terminal".into());
            let active = model
                .terminals
                .active
                .as_ref()
                .is_some_and(|tab| tab.id == id);
            (
                format!("Close terminal: {label}"),
                active.then(|| "Ctrl+Shift+W".into()),
            )
        }
        TerminalTabAction::New => ("New terminal".into(), Some("Shift+F8".into())),
        TerminalTabAction::Profiles => {
            ("Select shell profile".into(), Some("Ctrl+Shift+F8".into()))
        }
        TerminalTabAction::Previous => ("Previous terminal".into(), None),
        TerminalTabAction::Next => ("Next terminal".into(), Some("F12".into())),
        TerminalTabAction::Maximize => ("Toggle terminal maximize".into(), Some("F11".into())),
        TerminalTabAction::ToggleMouseRouting => {
            let is_native = model
                .terminals
                .active
                .as_ref()
                .is_some_and(|t| t.mouse_routing == MouseRouting::Native);
            (
                if is_native {
                    "Native TUI mouse drag; click to select text"
                } else {
                    "Text selection; click to pass native drag to TUI"
                }
                .into(),
                Some("Ctrl+Shift+M".into()),
            )
        }
        TerminalTabAction::Help => ("Terminal help".into(), Some("F4".into())),
        TerminalTabAction::Hide => ("Hide terminal".into(), Some("F8".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, Terminal};

    fn paint(app: &mut App, width: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, 15)).unwrap();
        terminal
            .draw(|frame| {
                draw_tab_strip(frame, Rect::new(0, 4, width, 10), app, workspace_palette());
            })
            .unwrap();
    }

    fn click(app: &mut App, action: TerminalTabAction) {
        let rect = app
            .terminal_tab_hits
            .iter()
            .find(|(_, a)| *a == action)
            .unwrap()
            .0;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[tokio::test]
    async fn profile_dropdown_selects_independent_shell_and_esc_dismisses() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.config.terminal = forge_config::TerminalConfig::default();
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let first = app.terminals.active.as_ref().unwrap().id;
        paint(&mut app, 100);
        click(&mut app, TerminalTabAction::Profiles);
        assert!(matches!(
            app.session.context_menu.as_ref().map(|m| &m.target),
            Some(ContextTarget::TerminalProfiles { set_default: false })
        ));
        for _ in 0..100 {
            app.poll_shell_discovery();
            if app.shell_profiles.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let profiles = app
            .shell_profiles
            .as_ref()
            .expect("shell discovery finished");
        assert!(!profiles.is_empty());
        let menu = app.session.context_menu.as_ref().unwrap();
        assert!(menu.labels.contains_key(&ContextAction::ShellProfile(0)));
        assert!(menu
            .items
            .iter()
            .any(|i| i.action == ContextAction::ShellDefaultMenu));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.session.context_menu.is_none());
        assert_eq!(app.terminals.active.as_ref().unwrap().id, first);
        app.handle_key(KeyEvent::new(
            KeyCode::F(8),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert!(app.session.context_menu.is_some());
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.session.context_menu.is_none());
        assert_ne!(app.terminals.active.as_ref().unwrap().id, first);
        assert_eq!(app.terminals.inactive().len(), 1);
        assert_eq!(app.terminals.inactive()[0].id, first);
    }

    #[tokio::test]
    async fn tab_bar_mouse_routing_is_per_terminal_and_keyboard_accessible() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let first = app.terminals.active.as_ref().unwrap().id;
        assert_eq!(
            app.terminals.active.as_ref().unwrap().mouse_routing,
            MouseRouting::Native
        );
        paint(&mut app, 95);
        let button = app
            .terminal_tab_hits
            .iter()
            .find(|(_, action)| *action == TerminalTabAction::ToggleMouseRouting)
            .unwrap()
            .0;
        assert!(button.width >= 5);
        click(&mut app, TerminalTabAction::ToggleMouseRouting);
        assert_eq!(
            app.terminals.active.as_ref().unwrap().mouse_routing,
            MouseRouting::SelectText
        );
        assert!(app.session.status.contains("select text"));
        app.handle_key(KeyEvent::new(
            KeyCode::Char('m'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(
            app.terminals.active.as_ref().unwrap().mouse_routing,
            MouseRouting::Native
        );

        // The preference belongs to the tab and cannot affect other panes.
        click(&mut app, TerminalTabAction::ToggleMouseRouting);
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        assert_ne!(app.terminals.active.as_ref().unwrap().id, first);
        assert_eq!(
            app.terminals.active.as_ref().unwrap().mouse_routing,
            MouseRouting::Native
        );
        paint(&mut app, 95);
        click(&mut app, TerminalTabAction::Activate(first));
        assert_eq!(
            app.terminals.active.as_ref().unwrap().mouse_routing,
            MouseRouting::SelectText
        );
    }

    #[tokio::test]
    async fn terminal_copy_notice_draws_and_expires_without_touching_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.session.status = "Unrelated operation".into();
        app.show_terminal_copy_notice("Copied to clipboard", false);
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let area = Rect::new(0, 1, 80, 10);
        terminal
            .draw(|frame| {
                draw_copy_notice(frame, area, &app, workspace_palette());
            })
            .unwrap();
        let row = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 1)].symbol())
            .collect::<String>();
        assert!(row.ends_with(" Copied to clipboard "));
        app.session
            .terminal_copy_notice
            .as_mut()
            .unwrap()
            .expires_at = std::time::Instant::now() - std::time::Duration::from_millis(1);
        app.expire_terminal_copy_notice();
        assert!(app.session.terminal_copy_notice.is_none());
        assert_eq!(app.session.status, "Unrelated operation");
        terminal
            .draw(|frame| {
                draw_copy_notice(frame, area, &app, workspace_palette());
            })
            .unwrap();
        let row = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 1)].symbol())
            .collect::<String>();
        assert!(!row.contains("Copied"));
    }

    #[tokio::test]
    async fn terminal_tooltip_appears_on_hover_and_clears_on_leave() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        paint(&mut app, 95);
        let new_button = app
            .terminal_tab_hits
            .iter()
            .find(|(_, action)| *action == TerminalTabAction::New)
            .unwrap()
            .0;
        let render = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(95, 15)).unwrap();
            terminal
                .draw(|frame| {
                    let area = Rect::new(0, 4, 95, 10);
                    draw_tab_strip(frame, area, app, workspace_palette());
                    // The tooltip is painted on top of the terminal's PTY output.
                    frame.render_widget(Paragraph::new("PTY OUTPUT"), Rect::new(0, 5, 95, 1));
                    draw_tab_tooltip(frame, area, app, workspace_palette());
                })
                .unwrap();
            (4..14)
                .map(|y| {
                    (0..95)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(!render(&mut app).contains("New terminal"));
        let before = app.terminals.active.as_ref().unwrap().id;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: new_button.x,
            row: new_button.y,
            modifiers: KeyModifiers::NONE,
        });
        let hovered = render(&mut app);
        assert!(hovered.contains("New terminal"), "{hovered}");
        assert!(hovered.contains("Shift+F8"), "{hovered}");
        assert_eq!(app.terminals.active.as_ref().unwrap().id, before);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 40,
            row: 9,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!render(&mut app).contains("New terminal"));
    }

    #[tokio::test]
    async fn terminal_tooltip_uses_full_tab_name_and_does_not_cover_modals() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        app.terminals.active.as_mut().unwrap().name = "very-long-terminal-label".into();
        let id = app.terminals.active.as_ref().unwrap().id;
        let (hint, key) = tab_action_hint(&app, TerminalTabAction::Activate(id));
        assert!(hint.contains("very-long-terminal-label"));
        assert_eq!(key.as_deref(), Some("Alt+1"));
        let (hint, key) = tab_action_hint(&app, TerminalTabAction::Hide);
        assert_eq!(hint, "Hide terminal");
        assert_eq!(key.as_deref(), Some("F8"));
        paint(&mut app, 80);
        let new_button = app
            .terminal_tab_hits
            .iter()
            .find(|(_, action)| *action == TerminalTabAction::New)
            .unwrap()
            .0;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: new_button.x,
            row: new_button.y,
            modifiers: KeyModifiers::NONE,
        });
        app.keys.open = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 15)).unwrap();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 4, 80, 10);
                draw_tab_strip(frame, area, &mut app, workspace_palette());
                draw_tab_tooltip(frame, area, &app, workspace_palette());
            })
            .unwrap();
        let rows = (4..14)
            .map(|y| {
                (0..80)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rows.contains("New terminal"));
    }

    #[tokio::test]
    async fn hovering_terminal_controls_changes_color_without_clicking() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let first = app.terminals.active.as_ref().unwrap().id;
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        let active_before = app.terminals.active.as_ref().unwrap().id;
        paint(&mut app, 95);
        let inactive_rect = app
            .terminal_tab_hits
            .iter()
            .find(|(_, action)| *action == TerminalTabAction::Activate(first))
            .unwrap()
            .0;
        let close_rect = app
            .terminal_tab_hits
            .iter()
            .find(|(_, action)| *action == TerminalTabAction::Close(first))
            .unwrap()
            .0;

        let painted_bg = |app: &mut App, col, row| {
            let mut terminal = Terminal::new(TestBackend::new(95, 15)).unwrap();
            terminal
                .draw(|frame| {
                    draw_tab_strip(frame, Rect::new(0, 4, 95, 10), app, workspace_palette());
                })
                .unwrap();
            terminal.backend().buffer()[(col, row)].bg
        };
        let moved = |app: &mut App, col, row| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: col,
                row,
                modifiers: KeyModifiers::NONE,
            });
        };
        assert_eq!(
            painted_bg(&mut app, inactive_rect.x, inactive_rect.y),
            workspace_palette().bg
        );
        moved(&mut app, inactive_rect.x, inactive_rect.y);
        assert_eq!(
            painted_bg(&mut app, inactive_rect.x, inactive_rect.y),
            workspace_palette().selection
        );
        assert_eq!(
            app.terminals.active.as_ref().unwrap().id,
            active_before,
            "hover must not activate tab"
        );
        moved(&mut app, close_rect.x, close_rect.y);
        assert_eq!(
            painted_bg(&mut app, close_rect.x, close_rect.y),
            workspace_palette().accent
        );
        assert_eq!(
            painted_bg(&mut app, inactive_rect.x, inactive_rect.y),
            workspace_palette().bg
        );
        moved(&mut app, 0, 0);
        assert_eq!(
            painted_bg(&mut app, close_rect.x, close_rect.y),
            workspace_palette().bg
        );
    }

    #[tokio::test]
    async fn click_switch_close_and_new_are_separate_targets() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let first = app.terminals.active.as_ref().unwrap().id;
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        let second = app.terminals.active.as_ref().unwrap().id;
        paint(&mut app, 95);
        let first_body = app
            .terminal_tab_hits
            .iter()
            .find(|(_, a)| *a == TerminalTabAction::Activate(first))
            .unwrap()
            .0;
        let first_close = app
            .terminal_tab_hits
            .iter()
            .find(|(_, a)| *a == TerminalTabAction::Close(first))
            .unwrap()
            .0;
        assert_eq!(first_body.right(), first_close.x);
        click(&mut app, TerminalTabAction::Activate(first));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, first);
        paint(&mut app, 95);
        click(&mut app, TerminalTabAction::Close(second));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, first);
        assert!(app.terminals.inactive().is_empty());
        paint(&mut app, 95);
        click(&mut app, TerminalTabAction::New);
        assert_ne!(app.terminals.active.as_ref().unwrap().id, first);
        assert_eq!(app.terminals.inactive().len(), 1);
        paint(&mut app, 95);
        click(&mut app, TerminalTabAction::Hide);
        assert!(!app.terminals.active.as_ref().unwrap().is_visible());
    }

    #[tokio::test]
    async fn alt_digits_select_stable_tab_numbers_and_help_returns_to_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        let first = app.terminals.active.as_ref().unwrap().id;
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        let second = app.terminals.active.as_ref().unwrap().id;
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        let third = app.terminals.active.as_ref().unwrap().id;
        app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, first);
        app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, third);
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(app.terminals.active.as_ref().unwrap().id, second);
        app.handle_key(KeyEvent::new(KeyCode::Char('8'), KeyModifiers::ALT));
        assert_eq!(
            app.terminals.active.as_ref().unwrap().id,
            second,
            "missing numbered tab must be no-op"
        );

        let footer = footer_shortcuts(&app, 120);
        assert!(footer.iter().any(|s| s.key == "Alt+1-8"));
        assert!(footer.iter().any(|s| s.key == "F4" && s.label == "Help"));
        app.handle_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
        assert!(app.session.help_open);
        assert!(!app.terminals.active.as_ref().unwrap().is_focused());
        assert!(app.terminal_help_return);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.session.help_open);
        assert!(app.terminals.active.as_ref().unwrap().is_focused());
        assert!(!app.terminal_help_return);
    }

    #[tokio::test]
    async fn help_button_opens_terminal_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        paint(&mut app, 110);
        click(&mut app, TerminalTabAction::Help);
        assert!(app.session.help_open);
        assert!(!app.session.help_reference);
        let text = terminal_guide(workspace_palette(), 90)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Alt+1..8"));
        assert!(text.contains("Shift+F8"));
        assert!(text.contains("F4"));
    }

    fn render_help(app: &mut App) {
        let mut t = Terminal::new(TestBackend::new(110, 36)).unwrap();
        t.draw(|frame| {
            draw_help_guide(frame, Rect::new(5, 2, 100, 30), app, workspace_palette());
        })
        .unwrap();
    }

    fn help_mouse(app: &mut App, x: u16, y: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[tokio::test]
    async fn help_from_terminal_question_mark_is_closable_by_mouse_and_keyboard() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        // Real route: click '?' in the terminal tab strip.
        paint(&mut app, 110);
        click(&mut app, TerminalTabAction::Help);
        assert!(app.session.help_open);
        assert!(app.terminal_help_return);
        render_help(&mut app);
        let close = app.session.help_close_rect;
        assert!(close.width > 0);
        help_mouse(&mut app, close.x, close.y);
        assert!(!app.session.help_open);
        assert!(app.terminals.active.as_ref().unwrap().is_focused());

        paint(&mut app, 110);
        click(&mut app, TerminalTabAction::Help);
        render_help(&mut app);
        help_mouse(&mut app, 0, 0); // Clicking outside overlay closes it.
        assert!(!app.session.help_open);
        assert!(app.terminals.active.as_ref().unwrap().is_focused());

        // A stale file-find must not steal Esc when Help owns keyboard focus.
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = true;
        paint(&mut app, 110);
        click(&mut app, TerminalTabAction::Help);
        assert!(app.session.help_open);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.session.help_open);
        assert!(app.terminals.active.as_ref().unwrap().is_focused());
    }

    #[tokio::test]
    async fn help_header_mouse_switches_reference_without_closing_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        paint(&mut app, 110);
        click(&mut app, TerminalTabAction::Help);
        render_help(&mut app);
        let reference = app.session.help_reference_rect;
        assert!(reference.width > 0);
        help_mouse(&mut app, reference.x, reference.y);
        assert!(app.session.help_reference);
        assert!(app.session.help_open);
        render_help(&mut app);
        let overview = app.session.help_overview_rect;
        help_mouse(&mut app, overview.x, overview.y);
        assert!(!app.session.help_reference);
        assert!(app.session.help_open);
    }

    #[tokio::test]
    async fn narrow_tab_strip_never_overflows_and_keeps_active_clickable() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
        for _ in 0..6 {
            app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT));
        }
        let current = app.terminals.active.as_ref().unwrap().id;
        for width in [10, 20, 36, 60, 120] {
            paint(&mut app, width);
            assert!(
                app.terminal_tab_hits
                    .iter()
                    .any(|(_, a)| *a == TerminalTabAction::Activate(current)),
                "active tab not visible at {width}"
            );
            for (rect, _) in &app.terminal_tab_hits {
                assert!(
                    rect.right() <= width,
                    "tab hit target exceeds {width}: {rect:?}"
                );
                assert_eq!(rect.y, 4);
            }
        }
    }
}

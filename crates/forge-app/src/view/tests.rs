//! Workspace rendering and interaction regression tests.
    #[test]
    fn blame_inline_degrades_without_overwriting_code() {
        let full = "Test User · 4d · abcd1234 fix navigation";
        assert_eq!(compact_blame_label(full, "Test User", "4d", 60).as_deref(), Some(full));
        assert_eq!(compact_blame_label(full, "Test User", "4d", 16).as_deref(), Some("Test User · 4d"));
        assert_eq!(compact_blame_label(full, "Test User", "4d", 9).as_deref(), Some("Test User"));
        assert_eq!(compact_blame_label(full, "Test User", "4d", 5), None);
    }

    #[test]
    fn overview_mapping_stays_in_bounds() {
        assert_eq!(overview_marker_y(0, 100, 10), 0);
        assert_eq!(overview_marker_y(99, 100, 10), 9);
        assert_eq!(overview_marker_y(50, 100, 10), 4);
        assert_eq!(overview_marker_y(200, 100, 10), 9);
        assert_eq!(overview_marker_y(0, 0, 0), 0);
    }

    #[test]
    fn scrollbar_position_top_middle_bottom_and_no_overflow() {
        let palette = workspace_palette();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(12, 12)).unwrap();
        let area = Rect::new(2, 1, 8, 10);
        let capture = |terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>, first, total| {
            terminal.draw(|frame| VerticalScrollbar::new(area, first, total).draw(frame, palette.bg, palette.fg_dim, palette.accent)).unwrap();
            (1..=10).map(|y| terminal.backend().buffer()[(9, y)].symbol().to_owned()).collect::<Vec<_>>()
        };
        let top = capture(&mut terminal, 0, 100);
        assert_eq!(top[0], "┃");
        assert_eq!(top[1], "│");
        let middle = capture(&mut terminal, 45, 100);
        assert_eq!(middle[5], "┃");
        let bottom = capture(&mut terminal, 90, 100);
        assert_eq!(bottom[9], "┃");
        // A short document has no track at all.
        let empty = capture(&mut terminal, 0, 10);
        assert!(empty.iter().all(|s| s != "┃" && s != "│"));
    }

    #[test]
    fn theme_presets_have_distinct_backgrounds_and_current_preserves_legacy() {
        assert_eq!(theme_palette(None).bg, workspace_palette().bg);
        assert_eq!(theme_palette(Some("current")).bg, workspace_palette().bg);
        let backgrounds: std::collections::HashSet<_> =
            ["vscode-dark", "vscode-black", "github-dark", "nord"]
                .into_iter()
                .map(|theme| format!("{:?}", theme_palette(Some(theme)).bg))
                .collect();
        assert_eq!(backgrounds.len(), 4);
        assert_eq!(theme_palette(Some("vscode-black")).bg, Color::Rgb(8, 8, 8));
    }

    use super::*;
    use ratatui_kit::test_util::render_frames;

    #[tokio::test]
    async fn content_context_layout_highlights_crops_and_keeps_mouse_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.rs");
        std::fs::write(
            &path,
            format!(
                "before-context\n{}needle[界]\nafter-context\n",
                "字".repeat(600)
            ),
        )
        .unwrap();
        for width in [130, 70, 24] {
            let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
            app.open_tools(tools::PanelKind::Content);
            let panel = app.tools.panel.as_mut().unwrap();
            panel.input = tui_input::Input::new("needle[界]".into());
            panel.rows = vec![tools::ToolRow {
                label: "code.rs:2 needle[界]".into(),
                value: tools::RowValue::File(path.clone(), 2),
            }];
            app.tools.context_path = Some(path.clone());
            app.tools.content_context = Some(Ok(forge_fs::workspace::ContentContext::load(
                &path,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap()));
            // Cancel the empty initial search; this frame represents completed results.
            app.tools.pending_read = None;
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 26)).unwrap();
            terminal
                .draw(|frame| draw_tools(frame, frame.area(), &mut app, Palette::default()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
            if width >= 70 {
                assert!(screen.contains("before-context"), "{screen}");
                assert!(screen.contains("after-context"));
                assert!(screen.contains("needle["));
                assert!(screen.contains(":2:601"));
                assert!(buffer
                    .content()
                    .iter()
                    .any(|cell| cell.bg == Palette::default().warning));
            }
            let list = app.tools.panel.as_ref().unwrap().list_area;
            app.handle_tools_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: list.x,
                row: list.y,
                modifiers: KeyModifiers::NONE,
            });
            assert_eq!(app.tools.panel.as_ref().unwrap().selected, 0);
            app.tools.panel.as_mut().unwrap().rows.clear();
            terminal
                .draw(|frame| draw_tools(frame, frame.area(), &mut app, Palette::default()))
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(screen.contains("Type to search") || width < 30);
        }
    }

    fn fixture(root: &Path) -> App {
        let mut app = create_workspace_at(ColorMode::TrueColor, root.to_owned());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.files.children.insert(
            root.to_owned(),
            vec![Entry {
                id: forge_model::PathId(1),
                name: "中文文件.rs".into(),
                path: root.join("中文文件.rs"),
                kind: EntryKind::File,
            }],
        );
        app.rebuild_visible();
        app.preview_state.preview = Some(Preview {
            path: root.join("中文文件.rs"),
            content: PreviewContent::Text {
                lines: vec!["fn main() {}".into()],
                truncated: false,
                bytes_read: 12,
            },
        });
        app
    }

    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        use unicode_width::UnicodeWidthStr;
        let mut skip = 0;
        buffer
            .content()
            .iter()
            .filter_map(|cell| {
                if skip > 0 {
                    skip -= 1;
                    return None;
                }
                skip = cell.symbol().width().saturating_sub(1);
                Some(cell.symbol())
            })
            .collect()
    }

    fn markdown_fixture(root: &Path) -> App {
        let mut app = fixture(root);
        let entry = &mut app.files.children.get_mut(root).unwrap()[0];
        entry.name = "README.MD".into();
        entry.path = root.join("README.MD");
        app.rebuild_visible();
        app.preview_state.preview = Some(Preview {
            path: root.join("README.MD"),
            content: PreviewContent::Text {
                lines: vec!["# Markdown 标题".into(), "".into(), "**bold** body".into()],
                truncated: false,
                bytes_read: 42,
            },
        });
        app
    }

    #[tokio::test]
    async fn footer_git_chip_shows_upstream_counts_and_opens_sync_panel() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.scm.git_status = Some(StatusSnapshot::new(dir.path().to_owned()));
        app.scm.branch = "dev".into();
        app.tools.sync = "dev...origin/dev [ahead 1]".into();

        for width in [100u16, 55, 35] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 12)).unwrap();
            let mut hitbox = Rect::default();
            terminal
                .draw(|frame| {
                    hitbox = draw_workspace_footer(
                        frame,
                        Rect::new(0, 9, width, 3),
                        &app,
                        workspace_palette(),
                    );
                })
                .unwrap();
            assert!(hitbox.width > 0);
            assert!(hitbox.right() <= width);
            assert_eq!(hitbox.y, 11);
            let line = (0..width)
                .map(|x| terminal.backend().buffer()[(x, 11)].symbol())
                .collect::<String>();
            assert!(line.contains("dev"), "{line}");
            assert!(
                line.contains("↓0") && line.contains("↑1"),
                "tracking counts must remain visible even at width {width}: {line}"
            );
        }

        let chip = git_status_chip(&app, workspace_palette(), 38).unwrap();
        assert_eq!(
            chip.0
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            " ⎇ dev ↓0 ↑1 ↻ "
        );
        app.session.layout.git_sync_status = Rect::new(0, 11, chip.1 as u16, 1);
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            app.tools.panel.as_ref().map(|p| &p.kind),
            Some(tools::PanelKind::Git(forge_git::GitQuery::Sync))
        ));
        assert!(
            app.scm.pending_write.is_none(),
            "click must NOT push or pull"
        );
    }

    #[tokio::test]
    async fn footer_git_chip_handles_no_upstream_and_narrow_or_non_git_windows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.scm.git_status = Some(StatusSnapshot::new(dir.path().to_owned()));
        app.scm.branch = "new-branch".into();
        app.tools.sync = "new-branch".into();
        let (line, width) = git_status_chip(&app, workspace_palette(), 35).unwrap();
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(text.contains("new-branch") && text.contains("↻"));
        assert!(
            !text.contains("↑") && !text.contains("↓"),
            "no upstream isn't zero divergence"
        );
        assert!(width <= 35);
        app.tools.sync = "new-branch...origin/new-branch [gone]".into();
        let (line, _) = git_status_chip(&app, workspace_palette(), 40).unwrap();
        assert!(line.spans.iter().any(|s| s.content.contains("gone")));
        assert!(git_status_chip(&app, workspace_palette(), 9).is_none());
        app.scm.git_status = None;
        assert!(git_status_chip(&app, workspace_palette(), 40).is_none());
        app.scm.branch.clear();
        assert!(git_status_chip(&app, workspace_palette(), 40).is_none());
    }

    #[tokio::test]
    async fn diff_heading_open_file_is_clickable_and_keeps_directory_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("中文文件.rs"), "new source\n").unwrap();
        let mut app = fixture(dir.path());
        let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
        snapshot.push_change(
            "中文文件.rs",
            forge_model::GitStatus::Modified,
            DiffSide::Unstaged,
        );
        app.scm.git_status = Some(snapshot);
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::Preview;
        app.scm.scm_selected = scm_jump_to_column(&app.scm_rows(), StagingColumn::Unstaged);
        app.set_file_diff(FileDiff {
            full_new_lines: None,
            repo_root: dir.path().to_owned(),
            path: "中文文件.rs".into(),
            side: DiffSide::Unstaged,
            lines: vec![],
        });
        app.preview_state.preview_mode = PreviewMode::Diff;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| {
                let _ = draw_preview_heading(
                    frame,
                    Rect::new(40, 2, 75, 22),
                    &mut app,
                    workspace_palette(),
                );
            })
            .unwrap();
        let chip = app.session.layout.open_file_tab;
        assert_eq!(chip.width, 14);
        let first_line = (chip.x..chip.right())
            .map(|x| terminal.backend().buffer()[(x, chip.y)].symbol())
            .collect::<String>();
        assert!(first_line.contains("Open file"), "{first_line}");
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: chip.x + 3,
            row: chip.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.session.activity, ActivityTab::Files);
        assert_eq!(
            app.files.visible[app.files.selected].entry.path,
            dir.path().join("中文文件.rs")
        );
    }

    #[test]
    fn scrollbar_geometry_matches_thumb_and_bounds() {
        assert_eq!(VerticalScrollbar::new(Rect::new(0, 0, 1, 10), 0, 10).geometry(), None);
        assert_eq!(VerticalScrollbar::new(Rect::new(0, 0, 1, 2), 0, 100).geometry(), None);
        let (thumb, top, max_scroll) = VerticalScrollbar::new(Rect::new(0, 0, 1, 20), 0, 200).geometry().unwrap();
        assert_eq!(thumb, 2);
        assert_eq!(top, 0);
        assert_eq!(max_scroll, 180);
        assert_eq!(VerticalScrollbar::new(Rect::new(0, 0, 1, 20), 180, 200).geometry().unwrap().1, 18);
        assert_eq!(VerticalScrollbar::new(Rect::new(0, 0, 1, 20), 999, 200).geometry().unwrap().1, 18);
    }

    #[tokio::test]
    async fn preview_scrollbar_click_drag_release_and_clamp() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.preview_state.preview = Some(Preview {
            path: dir.path().join("中文文件.rs"),
            content: PreviewContent::Text {
                lines: (0..200).map(|i| format!("line {i}")).collect(),
                truncated: false,
                bytes_read: 1600,
            },
        });
        app.session.layout.preview = Rect::new(30, 2, 70, 24);
        app.session.layout.preview_inner = Rect::new(31, 3, 67, 20);
        app.session.layout.preview_scrollbar = Rect::new(99, 3, 1, 20);
        let mouse = |kind, column, row| MouseEvent {
            kind, column, row, modifiers: KeyModifiers::NONE,
        };
        // Clicking the bottom track should jump to the last visible page.
        app.handle_mouse_content(mouse(MouseEventKind::Down(MouseButton::Left), 99, 22));
        assert_eq!(app.preview_state.preview_scroll, 180);
        assert!(matches!(app.scrollbar_drag, Some((ScrollbarTarget::Preview, _))));
        // Dragging outside the 1-column lane must keep scrolling.
        app.handle_mouse_content(mouse(MouseEventKind::Drag(MouseButton::Left), 90, 3));
        assert_eq!(app.preview_state.preview_scroll, 0);
        app.handle_mouse_content(mouse(MouseEventKind::Drag(MouseButton::Left), 101, 99));
        assert_eq!(app.preview_state.preview_scroll, 180);
        app.handle_mouse_content(mouse(MouseEventKind::Up(MouseButton::Left), 101, 99));
        assert!(app.scrollbar_drag.is_none());
        // Releasing the mouse ends capture; subsequent drags don't scroll.
        app.handle_mouse_content(mouse(MouseEventKind::Drag(MouseButton::Left), 101, 3));
        assert_eq!(app.preview_state.preview_scroll, 180);
        assert!(!app.preview_state.selection.is_dragging());
    }

    #[tokio::test]
    async fn files_sidebar_scrollbar_captures_far_horizontal_drag() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        // A long tree is necessary: a list fitting entirely in the viewport
        // has no thumb and should not consume mouse events.
        let row = app.files.visible[0].clone();
        app.files.visible = vec![row; 200];
        app.files.follow_selection = true;
        app.session.layout.list = Rect::new(0, 2, 30, 24);
        app.session.layout.list_inner = Rect::new(0, 3, 29, 20);
        app.session.layout.sidebar_scrollbar = Rect::new(29, 2, 1, 24);
        let event = |kind, column, row| MouseEvent {
            kind, column, row, modifiers: KeyModifiers::NONE,
        };
        // The scrollbar takes control and disables follow-selection.
        app.handle_mouse(event(MouseEventKind::Down(MouseButton::Left), 29, 25));
        assert!(matches!(app.scrollbar_drag, Some((ScrollbarTarget::Sidebar, _))));
        assert!(!app.files.follow_selection);
        assert_eq!(app.files.scroll, 176);
        // Pointer is now far away, over the preview and then the header.
        app.handle_mouse(event(MouseEventKind::Drag(MouseButton::Left), 110, 1));
        assert_eq!(app.files.scroll, 0);
        app.handle_mouse(event(MouseEventKind::Drag(MouseButton::Left), 0, 300));
        assert_eq!(app.files.scroll, 176);
        // Release outside the sidebar ends the global capture.
        app.handle_mouse(event(MouseEventKind::Up(MouseButton::Left), 110, 300));
        assert!(app.scrollbar_drag.is_none());
    }

    #[tokio::test]
    async fn git_sidebar_scrollbar_reuses_same_drag_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut status = StatusSnapshot::new(dir.path().to_owned());
        for i in 0..120 {
            status.push_change(
                format!("file-{i:03}.rs"),
                forge_model::GitStatus::Modified,
                DiffSide::Unstaged,
            );
        }
        app.scm.git_status = Some(status);
        app.session.activity = ActivityTab::Git;
        app.scm.follow_selection = true;
        let total = app.scm_rows().len();
        assert!(total > 25);
        app.session.layout.list = Rect::new(0, 2, 30, 24);
        app.session.layout.list_inner = Rect::new(0, 3, 29, 20);
        app.session.layout.sidebar_scrollbar = Rect::new(29, 2, 1, 24);
        let event = |kind, column, row| MouseEvent {
            kind, column, row, modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(event(MouseEventKind::Down(MouseButton::Left), 29, 25));
        assert!(!app.scm.follow_selection);
        assert!(app.scm.scroll_scm > 0);
        app.handle_mouse(event(MouseEventKind::Drag(MouseButton::Left), 110, 0));
        assert_eq!(app.scm.scroll_scm, 0);
        app.handle_mouse(event(MouseEventKind::Up(MouseButton::Left), 110, 0));
        assert!(app.scrollbar_drag.is_none());
    }

    #[tokio::test]
    async fn git_diff_scrollbar_moves_shared_split_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.activity = ActivityTab::Git;
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.preview_state.file_diff = Some(FileDiff {
            full_new_lines: None,
            repo_root: dir.path().to_owned(),
            path: "中文文件.rs".into(),
            side: DiffSide::Unstaged,
            lines: vec![],
        });
        app.preview_state.diff_row_count = 240;
        app.session.layout.preview = Rect::new(30, 2, 70, 24);
        app.session.layout.preview_inner = Rect::new(31, 3, 67, 20);
        app.session.layout.preview_scrollbar = Rect::new(99, 3, 1, 20);
        let event = |kind, y| MouseEvent {
            kind, column: 99, row: y, modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_content(event(MouseEventKind::Down(MouseButton::Left), 22));
        assert_eq!(app.preview_state.preview_scroll, 220);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        app.handle_mouse_content(event(MouseEventKind::Drag(MouseButton::Left), 3));
        assert_eq!(app.preview_state.preview_scroll, 0);
        app.handle_mouse_content(event(MouseEventKind::Up(MouseButton::Left), 3));
        assert!(app.scrollbar_drag.is_none());
    }

    #[tokio::test]
    async fn markdown_tabs_switch_source_with_mouse_and_keyboard() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = markdown_fixture(dir.path());
        assert!(app.markdown_available());
        assert!(matches!(
            preview_content(&app).1,
            PreviewView::Markdown { .. }
        ));
        assert!(app.context_shortcuts().iter().any(|s| s.key == "F3"));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                let body = draw_preview_heading(frame, frame.area(), &mut app, workspace_palette());
                assert_eq!(body, Rect::new(0, 2, 40, 8));
            })
            .unwrap();
        let tab = app.session.layout.markdown_code_tab;
        assert_eq!(tab.y, 0);
        assert!(app.session.layout.markdown_preview_tab.x >= 40 / 2 - 4);
        assert!(text(terminal.backend().buffer()).contains("F3"));
        app.preview_state.preview_scroll = 10;
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x + 1,
            row: tab.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.preview_state.markdown_code);
        assert_eq!(app.preview_state.preview_scroll, 0);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(matches!(
            preview_content(&app).1,
            PreviewView::Text {
                language: TextLang::Markdown,
                ..
            }
        ));
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
        // Clicking the selected tab preserves the current position.
        let tab = app.session.layout.markdown_preview_tab;
        app.preview_state.preview_scroll = 2;
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x + 1,
            row: tab.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.preview_state.preview_scroll, 2);
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
        assert!(!app.markdown_available());
        app = fixture(dir.path());
        assert!(!app.markdown_available());
        app.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        assert!(!app.preview_state.markdown_code);
    }

    #[tokio::test]
    async fn markdown_renders_preview_and_code_at_all_workspace_sizes() {
        let dir = tempfile::tempdir().unwrap();
        for (width, height) in [(120, 24), (50, 14), (24, 8), (8, 4), (1, 1)] {
            for code in [false, true] {
                let mut app = markdown_fixture(dir.path());
                app.session.focus = PaneFocus::Preview;
                app.preview_state.markdown_code = code;
                let buffer =
                    render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
                if width >= 50 {
                    let screen = text(&buffer);
                    assert!(
                        screen.contains("Preview") && screen.contains("Code"),
                        "{screen}"
                    );
                    assert_eq!(screen.contains("# Markdown"), code, "{screen}");
                    assert_eq!(screen.contains("**bold**"), code, "{screen}");
                }
                if width >= 24 {
                    let heading_y = if height >= 12 { 2 } else { 1 };
                    let footer_height = if height >= 12 { 3 } else { 2 };
                    let pane_height = height - heading_y - footer_height;
                    let heading = (0..width)
                        .map(|x| buffer[(x, heading_y)].symbol())
                        .collect::<String>();
                    assert!(heading.contains("Code"), "{heading}");
                    // The first content line follows the usual heading gap.
                    let content_y = heading_y + 1 + u16::from(pane_height >= 6);
                    let first_content = (0..width)
                        .map(|x| buffer[(x, content_y)].symbol())
                        .collect::<String>();
                    assert!(first_content.contains("Markdown"), "{first_content}");
                }
            }
        }
    }

    #[tokio::test]
    async fn markdown_reveal_uses_source_lines_and_refresh_clears_layout() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = markdown_fixture(dir.path());
        let path = dir.path().join("README.MD");
        app.tools.reveal = Some((path.clone(), 3));
        let preview = app.preview_state.preview.clone().unwrap();
        app.apply_preview_result(path, Ok(Ok(preview)));
        assert!(app.preview_state.markdown_code);
        assert_eq!(app.preview_state.preview_scroll, 2);
        assert_eq!(app.preview_source_line(), 3);
        app.set_markdown_code(false);
        app.preview_state.markdown.ensure(
            &["text ".repeat(50)],
            false,
            250,
            12,
            &forge_theme::Theme::default(),
        );
        let total = app.preview_total_lines();
        assert!(total > 3);
        app.session.layout.preview_inner = Rect::new(0, 0, 12, 3);
        app.scroll_preview(isize::MAX);
        assert_eq!(app.preview_state.preview_scroll, total - 3);
        assert_eq!(app.preview_source_line(), 1);
        app.request_preview_for_selection();
        assert_eq!(app.preview_state.markdown.line_count(), 0);
    }

    #[tokio::test]
    async fn sticky_title_click_jumps_to_directory_without_replacing_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let original_preview = app.preview_state.preview.clone();
        let mut file = app.files.visible[0].clone();
        file.depth = 1;
        let folder = TreeRow {
            entry: Entry {
                id: forge_model::PathId(77),
                name: "src".into(),
                path: dir.path().join("src"),
                kind: EntryKind::Directory,
            },
            depth: 0,
            expanded: true,
        };
        app.files.visible = vec![folder, file];
        app.files.selected = 1;
        app.files.scroll = 1;
        app.files.sticky_headers = vec![0];
        app.session.layout.list = Rect::new(0, 2, 30, 15);
        app.session.layout.list_inner = Rect::new(0, 2, 30, 15);
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.files.selected, 0);
        assert_eq!(app.files.scroll, 0);
        assert_eq!(app.preview_state.preview, original_preview);
        assert!(
            app.files.expanded.is_empty(),
            "sticky click should not toggle folder"
        );
    }

    #[tokio::test]
    async fn files_view_omits_sidebar_count_and_footer_position() {
        let dir = tempfile::tempdir().unwrap();
        let app = fixture(dir.path());
        let count = app.files.visible.len();
        assert!(count > 0);
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 120, 30, 3);
        let screen = text(&buffer);
        assert!(screen.contains("F1  Files"), "{screen}");
        assert!(!screen.contains(&format!("Files  {count}")), "{screen}");
        assert!(!screen.contains("Files  1 /"), "{screen}");
    }

    #[tokio::test]
    async fn workspace_uses_one_divider_and_neutral_full_width_selection() {
        let dir = tempfile::tempdir().unwrap();
        let app = fixture(dir.path());
        let palette = theme_palette(app.config.theme.as_deref());
        let buffer = render_frames(
            element!(Workspace(initial: Some(app))),
            120,
            30,
            3,
        );
        let screen = text(&buffer);
        assert!(screen.contains("Files"), "{screen}");
        assert!(screen.contains("中文文件.rs"), "{screen}");
        assert!(screen.contains("fn main() {}"), "{screen}");
        assert!(
            buffer.content().iter().any(|cell| cell.fg == palette.accent),
            "focused accents should use the configured palette {:?} (marker {:?})", palette.accent, buffer[(0, 2)].fg
        );
        assert_eq!(buffer[(1, 3)].bg, palette.bg);
        assert_eq!(buffer[(30, 4)].symbol(), "│");
        assert_eq!(buffer[(0, 2)].symbol(), "›");
        assert_ne!(buffer[(29, 2)].bg, palette.bg, "selected row must have visible contrast");
        assert!(!screen.contains('┌') && !screen.contains('┐'), "{screen}");
        assert!(!buffer.content().iter().any(|cell| cell.bg == Color::Cyan));
    }

    #[tokio::test]
    async fn sidebar_history_renders_lanes_badges_and_commit_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let head = forge_git::GraphCommit {
            id: "a".repeat(40),
            short: "aaaaaaa".into(),
            parents: vec!["b".repeat(40)],
            refs: vec![
                forge_git::GraphRef {
                    name: "main".into(),
                    kind: forge_git::GraphRefKind::LocalBranch,
                },
                forge_git::GraphRef {
                    name: "HEAD".into(),
                    kind: forge_git::GraphRefKind::Head,
                },
            ],
            author: "fxh".into(),
            time: 1_700_000_000,
            subject: "feat: sidebar history".into(),
        };
        let base = forge_git::GraphCommit {
            id: "b".repeat(40),
            short: "bbbbbbb".into(),
            parents: Vec::new(),
            refs: Vec::new(),
            author: "fxh".into(),
            time: 1_699_000_000,
            subject: "chore: base".into(),
        };
        let oid = head.id.clone();
        app.session.activity = ActivityTab::Git;
        app.history.expanded = true;
        app.history.seed(vec![head, base]);
        app.history.files.insert(
            oid.clone(),
            vec![forge_git::CommitChange {
                path: "crates/forge-ui/src/graph.rs".into(),
                old_path: None,
                kind: forge_git::CommitChangeKind::Modified,
            }],
        );
        app.history.expanded_commits.insert(oid.clone());
        for dir in ["crates", "crates/forge-ui", "crates/forge-ui/src"] {
            app.history
                .expanded_dirs
                .insert(forge_ui::dir_key(&oid, dir));
        }

        let buffer = render_frames(element!(Workspace(initial: Some(app))), 140, 32, 3);
        let screen = text(&buffer);
        assert!(screen.contains("HISTORY"), "{screen}");
        // Subjects are truncated to the pane width, so match a prefix.
        assert!(screen.contains("feat: sidebar hi"), "{screen}");
        assert!(screen.contains("chore: base"), "{screen}");
        assert!(screen.contains("main"), "{screen}");
        assert!(
            screen.contains("crates") && screen.contains("forge-ui") && screen.contains("graph.rs"),
            "{screen}"
        );
        assert!(screen.contains('●'), "{screen}");
    }

    #[tokio::test]
    async fn footer_advertises_the_history_entry_in_both_tabs() {
        let dir = tempfile::tempdir().unwrap();
        for width in [100, 110, 130] {
            let app = fixture(dir.path());
            let screen = text(&render_frames(
                element!(Workspace(initial: Some(app))),
                width,
                30,
                3,
            ));
            let footer = screen.lines().find(|l| l.contains("Help")).unwrap_or("");
            assert!(footer.contains("G History"), "files w={width}: {footer}");
        }
        // The Git footer is full of contextual hints; the history entry must not
        // push them out.
        let mut app = fixture(dir.path());
        app.enter_git_sidebar();
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            30,
            3,
        ));
        let footer = screen.lines().find(|l| l.contains("Help")).unwrap_or("");
        assert!(footer.contains("Select a file"), "{footer}");
        let mut app = fixture(dir.path());
        assert!(app.history.expanded);
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        assert!(app.history.expanded);
    }

    #[tokio::test]
    async fn narrow_workspace_shows_focused_preview_and_handles_tiny_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 45, 12, 3);
        assert!(text(&buffer).contains("fn main() {}"));
        for (w, h) in [(1, 1), (1, 12), (20, 3), (40, 12), (79, 10), (80, 12)] {
            let _ = render_frames(
                element!(Workspace(initial: Some(fixture(dir.path())))),
                w,
                h,
                3,
            );
        }
    }

    #[tokio::test]
    async fn footer_shortcut_row_follows_file_find_keyboard_focus() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = true;
        app.preview_state.find.path = dir.path().join("中文文件.rs");

        let keys = footer_shortcuts(&app, 120)
            .into_iter()
            .map(|shortcut| shortcut.key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            ["Enter", "Shift+Enter", "Alt+C", "Alt+W", "Alt+R", "Esc"]
        );
        assert!(file_find_has_focus(&app));
        let rendered = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            120, 28, 3,
        ));
        for text in [
            "Enter", "Next", "Shift+Enter", "Prev", "Alt+C", "Case",
            "Alt+W", "Word", "Alt+R", "Regex", "Esc", "Close",
        ] {
            assert!(rendered.contains(text), "missing {text} in {rendered}");
        }

        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        app.preview_state.find.open = true;
        app.preview_state.find.input_active = true;
        let compact = footer_shortcuts(&app, 65);
        assert!(compact.iter().any(|key| key.key == "Enter"));
        assert!(compact.iter().any(|key| key.key == "Shift+Enter"));
        assert!(compact.iter().any(|key| key.key == "Esc"));
        assert!(compact.iter().all(|key| key.key != "F4" && key.key != "?"));

        app.preview_state.find.input_active = false;
        assert!(!file_find_has_focus(&app));
        let unfocused = footer_shortcuts(&app, 120);
        assert!(unfocused.iter().any(|key| key.key == "Ctrl-F"));
        assert!(unfocused.iter().any(|key| key.key == "?"));

        app.preview_state.find.input_active = true;
        app.preview_state.find.open = false;
        assert!(!file_find_has_focus(&app));
    }

    #[tokio::test]
    async fn footer_preserves_errors_and_changes_shortcuts_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.status = "commit failed: rejected".into();
        app.session.activity = ActivityTab::Git;
        let palette = theme_palette(app.config.theme.as_deref());
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 100, 25, 3);
        let screen = text(&buffer);
        assert!(screen.contains("commit failed: rejected"), "{screen}");
        assert!(
            screen.contains("Select a file") && screen.contains("? Help"),
            "{screen}"
        );
        assert!(!screen.contains("s/u"), "{screen}");
        assert_eq!(buffer[(2, 23)].fg, palette.error);
        let mut app = fixture(dir.path());
        app.session.focus = PaneFocus::Preview;
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 80, 20, 3);
        let screen = text(&buffer);
        assert!(
            screen.contains("j/k") && screen.contains("Scroll"),
            "{screen}"
        );
        assert!(screen.contains("? Help"), "{screen}");
    }

    #[tokio::test]
    async fn files_and_git_keep_separate_previews_without_rendering_directory_details() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let original = app.preview_state.preview.clone();
        app.preview_state.preview_scroll = 2;
        app.files.children.get_mut(dir.path()).unwrap().insert(
            0,
            Entry {
                id: forge_model::PathId(3),
                name: "src".into(),
                path: dir.path().join("src"),
                kind: EntryKind::Directory,
            },
        );
        app.rebuild_visible();
        app.files.selected = 0; // Deliberately select a directory.
        app.enter_git_sidebar();
        assert!(matches!(app.preview_state.preview_mode, PreviewMode::Diff));
        app.preview_state.preview = None;
        app.leave_git_sidebar();
        assert_eq!(app.preview_state.preview, original);
        assert_eq!(app.preview_state.preview_scroll, 2);
        assert!(app.files.pending_lists.is_empty());
        assert!(app.preview_state.pending_preview.is_none());
    }

    #[tokio::test]
    async fn compact_commands_and_narrow_tab_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.open_tools(tools::PanelKind::Commands);
        let panel = app.tools.panel.as_mut().unwrap();
        panel.input = tui_input::Input::new("stage".into());
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 120, 30, 3);
        let lines: Vec<String> = (0..30)
            .map(|row| {
                (0..120)
                    .map(|x| buffer[(x, row)].symbol())
                    .collect::<String>()
            })
            .collect();
        let title = lines
            .iter()
            .position(|line| line.contains("Commands"))
            .unwrap();
        assert!(
            title > 5,
            "command palette should be compact: title at row {title}"
        );

        let app = fixture(dir.path());
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 75, 24, 3);
        let footer: String = (0..75).map(|x| buffer[(x, 23)].symbol()).collect();
        assert!(
            footer.contains("Tab") && footer.contains("Preview"),
            "{footer}"
        );
    }

    #[tokio::test]
    async fn files_and_git_tabs_show_active_state_and_accept_mouse_and_function_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        terminal
            .draw(|frame| {
                draw_workspace_header(frame, Rect::new(0, 0, 90, 2), &mut app, workspace_palette())
            })
            .unwrap();
        let screen = text(terminal.backend().buffer());
        assert!(screen.contains("F1  Files") && screen.contains("F2  Git"));
        let files = app.session.layout.files_tab;
        let git = app.session.layout.git_tab;
        assert_eq!(
            terminal.backend().buffer()[(files.x, files.y)].bg,
            workspace_palette().selection
        );
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: git.x + 1,
            row: git.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.in_git_sidebar());
        terminal
            .draw(|frame| {
                draw_workspace_header(frame, Rect::new(0, 0, 90, 2), &mut app, workspace_palette())
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(git.x, git.y)].bg,
            workspace_palette().selection
        );
        assert_ne!(
            terminal.backend().buffer()[(files.x, files.y)].bg,
            workspace_palette().selection
        );
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: files.x + 1,
            row: files.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert!(app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(!app.in_git_sidebar());
        assert!(app
            .context_shortcuts()
            .iter()
            .all(|s| s.key != "s" && s.key != "u"));
    }

    #[tokio::test]
    async fn commit_modal_and_context_menu_render_under_input_layers() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.commit_open = true;
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            25,
            3,
        ));
        assert!(
            screen.contains("STAGED") && screen.contains("SUBJECT") && screen.contains("review"),
            "{screen}"
        );
        let mut app = fixture(dir.path());
        app.open_file_context_menu(0, 5, 5);
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            25,
            3,
        ));
        assert!(screen.contains("Copy path"), "{screen}");
        for (width, height) in [(1, 1), (12, 5)] {
            let mut app = fixture(dir.path());
            app.open_file_context_menu(0, 50, 50);
            let _ = render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
        }
    }

    #[tokio::test]
    async fn command_and_confirmation_dialogs_render_including_tiny_windows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.open_tools(tools::PanelKind::Commands);
        let screen = text(&render_frames(
            element!(Workspace(initial:Some(app))),
            100,
            30,
            3,
        ));
        assert!(screen.contains("Commands"), "{screen}");
        assert!(screen.contains("Find file"), "{screen}");
        for (width, height) in [(1, 1), (12, 5), (40, 10), (80, 24)] {
            let mut app = fixture(dir.path());
            app.open_tools(tools::PanelKind::Commands);
            let panel = app.tools.panel.as_mut().unwrap();
            panel.input = tui_input::Input::new("中文中文中文中文中文中文".into());
            let _ = render_frames(element!(Workspace(initial:Some(app))), width, height, 3);
            let mut app = fixture(dir.path());
            app.run_command(tools::Command::Delete);
            let _ = render_frames(element!(Workspace(initial:Some(app))), width, height, 3);
        }
    }

    #[tokio::test]
    async fn footer_names_file_folder_and_hunk_actions_even_in_narrow_windows() {
        let dir = tempfile::tempdir().unwrap();
        for (side, focus, folder, expected) in [
            (DiffSide::Unstaged, PaneFocus::List, false, "Stage file"),
            (DiffSide::Staged, PaneFocus::List, false, "Unstage file"),
            (DiffSide::Unstaged, PaneFocus::List, true, "Stage folder"),
            (DiffSide::Staged, PaneFocus::List, true, "Unstage folder"),
            (DiffSide::Unstaged, PaneFocus::Preview, false, "Stage hunk"),
            (DiffSide::Staged, PaneFocus::Preview, false, "Unstage hunk"),
        ] {
            for width in [32, 45, 80, 120] {
                let mut app = fixture(dir.path());
                app.session.status.clear();
                app.session.activity = ActivityTab::Git;
                app.session.focus = focus;
                let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
                let path = if folder {
                    "folder/中文文件.rs"
                } else {
                    "中文文件.rs"
                };
                snapshot.push_change(path, forge_model::GitStatus::Modified, side);
                app.scm.git_status = Some(snapshot);
                app.scm.scm_selected = if folder {
                    app.scm_rows()
                        .iter()
                        .position(
                            |r| matches!(r, ScmRow::Folder { column, .. } if column.side() == side),
                        )
                        .unwrap()
                } else {
                    scm_jump_to_column(
                        &app.scm_rows(),
                        if side == DiffSide::Unstaged {
                            StagingColumn::Unstaged
                        } else {
                            StagingColumn::Staged
                        },
                    )
                };
                app.preview_state.preview_mode = PreviewMode::Diff;
                app.preview_state.hunk_index = Some(0);
                app.set_file_diff(FileDiff {
                    full_new_lines: None,
                    repo_root: dir.path().to_owned(),
                    path: "中文文件.rs".into(),
                    side,
                    lines: vec![forge_model::DiffLine {
                        kind: forge_model::DiffLineKind::Header,
                        text: "@@ -1 +1 @@".into(),
                    }],
                });
                let buffer = render_frames(element!(Workspace(initial: Some(app))), width, 20, 3);
                let footer: String = (0..width).map(|x| buffer[(x, 19)].symbol()).collect();
                assert!(
                    footer.contains(expected) && footer.contains("? Help"),
                    "{width}: {footer}"
                );
                assert!(!footer.contains("s/u") && !footer.contains("sync"));
            }
        }
    }

    #[tokio::test]
    async fn help_wraps_and_renders_at_small_sizes_without_changing_focus() {
        let dir = tempfile::tempdir().unwrap();
        for (width, height) in [(120, 35), (45, 12), (20, 5), (1, 1)] {
            let mut app = fixture(dir.path());
            app.session.help_open = true;
            app.session.focus = PaneFocus::Preview;
            let buffer = render_frames(element!(Workspace(initial: Some(app))), width, height, 3);
            if width >= 45 {
                assert!(text(&buffer).contains("QUICK START"));
            }
        }
        assert!(wrap_help(30).len() > wrap_help(100).len());
        let mut app = fixture(dir.path());
        app.session.help_open = true;
        app.session.help_reference = true;
        let reference = render_frames(element!(Workspace(initial: Some(app))), 95, 28, 3);
        assert!(text(&reference).contains("REFERENCE"));
        assert!(text(&reference).contains("Stage:"));
    }

    #[tokio::test]
    async fn git_marks_count_each_side() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let path = "中文文件.rs";
        let mut snapshot = StatusSnapshot::new(dir.path().to_owned());
        snapshot.push_change(path, forge_model::GitStatus::Modified, DiffSide::Unstaged);
        snapshot.push_change(path, forge_model::GitStatus::Modified, DiffSide::Staged);
        snapshot.finish();
        app.scm.git_status = Some(snapshot);
        app.session.activity = ActivityTab::Git;
        app.session.focus = PaneFocus::List;
        app.scm.scm_selected = 1;
        for side in [DiffSide::Unstaged, DiffSide::Staged] {
            app.tools.marks.git.insert(crate::tools::GitMark {
                path: dir.path().join(path),
                side,
            });
        }
        assert_eq!(app.tools.marks.len(), 2);
        let buffer = render_frames(element!(Workspace(initial: Some(app))), 110, 30, 3);
        let screen = text(&buffer);
        assert!(screen.contains("CHANGES (1)"), "{screen}");
        assert!(screen.contains("STAGED (1)"), "{screen}");
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "●")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn stale_preview_is_rejected_and_modes_keep_file_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.apply_preview_result(
            dir.path().join("old.rs"),
            Ok(Ok(Preview::error(dir.path().join("old.rs"), "stale"))),
        );
        assert_eq!(
            app.preview_state.preview.as_ref().unwrap().path,
            dir.path().join("中文文件.rs")
        );
        app.enter_git_sidebar();
        app.leave_git_sidebar();
        assert_eq!(app.files.selected, 0);
        assert_eq!(app.selected_entry().unwrap().name, "中文文件.rs");
    }

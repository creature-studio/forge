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
    async fn unwrapped_preview_renders_horizontal_scrollbar_only_when_needed() {
        let dir = tempfile::tempdir().unwrap();
        let render = |wrap, line: &str| {
            let mut app = fixture(dir.path());
            app.config.wrap_code = wrap;
            app.session.focus = PaneFocus::Preview;
            app.preview_state.preview.as_mut().unwrap().content = PreviewContent::Text {
                lines: vec![line.to_owned()],
                truncated: false,
                bytes_read: line.len(),
            };
            let buffer = render_frames(
                element!(Workspace(initial: Some(app))), 110, 30, 3,
            );
            text(&buffer)
        };
        let wide = render(false, &"very_long_column_".repeat(20));
        assert!(wide.contains('━'), "unwrapped overflow must paint the thumb");
        let wrapped = render(true, &"very_long_column_".repeat(20));
        assert!(!wrapped.contains('━'), "wrapping must hide the horizontal bar");
        let short = render(false, "short");
        assert!(!short.contains('━'), "no overflow must not steal a content row");
    }

    #[tokio::test]
    async fn file_preview_horizontal_scrollbar_click_drag_release_and_clamp() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.config.wrap_code = false;
        app.session.layout.preview = Rect::new(30, 2, 70, 24);
        app.session.layout.preview_inner = Rect::new(31, 3, 67, 19);
        app.session.layout.preview_horizontal_scrollbar = Rect::new(36, 22, 62, 1);
        app.session.layout.preview_horizontal_max = 180;
        app.session.focus = PaneFocus::List;
        let event = |kind, column, row| MouseEvent {
            kind, column, row, modifiers: KeyModifiers::NONE,
        };

        // Track click centers the thumb and immediately moves the viewport.
        app.handle_mouse_content(event(MouseEventKind::Down(MouseButton::Left), 97, 22));
        assert_eq!(app.preview_state.preview_scroll_x, 180);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(matches!(app.scrollbar_drag,
            Some((ScrollbarTarget::PreviewHorizontal, _))));
        assert!(!app.preview_state.selection.is_dragging());
        // Capture stays alive far outside the lane, in either direction.
        app.handle_mouse_content(event(MouseEventKind::Drag(MouseButton::Left), 0, 90));
        assert_eq!(app.preview_state.preview_scroll_x, 0);
        app.handle_mouse_content(event(MouseEventKind::Drag(MouseButton::Left), 999, 0));
        assert_eq!(app.preview_state.preview_scroll_x, 180);
        app.handle_mouse_content(event(MouseEventKind::Up(MouseButton::Left), 0, 90));
        assert_eq!(app.preview_state.preview_scroll_x, 0);
        assert!(app.scrollbar_drag.is_none());
        app.handle_mouse_content(event(MouseEventKind::Drag(MouseButton::Left), 999, 0));
        assert_eq!(app.preview_state.preview_scroll_x, 0);
        assert!(!app.preview_state.selection.is_dragging());

        // A hidden/disabled bar is not a mouse target.
        app.session.layout.preview_horizontal_scrollbar = Rect::default();
        app.session.layout.preview_horizontal_max = 0;
        app.handle_mouse_content(event(MouseEventKind::Down(MouseButton::Left), 97, 22));
        assert!(app.scrollbar_drag.is_none());
    }

    #[tokio::test]
    async fn git_diff_horizontal_scrollbar_shares_preview_offset_and_capture() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.session.activity = ActivityTab::Git;
        app.preview_state.preview_mode = PreviewMode::Diff;
        app.session.layout.preview = Rect::new(30, 2, 70, 24);
        app.session.layout.preview_inner = Rect::new(31, 3, 67, 19);
        app.session.layout.preview_horizontal_scrollbar = Rect::new(31, 22, 68, 1);
        app.session.layout.preview_horizontal_max = 100;
        let event = |kind, column| MouseEvent {
            kind, column, row: 22, modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(event(MouseEventKind::Down(MouseButton::Left), 98));
        assert_eq!(app.preview_state.preview_scroll_x, 100);
        assert!(matches!(app.scrollbar_drag,
            Some((ScrollbarTarget::PreviewHorizontal, _))));
        app.handle_mouse(event(MouseEventKind::Drag(MouseButton::Left), 31));
        assert_eq!(app.preview_state.preview_scroll_x, 0);
        app.handle_mouse(event(MouseEventKind::Up(MouseButton::Left), 31));
        assert!(app.scrollbar_drag.is_none());
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
                if width >= 28 { // Activity Bar reserves four columns
                    let heading_y = 1; // compact single-row workspace header
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
        assert!(screen.contains("EXPLORER"), "{screen}");
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
        assert!(screen.contains("EXPLORER"), "{screen}");
        assert!(screen.contains("中文文件.rs"), "{screen}");
        assert!(screen.contains("fn main() {}"), "{screen}");
        assert!(
            buffer.content().iter().any(|cell| cell.fg == palette.accent),
            "focused accents should use the configured palette {:?} (marker {:?})", palette.accent, buffer[(0, 2)].fg
        );
        assert_eq!(buffer[(1, 3)].bg, palette.bg);
        assert_eq!(buffer[(35, 4)].symbol(), "│", "single Sidebar divider");
        assert!(screen.contains('›'), "selected row should retain focus marker");
        assert!(buffer.content().iter().any(|cell| cell.bg == palette.selection),
            "selected row must have visible contrast");
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
        assert!(screen.contains("feat: sideb"), "{screen}");
        assert!(screen.contains("chore: base"), "{screen}");
        assert!(screen.contains("main"), "{screen}");
        assert!(
            screen.contains("crates") && screen.contains("graph.rs"),
            "{screen}"
        );
        assert!(screen.contains('●'), "{screen}");
    }

    #[tokio::test]
    async fn footer_shows_local_actions_instead_of_global_history_entry() {
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
            assert!(footer.contains("Tab Preview") && !footer.contains("G History")
                && !footer.contains("Ctrl-P"), "files w={width}: {footer}");
        }
        // Git list advertises navigation in that list, not global commands.
        let mut app = fixture(dir.path());
        app.enter_git_sidebar();
        let screen = text(&render_frames(
            element!(Workspace(initial: Some(app))),
            100,
            30,
            3,
        ));
        let footer = screen.lines().find(|l| l.contains("Help")).unwrap_or("");
        assert!(footer.contains("Tab Preview") && !footer.contains("G History"), "{footer}");
        let mut app = fixture(dir.path());
        assert!(app.history.expanded);
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
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
        assert!(unfocused.iter().any(|key| key.key == "Esc"));
        assert!(unfocused.iter().all(|key| key.key != "Ctrl-F"));

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
            screen.contains("Tab Preview") && screen.contains("F5 Help"),
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
        assert!(screen.contains("F5 Help"), "{screen}");
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
    async fn shared_form_buttons_match_keyboard_submit_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("remove-me.txt"), "fixture").unwrap();
        let mut app = fixture(dir.path());
        let paint = |app: &mut App| {
            let mut term = ratatui::Terminal::new(
                ratatui::backend::TestBackend::new(90, 22)).unwrap();
            term.draw(|frame| draw_tools(
                frame, Rect::new(5, 5, 78, 12), app, workspace_palette()
            )).unwrap();
            term.backend().buffer().content().iter()
                .map(|cell| cell.symbol()).collect::<String>()
        };
        let click = |app: &mut App, target: Rect| {
            app.handle_tools_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: target.x + target.width / 2, row: target.y,
                modifiers: KeyModifiers::NONE,
            });
        };
        // Canceling a destructive confirmation must not invoke a job.
        let path = dir.path().join("remove-me.txt");
        app.tools.marks.files.insert(path.clone());
        app.run_command(tools::Command::Delete);
        let screen = paint(&mut app);
        assert!(screen.contains("Delete items") && screen.contains("Enter delete"), "{screen}");
        assert!(screen.contains("Esc cancel"));
        let cancel = app.tools.panel.as_ref().unwrap().form_cancel;
        let submit = app.tools.panel.as_ref().unwrap().form_submit;
        assert!(cancel.width > 0 && submit.width > 0);
        click(&mut app, cancel);
        assert!(app.tools.pending_task.is_none());
        assert!(path.exists());
        assert!(app.tools.panel.is_none());

        // Prompt mouse Submit calls exactly the same validation as Enter.
        app.prompt(tools::PromptAction::OpenWorkspace, "Open workspace",
            String::new(), "Enter a directory path");
        let screen = paint(&mut app);
        assert!(screen.contains("Path") && screen.contains("Enter open"));
        let prompt_submit = app.tools.panel.as_ref().unwrap().form_submit;
        click(&mut app, prompt_submit);
        assert!(app.tools.panel.as_ref().unwrap().message.contains("required"));
        let screen = paint(&mut app);
        assert!(screen.contains("Directory path is required"));
        let prompt_cancel = app.tools.panel.as_ref().unwrap().form_cancel;
        click(&mut app, prompt_cancel);
        assert!(app.tools.panel.is_none());

        // The destructive action still requires an explicit confirmation,
        // and mouse Confirm runs exactly the same queued write as Enter.
        app.run_command(tools::Command::Delete);
        paint(&mut app);
        let confirmed = app.tools.panel.as_ref().unwrap().form_submit;
        click(&mut app, confirmed);
        assert!(app.tools.pending_task.is_some(),
            "mouse confirmation must start the normal checked write");
        for _ in 0..80 {
            app.poll_tools();
            if !path.exists() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!path.exists(), "the confirmed fixture deletion must finish");
    }

    #[test]
    fn rename_form_is_compact_with_focused_field_and_semantic_actions() {
        let file = std::path::PathBuf::from("old-name.rs");
        let mut panel = tools::Panel::new(
            tools::PanelKind::Prompt(tools::PromptAction::Rename(file)), "Rename");
        panel.input = tui_input::Input::new("src/new-name.rs".into());
        panel.detail = "New path relative to workspace · no overwrite".into();
        let palette = workspace_palette();
        let size = dialog::DialogSize::tools(&panel, (112, 30));
        assert_eq!(size.width, 53);
        assert_eq!(size.height, 7);
        let area = Rect::new((112 - size.width) / 2, (30 - size.height) / 2,
            size.width, size.height);
        let mut term = ratatui::Terminal::new(
            ratatui::backend::TestBackend::new(112, 30)).unwrap();
        term.draw(|f| dialog::draw_form(f, area, &mut panel,
            palette, dialog::FormMode::Prompt)).unwrap();
        let display = term.backend().buffer().content().iter()
            .map(|c| c.symbol()).collect::<String>();
        for label in ["Rename", "New path", "src/new-name.rs",
            "New path relative to workspace", "Esc cancel", "Enter rename"] {
            assert!(display.contains(label), "missing {label}: {display}");
        }
        assert!(panel.form_cancel.width > 0 && panel.form_submit.width > 0);
        assert!(area.contains(ratatui::layout::Position::new(
            panel.form_submit.x, panel.form_submit.y)));
        let buf = term.backend().buffer();
        assert_eq!(buf[(panel.form_submit.x + 1, panel.form_submit.y)].fg, palette.accent);
        assert_eq!(buf[(panel.form_cancel.x + 1, panel.form_cancel.y)].fg, palette.accent);
        assert_eq!(buf[(area.x + 2, area.y + 3)].symbol(), "›",
            "keyboard focus is a caret, not a second bordered text field");
        assert_eq!(buf[(area.x + 4, area.y + 3)].bg, palette.bg,
            "native TUI forms should not simulate filled web inputs");
    }

    #[test]
    fn form_variants_share_layout_and_have_specific_primary_labels() {
        for (kind, title, action) in [
            (tools::PanelKind::Prompt(tools::PromptAction::NewFile),
                "New file", "Enter create"),
            (tools::PanelKind::Prompt(tools::PromptAction::OpenWorkspace),
                "Open workspace", "Enter open"),
        ] {
            let mut panel = tools::Panel::new(kind, title);
            panel.detail = "Path relative to workspace".into();
            let size = dialog::DialogSize::tools(&panel, (72, 24));
            let mut term = ratatui::Terminal::new(
                ratatui::backend::TestBackend::new(72, 24)).unwrap();
            term.draw(|f| dialog::draw_form(
                f, Rect::new(3, 3, size.width, size.height),
                &mut panel, workspace_palette(), dialog::FormMode::Prompt,
            )).unwrap();
            let display = term.backend().buffer().content().iter()
                .map(|c| c.symbol()).collect::<String>();
            assert!(display.contains(action));
        }
    }

    #[test]
    fn form_dialog_narrow_and_long_instructions_preserve_button_bounds() {
        let mut panel = tools::Panel::new(
            tools::PanelKind::Prompt(tools::PromptAction::OpenWorkspace), "Open workspace");
        panel.detail = "长说明，输入路径后确认。".repeat(30);
        for (w, h) in [(100, 30), (45, 15), (25, 8), (8, 3)] {
            let size = dialog::DialogSize::tools(&panel, (w, h));
            assert!(size.width <= w && size.height <= h);
            let mut terminal = ratatui::Terminal::new(
                ratatui::backend::TestBackend::new(w, h)).unwrap();
            let area = Rect::new((w - size.width) / 2, (h - size.height) / 2,
                size.width, size.height);
            terminal.draw(|frame| dialog::draw_form(frame, area, &mut panel,
                workspace_palette(), dialog::FormMode::Prompt)).unwrap();
            if panel.form_submit.width > 0 {
                assert!(area.contains(ratatui::layout::Position::new(
                    panel.form_submit.x, panel.form_submit.y)));
                assert!(area.contains(ratatui::layout::Position::new(
                    panel.form_cancel.x, panel.form_cancel.y)));
            }
        }
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
    async fn collapsing_sidebar_expands_preview_without_removing_activity_bar() {
        let dir = tempfile::tempdir().unwrap();
        let visible = fixture(dir.path());
        let mut collapsed = fixture(dir.path());
        collapsed.set_sidebar_visible(false);
        let visible_buffer = render_frames(element!(Workspace(initial: Some(visible))), 120, 30, 3);
        let collapsed_buffer = render_frames(element!(Workspace(initial: Some(collapsed))), 120, 30, 3);
        let line = |buffer: &ratatui::buffer::Buffer| (0..120)
            .map(|x| buffer[(x, 1)].symbol()).collect::<String>();
        let before = line(&visible_buffer);
        let after = line(&collapsed_buffer);
        assert!(before.contains("EXPLORER"), "{before}");
        assert!(!after.contains("EXPLORER"), "{after}");
        assert!(before.contains("Preview") && after.contains("Preview"));
        assert!(after.find("Preview").unwrap() + 20 < before.find("Preview").unwrap(),
            "Preview should reclaim Sidebar columns: {before:?} => {after:?}");
        assert_eq!(collapsed_buffer[(2, 2)].symbol(), forge_ui::Icons::activity(forge_ui::ActivityIcon::Files), "Activity Bar must remain");
    }

    #[tokio::test]
    async fn sidebar_drag_and_toggle_preserve_files_and_git_scroll_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        app.files.scroll = 7;
        app.scm.scroll_scm = 4;
        app.session.layout.activity = Rect::new(0, 1, 4, 23);
        app.session.layout.sidebar_divider = Rect::new(34, 1, 1, 23);
        app.session.layout.window = Rect::new(0, 0, 120, 30);
        let mouse = |kind, column| MouseEvent { kind, column, row: 10, modifiers: KeyModifiers::NONE };
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 34));
        assert!(app.session.sidebar_resizing);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 44));
        assert_eq!(app.session.sidebar_width, 40);
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 44));
        assert!(!app.session.sidebar_resizing);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::ALT));
        assert_eq!(app.session.sidebar_width, 38);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::ALT));
        assert_eq!(app.session.sidebar_width, 40);
        app.set_sidebar_visible(false);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        assert!(footer_shortcuts(&app, 120).iter().any(|s| s.key == "Ctrl+B"));
        assert_eq!(app.files.scroll, 7);
        assert_eq!(app.scm.scroll_scm, 4);
        app.set_sidebar_visible(true);
        assert_eq!(app.session.sidebar_width, 40);
        assert_eq!(app.files.scroll, 7);
        assert_eq!(app.scm.scroll_scm, 4);
        // A collapsed panel must not capture Tab focus or keyboard scrolling.
        app.set_sidebar_visible(false);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.session.focus, PaneFocus::Preview);
    }

    #[tokio::test]
    async fn sidebar_header_collapse_button_uses_the_same_hit_region_as_hover() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 22)).unwrap();
        terminal.draw(|frame| {
            activity_bar::draw_activity_bar(frame, Rect::new(0, 1, 5, 19), &mut app, workspace_palette());
            activity_bar::draw_sidebar_heading(frame, Rect::new(5, 1, 30, 19), &mut app, workspace_palette());
        }).unwrap();
        let collapse = app.session.layout.sidebar_toggle;
        assert_eq!(app.activity_hits.action_at(collapse.x, collapse.y), Some(ActivityAction::Collapse));
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left),
            column: collapse.x, row: collapse.y, modifiers: KeyModifiers::NONE });
        assert!(app.session.sidebar_visible, "press alone does not navigate");
        app.handle_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left),
            column: collapse.x, row: collapse.y, modifiers: KeyModifiers::NONE });
        assert!(!app.session.sidebar_visible);
        assert_eq!(app.session.focus, PaneFocus::Preview);
    }

    #[tokio::test]
    async fn activity_bar_hover_and_search_settings_are_separate_from_terminal_pty() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 22)).unwrap();
        let paint = |app: &mut App, term: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
            term.draw(|frame| activity_bar::draw_activity_bar(
                frame, Rect::new(0, 1, 5, 19), app, workspace_palette())).unwrap();
        };
        let moved = |app: &mut App, rect: Rect| app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved, column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE
        });
        let click = |app: &mut App, rect: Rect| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE,
            });
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE,
            });
        };
        paint(&mut app, &mut terminal);
        let git = app.session.layout.git_tab;
        let search = app.session.layout.search_tab;
        let settings = app.session.layout.settings_tab;
        let terminal_icon = app.session.layout.terminal_activity;
        moved(&mut app, git);
        paint(&mut app, &mut terminal);
        assert_eq!(terminal.backend().buffer()[(git.x,git.y)].bg, workspace_palette().selection);
        assert!(!app.in_git_sidebar(), "hover does not switch activity");
        moved(&mut app, Rect::new(50,0,1,1));
        paint(&mut app, &mut terminal);
        assert_eq!(terminal.backend().buffer()[(git.x,git.y)].bg, workspace_palette().bg);
        click(&mut app, search);
        assert!(app.tools.panel.as_ref().is_some_and(|p| matches!(p.kind, tools::PanelKind::Content)));
        app.tools.panel = None;
        click(&mut app, settings);
        assert!(app.tools.panel.as_ref().is_some_and(|p| matches!(p.kind, tools::PanelKind::Settings)));
        app.tools.panel = None;
        click(&mut app, terminal_icon);
        let session_id = app.terminal.as_ref().expect("terminal opened").id;
        assert!(app.terminal.as_ref().unwrap().visible);
        click(&mut app, terminal_icon);
        assert!(!app.terminal.as_ref().unwrap().visible);
        assert_eq!(app.terminal.as_ref().unwrap().id, session_id, "terminal must not restart when hidden");
    }

    #[tokio::test]
    async fn activity_bar_switches_and_collapses_sidebar_without_losing_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = fixture(dir.path());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        let draw = |app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
            terminal.draw(|frame| {
                activity_bar::draw_activity_bar(frame, Rect::new(0, 1, 5, 16), app, workspace_palette());
            }).unwrap();
        };
        let click = |app: &mut App, rect: Rect| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE,
            });
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: rect.x, row: rect.y, modifiers: KeyModifiers::NONE,
            });
        };
        draw(&mut app, &mut terminal);
        let files = app.session.layout.files_tab;
        let git = app.session.layout.git_tab;
        assert_eq!(terminal.backend().buffer()[(files.x, files.y)].bg, workspace_palette().selection);
        click(&mut app, git);
        assert!(app.in_git_sidebar());
        assert!(app.session.sidebar_visible);
        draw(&mut app, &mut terminal);
        assert_eq!(terminal.backend().buffer()[(git.x, git.y)].bg, workspace_palette().selection);
        click(&mut app, git); // clicking active entry collapses Sidebar
        assert!(!app.session.sidebar_visible);
        assert_eq!(app.session.focus, PaneFocus::Preview);
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert!(app.session.sidebar_visible);
        assert!(app.in_git_sidebar());
        app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(!app.in_git_sidebar());
        assert!(app.session.sidebar_visible);
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(!app.session.sidebar_visible);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.session.focus, PaneFocus::Preview);
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(app.session.sidebar_visible);
        assert!(app.context_shortcuts().iter().all(|s| s.key != "s" && s.key != "u"));
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
                    app.scm_rows()
                        .iter()
                        .position(|r| matches!(r, ScmRow::Change { column, .. } if column.side() == side))
                        .expect("test must focus an actionable change row")
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
                // Draw just the footer: running Workspace's async refresh
                // would replace the synthetic Git snapshot before the assertion.
                let mut terminal = ratatui::Terminal::new(
                    ratatui::backend::TestBackend::new(width, 3)).unwrap();
                terminal.draw(|frame| {
                    footer::draw_workspace_footer(
                        frame, Rect::new(0, 0, width, 3), &app, workspace_palette());
                }).unwrap();
                let footer: String = (0..width)
                    .map(|x| terminal.backend().buffer()[(x, 2)].symbol())
                    .collect();
                assert!(
                    footer.contains(expected) && (width < 100 || footer.contains("F5 Help")),
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

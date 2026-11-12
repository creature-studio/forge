use super::*;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Blame Test")
        .env("GIT_AUTHOR_EMAIL", "blame@example.invalid")
        .env("GIT_COMMITTER_NAME", "Blame Test")
        .env("GIT_COMMITTER_EMAIL", "blame@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.md");
    git(dir.path(), &["init", "-q"]);
    std::fs::write(
        &path,
        format!("# {}\nsecond\nthird\n", "中文长行".repeat(20)),
    )
    .unwrap();
    git(dir.path(), &["add", "sample.md"]);
    git(
        dir.path(),
        &[
            "commit",
            "-qm",
            "first subject",
            "-m",
            "Complete commit message body.",
        ],
    );
    (dir, path)
}

async fn app_with_blame(root: &Path, path: &Path) -> App {
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    app.preview_state.preview = Some(forge_fs::preview_from_bytes(
        path,
        &std::fs::read(path).unwrap(),
        std::fs::metadata(path).unwrap().len(),
    ));
    app.preview_state.preview_loading = false;
    app.preview_state.markdown_code = true;
    app.session.focus = PaneFocus::Preview;
    let read = read_blame(
        app.services.git,
        root.to_owned(),
        path.to_owned(),
        source_hash(app.preview_state.preview.as_ref().unwrap()),
        None,
    )
    .await
    .unwrap();
    app.blame.active = Some(read.cached.clone());
    app.blame.cache.push(read.cached);
    app.blame.checked = Some((path.to_owned(), Instant::now()));
    measure(&mut app, 24);
    app
}

fn measure(app: &mut App, width: u16) {
    let Some(Preview {
        content:
            PreviewContent::Text {
                lines,
                truncated,
                bytes_read,
            },
        ..
    }) = app.preview_state.preview.as_ref()
    else {
        panic!("expected text");
    };
    app.preview_state.text_rows.ensure(
        &forge_theme::Theme::modern_ide(),
        lines,
        *truncated,
        *bytes_read,
        TextLang::Markdown,
        width,
    );
}

#[tokio::test]
async fn cache_reuses_reads_and_invalidates_on_content_and_head() {
    let (dir, path) = fixture();
    let git_service = GitService::new();
    let first = read_blame(git_service, dir.path().to_owned(), path.clone(), 0, None)
        .await
        .unwrap();
    let old_key = first.cached.key.clone();
    let mut sentinel = first.cached;
    sentinel.lines = Err("cached negative result".into());
    let second = read_blame(
        git_service,
        dir.path().to_owned(),
        path.clone(),
        0,
        Some(sentinel.clone()),
    )
    .await
    .unwrap();
    assert_eq!(second.cached.lines.unwrap_err(), "cached negative result");
    // A content fingerprint catches edits even if length or mtime are unchanged.
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("second", "edited");
    std::fs::write(&path, text).unwrap();
    let edited = read_blame(
        git_service,
        dir.path().to_owned(),
        path.clone(),
        0,
        Some(sentinel.clone()),
    )
    .await
    .unwrap();
    assert_ne!(edited.cached.key.content, old_key.content);
    assert!(edited.cached.lines.as_ref().unwrap()[1].uncommitted());
    let mut cached = edited.cached;
    cached.lines = Err("old HEAD cache".into());
    git(dir.path(), &["commit", "--allow-empty", "-qm", "move HEAD"]);
    let moved = read_blame(git_service, dir.path().to_owned(), path, 0, Some(cached))
        .await
        .unwrap();
    assert_ne!(moved.cached.key.head, old_key.head);
    assert!(moved.cached.lines.is_ok());
}

#[tokio::test]
async fn cache_invalidates_when_index_removes_an_unchanged_file() {
    let (dir, path) = fixture();
    let git_service = GitService::new();
    let first = read_blame(git_service, dir.path().to_owned(), path.clone(), 0, None)
        .await
        .unwrap();
    assert!(!first.cached.lines.as_ref().unwrap()[0].uncommitted());
    git(dir.path(), &["rm", "--cached", "sample.md"]);
    let untracked = read_blame(
        git_service,
        dir.path().to_owned(),
        path,
        0,
        Some(first.cached),
    )
    .await
    .unwrap();
    assert!(untracked.cached.lines.unwrap()[0].uncommitted());
}

#[tokio::test]
async fn markdown_code_cjk_continuations_clicks_and_scrolling_use_source_lines() {
    let (dir, path) = fixture();
    let mut app = app_with_blame(dir.path(), &path).await;
    let second = app.preview_state.text_rows.source_to_row(2);
    assert!(second > 2, "fixture must wrap CJK");
    for row in 0..second {
        app.preview_state.preview_scroll = row;
        assert_eq!(app.current_blame().unwrap().1, 1);
        app.tick_blame();
        assert!(
            app.blame.pending.is_none(),
            "cursor moves must reuse the cache"
        );
    }
    app.preview_state.preview_scroll = 0;
    app.blame.cursor = Some((path.clone(), second, 0));
    assert_eq!(app.current_blame().unwrap().1, 2);
    app.preview_state.preview_scroll = second + 1;
    assert_eq!(
        app.current_blame().unwrap().1,
        3,
        "scroll clears stale clicked-row origin"
    );
    app.preview_state.markdown_code = false;
    assert!(
        app.current_blame().is_none(),
        "formatted Markdown must never guess source offsets"
    );
    app.preview_state.markdown_code = true;
    app.preview_state.preview_scroll = 999;
    assert!(
        app.current_blame().is_none(),
        "non-source rows have no blame"
    );
}

#[tokio::test]
async fn popup_is_modal_and_escape_keeps_file_scroll_focus_and_line() {
    let (dir, path) = fixture();
    let mut app = app_with_blame(dir.path(), &path).await;
    app.preview_state.preview_scroll = 2;
    app.handle_key(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE));
    assert!(app.blame.popup.is_some());
    let source = app.current_blame().unwrap().1;
    for key in [KeyCode::Char('d'), KeyCode::Char('h'), KeyCode::F(6)] {
        app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        assert_eq!(app.preview_state.preview.as_ref().unwrap().path, path);
        assert_eq!(app.preview_state.preview_scroll, 2);
    }
    app.handle_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
    assert!(!app.keys.open, "popup owns shortcut input");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.blame.popup.is_none());
    assert_eq!(app.preview_state.preview.as_ref().unwrap().path, path);
    assert_eq!(app.preview_state.preview_scroll, 2);
    assert_eq!(app.current_blame().unwrap().1, source);
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert!(!app.session.should_quit);
}

#[tokio::test]
async fn popup_renders_and_scrolls_at_narrow_widths_without_losing_source_state() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let (dir, path) = fixture();
    for (width, height) in [(118, 32), (42, 12), (12, 5), (5, 2)] {
        let mut app = app_with_blame(dir.path(), &path).await;
        app.open_blame_popup();
        app.services.jobs.cancel_key("git:blame-popup");
        let popup = app.blame.popup.as_mut().unwrap();
        popup.pending = None;
        popup.text = format!("{}\nEND", "中文提交消息 ".repeat(1000));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                super::super::view::draw_blame_popup(
                    frame,
                    Rect::new(0, 0, width, height),
                    &mut app,
                    Palette::default(),
                )
            })
            .unwrap();
        if width > 10 && height > 4 {
            assert!(app.blame.popup.as_ref().unwrap().max_scroll > 0);
            app.handle_blame_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
            assert_eq!(
                app.blame.popup.as_ref().unwrap().scroll,
                app.blame.popup.as_ref().unwrap().max_scroll
            );
        }
        assert_eq!(app.preview_state.preview.as_ref().unwrap().path, path);
        assert_eq!(app.preview_state.preview_scroll, 0);
    }
}

#[tokio::test]
async fn uncommitted_config_switches_and_non_text_are_respected() {
    let (dir, path) = fixture();
    let mut app = app_with_blame(dir.path(), &path).await;
    let mut cached = app.blame.active.take().unwrap();
    let mut lines = cached.lines.unwrap().as_ref().clone();
    lines[0].oid = "0".repeat(40);
    cached.lines = Ok(Arc::new(lines));
    app.blame.active = Some(cached);
    assert!(app.current_blame().unwrap().0.uncommitted());
    app.config.git.blame.show_uncommitted = false;
    assert!(app.current_blame().is_none());
    app.config.git.blame.show_uncommitted = true;
    app.config.git.blame.show_popup = false;
    app.open_blame_popup();
    assert!(app.blame.popup.is_none());
    app.config.git.blame.enabled = false;
    assert!(app.blame_path().is_none());
    app.config.git.blame.enabled = true;
    for content in [
        PreviewContent::Empty,
        PreviewContent::Binary {
            size: 1,
            notice: "binary".into(),
        },
        PreviewContent::Directory {
            name: "dir".into(),
            children: vec![],
            truncated: false,
        },
    ] {
        app.preview_state.preview.as_mut().unwrap().content = content;
        assert!(app.blame_path().is_none());
        assert!(app.current_blame().is_none());
    }
}

#[tokio::test]
async fn blame_never_overwrites_a_partially_loaded_source_preview() {
    let (dir, path) = fixture();
    std::fs::write(&path,
        (0..2400).map(|i| format!("// very long source line {i:04} with padding padding padding\n"))
            .collect::<String>()
    ).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > forge_fs::PREVIEW_MAX_BYTES as u64);
    let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    app.preview_state.preview = Some(
        app.services.fs.preview_now(path.clone(), EntryKind::File).await.unwrap()
    );
    app.preview_state.preview_loading = false;
    app.preview_state.markdown_code = true;
    app.session.layout.preview_inner.height = 24;
    let before_offset = match &app.preview_state.preview.as_ref().unwrap().content {
        PreviewContent::Text { lines, truncated: true, bytes_read } => {
            assert_eq!(lines.len(), 400);
            *bytes_read
        }
        other => panic!("expected first partial page, got {other:?}"),
    };

    app.tick_blame();
    tokio::time::timeout(Duration::from_secs(7), async {
        while app.blame.pending.is_some() {
            app.poll_blame();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    let PreviewContent::Text { lines, bytes_read, .. } =
        &app.preview_state.preview.as_ref().unwrap().content else { panic!() };
    assert_eq!(lines.len(), 400, "blame must not install a truncated file snapshot");
    assert_eq!(*bytes_read, before_offset);

    // The second page must follow line 400, not jump to the 64KiB offset
    // of blame's independent snapshot.
    let page = forge_fs::read_text_page(path.clone(), *bytes_read as u64).await.unwrap();
    assert_eq!(page.lines[0], "// very long source line 0400 with padding padding padding");
    assert!(page.next_offset > *bytes_read as u64);
    assert!(app.blame.active.is_some(), "blame should stay available for source lines");
    assert!(app.blame.active.as_ref().unwrap().lines.as_ref().unwrap().len() >= 2400);
}

#[tokio::test]
async fn background_edit_refresh_installs_matching_preview_and_attribution() {
    let (dir, path) = fixture();
    let mut app = app_with_blame(dir.path(), &path).await;
    app.preview_state.preview_scroll = 2;
    let content = std::fs::read_to_string(&path)
        .unwrap()
        .replace("second", "changed");
    std::fs::write(&path, content).unwrap();
    app.blame.checked = None;
    app.tick_blame();
    tokio::time::timeout(Duration::from_secs(6), async {
        while app.blame.pending.is_some() {
            app.poll_blame();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    measure(&mut app, 24);
    assert_eq!(app.preview_state.preview_scroll, 2);
    assert!(app.blame.active.as_ref().unwrap().lines.as_ref().unwrap()[1].uncommitted());
    let PreviewContent::Text { lines, .. } = &app.preview_state.preview.as_ref().unwrap().content
    else {
        panic!();
    };
    assert_eq!(lines[1], "changed");
}

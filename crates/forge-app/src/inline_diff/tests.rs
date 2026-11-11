use super::*;

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn fixture(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    let path = dir.path().join("sample.md");
    std::fs::write(&path, text).unwrap();
    git(dir.path(), &["add", "sample.md"]);
    (dir, path)
}

async fn load(root: &Path, path: &Path) -> Read {
    read(GitService::new(), root.to_owned(), path.to_owned(), None)
        .await
        .unwrap()
}

async fn app(root: &Path, path: &Path) -> App {
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    let read = load(root, path).await;
    app.preview_state.preview = Some(read.fresh.clone());
    app.preview_state.preview_loading = false;
    app.preview_state.preview_mode = PreviewMode::File;
    app.preview_state.markdown_code = true;
    app.session.focus = PaneFocus::Preview;
    if let PreviewContent::Text {
        lines,
        truncated,
        bytes_read,
    } = &read.fresh.content
    {
        app.preview_state.text_rows.ensure(
            &forge_theme::Theme::modern_ide(),
            lines,
            *truncated,
            *bytes_read,
            TextLang::Markdown,
            24,
        );
    }
    app.inline_diff.active = Some(read);
    app
}

#[tokio::test]
async fn real_git_added_deleted_replaced_and_no_writes() {
    for (before, after, expected) in [
        ("one\n", "one\nadded\n", vec![(2, Change::Added)]),
        (
            "one\ntwo\nthree\n",
            "one\nthree\n",
            vec![(2, Change::Deleted)],
        ),
        ("one\ntwo\n", "one\n", vec![(1, Change::Deleted)]),
        ("one\n", "", vec![(1, Change::Deleted)]),
        (
            "one\nold\n",
            "one\nnew\nextra\n",
            vec![(2, Change::Modified), (3, Change::Modified)],
        ),
        ("--old", "++new", vec![(1, Change::Modified)]),
    ] {
        let (dir, path) = fixture(before);
        std::fs::write(&path, after).unwrap();
        let status = git(dir.path(), &["status", "--porcelain=v1"]);
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        let read = load(dir.path(), &path).await;
        assert_eq!(
            read.hunks
                .iter()
                .flat_map(|h| h.marks.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(std::fs::read(&path).unwrap(), after.as_bytes());
        assert_eq!(std::fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert_eq!(git(dir.path(), &["status", "--porcelain=v1"]), status);
        assert!(!dir.path().join(".git/index.lock").exists());
    }
}

#[tokio::test]
async fn untracked_binary_large_and_cache_invalidation() {
    let (dir, path) = fixture("before\n");
    std::fs::write(&path, "after\n").unwrap();
    let first = load(dir.path(), &path).await;
    // A sentinel proves the unchanged key reuses the cache without a query.
    let reused = read(
        GitService::new(),
        dir.path().to_owned(),
        path.clone(),
        Some((first.key.clone(), Vec::new())),
    )
    .await
    .unwrap();
    assert!(reused.hunks.is_empty());
    std::fs::write(&path, "changed again\n").unwrap();
    let next = read(
        GitService::new(),
        dir.path().to_owned(),
        path.clone(),
        Some((first.key.clone(), Vec::new())),
    )
    .await
    .unwrap();
    assert_ne!(first.key, next.key);
    assert!(!next.hunks.is_empty());
    git(dir.path(), &["add", "sample.md"]);
    assert!(load(dir.path(), &path).await.hunks.is_empty());
    let untracked = dir.path().join("untracked.txt");
    std::fs::write(&untracked, "new\n").unwrap();
    assert!(load(dir.path(), &untracked).await.hunks.is_empty());
    std::fs::write(&path, [0, 1, 0, 2]).unwrap();
    assert!(
        read(GitService::new(), dir.path().to_owned(), path.clone(), None)
            .await
            .is_err()
    );
    std::fs::write(&path, vec![b'x'; forge_fs::PREVIEW_MAX_BYTES + 1]).unwrap();
    assert!(read(GitService::new(), dir.path().to_owned(), path, None)
        .await
        .is_err());
}

#[tokio::test]
async fn multiple_hunks_and_git_display_preferences_keep_source_coordinates() {
    let original = (1..=40).map(|n| format!("line {n}\n")).collect::<String>();
    let (dir, path) = fixture(&original);
    git(dir.path(), &["config", "diff.suppressBlankEmpty", "true"]);
    git(dir.path(), &["config", "diff.outputIndicatorNew", ">"]);
    git(dir.path(), &["config", "diff.outputIndicatorOld", "<"]);
    std::fs::write(
        &path,
        original
            .replace("line 2\n", "中文第二行\n")
            .replace("line 35\n", "new 35\n"),
    )
    .unwrap();
    let mut app = app(dir.path(), &path).await;
    assert_eq!(app.inline_active().unwrap().hunks.len(), 2);
    assert_eq!(
        app.inline_active().unwrap().hunks[0].marks,
        [(2, Change::Modified)]
    );
    assert_eq!(
        app.inline_active().unwrap().hunks[1].marks,
        [(35, Change::Modified)]
    );
    app.preview_state.preview_scroll = app.preview_state.text_rows.source_to_row(30);
    app.toggle_inline_peek();
    assert_eq!(app.inline_diff.open, Some(1));
    assert!(app.inline_active().unwrap().hunks[1]
        .lines
        .iter()
        .any(|(kind, text)| *kind == DiffLineKind::Add && text == "+new 35"));
}

#[tokio::test]
async fn wrap_gutter_click_overlay_and_markdown_exclusion() {
    let (dir, path) = fixture("old\nsecond\nthird\n");
    std::fs::write(&path, format!("{}\nsecond\nthird\n", "中文".repeat(30))).unwrap();
    let mut app = app(dir.path(), &path).await;
    app.session.layout.preview_inner = Rect::new(10, 2, 24, 14);
    let second = app.preview_state.text_rows.source_to_row(2);
    assert!(second > 1);
    assert_eq!(
        app.preview_state.text_rows.source_line_at(second - 1),
        Some(1)
    );
    let click = |row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 11,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_content(click(2));
    assert_eq!(app.inline_diff.open, Some(0));
    assert!(matches!(app.preview_state.preview_mode, PreviewMode::File));
    let backend = ratatui::backend::TestBackend::new(40, 20);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|f| app.draw_inline_diff(f, app.session.layout.preview_inner))
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(10, 2)].fg, Color::Yellow);
    assert_ne!(
        buffer[(10, 3)].fg,
        Color::Yellow,
        "continuation must not be marked"
    );
    assert!(app.inline_diff.popup.width > 0);
    assert_eq!(app.preview_state.text_rows.source_to_row(2), second);
    let popup = app.inline_diff.popup;
    assert!(app.inline_mouse(MouseEvent {
        column: popup.right() - 1,
        row: popup.y,
        ..click(2)
    }));
    assert!(app.inline_diff.open.is_none());
    app.handle_mouse_content(click(2));
    assert_eq!(app.inline_diff.open, Some(0));
    app.handle_mouse_content(click(2));
    assert!(app.inline_diff.open.is_none());
    assert!(
        !app.inline_mouse(click(3)),
        "wrapped continuation is not a gutter target"
    );
    app.handle_key(KeyEvent::new(KeyCode::F(7), KeyModifiers::NONE));
    assert_eq!(app.inline_diff.open, Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.inline_diff.open.is_none());
    app.preview_state.markdown_code = false;
    assert!(app.inline_active().is_none());
    let mut rendered = false;
    terminal
        .draw(|f| {
            let before = f.buffer_mut().clone();
            app.draw_inline_diff(f, app.session.layout.preview_inner);
            rendered = before != *f.buffer_mut();
        })
        .unwrap();
    assert!(!rendered, "rendered Markdown buffer must remain unchanged");
    assert!(!app.inline_mouse(click(2)));
    app.preview_state.markdown_code = true;
    app.preview_state.preview_mode = PreviewMode::Diff;
    assert!(app.inline_active().is_none());
}

#[tokio::test]
async fn pending_result_guards_and_external_refresh() {
    let (dir, path) = fixture("old\n");
    std::fs::write(&path, "new\n").unwrap();
    let mut app = app(dir.path(), &path).await;
    for change_path in [true, false] {
        let expected = preview_hash(app.preview_state.preview.as_ref().unwrap()).unwrap();
        let read = load(dir.path(), &path).await;
        let handle = app
            .services
            .jobs
            .spawn(JobPriority::Low, None, async move { Ok(read) });
        app.inline_diff.pending = Some((path.clone(), expected, handle));
        app.inline_diff.active = None;
        let original = app.preview_state.preview.clone();
        if change_path {
            app.preview_state.preview.as_mut().unwrap().path = dir.path().join("other.md");
        } else {
            app.preview_state.preview =
                Some(forge_fs::preview_from_bytes(&path, b"other content\n", 14));
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while app.inline_diff.pending.is_some() {
                app.poll_inline_diff();
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(app.inline_diff.active.is_none());
        app.preview_state.preview = original;
    }
    std::fs::write(&path, "external edit\n").unwrap();
    app.inline_diff.checked = None;
    app.tick_inline_diff();
    tokio::time::timeout(Duration::from_secs(3), async {
        while app.inline_diff.pending.is_some() {
            app.poll_inline_diff();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        preview_hash(app.preview_state.preview.as_ref().unwrap()),
        preview_hash(&load(dir.path(), &path).await.fresh)
    );
    assert!(app.inline_active().is_some());
    app.reset_inline_diff();
    assert!(app.inline_diff.active.is_none());
    assert!(app.inline_diff.pending.is_none());
}

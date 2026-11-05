use super::*;

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-qb", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.test"]);
    std::fs::create_dir_all(root.join("folder/nested")).unwrap();
    for path in ["a.rs", "folder/nested/b.rs", "outside.txt"] {
        std::fs::write(root.join(path), "base\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    for path in ["a.rs", "folder/nested/b.rs", "outside.txt"] {
        std::fs::write(root.join(path), "changed\n").unwrap();
    }
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    (dir, app)
}

async fn finish(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            app.poll_tools();
            app.poll_pending_list();
            app.poll_pending_status();
            app.poll_pending_preview();
            app.poll_pending_diff();
            if app.tools.pending_task.is_none()
                && app.tools.pending_read.is_none()
                && app.files.pending_lists.is_empty()
                && app.preview_state.pending_preview.is_none()
                && app.preview_state.pending_diff.is_none()
                && app.scm.pending_status.is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        app.tools
            .panel
            .as_ref()
            .is_none_or(|p| p.title != "Operation failed"),
        "{}",
        app.tools
            .panel
            .as_ref()
            .map(|p| p.detail.as_str())
            .unwrap_or_default()
    );
}

#[tokio::test]
async fn command_input_is_isolated_from_workspace_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
    app.open_tools(PanelKind::Commands);
    for c in "qgsp".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert!(!app.session.should_quit);
    assert!(app.scm.pending_write.is_none());
    assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "qgsp");
}

#[tokio::test]
async fn files_menu_creates_files_and_folders_in_clicked_directory_and_at_root() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;
    for (action, name, directory) in [
        (ContextAction::NewFile, "new.txt", false),
        (ContextAction::NewDirectory, "new-dir", true),
    ] {
        let index = app
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == root.join("folder"))
            .unwrap();
        app.open_file_context_menu(index, 2, 4);
        let menu = app.session.context_menu.as_ref().unwrap();
        assert!(menu.items.iter().any(|i| i.action == action));
        assert!(menu.items.iter().all(|i| !matches!(
            i.action,
            ContextAction::Stage
                | ContextAction::Unstage
                | ContextAction::StageFolder
                | ContextAction::UnstageFolder
        )));
        app.apply_context_action(action);
        assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "folder/");
        for c in name.chars() {
            app.handle_tools_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        finish(&mut app).await;
        assert_eq!(root.join("folder").join(name).is_dir(), directory);
        assert!(root.join("folder").join(name).exists());
        app.close_tools();
    }
    app.session.layout.list = ratatui::layout::Rect::new(0, 2, 30, 20);
    app.session.layout.list_inner = ratatui::layout::Rect::new(0, 3, 30, 19);
    app.handle_mouse_content(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 3,
        row: 20,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(
        app.session.context_menu.as_ref().unwrap().target,
        ContextTarget::FileRoot { .. }
    ));
    app.apply_context_action(ContextAction::NewFile);
    assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "");
    for c in "root.txt".chars() {
        app.handle_tools_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    finish(&mut app).await;
    assert!(root.join("root.txt").is_file());
    // Creating alongside an existing file uses that file's containing folder.
    app.close_tools();
    app.reveal_file(root.join("folder/new.txt"), 1);
    finish(&mut app).await;
    let index = app
        .files
        .visible
        .iter()
        .position(|r| r.entry.path == root.join("folder/new.txt"))
        .unwrap();
    app.open_file_context_menu(index, 2, 4);
    app.apply_context_action(ContextAction::NewFile);
    assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "folder/");
}

#[tokio::test]
async fn clicked_file_operations_keep_exact_target_and_delete_requires_confirmation() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;
    app.tools.marks.files.insert(root.join("outside.txt"));
    let index = app
        .files
        .visible
        .iter()
        .position(|r| r.entry.path == root.join("folder"))
        .unwrap();
    app.open_file_context_menu(index, 2, 4);
    app.apply_context_action(ContextAction::Copy);
    assert!(
        matches!(&app.tools.panel.as_ref().unwrap().kind,PanelKind::Prompt(PromptAction::Copy(paths)) if paths==&[root.join("folder")])
    );
    assert!(app.tools.marks.contains_file(&root.join("outside.txt")));
    app.close_tools();
    for (name, action) in [
        ("a.rs", ContextAction::DeleteFile),
        ("folder", ContextAction::DeleteDirectory),
    ] {
        let index = app
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == root.join(name))
            .unwrap();
        app.open_file_context_menu(index, 2, 4);
        app.apply_context_action(action);
        assert!(
            matches!(&app.tools.panel.as_ref().unwrap().kind,PanelKind::Confirm(Operation::Files(FileOperation::Delete(paths))) if paths==&[root.join(name)])
        );
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(root.join(name).exists());
        app.open_file_context_menu(index, 2, 4);
        app.apply_context_action(action);
        app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        finish(&mut app).await;
        assert!(!root.join(name).exists());
        assert!(root.join("outside.txt").exists());
        app.close_tools();
    }
}

#[tokio::test]
async fn files_tab_has_no_stage_commands_menus_or_shortcuts() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.tools.marks.files.insert(dir.path().join("a.rs"));
    for code in ['s', 'u'] {
        app.handle_key(KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE));
    }
    assert!(app.tools.pending_task.is_none());
    assert!(app.scm.pending_write.is_none());
    assert!(git(dir.path(), &["diff", "--cached"]).is_empty());
    assert!(app
        .context_shortcuts()
        .iter()
        .all(|s| s.key != "s" && s.key != "u"));
    app.open_tools(PanelKind::Commands);
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .all(|r| !matches!(
            r.value,
            RowValue::Command(Command::Stage | Command::Unstage | Command::Lines)
        )));
    app.close_tools();
    app.batch_stage(true);
    assert!(app.tools.pending_task.is_none());
    assert!(app.session.status.contains("Git tab"));
}

#[tokio::test]
async fn batch_stage_and_unstage_use_marked_paths_and_include_collapsed_children() {
    let (dir, mut app) = fixture();
    app.session.activity = ActivityTab::Git;
    let root = dir.path();
    for path in [root.join("a.rs"), root.join("folder")] {
        app.tools.marks.git.insert(GitMark {
            path,
            side: DiffSide::Unstaged,
        });
    }
    app.batch_stage(true);
    assert!(app.tools.pending_task.is_some());
    // A second write cannot cancel the batch job.
    app.request_commit("blocked".into());
    assert!(app.scm.pending_write.is_none());
    app.close_tools();
    app.open_tools(PanelKind::Commands);
    finish(&mut app).await;
    assert!(matches!(
        app.tools.panel.as_ref().unwrap().kind,
        PanelKind::Commands
    ));
    assert_eq!(
        git(root, &["diff", "--cached", "--name-only"]),
        "a.rs\nfolder/nested/b.rs\n"
    );
    for path in [root.join("a.rs"), root.join("folder")] {
        app.tools.marks.git.insert(GitMark {
            path,
            side: DiffSide::Staged,
        });
    }
    app.batch_stage(false);
    finish(&mut app).await;
    assert!(git(root, &["diff", "--cached", "--name-only"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("folder/nested/b.rs")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn deletion_requires_confirmation_and_cancel_preserves_files() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.tools.marks.files.insert(dir.path().join("folder"));
    app.run_command(Command::Delete);
    assert!(matches!(
        app.tools.panel.as_ref().unwrap().kind,
        PanelKind::Confirm(_)
    ));
    assert!(app.tools.pending_task.is_none());
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(dir.path().join("folder/nested/b.rs").exists());
    app.run_command(Command::Delete);
    app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    finish(&mut app).await;
    assert!(!dir.path().join("folder").exists());
    assert!(dir.path().join("outside.txt").exists());
}

#[tokio::test]
async fn search_jumps_to_nested_result_line_and_stale_results_are_cancelled() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    std::fs::write(dir.path().join("folder/nested/b.rs"), "one\ntwo\nneedle\n").unwrap();
    app.open_tools(PanelKind::Content);
    for c in "needle".chars() {
        app.handle_tools_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    finish(&mut app).await;
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 1);
    app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    finish(&mut app).await;
    assert!(app.current_path().unwrap().ends_with("folder/nested/b.rs"));
    assert_eq!(app.preview_state.preview_scroll, 2);
    assert_eq!(app.session.focus, PaneFocus::Preview);
    app.open_tools(PanelKind::Files);
    app.close_tools();
    assert!(app.tools.pending_read.is_none());
    app.poll_tools();
    assert!(app.tools.panel.is_none());
}

#[tokio::test]
async fn filters_preserve_parent_paths_and_hide_build_and_git_metadata() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;
    for path in [".hidden", "fresh.rs"] {
        std::fs::write(root.join(path), "new").unwrap();
    }
    std::fs::create_dir(root.join("target")).unwrap();
    app.request_list();
    app.request_status();
    finish(&mut app).await;
    assert!(app
        .files
        .visible
        .iter()
        .all(|r| r.entry.name != ".git" && r.entry.name != "target" && r.entry.name != ".hidden"));
    app.run_command(Command::Untracked);
    assert_eq!(app.files.visible.len(), 1);
    assert_eq!(app.files.visible[0].entry.name, "fresh.rs");
    app.run_command(Command::All);
    app.tools.filter.extension = "rs".into();
    app.rebuild_visible();
    assert!(app
        .files
        .visible
        .iter()
        .all(|r| r.entry.kind == EntryKind::Directory
            || r.entry.path.extension().is_some_and(|e| e == "rs")));
}

#[tokio::test]
async fn git_marks_track_side_and_batch_stage_never_crosses_sides() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;
    git(root, &["add", "a.rs"]);
    std::fs::write(root.join("a.rs"), "changed again\n").unwrap();
    app.request_status();
    finish(&mut app).await;
    app.enter_git_sidebar();
    finish(&mut app).await;

    let select = |app: &mut App, side: DiffSide| {
        let (u, s) = app.scm_slices();
        let index = app
            .scm_rows()
            .iter()
            .position(|row| {
                scm_entry(row, u, s)
                    .is_some_and(|entry| entry.path == "a.rs" && entry.side == side)
            })
            .unwrap();
        app.scm.scm_selected = index;
    };

    select(&mut app, DiffSide::Unstaged);
    app.toggle_mark();
    select(&mut app, DiffSide::Staged);
    app.toggle_mark();
    assert_eq!(app.tools.marks.len(), 2);
    assert!(app
        .tools
        .marks
        .contains_git(&root.join("a.rs"), DiffSide::Unstaged));
    assert!(app
        .tools
        .marks
        .contains_git(&root.join("a.rs"), DiffSide::Staged));
    let (u, s) = app.scm_slices();
    assert_eq!(
        app.scm_rows()
            .iter()
            .filter(|row| scm_entry(row, u, s).is_some_and(|entry| entry.path == "a.rs"))
            .count(),
        2
    );

    // A STAGED-only mark must not stage the worktree side.
    app.tools.marks.clear();
    select(&mut app, DiffSide::Staged);
    app.toggle_mark();
    app.batch_stage(true);
    assert!(app.tools.pending_task.is_none());
    assert!(app.session.status.contains("No unstaged selection"));
    assert_eq!(git(root, &["show", ":a.rs"]), "changed\n");

    // The explicit CHANGES mark stages the worktree snapshot.
    select(&mut app, DiffSide::Unstaged);
    app.toggle_mark();
    app.batch_stage(true);
    finish(&mut app).await;
    assert_eq!(git(root, &["show", ":a.rs"]), "changed again\n");

    // Keep both sides present, then prove a CHANGES-only mark cannot unstage.
    std::fs::write(root.join("a.rs"), "worktree after stage\n").unwrap();
    app.request_status();
    finish(&mut app).await;
    app.tools.marks.clear();
    select(&mut app, DiffSide::Unstaged);
    app.toggle_mark();
    app.batch_stage(false);
    assert!(app.tools.pending_task.is_none());
    assert_eq!(git(root, &["show", ":a.rs"]), "changed again\n");

    select(&mut app, DiffSide::Staged);
    app.toggle_mark();
    app.batch_stage(false);
    finish(&mut app).await;
    assert!(git(root, &["diff", "--cached", "--name-only"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "worktree after stage\n"
    );
}

#[tokio::test]
async fn filters_keep_selected_path_expansion_and_focus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-qb", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.test"]);
    std::fs::create_dir_all(root.join("crates/forge-app/src")).unwrap();
    std::fs::create_dir_all(root.join("crates/forge-git-cli/src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "base\n").unwrap();
    std::fs::write(root.join("crates/forge-app/src/view.rs"), "base\n").unwrap();
    std::fs::write(root.join("crates/forge-git-cli/src/lib.rs"), "base\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    std::fs::write(root.join("crates/forge-app/src/view.rs"), "changed\n").unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    app.reveal_file(root.join("crates/forge-app/src/view.rs"), 1);
    finish(&mut app).await;
    assert_eq!(
        app.selected_entry().unwrap().path,
        root.join("crates/forge-app/src/view.rs")
    );
    app.session.focus = PaneFocus::List;
    let expanded = app.files.expanded.clone();
    let scroll = app.files.scroll;

    app.run_command(Command::Modified);
    assert_eq!(
        app.selected_entry().unwrap().path,
        root.join("crates/forge-app/src/view.rs")
    );
    assert_eq!(app.files.expanded, expanded);
    assert_eq!(app.session.focus, PaneFocus::List);
    assert_eq!(app.files.scroll, scroll);

    app.run_command(Command::All);
    assert_eq!(
        app.selected_entry().unwrap().path,
        root.join("crates/forge-app/src/view.rs")
    );
    assert_eq!(app.files.expanded, expanded);
    assert_eq!(app.session.focus, PaneFocus::List);

    // A filtered-out selection falls back to its nearest visible ancestor,
    // not to whichever row happened to have the previous index.
    app.reveal_file(root.join("crates/forge-git-cli/src/lib.rs"), 1);
    finish(&mut app).await;
    app.session.focus = PaneFocus::List;
    let expanded = app.files.expanded.clone();
    app.run_command(Command::Modified);
    assert_eq!(app.selected_entry().unwrap().path, root.join("crates"));
    assert_eq!(app.files.expanded, expanded);
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn editor_resume_restores_file_and_git_diff_locations() {
    for git_mode in [false, true] {
        let (dir, mut app) = fixture();
        finish(&mut app).await;
        if git_mode {
            app.enter_git_sidebar();
            app.tools.resume_diff = Some(DiffResume {
                path: "a.rs".into(),
                side: DiffSide::Unstaged,
                scroll: 1,
                focus: PaneFocus::Preview,
                hunk: Some(0),
            });
            app.request_status();
            finish(&mut app).await;
        } else {
            app.reveal_file(dir.path().join("folder/nested/b.rs"), 1);
            finish(&mut app).await;
        }
        app.session.focus = PaneFocus::Preview;
        app.preview_state.preview_scroll = 1;
        app.open_editor(app.current_path(), 2);
        let captured = EditorResume::capture(&mut app).unwrap();
        let mut resumed = captured.restore(ColorMode::Ansi16, Ok(()));
        finish(&mut resumed).await;
        assert_eq!(resumed.session.focus, PaneFocus::Preview);
        assert_eq!(
            resumed.preview_state.preview_scroll, 1,
            "git_mode={git_mode}"
        );
        if git_mode {
            assert_eq!(resumed.selected_scm_entry().unwrap().path, "a.rs");
            assert!(matches!(
                resumed.preview_state.preview_mode,
                PreviewMode::Diff
            ));
        } else {
            assert!(resumed
                .current_path()
                .unwrap()
                .ends_with("folder/nested/b.rs"));
        }
    }
}

use super::*;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn contents(changed_first: bool, changed_second: bool) -> String {
    (0..50)
        .map(|i| {
            if i == 2 && changed_first {
                "changed first\n".into()
            } else if i == 40 && changed_second {
                "changed second\n".into()
            } else {
                format!("line {i}\n")
            }
        })
        .collect()
}

fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Forge Test"]);
    git(root, &["config", "user.email", "forge@example.test"]);
    std::fs::write(root.join("a.txt"), contents(false, false)).unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-qm", "baseline"]);
    std::fs::write(root.join("a.txt"), contents(true, true)).unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    app.session.activity = ActivityTab::Git;
    app.session.status.clear();
    app.files.children.insert(
        root.to_owned(),
        vec![Entry {
            id: forge_model::PathId(1),
            name: "a.txt".into(),
            path: root.join("a.txt"),
            kind: EntryKind::File,
        }],
    );
    app.rebuild_visible();
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    (dir, app)
}

async fn show_diff(app: &mut App, side: DiffSide) {
    let root = app.files.tree_root.clone();
    app.scm.git_status = Some(app.services.git.status(&root).unwrap());
    let column = if side == DiffSide::Unstaged {
        StagingColumn::Unstaged
    } else {
        StagingColumn::Staged
    };
    app.scm.scm_selected = scm_jump_to_column(&app.scm_rows(), column);
    let diff = app
        .services
        .git
        .file_diff(&root, "a.txt", side)
        .await
        .unwrap();
    assert_eq!(diff.hunks().len(), 2);
    app.set_file_diff(diff);
    app.preview_state.preview_mode = PreviewMode::Diff;
    app.preview_state.preview_loading = false;
    app.preview_state.hunk_index = Some(0);
    app.preview_state.pending_diff = None;
}

fn press(app: &mut App, key: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
}

async fn finish_write(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !app.poll_pending_write() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !app.session.status.contains("failed:"),
        "{}",
        app.session.status
    );
    // Follow-up read jobs cannot hide the operation's scope feedback.
    assert!(app.session.write_status.is_some());
    app.services.jobs.cancel_key("git:diff");
    app.preview_state.pending_diff = None;
    app.scm.pending_status = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !app.files.pending_lists.is_empty() {
            app.poll_pending_list();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn whole_file_and_hunk_stage_unstage_follow_focus() {
    for (activity, focus) in [
        (ActivityTab::Git, PaneFocus::List),
        (ActivityTab::Git, PaneFocus::Preview),
    ] {
        let (dir, mut app) = fixture();
        show_diff(&mut app, DiffSide::Unstaged).await;
        app.session.activity = activity;
        app.session.focus = focus;
        press(&mut app, 's');
        finish_write(&mut app).await;
        assert_eq!(
            git(dir.path(), &["show", ":a.txt"]),
            contents(true, focus == PaneFocus::List)
        );
        let feedback = app.session.write_status.as_ref().unwrap();
        assert!(feedback.contains("a.txt"));
        assert_eq!(feedback.contains("hunk"), focus == PaneFocus::Preview);

        git(dir.path(), &["add", "a.txt"]);
        show_diff(&mut app, DiffSide::Staged).await;
        app.session.focus = focus;
        press(&mut app, 'u');
        finish_write(&mut app).await;
        assert_eq!(
            git(dir.path(), &["show", ":a.txt"]),
            contents(false, focus == PaneFocus::Preview)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            contents(true, true)
        );
    }
}

async fn finish_write_and_status(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            app.poll_pending_write();
            app.poll_pending_status();
            app.poll_pending_diff();
            app.poll_pending_list();
            if app.scm.pending_write.is_none()
                && app.scm.pending_status.is_none()
                && !app.scm.status_rerun
                && app.preview_state.pending_diff.is_none()
                && app.files.pending_lists.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !app.session.status.contains("failed:"),
        "{}",
        app.session.status
    );
}

#[tokio::test]
async fn stage_then_unstage_follows_same_file_across_git_sections() {
    let (dir, mut app) = fixture();
    app.scm.scm_selected = scm_jump_to_column(&app.scm_rows(), StagingColumn::Unstaged);
    app.session.focus = PaneFocus::List;
    assert_eq!(app.selected_scm_entry().unwrap().path, "a.txt");
    assert_eq!(app.selected_scm_entry().unwrap().side, DiffSide::Unstaged);

    // No manual navigation between actions. Previously this selected a
    // STAGED group heading after the list shrank.
    press(&mut app, 's');
    finish_write_and_status(&mut app).await;
    assert_eq!(app.selected_scm_entry().unwrap().path, "a.txt");
    assert_eq!(app.selected_scm_entry().unwrap().side, DiffSide::Staged);
    assert!(matches!(
        app.selected_scm_row(),
        Some(ScmRow::Change { .. })
    ));
    assert!(app.scm.pending_transfer.is_none());

    press(&mut app, 'u');
    finish_write_and_status(&mut app).await;
    assert_eq!(app.selected_scm_entry().unwrap().path, "a.txt");
    assert_eq!(app.selected_scm_entry().unwrap().side, DiffSide::Unstaged);
    assert!(app.scm.pending_transfer.is_none());
    assert_eq!(
        git(dir.path(), &["show", "HEAD:a.txt"]),
        git(dir.path(), &["show", ":a.txt"])
    );
}

#[tokio::test]
async fn manual_navigation_cancels_pending_stage_focus_transfer() {
    let (_dir, mut app) = fixture();
    app.scm.scm_selected = scm_jump_to_column(&app.scm_rows(), StagingColumn::Unstaged);
    app.session.focus = PaneFocus::List;
    press(&mut app, 's');
    assert!(app.scm.pending_transfer.is_some());
    // Moving away while the write is pending must not steal the user's focus
    // after the asynchronous Git status response.
    app.move_scm(-1);
    assert!(app.scm.pending_transfer.is_none());
    finish_write_and_status(&mut app).await;
    assert!(matches!(
        app.selected_scm_row(),
        Some(ScmRow::Section { .. })
    ));
}

#[tokio::test]
async fn navigation_and_menus_use_the_same_scope_as_shortcuts() {
    let (_dir, mut app) = fixture();
    show_diff(&mut app, DiffSide::Unstaged).await;
    app.session.focus = PaneFocus::List;
    press(&mut app, 'n');
    assert_eq!(app.preview_state.hunk_index, Some(0));
    assert!(app
        .context_shortcuts()
        .iter()
        .any(|s| s.label == "Stage file"));
    app.open_staging_context_menu(StagingColumn::Unstaged, 0, 0, 0);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .any(|i| i.action == ContextAction::Stage && i.enabled));
    app.dismiss_menu();
    app.session.focus = PaneFocus::Preview;
    press(&mut app, 'n');
    assert_eq!(app.preview_state.hunk_index, Some(1));
    assert!(app
        .context_shortcuts()
        .iter()
        .any(|s| s.label == "Stage hunk"));
    app.open_diff_context_menu(0, 0);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .any(|i| i.action == ContextAction::StageHunk && i.enabled));
    app.dismiss_menu();
    press(&mut app, 'u');
    assert!(app.scm.pending_write.is_none());
    assert!(app.session.status.contains("staged diff"));
}

#[tokio::test]
async fn invalid_loading_stale_and_empty_targets_never_start_writes() {
    let (dir, mut app) = fixture();
    // A group heading is navigation only.
    app.scm.scm_selected = 0;
    press(&mut app, 's');
    assert!(app.scm.pending_write.is_none());
    assert!(app.session.status.contains("Select a file"));
    show_diff(&mut app, DiffSide::Unstaged).await;
    app.session.focus = PaneFocus::Preview;
    app.preview_state.preview_loading = true;
    press(&mut app, 's');
    assert!(app.scm.pending_write.is_none());
    assert!(app.session.status.contains("loading"));
    app.preview_state.preview_loading = false;
    app.preview_state.file_diff.as_mut().unwrap().path = "old.txt".into();
    press(&mut app, 's');
    assert!(app.scm.pending_write.is_none());
    assert!(app.session.status.contains("Selection changed"));

    show_diff(&mut app, DiffSide::Unstaged).await;
    app.preview_state.file_diff.as_mut().unwrap().lines.clear();
    app.preview_state.hunk_index = None;
    press(&mut app, 's');
    assert!(app.scm.pending_write.is_none());
    app.open_diff_context_menu(0, 0);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .all(|i| i.action != ContextAction::StageHunk || !i.enabled));
    assert!(git(dir.path(), &["diff", "--cached"]).is_empty());

    app.dismiss_menu();
    app.session.focus = PaneFocus::List;
    app.scm.git_status.as_mut().unwrap().push_change(
        "folder/a.txt",
        forge_model::GitStatus::Modified,
        DiffSide::Unstaged,
    );
    app.scm.scm_selected = app
        .scm_rows()
        .iter()
        .position(|r| matches!(r, ScmRow::Folder { .. }))
        .unwrap();
    press(&mut app, 'u');
    assert!(app.scm.pending_write.is_none());
    assert!(app.context_description().contains("s Stage"));
}

#[tokio::test]
async fn help_consumes_keys_and_mouse_then_restores_focus_and_selection() {
    let (dir, mut app) = fixture();
    show_diff(&mut app, DiffSide::Unstaged).await;
    app.session.focus = PaneFocus::Preview;
    let selected = app.scm.scm_selected;
    press(&mut app, '?');
    app.session.help_scroll_limit = 20;
    press(&mut app, 's');
    press(&mut app, 'c');
    press(&mut app, 'n');
    press(&mut app, 'j');
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.session.help_scroll, 4);
    assert_eq!(app.preview_state.hunk_index, Some(0));
    assert!(app.scm.pending_write.is_none());
    assert!(!app.session.commit_open);
    assert!(git(dir.path(), &["diff", "--cached"]).is_empty());
    press(&mut app, 'q');
    assert!(!app.session.help_open && !app.session.should_quit);
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert_eq!(app.scm.scm_selected, selected);
    press(&mut app, '?');
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.session.should_quit);
    assert!(!app.session.help_open);
}

#[tokio::test]
async fn a_second_write_cannot_cancel_an_existing_write() {
    let (_dir, mut app) = fixture();
    show_diff(&mut app, DiffSide::Unstaged).await;
    press(&mut app, 's');
    let id = app.scm.pending_write.as_ref().unwrap().id;
    press(&mut app, 'c');
    assert!(!app.session.commit_open);
    press(&mut app, 'u');
    assert_eq!(app.scm.pending_write.as_ref().unwrap().id, id);
    finish_write(&mut app).await;
}

#[tokio::test]
async fn an_empty_workspace_has_no_stage_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
    app.files.pending_lists.clear();
    app.scm.pending_status = None;
    press(&mut app, 's');
    assert!(app.scm.pending_write.is_none());
    assert!(!app
        .context_shortcuts()
        .iter()
        .any(|s| s.key == "s" || s.key == "u"));
}

fn select_folder(app: &mut App, path: &str, activity: ActivityTab, side: DiffSide) {
    let root = app.files.tree_root.clone();
    app.scm.git_status = Some(app.services.git.status(&root).unwrap());
    app.session.activity = activity;
    app.session.focus = PaneFocus::List;
    app.session.status.clear();
    if activity == ActivityTab::Git {
        app.scm.scm_selected = app.scm_rows().iter().position(|r| {
            matches!(r, ScmRow::Folder { path: p, column, .. } if p == path && column.side() == side)
        }).unwrap();
    } else {
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == root.join(path))
            .unwrap();
    }
}

fn names(root: &Path, args: &[&str]) -> HashSet<String> {
    git(root, args).lines().map(str::to_owned).collect()
}

#[tokio::test]
async fn folder_actions_cover_descendants_and_preserve_neighbor_changes() {
    for activity in [ActivityTab::Git] {
        for menu in [false, true] {
            for expanded in [false, true] {
                let (dir, mut app) = fixture();
                let root = dir.path();
                std::fs::create_dir_all(root.join("folder/nested")).unwrap();
                std::fs::create_dir(root.join("folder-sibling")).unwrap();
                for path in [
                    "folder/a.txt",
                    "folder/nested/b.txt",
                    "folder/deleted.txt",
                    "folder/rename-old.txt",
                    "folder-sibling/keep.txt",
                    "outside.txt",
                ] {
                    std::fs::write(root.join(path), format!("baseline {path}\n")).unwrap();
                }
                std::fs::write(root.join(".gitignore"), "folder/ignored.txt\n").unwrap();
                git(root, &["add", "."]);
                git(root, &["commit", "-qm", "folders"]);
                for path in [
                    "folder/a.txt",
                    "folder/nested/b.txt",
                    "folder-sibling/keep.txt",
                    "outside.txt",
                ] {
                    std::fs::write(root.join(path), format!("changed {path}\n")).unwrap();
                }
                git(root, &["add", "folder/nested/b.txt", "outside.txt"]);
                std::fs::write(
                    root.join("folder/nested/b.txt"),
                    "partially staged then edited again\n",
                )
                .unwrap();
                std::fs::remove_file(root.join("folder/deleted.txt")).unwrap();
                std::fs::rename(
                    root.join("folder/rename-old.txt"),
                    root.join("folder/renamed.txt"),
                )
                .unwrap();
                std::fs::write(root.join("folder/new.txt"), "new file\n").unwrap();
                std::fs::write(root.join("folder/ignored.txt"), "ignored\n").unwrap();
                app.files.children.insert(
                    root.to_owned(),
                    vec![Entry {
                        id: forge_model::PathId(2),
                        name: "folder".into(),
                        path: root.join("folder"),
                        kind: EntryKind::Directory,
                    }],
                );
                app.rebuild_visible();
                if expanded {
                    app.scm.scm_expanded_unstaged.insert("folder".into());
                    app.scm.scm_expanded_staged.insert("folder".into());
                }
                for (side, key, action, label) in [
                    (
                        DiffSide::Unstaged,
                        's',
                        ContextAction::StageFolder,
                        "Stage folder",
                    ),
                    (
                        DiffSide::Staged,
                        'u',
                        ContextAction::UnstageFolder,
                        "Unstage folder",
                    ),
                ] {
                    select_folder(&mut app, "folder", activity, side);
                    assert!(app
                        .context_shortcuts()
                        .iter()
                        .any(|s| s.key == key.to_string() && s.label == label));
                    if menu {
                        if activity == ActivityTab::Git {
                            app.scm.scroll_scm = 0;
                            app.session.layout.list = ratatui::layout::Rect::new(0, 0, 40, 40);
                            app.session.layout.list_inner = app.session.layout.list;
                            app.handle_mouse_content(MouseEvent {
                                kind: MouseEventKind::Down(MouseButton::Right),
                                column: 1,
                                row: app.scm.scm_selected as u16,
                                modifiers: KeyModifiers::NONE,
                            });
                            assert!(
                                matches!(app.selected_scm_row(), Some(ScmRow::Folder { expanded: e, .. }) if e == expanded)
                            );
                        } else {
                            app.open_file_context_menu(app.files.selected, 0, 0);
                        }
                        assert!(app
                            .session
                            .context_menu
                            .as_ref()
                            .unwrap()
                            .items
                            .iter()
                            .any(|i| i.action == action && i.enabled));
                        app.apply_context_action(action);
                    } else {
                        press(&mut app, key);
                    }
                    finish_write(&mut app).await;
                    let feedback = app.session.write_status.as_ref().unwrap();
                    assert!(feedback.contains("folder folder/ · 6 files"), "{feedback}");
                    let staged = names(root, &["diff", "--cached", "--name-only", "--no-renames"]);
                    assert!(staged.contains("outside.txt"));
                    assert!(!staged.contains("folder-sibling/keep.txt"));
                    assert!(!staged.contains("folder/ignored.txt"));
                    assert_eq!(staged.len(), if key == 's' { 7 } else { 1 });
                    if key == 's' {
                        assert_eq!(
                            git(root, &["show", ":folder/nested/b.txt"]),
                            "partially staged then edited again\n"
                        );
                    }
                }
                assert_eq!(
                    std::fs::read_to_string(root.join("folder/new.txt")).unwrap(),
                    "new file\n"
                );
                assert_eq!(
                    std::fs::read_to_string(root.join("folder/nested/b.txt")).unwrap(),
                    "partially staged then edited again\n"
                );
                assert!(!root.join("folder/deleted.txt").exists());
                assert!(!root.join("folder/rename-old.txt").exists());
            }
        }
    }
}

#[tokio::test]
async fn deleted_folders_can_be_staged_and_unstaged() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    std::fs::create_dir_all(root.join("gone/nested")).unwrap();
    std::fs::write(root.join("gone/nested/file.txt"), "old\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "before removal"]);
    std::fs::remove_dir_all(root.join("gone")).unwrap();
    select_folder(&mut app, "gone", ActivityTab::Git, DiffSide::Unstaged);
    press(&mut app, 's');
    finish_write(&mut app).await;
    assert!(git(root, &["diff", "--cached", "--name-status"]).contains("D\tgone/nested/file.txt"));
    select_folder(&mut app, "gone", ActivityTab::Git, DiffSide::Staged);
    press(&mut app, 'u');
    finish_write(&mut app).await;
    assert!(git(root, &["diff", "--cached"]).is_empty());
    assert!(!root.join("gone").exists());
}

#[tokio::test]
async fn folder_paths_are_literal_and_unstage_works_before_first_commit() {
    for committed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.name", "Forge Test"]);
        git(root, &["config", "user.email", "forge@example.test"]);
        for folder in ["pkg[1]", "pkg1", "pkg[1]-sibling"] {
            std::fs::create_dir(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("a.txt"), "old\n").unwrap();
        }
        if committed {
            git(root, &["add", "."]);
            git(root, &["commit", "-qm", "baseline"]);
            for folder in ["pkg[1]", "pkg1", "pkg[1]-sibling"] {
                std::fs::write(root.join(folder).join("a.txt"), "changed\n").unwrap();
            }
        }
        let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        select_folder(&mut app, "pkg[1]", ActivityTab::Git, DiffSide::Unstaged);
        press(&mut app, 's');
        finish_write(&mut app).await;
        assert_eq!(
            names(root, &["diff", "--cached", "--name-only"]),
            HashSet::from(["pkg[1]/a.txt".into()])
        );
        // Unstage must keep subsequent edits even on an unborn branch.
        std::fs::write(root.join("pkg[1]/a.txt"), "latest edits\n").unwrap();
        select_folder(&mut app, "pkg[1]", ActivityTab::Git, DiffSide::Staged);
        press(&mut app, 'u');
        finish_write(&mut app).await;
        assert!(git(root, &["diff", "--cached"]).is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("pkg[1]/a.txt")).unwrap(),
            "latest edits\n"
        );
    }
}

#[tokio::test]
async fn clean_and_stale_folder_targets_never_start_writes() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    std::fs::create_dir(root.join("clean")).unwrap();
    app.files.children.insert(
        root.to_owned(),
        vec![Entry {
            id: forge_model::PathId(2),
            name: "clean".into(),
            path: root.join("clean"),
            kind: EntryKind::Directory,
        }],
    );
    app.rebuild_visible();
    app.session.activity = ActivityTab::Files;
    for key in ['s', 'u'] {
        press(&mut app, key);
        assert!(app.scm.pending_write.is_none());
    }
    app.open_file_context_menu(0, 0, 0);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .all(|i| !matches!(
            i.action,
            ContextAction::StageFolder | ContextAction::UnstageFolder
        )));
    app.dismiss_menu();
    std::fs::write(root.join("clean/new.txt"), "new\n").unwrap();
    select_folder(&mut app, "clean", ActivityTab::Git, DiffSide::Unstaged);
    app.open_staging_folder_menu(app.scm.scm_selected, 0, 0);
    // The folder vanishes while its menu is open: do not operate on another row.
    std::fs::remove_file(root.join("clean/new.txt")).unwrap();
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    app.apply_context_action(ContextAction::StageFolder);
    assert!(app.scm.pending_write.is_none());
    assert!(app.session.status.contains("select the folder again"));
    assert!(git(root, &["diff", "--cached"]).is_empty());

    // Files menus also retain path identity while a directory refresh changes rows.
    std::fs::write(root.join("clean/new.txt"), "new\n").unwrap();
    select_folder(&mut app, "clean", ActivityTab::Files, DiffSide::Unstaged);
    app.open_file_context_menu(app.files.selected, 0, 0);
    std::fs::create_dir(root.join("neighbor")).unwrap();
    std::fs::write(root.join("neighbor/keep.txt"), "do not stage\n").unwrap();
    app.files.children.insert(
        root.to_owned(),
        vec![Entry {
            id: forge_model::PathId(3),
            name: "neighbor".into(),
            path: root.join("neighbor"),
            kind: EntryKind::Directory,
        }],
    );
    app.rebuild_visible();
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    app.apply_context_action(ContextAction::DeleteDirectory);
    assert!(app.scm.pending_write.is_none());
    assert!(app
        .session
        .status
        .contains("select the file or folder again"));
    assert!(git(root, &["diff", "--cached"]).is_empty());
}

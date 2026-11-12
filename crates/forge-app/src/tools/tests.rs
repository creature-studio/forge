use super::*;
use ratatui::layout::Rect;

#[tokio::test]
async fn transient_write_notice_expires_without_erasing_newer_context() {
    let (_dir, mut app) = fixture();
    app.session.status = "pushed: origin".into();
    app.session.write_status = Some("pushed: origin".into());
    app.session.notice_lease = Some((
        "pushed: origin".into(),
        "pushed: origin".into(),
        Instant::now() - Duration::from_secs(10),
    ));
    app.expire_write_notice();
    assert!(app.session.write_status.is_none());
    assert!(app.session.status.is_empty());

    app.session.status = "new operation running".into();
    app.session.write_status = Some("previous operation done".into());
    app.session.notice_lease = Some((
        "previous operation done".into(),
        "previous operation done".into(),
        Instant::now() - Duration::from_secs(10),
    ));
    app.expire_write_notice();
    assert!(app.session.write_status.is_none());
    assert_eq!(app.session.status, "new operation running");
}

#[tokio::test]
async fn current_branch_is_no_op_and_key_menu_has_file_local_actions_only() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Git(GitQuery::Branches));
    finish(&mut app).await;
    let branch = app.scm.branch.clone();
    let row = app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|r| matches!(&r.value, RowValue::Git(it) if it.id == branch))
        .unwrap();
    app.tools.panel.as_mut().unwrap().selected = row;
    app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Branches))
    ));
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .message
        .contains("Already on branch"));
    assert!(app.tools.pending_task.is_none());
    app.close_tools();

    app.files.selected = 0;
    app.session.layout.list_inner = Rect::new(0, 3, 42, 15);
    app.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT));
    assert!(
        app.session.context_menu.is_some(),
        "Shift-F10 opens Files menu"
    );
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.session.context_menu.is_none());

    app.enter_git_sidebar();
    let index = app
        .scm_rows()
        .iter()
        .position(|r| matches!(r, ScmRow::Change { .. }))
        .unwrap();
    app.scm.scm_selected = index;
    app.handle_key(KeyEvent::new(KeyCode::Menu, KeyModifiers::NONE));
    let menu = app.session.context_menu.as_ref().expect("Git menu");
    let actions: Vec<_> = menu.items.iter().map(|i| i.action).collect();
    assert!(actions.contains(&ContextAction::Stage));
    assert!(!actions.contains(&ContextAction::Pull));
    assert!(!actions.contains(&ContextAction::Push));
    assert!(!actions.contains(&ContextAction::Commit));
}

#[test]
fn file_name_search_rows_do_not_claim_a_content_line_match() {
    let root = std::path::Path::new("/workspace");
    let file = search_row(
        root,
        forge_fs::workspace::SearchHit {
            path: root.join("src/worker.rs"),
            line: 1,
            text: String::new(),
            score: 10,
            content: false,
        },
    );
    assert_eq!(file.label, "src/worker.rs");
    assert!(matches!(file.value, RowValue::File(_, 1)));
    let content = search_row(
        root,
        forge_fs::workspace::SearchHit {
            path: root.join("src/worker.rs"),
            line: 42,
            text: "the match".into(),
            score: 0,
            content: true,
        },
    );
    assert_eq!(content.label, "src/worker.rs:42  the match");
    let empty_line = search_row(
        root,
        forge_fs::workspace::SearchHit {
            path: root.join("src/worker.rs"),
            line: 2,
            text: String::new(),
            score: 0,
            content: true,
        },
    );
    assert_eq!(empty_line.label, "src/worker.rs:2  (empty line)");
}

#[tokio::test]
async fn worktree_switch_restores_per_workspace_directory_navigation() {
    let (dir, mut app) = fixture();
    let root = dir.path().to_path_buf();
    let other = tempfile::tempdir().unwrap();
    let other_root = other.path().join("linked");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "audit-alt",
            other_root.to_str().unwrap(),
        ],
    );
    finish(&mut app).await;

    // Select a nested file in the original worktree, with a filter & scroll.
    app.files.expanded.insert(root.join("folder"));
    app.files.expanded.insert(root.join("folder/nested"));
    app.request_missing_expanded();
    finish(&mut app).await;
    let selected = root.join("folder/nested/b.rs");
    app.files.selected = app
        .files
        .visible
        .iter()
        .position(|r| r.entry.path == selected)
        .unwrap();
    app.files.scroll = 2;
    app.tools.filter.show_hidden = true;

    app.switch_workspace(other_root.clone());
    finish(&mut app).await;
    assert_eq!(app.files.tree_root, other_root);
    assert!(app.files.expanded.is_empty());
    assert_eq!(app.files.selected, 0);
    assert!(app.tools.marks.is_empty());
    app.files.expanded.insert(other_root.join("folder"));
    app.request_missing_expanded();
    finish(&mut app).await;
    let alt_selected = other_root.join("folder/nested");
    app.files.selected = app
        .files
        .visible
        .iter()
        .position(|r| r.entry.path == alt_selected)
        .unwrap();
    app.files.scroll = 1;

    app.switch_workspace(root.clone());
    finish(&mut app).await;
    assert_eq!(app.selected_entry().unwrap().path, selected);
    assert!(app.files.expanded.contains(&root.join("folder/nested")));
    assert!(app.tools.filter.show_hidden);
    assert!(app.files.scroll <= app.files.selected);
    app.switch_workspace(other_root.clone());
    finish(&mut app).await;
    assert_eq!(app.selected_entry().unwrap().path, alt_selected);
    assert!(app.files.expanded.contains(&other_root.join("folder")));
    // No state should bleed into a third newly visited worktree.
}

#[tokio::test]
async fn content_context_selection_cache_races_and_invalidation() {
    let (dir, mut app) = fixture();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("outside.txt");
    std::fs::write(&a, "before\nneedle\nneedle\nafter").unwrap();
    std::fs::write(&b, "other needle").unwrap();
    app.open_tools(PanelKind::Content);
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("needle".into());
    app.reload_tools();
    finish(&mut app).await;
    // Parallel streams preserve whichever hit arrived first; choose the first
    // sorted result explicitly before checking navigation.
    app.tools.panel.as_mut().unwrap().selected = 0;
    app.request_search_context();
    finish(&mut app).await;
    assert_eq!(app.tools.context_path.as_ref(), Some(&a));
    // A second hit in the same file uses the cached text even if disk changes.
    std::fs::write(&a, "fresh needle\nneedle\nneedle").unwrap();
    app.handle_tools_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.tools.pending_context.is_none());
    assert_eq!(
        app.tools
            .content_context
            .as_ref()
            .unwrap()
            .as_ref()
            .ok()
            .unwrap()
            .lines[0],
        "before"
    );
    app.handle_tools_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.tools.context_path.as_ref(), Some(&b));
    assert!(app.tools.content_context.is_none());
    // Replace an in-flight file read before polling its result.
    app.handle_tools_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    finish(&mut app).await;
    assert_eq!(app.tools.context_path.as_ref(), Some(&a));
    assert_eq!(
        app.tools
            .content_context
            .as_ref()
            .unwrap()
            .as_ref()
            .ok()
            .unwrap()
            .lines[0],
        "fresh needle"
    );
    std::fs::write(&a, "external needle\nneedle\nneedle").unwrap();
    app.invalidate_search_cache();
    finish(&mut app).await;
    assert_eq!(
        app.tools
            .content_context
            .as_ref()
            .unwrap()
            .as_ref()
            .ok()
            .unwrap()
            .lines[0],
        "external needle"
    );
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.tools.context_path.is_none());
    assert!(app.tools.pending_context.is_none());
}

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
                && app.tools.pending_index.is_none()
                && app.tools.pending_context.is_none()
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

/// Branch/Worktree dialogs are navigation children, not workspace exits.
/// These use a real temporary Git repository; Esc must never mutate refs or
/// remove worktree files.
#[tokio::test]
async fn branch_worktree_escape_returns_to_selected_parent_without_git_writes() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    git(root, &["branch", "feature/testing"]);
    // Use an isolated owner so the linked worktree stays alive until the end.
    let linked_owner = tempfile::tempdir().unwrap();
    let other = linked_owner.path().join("linked");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature/worktree",
            other.to_str().unwrap(),
        ],
    );
    finish(&mut app).await;

    app.open_tools(PanelKind::Git(GitQuery::Branches));
    finish(&mut app).await;
    let feature_row = app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|row| matches!(&row.value, RowValue::Git(item) if item.id == "feature/testing"))
        .expect("feature branch exists");
    app.tools.panel.as_mut().unwrap().selected = feature_row;

    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    app.handle_tools_key(enter);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm(Operation::Git(GitAction::SwitchBranch(
            _
        ))))
    ));
    app.handle_tools_key(esc);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Branches))
    ));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, feature_row);
    assert!(app.tools.after.is_none());
    assert_eq!(git(root, &["branch", "--show-current"]).trim(), "main");

    // Rename prompt and its cancel path preserve the selected branch.
    app.handle_tools_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Prompt(PromptAction::RenameBranch(_)))
    ));
    app.handle_tools_key(esc);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, feature_row);
    assert!(matches!(
        app.tools.panel.as_ref().unwrap().kind,
        PanelKind::Git(GitQuery::Branches)
    ));

    app.handle_tools_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm(Operation::Git(GitAction::DeleteBranch(
            _
        ))))
    ));
    app.handle_tools_key(esc);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, feature_row);
    assert!(git(root, &["branch", "--list", "feature/testing"]).contains("feature/testing"));

    app.handle_tools_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::BranchDiff(_)))
    ));
    app.handle_tools_key(esc);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Branches))
    ));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, feature_row);

    app.handle_tools_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Prompt(PromptAction::Branch))
    ));
    app.handle_tools_key(esc);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, feature_row);

    app.handle_tools_key(esc);
    assert!(app.tools.panel.is_none());

    app.open_tools(PanelKind::Git(GitQuery::Worktrees));
    finish(&mut app).await;
    let worktree_row = app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(
            |row| matches!(&row.value, RowValue::Git(item) if item.id == other.to_string_lossy()),
        )
        .expect("linked worktree exists");
    app.tools.panel.as_mut().unwrap().selected = worktree_row;

    app.handle_tools_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Prompt(PromptAction::Worktree))
    ));
    app.handle_tools_key(esc);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Worktrees))
    ));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, worktree_row);

    app.handle_tools_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm(Operation::Git(
            GitAction::RemoveWorktree(_)
        )))
    ));
    app.handle_tools_key(esc);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Worktrees))
    ));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, worktree_row);
    assert!(other.exists(), "Esc must not remove a linked worktree");

    app.handle_tools_key(esc);
    assert!(app.tools.panel.is_none());
    git(root, &["worktree", "remove", other.to_str().unwrap()]);
}

#[tokio::test]
async fn branch_worktree_esc_restores_loading_parent_by_restarting_read() {
    let (_dir, mut app) = fixture();
    for (parent, key) in [
        (GitQuery::Branches, KeyCode::Char('A')),
        (GitQuery::Worktrees, KeyCode::Char('A')),
    ] {
        app.open_tools(PanelKind::Git(parent));
        assert_eq!(app.tools.panel.as_ref().unwrap().message, "Loading…");
        app.handle_tools_key(KeyEvent::new(key, KeyModifiers::NONE));
        assert!(matches!(
            app.tools.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Prompt(_))
        ));
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            app.tools.pending_read.is_some(),
            "return must restart cancelled list query"
        );
        finish(&mut app).await;
        assert!(
            !app.tools.panel.as_ref().unwrap().rows.is_empty(),
            "list remained empty after pending read was cancelled"
        );
        assert!(matches!(
            app.tools.panel.as_ref().unwrap().kind,
            PanelKind::Git(_)
        ));
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    }
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
async fn filename_queries_reuse_index_and_refresh_on_filesystem_changes() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Files);
    let enumeration = app.tools.pending_index.as_ref().unwrap().id;
    app.handle_tools_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_eq!(app.tools.pending_index.as_ref().unwrap().id, enumeration);
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .message
        .contains("Searching"));
    finish(&mut app).await;
    let index = app.tools.file_index.as_ref().unwrap().clone();
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("b".into());
    app.reload_tools();
    assert!(app.tools.pending_index.is_none());
    finish(&mut app).await;
    assert!(Arc::ptr_eq(&index, app.tools.file_index.as_ref().unwrap()));
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.label.contains("b.rs")));

    std::fs::write(dir.path().join("brand-new.rs"), "needle\n").unwrap();
    app.refresh_external_changes(false);
    assert!(app.tools.pending_index.is_some());
    finish(&mut app).await;
    assert!(!Arc::ptr_eq(&index, app.tools.file_index.as_ref().unwrap()));
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .any(|row| row.label.contains("brand-new.rs")));
    app.close_tools();
    app.open_tools(PanelKind::Files);
    assert!(
        app.tools.pending_index.is_none(),
        "completed cache survives close/reopen"
    );
    finish(&mut app).await;
    app.switch_workspace(dir.path().join("folder"));
    assert!(app.tools.panel.is_none());
    assert!(app.tools.file_index.is_none());
    finish(&mut app).await;
    app.open_tools(PanelKind::Files);
    finish(&mut app).await;
    assert!(app.tools.panel.as_ref().unwrap().rows.iter().all(|row| {
        matches!(&row.value, RowValue::File(path, _) if path.starts_with(dir.path().join("folder")))
    }));
}

#[tokio::test]
async fn filename_debounce_coalesces_rapid_input_and_keeps_previous_results() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Files);
    finish(&mut app).await;
    let index = app.tools.file_index.as_ref().unwrap().clone();
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);

    // First keystroke schedules a query but must not start matching yet.
    app.handle_tools_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let initial_job = app.tools.pending_read.as_ref().unwrap().id;
    tokio::time::sleep(Duration::from_millis(65)).await;
    app.poll_tools();
    assert!(
        app.tools.pending_read.is_some(),
        "filename query ran before debounce"
    );
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);

    // A second query within the debounce window cancels the first one.
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("b".into());
    app.reload_tools();
    let final_job = app.tools.pending_read.as_ref().unwrap().id;
    assert_ne!(initial_job, final_job);
    tokio::time::sleep(Duration::from_millis(75)).await;
    app.poll_tools();
    assert_eq!(app.tools.pending_read.as_ref().unwrap().id, final_job);
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .message
        .contains("Searching"));

    finish(&mut app).await;
    let rows = &app.tools.panel.as_ref().unwrap().rows;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].label.contains("b.rs"));
    assert!(Arc::ptr_eq(&index, app.tools.file_index.as_ref().unwrap()));
    assert!(app.tools.panel.as_ref().unwrap().message.is_empty());
}

#[tokio::test]
async fn status_filtered_search_refreshes_after_the_new_git_snapshot_arrives() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.tools.filter.status = 2;
    app.open_tools(PanelKind::Files);
    finish(&mut app).await;
    assert!(app.tools.panel.as_ref().unwrap().rows.is_empty());
    std::fs::write(dir.path().join("new-untracked.rs"), "new\n").unwrap();
    app.refresh_external_changes(false);
    finish(&mut app).await;
    let rows = &app.tools.panel.as_ref().unwrap().rows;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].label.contains("new-untracked.rs"));
}

#[tokio::test]
async fn content_debounce_retains_rows_then_latest_query_wins_and_regex_errors_show() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.tools.filter.excluded = "custom".into();
    let options = app.search_options(true, false);
    assert!(options.excluded.contains("node_modules"));
    assert!(options.excluded.contains("custom"));
    app.open_tools(PanelKind::Content);
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("changed".into());
    app.reload_tools();
    finish(&mut app).await;
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);
    app.tools.panel.as_mut().unwrap().selected = 1;
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("absent".into());
    app.reload_tools();
    tokio::time::sleep(Duration::from_millis(40)).await;
    app.poll_tools();
    assert!(
        app.tools.pending_read.is_some(),
        "real debounce has not elapsed"
    );
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 1);
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .message
        .contains("Searching"));
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("changed".into());
    app.reload_tools();
    finish(&mut app).await;
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 3);
    assert!(app.tools.panel.as_ref().unwrap().message.is_empty());
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("[".into());
    app.handle_tools_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    finish(&mut app).await;
    assert!(app.tools.panel.as_ref().unwrap().regex);
    assert!(app
        .tools
        .panel
        .as_ref()
        .unwrap()
        .message
        .starts_with("Failed:"));
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.poll_tools();
    assert!(app.tools.panel.is_none());
    assert!(app.tools.search_hits.is_none());
}

#[tokio::test]
async fn incremental_content_hits_show_searching_before_completion_and_modes_clear_rows() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Content);
    app.cancel_tool_read();
    let generation = app.tools.search_generation;
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    app.tools.search_hits = Some(Mutex::new(rx));
    app.tools.streamed = false;
    app.tools.panel.as_mut().unwrap().rows.push(ToolRow {
        label: "previous query".into(),
        value: RowValue::File(app.files.tree_root.join("old"), 1),
    });
    tx.send((
        generation,
        forge_fs::workspace::SearchHit {
            path: app.files.tree_root.join("new.rs"),
            line: 17,
            text: "current query".into(),
            score: 0,
            content: true,
        },
    ))
    .unwrap();
    app.poll_tools();
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(panel.rows.len(), 1);
    assert!(panel.rows[0].label.contains("new.rs:17"));
    assert_eq!(panel.message, "Searching… 1 results");
    app.open_tools(PanelKind::Files);
    assert!(app.tools.panel.as_ref().unwrap().rows.is_empty());
    app.close_tools();
    assert!(app.tools.pending_index.is_none());
    assert!(app.tools.search_hits.is_none());
}

#[tokio::test]
async fn sorted_content_results_default_to_first_unless_user_selected_a_hit() {
    async fn settle_with_sorted_results(app: &mut App) {
        let root = app.files.tree_root.clone();
        let generation = app.tools.search_generation;
        let hits = vec![
            search_row(
                &root,
                forge_fs::workspace::SearchHit {
                    path: root.join("a.rs"),
                    line: 1,
                    text: "first sorted hit".into(),
                    score: 0,
                    content: true,
                },
            ),
            search_row(
                &root,
                forge_fs::workspace::SearchHit {
                    path: root.join("outside.txt"),
                    line: 1,
                    text: "later sorted hit".into(),
                    score: 0,
                    content: true,
                },
            ),
        ];
        app.tools.pending_read = Some(app.services.jobs.spawn(
            JobPriority::High,
            None,
            async move {
                Ok(ReadReply {
                    generation: Some(generation),
                    history_has_older: false,
                    rows: hits,
                    detail: "2 results".into(),
                })
            },
        ));
        finish(app).await;
    }

    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Content);
    app.cancel_tool_read();
    let root = app.files.tree_root.clone();
    // The first parallel worker returned outside.txt. The final sorted
    // response begins with a.rs: the automatic selection must move to it.
    {
        let panel = app.tools.panel.as_mut().unwrap();
        panel.rows = vec![
            ToolRow {
                label: "outside.txt:1 late result".into(),
                value: RowValue::File(root.join("outside.txt"), 1),
            },
            ToolRow {
                label: "a.rs:1 first result".into(),
                value: RowValue::File(root.join("a.rs"), 1),
            },
        ];
        panel.selected = 0;
        panel.search_selection_touched = false;
    }
    settle_with_sorted_results(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(
        panel.selected, 0,
        "automatic selection must choose sorted first hit"
    );
    assert!(matches!(&panel.rows[0].value, RowValue::File(p, _) if p.ends_with("a.rs")));

    // When the user moves during streaming, preserve the chosen file and
    // line through the final reorder, even if its numerical index changes.
    {
        let panel = app.tools.panel.as_mut().unwrap();
        panel.rows.swap(0, 1);
        panel.selected = 0;
        panel.search_selection_touched = false;
    }
    app.handle_tools_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.tools.panel.as_ref().unwrap().search_selection_touched);
    settle_with_sorted_results(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(panel.selected, 0);
    assert!(
        matches!(&panel.rows[panel.selected].value, RowValue::File(p, _) if p.ends_with("a.rs"))
    );

    // A new query no longer preserves an explicit selection from the old one.
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("new query".into());
    app.reload_tools();
    assert!(!app.tools.panel.as_ref().unwrap().search_selection_touched);
    app.close_tools();
}

#[tokio::test]
async fn stale_search_generations_cannot_publish_rows_or_status() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Content);
    app.cancel_tool_read();
    let old_generation = app.tools.search_generation.wrapping_sub(1);
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    tx.send((
        old_generation,
        forge_fs::workspace::SearchHit {
            path: app.files.tree_root.join("stale.rs"),
            line: 12,
            text: "stale".into(),
            score: 0,
            content: true,
        },
    ))
    .unwrap();
    app.tools.search_hits = Some(Mutex::new(rx));
    app.tools.pending_read = Some(
        app.services
            .jobs
            .spawn(JobPriority::High, None, async move {
                Ok(ReadReply {
                    generation: Some(old_generation),
                    history_has_older: false,
                    rows: Vec::new(),
                    detail: "stale result".into(),
                })
            }),
    );
    app.tools.panel.as_mut().unwrap().message = "Searching… newest query".into();
    finish(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert!(panel.rows.is_empty());
    assert_eq!(panel.message, "Searching… newest query");
    assert!(!panel.detail.contains("stale result"));
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
                scm_entry(row, u, s).is_some_and(|entry| entry.path == "a.rs" && entry.side == side)
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

#[tokio::test]
async fn sync_hotkeys_are_scoped_to_git_list_focus() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;

    // p/P must never initiate Git writes while browsing Files or reading
    // either Preview. No real pull/push command is issued by this test.
    for activity in [ActivityTab::Files, ActivityTab::Git] {
        for focus in [PaneFocus::List, PaneFocus::Preview] {
            app.session.activity = activity;
            app.session.focus = focus;
            let enabled = matches!((activity, focus), (ActivityTab::Git, PaneFocus::List));
            assert_eq!(app.git_sync_shortcut_enabled(), enabled);

            let advertised = app.context_shortcuts();
            for key in ["p", "P"] {
                assert_eq!(
                    advertised.iter().any(|shortcut| shortcut.key == key),
                    enabled,
                    "activity={activity:?}, focus={focus:?}, shortcut={key}"
                );
            }

            if !enabled {
                for key in ['p', 'P'] {
                    app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
                    assert!(app.scm.pending_write.is_none());
                    assert!(app.tools.pending_task.is_none());
                }
            }
        }
    }
}

#[tokio::test]
async fn preview_focus_keeps_file_tree_unmodified_for_list_only_keys() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let directory = dir.path().join("folder");
    app.files.selected = app
        .files
        .visible
        .iter()
        .position(|row| row.entry.path == directory)
        .expect("folder is visible");
    let selected = app.files.selected;
    let expanded = app.files.expanded.clone();
    let visible = app.files.visible.len();
    app.session.focus = PaneFocus::Preview;

    for key in [KeyCode::Enter, KeyCode::Char('l'), KeyCode::Right] {
        app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        assert_eq!(app.files.selected, selected);
        assert_eq!(app.files.expanded, expanded, "{key:?} expanded left tree");
        assert_eq!(app.files.visible.len(), visible);
        assert_eq!(app.session.focus, PaneFocus::Preview);
    }
    for key in [KeyCode::Char('v'), KeyCode::Char('V')] {
        app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        assert!(app.tools.marks.is_empty(), "{key:?} selected left file");
    }
    let shortcuts = app.context_shortcuts();
    assert!(!shortcuts.iter().any(|s| s.key == "Enter" || s.key == "v"));

    // The same keys still work when the file tree actually owns focus.
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::List);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.files.expanded.contains(&directory));
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.tools.marks.len(), 1);
}

#[tokio::test]
async fn preview_focus_does_not_change_git_sidebar_selection() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    app.scm.scm_selected = app
        .scm_rows()
        .iter()
        .position(|r| matches!(r, ScmRow::Change { .. }))
        .expect("git change row available");
    let selected = app.scm.scm_selected;
    let expanded = app.scm.scm_expanded_unstaged.clone();
    app.session.focus = PaneFocus::Preview;
    for key in [
        KeyCode::Enter,
        KeyCode::Char('l'),
        KeyCode::Right,
        KeyCode::Char('v'),
        KeyCode::Char('V'),
    ] {
        app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        assert_eq!(app.scm.scm_selected, selected);
        assert_eq!(app.scm.scm_expanded_unstaged, expanded);
        assert!(app.tools.marks.is_empty(), "{key:?} marked Git list");
        assert_eq!(app.session.focus, PaneFocus::Preview);
    }
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(app.tools.marks.len(), 1);
}

#[tokio::test]
async fn home_end_follow_preview_focus_without_moving_file_selection() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let path = dir.path().join("a.rs");
    app.files.selected = app
        .files
        .visible
        .iter()
        .position(|row| row.entry.path == path)
        .expect("a.rs is visible");
    app.preview_state.preview = Some(Preview {
        path,
        content: PreviewContent::Text {
            lines: (0..30).map(|i| format!("line {i}")).collect(),
            truncated: false,
            bytes_read: 240,
        },
    });
    app.preview_state.preview_mode = PreviewMode::File;
    app.preview_state.preview_loading = false;
    app.session.layout.preview_inner.height = 5;
    let selected = app.files.selected;
    let expanded = app.files.expanded.clone();
    app.session.focus = PaneFocus::Preview;
    app.preview_state.preview_scroll = 12;

    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert_eq!(app.preview_state.preview_scroll, 0);
    assert_eq!(app.files.selected, selected);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert_eq!(app.preview_state.preview_scroll, 25);
    assert_eq!(app.files.selected, selected);
    assert_eq!(app.files.expanded, expanded);

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.files.selected, 0);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.files.selected, app.files.visible.len() - 1);
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn home_end_follow_git_diff_focus_without_moving_sidebar_selection() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    app.scm.scm_selected = app
        .scm_rows()
        .iter()
        .position(|row| matches!(row, ScmRow::Change { .. }))
        .expect("change is visible");
    app.request_diff_for_scm_selection();
    finish(&mut app).await;
    assert!(app.preview_state.file_diff.is_some());
    app.preview_state.diff_row_count = 35;
    app.session.layout.preview_inner.height = 8;
    app.session.focus = PaneFocus::Preview;
    app.preview_state.preview_scroll = 15;
    let selected = app.scm.scm_selected;
    let expanded = app.scm.scm_expanded_unstaged.clone();

    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert_eq!(app.preview_state.preview_scroll, 0);
    assert_eq!(app.scm.scm_selected, selected);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert_eq!(app.preview_state.preview_scroll, 27);
    assert_eq!(app.scm.scm_selected, selected);
    assert_eq!(app.scm.scm_expanded_unstaged, expanded);

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.scm.scm_selected, 0);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.scm.scm_selected, app.scm_rows().len() - 1);
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn mouse_wheel_scrolls_hovered_files_pane_without_changing_keyboard_focus() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.session.layout.list = Rect::new(0, 0, 20, 20);
    app.session.layout.preview = Rect::new(21, 0, 60, 20);
    app.session.layout.preview_inner = Rect::new(21, 0, 60, 5);
    let wheel = |kind, column| MouseEvent {
        kind,
        column,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    let selected = app.files.selected;
    app.preview_state.preview = Some(Preview {
        path: app.files.tree_root.join("a.rs"),
        content: PreviewContent::Text {
            lines: (0..35).map(|i| format!("line {i}")).collect(),
            truncated: false,
            bytes_read: 300,
        },
    });
    app.preview_state.preview_loading = false;
    app.session.focus = PaneFocus::List;

    // Hovered preview scrolls, but keyboard keys must still target Files.
    app.handle_mouse_content(wheel(MouseEventKind::ScrollDown, 30));
    assert_eq!(app.preview_state.preview_scroll, 3);
    assert_eq!(app.files.selected, selected);
    assert_eq!(app.session.focus, PaneFocus::List);
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(app.files.selected, selected + 1);

    // Wheel moves the Files viewport without moving the selected file;
    // keyboard focus remains on Preview (Tab still returns to Files).
    app.files.selected = 0;
    app.session.focus = PaneFocus::Preview;
    app.handle_mouse_content(wheel(MouseEventKind::ScrollDown, 5));
    assert_eq!(app.files.selected, 0);
    assert_eq!(app.files.scroll, 3.min(app.files.visible.len() - 1));
    assert!(!app.files.follow_selection);
    assert_eq!(app.session.focus, PaneFocus::Preview);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::List);
    // Explicit mouse clicks retain their existing focus-changing semantics.
    app.session.focus = PaneFocus::Preview;
    app.handle_mouse_content(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn mouse_wheel_scrolls_hovered_git_pane_without_changing_keyboard_focus() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    finish(&mut app).await;
    app.session.layout.list = Rect::new(0, 0, 20, 20);
    app.session.layout.preview = Rect::new(21, 0, 60, 20);
    app.session.layout.preview_inner = Rect::new(21, 0, 60, 5);
    let wheel = |kind, column| MouseEvent {
        kind,
        column,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.scm_rows().len() > 1);
    app.preview_state.diff_row_count = 40;
    app.session.focus = PaneFocus::List;
    let selected = app.scm.scm_selected;

    // Wheel over Diff must not turn a subsequent Git shortcut into a hunk action.
    app.handle_mouse_content(wheel(MouseEventKind::ScrollDown, 30));
    assert_eq!(app.preview_state.preview_scroll, 3);
    assert_eq!(app.scm.scm_selected, selected);
    assert_eq!(app.session.focus, PaneFocus::List);
    assert!(app.git_sync_shortcut_enabled());

    // Wheel over the Git sidebar keeps Diff ownership and selection while
    // its tree viewport scrolls independently.
    app.scm.scm_selected = 0;
    app.session.focus = PaneFocus::Preview;
    app.handle_mouse_content(wheel(MouseEventKind::ScrollDown, 5));
    assert_eq!(app.scm.scm_selected, 0);
    assert_eq!(app.scm.scroll_scm, 3);
    assert!(!app.scm.follow_selection);
    assert_eq!(app.session.focus, PaneFocus::Preview);
    assert!(!app.git_sync_shortcut_enabled());
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn history_escape_restores_each_parent_and_cancels_pending_child() {
    let (_dir, mut app) = fixture();
    for kind in [
        PanelKind::Git(GitQuery::History(None)),
        PanelKind::Git(GitQuery::HistoryPage {
            path: Some("a.rs".into()),
            skip: 200,
        }),
    ] {
        app.open_tools(kind);
        finish(&mut app).await;
        let panel = app.tools.panel.as_mut().unwrap();
        panel.selected = 7;
        panel.scroll = 13;
        panel.detail = "parent snapshot".into();
        let parent_kind = panel.kind.clone();
        app.activate_tool_row(
            parent_kind,
            RowValue::Git(GitItem {
                id: "HEAD".into(),
                label: "commit".into(),
                path: None,
            }),
        );
        finish(&mut app).await;
        let panel = app.tools.panel.as_mut().unwrap();
        panel.scroll = 9;
        let value = panel.rows.first().unwrap().value.clone();
        app.activate_tool_row(PanelKind::Git(GitQuery::Commit("HEAD".into())), value);
        // Escape before the asynchronous diff has completed.
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        finish(&mut app).await;
        let panel = app.tools.panel.as_ref().unwrap();
        assert!(matches!(panel.kind, PanelKind::Git(GitQuery::Commit(_))));
        assert_eq!(panel.scroll, 9);
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let panel = app.tools.panel.as_ref().unwrap();
        assert_eq!(panel.selected, 7);
        assert_eq!(panel.scroll, 13);
        assert_eq!(panel.detail, "parent snapshot");
        assert!(matches!(
            &panel.kind,
            PanelKind::Git(GitQuery::History(None))
                | PanelKind::Git(GitQuery::HistoryPage {
                    path: Some(_),
                    skip: 200
                })
        ));
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.tools.panel.is_none());
        assert!(app.tools.history_parents.is_empty());
    }
}

#[tokio::test]
async fn history_paging_stops_at_exact_boundary_and_preserves_file_scope() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    // fixture has one commit; make exactly one full page.
    for _ in 1..200 {
        git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "page",
            ],
        );
    }
    app.open_tools(PanelKind::Git(GitQuery::History(None)));
    finish(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(panel.rows.len(), 200);
    assert!(!panel.history_has_older);
    assert!(panel.detail.contains("Commits 1–200"));
    app.handle_tools_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert!(app.tools.pending_read.is_none());
    git(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "201",
        ],
    );
    app.open_tools(PanelKind::Git(GitQuery::History(None)));
    finish(&mut app).await;
    assert!(app.tools.panel.as_ref().unwrap().history_has_older);
    app.handle_tools_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    app.handle_tools_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    finish(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(panel.rows.len(), 1);
    assert!(panel.detail.contains("Commits 201–201"));
    assert!(!panel.history_has_older);
    app.handle_tools_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    finish(&mut app).await;
    assert_eq!(app.tools.panel.as_ref().unwrap().rows.len(), 200);
    for path in ["a.rs", "missing.rs"] {
        app.open_tools(PanelKind::Git(GitQuery::History(Some(path.into()))));
        finish(&mut app).await;
        let panel = app.tools.panel.as_ref().unwrap();
        assert!(!panel.history_has_older);
        if path == "missing.rs" {
            assert!(panel.detail.starts_with("No commits found"));
        }
        app.handle_tools_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert!(app.tools.pending_read.is_none());
    }
}

#[tokio::test]
async fn directory_navigation_preserves_file_preview_and_pending_reads() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let select = |app: &mut App, path: &Path| {
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|r| r.entry.path == path)
            .unwrap();
        app.request_preview_for_selection();
    };
    let folder = dir.path().join("folder");
    // An initial directory selection leaves the viewer empty.
    app.preview_state.preview = None;
    select(&mut app, &folder);
    assert!(app.preview_state.pending_preview.is_none());
    assert!(app.preview_state.preview.is_none());
    // Selecting a folder while a file loads keeps that requested file.
    select(&mut app, &dir.path().join("a.rs"));
    select(&mut app, &folder);
    finish(&mut app).await;
    assert_eq!(
        app.preview_state.preview.as_ref().unwrap().path,
        dir.path().join("a.rs")
    );
    assert!(!app.preview_state.preview_loading);
    let previous = app.preview_state.preview.clone();
    app.preview_state.preview_scroll = 3;
    app.toggle_expand_at(app.files.selected);
    finish(&mut app).await;
    assert_eq!(app.preview_state.preview, previous);
    assert_eq!(app.preview_state.preview_scroll, 3);
    app.toggle_expand_at(app.files.selected);
    finish(&mut app).await;
    assert_eq!(app.preview_state.preview, previous);
    assert_eq!(app.preview_state.preview_scroll, 3);
    // Another file still replaces the viewer normally.
    select(&mut app, &dir.path().join("outside.txt"));
    finish(&mut app).await;
    assert_eq!(
        app.preview_state.preview.as_ref().unwrap().path,
        dir.path().join("outside.txt")
    );
}

#[tokio::test]
async fn git_folder_navigation_keeps_diff_and_scroll_including_pending_read() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.session.activity = ActivityTab::Git;
    let file = app
        .scm_rows()
        .iter()
        .position(|r| matches!(r, ScmRow::Change { .. }))
        .unwrap();
    app.scm.scm_selected = file;
    app.request_diff_for_scm_selection();
    // Move onto a folder before the file read completes.
    app.scm.scm_selected = app
        .scm_rows()
        .iter()
        .position(|r| matches!(r, ScmRow::Folder { .. }))
        .unwrap();
    app.request_diff_for_scm_selection();
    finish(&mut app).await;
    let path = app
        .preview_state
        .file_diff
        .as_ref()
        .expect("pending diff retained")
        .path
        .clone();
    app.preview_state.preview_scroll = 3;
    for _ in 0..2 {
        app.toggle_scm_expand_at(app.scm.scm_selected);
        app.request_diff_for_scm_selection();
        assert_eq!(app.preview_state.file_diff.as_ref().unwrap().path, path);
        assert_eq!(app.preview_state.preview_scroll, 3);
    }
    // Folder selection must not authorize a hunk operation on the retained file.
    assert!(app
        .stage_target(forge_ui::ContextAction::StageHunk)
        .is_err());
}

#[tokio::test]
async fn open_changed_file_switches_from_git_diff_to_files_view() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    let target = "folder/nested/b.rs";
    expand_ancestors_for_path(&mut app.scm.scm_expanded_unstaged, target);
    let idx = app
        .scm_rows()
        .iter()
        .position(|row| {
            scm_entry(
                row,
                &app.scm.git_status.as_ref().unwrap().unstaged,
                &app.scm.git_status.as_ref().unwrap().staged,
            )
            .is_some_and(|e| e.path == target && e.side == DiffSide::Unstaged)
        })
        .unwrap();
    app.scm.scm_selected = idx;
    app.open_selected_worktree_file();
    assert_eq!(app.session.activity, ActivityTab::Files);
    finish(&mut app).await;
    assert_eq!(
        app.files.visible[app.files.selected].entry.path,
        dir.path().join(target)
    );
    assert_eq!(
        app.preview_state.preview.as_ref().unwrap().path,
        dir.path().join(target)
    );
    assert!(matches!(app.preview_state.preview_mode, PreviewMode::File));

    // Reopening the same loaded file must consume the reveal and focus
    // handoff instead of leaking it into an unrelated later preview.
    app.enter_git_sidebar();
    let reselected = app
        .scm_rows()
        .iter()
        .position(|row| {
            scm_entry(
                row,
                &app.scm.git_status.as_ref().unwrap().unstaged,
                &app.scm.git_status.as_ref().unwrap().staged,
            )
            .is_some_and(|entry| entry.path == target && entry.side == DiffSide::Unstaged)
        })
        .unwrap();
    app.scm.scm_selected = reselected;
    app.open_selected_worktree_file();
    assert!(app.tools.reveal.is_none());
    assert!(app.tools.return_focus.is_none());
    assert_eq!(app.session.focus, PaneFocus::List);

    // A deleted tracked file must not be recreated or navigated to.
    app.enter_git_sidebar();
    std::fs::remove_file(dir.path().join(target)).unwrap();
    app.open_selected_worktree_file();
    assert_eq!(app.session.activity, ActivityTab::Git);
    assert!(app.session.status.contains("missing from this checkout"));
    assert!(!dir.path().join(target).exists());
}

#[tokio::test]
async fn open_file_reveals_cold_nested_directory_through_filters_and_keeps_tree_focused() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;

    // Only the root is cached; children must be asynchronously listed.
    app.files.children.retain(|path, _| path == root);
    app.files.expanded.clear();
    app.rebuild_visible();
    app.files.external_anchor = Some((root.join("outside.txt"), 50));
    app.files.scroll = 25;
    // This user filter normally hides the changed .rs file.
    app.tools.filter.extension = "txt".into();
    app.session.layout.list_inner = Rect::new(0, 3, 29, 9);

    app.enter_git_sidebar();
    let target = "folder/nested/b.rs";
    expand_ancestors_for_path(&mut app.scm.scm_expanded_unstaged, target);
    let idx = app
        .scm_rows()
        .iter()
        .position(|row| {
            scm_entry(
                row,
                &app.scm.git_status.as_ref().unwrap().unstaged,
                &app.scm.git_status.as_ref().unwrap().staged,
            )
            .is_some_and(|entry| entry.path == target && entry.side == DiffSide::Unstaged)
        })
        .unwrap();
    app.scm.scm_selected = idx;
    app.open_selected_worktree_file();
    finish(&mut app).await;

    assert!(app.files.expanded.contains(&root.join("folder")));
    assert!(app.files.expanded.contains(&root.join("folder/nested")));
    let expected = root.join(target);
    assert_eq!(app.selected_entry().unwrap().path, expected);
    assert!(app
        .files
        .visible
        .iter()
        .any(|r| r.entry.path == root.join("folder/nested") && r.expanded));
    assert!(app.files.scroll <= app.files.selected);
    assert!(app.files.selected < app.files.scroll + 9);
    assert_eq!(app.preview_state.preview.as_ref().unwrap().path, expected);
    assert_eq!(app.session.activity, ActivityTab::Files);
    assert_eq!(app.session.focus, PaneFocus::List);
    assert_eq!(app.tools.filter.extension, "txt");
    assert_eq!(app.files.revealed_path.as_deref(), Some(expected.as_path()));
    assert!(app.files.external_anchor.is_none());
}

#[tokio::test]
async fn open_file_from_narrowed_worktree_re_roots_to_reach_external_repository_file() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    finish(&mut app).await;

    app.switch_workspace(root.join("folder"));
    finish(&mut app).await;
    assert_eq!(app.files.tree_root, root.join("folder"));
    app.enter_git_sidebar();
    expand_ancestors_for_path(&mut app.scm.scm_expanded_unstaged, "a.rs");
    let idx = app
        .scm_rows()
        .iter()
        .position(|row| {
            scm_entry(
                row,
                &app.scm.git_status.as_ref().unwrap().unstaged,
                &app.scm.git_status.as_ref().unwrap().staged,
            )
            .is_some_and(|entry| entry.path == "a.rs" && entry.side == DiffSide::Unstaged)
        })
        .unwrap();
    app.scm.scm_selected = idx;
    app.open_selected_worktree_file();
    finish(&mut app).await;
    assert_eq!(app.files.tree_root, root);
    assert_eq!(app.selected_entry().unwrap().path, root.join("a.rs"));
    assert_eq!(
        app.preview_state.preview.as_ref().unwrap().path,
        root.join("a.rs")
    );
    assert_eq!(app.session.focus, PaneFocus::List);
}

#[tokio::test]
async fn discard_directory_only_affects_tracked_unstaged_files_under_clicked_scope() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let root = dir.path();
    git(root, &["add", "--", "folder/nested/b.rs"]);
    std::fs::write(root.join("folder/nested/b.rs"), "second worktree edit\n").unwrap();
    std::fs::write(root.join("folder/nested/keep-untracked.tmp"), "KEEP\n").unwrap();
    app.scm.git_status = Some(app.services.git.status(root).unwrap());

    app.enter_git_sidebar();
    let idx = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(row, ScmRow::Folder {
            column: StagingColumn::Unstaged, path, .. } if path == "folder")
        })
        .unwrap();
    app.scm.scm_selected = idx;
    app.discard_selection();
    let panel = app.tools.panel.as_ref().expect("requires confirmation");
    match &panel.kind {
        PanelKind::Confirm(Operation::Git(GitAction::Discard(paths))) => {
            assert_eq!(paths, &["folder/nested/b.rs"]);
        }
        _ => panic!("expected scoped discard confirmation"),
    }
    assert_eq!(
        std::fs::read_to_string(root.join("folder/nested/b.rs")).unwrap(),
        "second worktree edit\n"
    );
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(
        std::fs::read_to_string(root.join("folder/nested/b.rs")).unwrap(),
        "second worktree edit\n"
    );

    // The right-click menu must ignore unrelated marked Git rows.
    app.tools
        .marks
        .toggle_git(root.join("outside.txt"), DiffSide::Unstaged);
    app.open_staging_folder_menu(idx, 4, 6);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .any(|i| i.action == ContextAction::DiscardChanges));
    app.apply_context_action(ContextAction::DiscardChanges);
    match &app.tools.panel.as_ref().unwrap().kind {
        PanelKind::Confirm(Operation::Git(GitAction::Discard(paths))) => {
            assert_eq!(paths, &["folder/nested/b.rs"]);
        }
        _ => panic!("right-click must be scoped to folder, not marks"),
    }
    app.handle_tools_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    finish(&mut app).await;
    assert_eq!(
        std::fs::read_to_string(root.join("folder/nested/b.rs")).unwrap(),
        "changed\n"
    );
    assert_eq!(git(root, &["show", ":folder/nested/b.rs"]), "changed\n");
    assert_eq!(
        std::fs::read_to_string(root.join("folder/nested/keep-untracked.tmp")).unwrap(),
        "KEEP\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("outside.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn files_directory_discard_context_targets_directory_not_marks() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let root = dir.path();
    app.tools.marks.files.insert(root.join("outside.txt"));
    let folder_idx = app
        .files
        .visible
        .iter()
        .position(|r| r.entry.path == root.join("folder"))
        .unwrap();
    app.open_file_context_menu(folder_idx, 3, 5);
    assert!(app
        .session
        .context_menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .any(|i| i.action == ContextAction::DiscardChanges));
    app.apply_context_action(ContextAction::DiscardChanges);
    match &app.tools.panel.as_ref().unwrap().kind {
        PanelKind::Confirm(Operation::Git(GitAction::Discard(paths))) => {
            assert_eq!(paths, &["folder/nested/b.rs"]);
        }
        _ => panic!("file tree folder discard must be precisely scoped"),
    }
    assert!(app.tools.marks.files.contains(&root.join("outside.txt")));
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

#[tokio::test]
async fn compact_git_folder_click_expands_entire_single_child_chain_once() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    let rows = app.scm_rows();
    let index = rows
        .iter()
        .position(|row| {
            matches!(row, ScmRow::Folder { column: StagingColumn::Unstaged,
            path, expanded: false, .. } if path == "folder")
        })
        .unwrap();
    // This is a mouse click, not a second click on an already selected row.
    app.session.layout.list = Rect::new(0, 2, 40, 20);
    app.session.layout.list_inner = Rect::new(0, 2, 40, 20);
    app.scm.scroll_scm = 0;
    app.scm.sticky_headers.clear();
    app.scm.scm_selected = 0;
    app.handle_mouse_content(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 4,
        row: 2 + index as u16,
        modifiers: KeyModifiers::NONE,
    });
    let rows = app.scm_rows();
    assert!(app.scm.scm_expanded_unstaged.contains("folder"));
    assert!(app.scm.scm_expanded_unstaged.contains("folder/nested"));
    let compressed = rows
        .iter()
        .position(|row| {
            matches!(row, ScmRow::Folder { path, name, expanded: true, .. }
            if path == "folder/nested" && name == "folder / nested")
        })
        .unwrap();
    assert_eq!(app.scm.scm_selected, compressed);
    assert!(rows.iter().any(
        |row| app.scm.git_status.as_ref().is_some_and(|status| scm_entry(
            row,
            &status.unstaged,
            &status.staged
        )
        .is_some_and(|entry| entry.path == "folder/nested/b.rs"))
    ));
    // The next click collapses the one visible compact row, not another
    // hidden segment.
    app.toggle_scm_expand_at(compressed);
    assert!(!app.scm.scm_expanded_unstaged.contains("folder/nested"));
    assert!(!app.scm_rows().iter().any(|row| matches!(
        row,
        ScmRow::Change {
            column: StagingColumn::Unstaged,
            ..
        }
    ) && scm_entry(
        row,
        &app.scm.git_status.as_ref().unwrap().unstaged,
        &app.scm.git_status.as_ref().unwrap().staged
    )
    .is_some_and(|entry| entry.path == "folder/nested/b.rs")));
}

#[test]
fn implicit_compact_chain_stops_at_direct_files_or_sibling_folders() {
    let single = App::compact_expand_paths(
        "forge-config",
        ["forge-config/src/lib.rs", "forge-config/src/mod.rs"],
    );
    assert_eq!(single, ["forge-config", "forge-config/src"]);
    let branch = App::compact_expand_paths(
        "forge-config",
        ["forge-config/src/lib.rs", "forge-config/tests/a.rs"],
    );
    assert_eq!(branch, ["forge-config"]);
    let file = App::compact_expand_paths(
        "forge-config",
        ["forge-config/src/lib.rs", "forge-config/Cargo.toml"],
    );
    assert_eq!(file, ["forge-config"]);
}

#[tokio::test]
async fn compact_staged_folder_expands_to_file_in_one_action() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    git(dir.path(), &["add", "--", "folder/nested/b.rs"]);
    app.scm.git_status = Some(app.services.git.status(dir.path()).unwrap());
    app.enter_git_sidebar();
    let first = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(row, ScmRow::Folder {
            column: StagingColumn::Staged, path, expanded: false, ..
        } if path == "folder")
        })
        .unwrap();
    app.toggle_scm_expand_at(first);
    assert!(app.scm.scm_expanded_staged.contains("folder/nested"));
    assert!(app.scm_rows().iter().any(|row| {
        let (unstaged, staged) = app.scm_slices();
        scm_entry(row, unstaged, staged).is_some_and(|entry| {
            entry.path == "folder/nested/b.rs" && entry.side == DiffSide::Staged
        })
    }));
}

#[tokio::test]
async fn compact_folders_disabled_keeps_one_level_per_action() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.config.compact_folders = false;
    app.enter_git_sidebar();
    let first = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(row, ScmRow::Folder {
            column: StagingColumn::Unstaged, path, expanded: false, ..
        } if path == "folder")
        })
        .unwrap();
    app.toggle_scm_expand_at(first);
    assert!(app.scm.scm_expanded_unstaged.contains("folder"));
    assert!(!app.scm.scm_expanded_unstaged.contains("folder/nested"));
}

#[tokio::test]
async fn compact_files_folder_one_click_loads_uncached_chain_async() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("forge-config/src")).unwrap();
    std::fs::write(root.join("forge-config/src/lib.rs"), "hello\n").unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, root.to_owned());
    finish(&mut app).await;
    let first = app
        .files
        .visible
        .iter()
        .position(|row| row.entry.path == root.join("forge-config"))
        .unwrap();
    assert!(!app.files.children.contains_key(&root.join("forge-config")));
    app.toggle_expand_at(first);
    finish(&mut app).await;
    assert!(app.files.expanded.contains(&root.join("forge-config")));
    assert!(app.files.expanded.contains(&root.join("forge-config/src")));
    assert!(app.files.compact_expand_pending.is_none());
    let rows = &app.files.visible;
    let merged = rows
        .iter()
        .position(|row| {
            row.entry.path == root.join("forge-config/src")
                && row.entry.name == "forge-config / src"
                && row.expanded
        })
        .unwrap();
    assert_eq!(app.files.selected, merged);
    assert!(rows
        .iter()
        .any(|row| row.entry.path == root.join("forge-config/src/lib.rs")));
    app.toggle_expand_at(merged);
    assert!(!app
        .files
        .visible
        .iter()
        .any(|row| row.entry.path == root.join("forge-config/src/lib.rs")));
}

#[tokio::test]
async fn git_double_click_opens_worktree_file_but_single_click_keeps_diff() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    let rows = app.scm_rows();
    let index = rows
        .iter()
        .position(|row| {
            scm_entry(
                row,
                &app.scm.git_status.as_ref().unwrap().unstaged,
                &app.scm.git_status.as_ref().unwrap().staged,
            )
            .is_some_and(|e| e.path == "a.rs")
        })
        .unwrap();
    app.session.layout.list = Rect::new(0, 2, 40, 20);
    app.session.layout.list_inner = Rect::new(0, 2, 40, 20);
    app.scm.scroll_scm = 0;
    app.scm.sticky_headers.clear();
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 2 + index as u16,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_content(click);
    assert_eq!(app.session.activity, ActivityTab::Git);
    assert_eq!(app.scm.scm_selected, index);
    app.handle_mouse_content(click);
    assert_eq!(app.session.activity, ActivityTab::Files);
    finish(&mut app).await;
    assert_eq!(
        app.files.visible[app.files.selected].entry.path,
        dir.path().join("a.rs")
    );
    assert_eq!(
        app.preview_state.preview.as_ref().unwrap().path,
        dir.path().join("a.rs")
    );
}

#[tokio::test]
async fn mouse_wheel_scrolls_files_without_changing_selection_or_preview() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    let root = dir.path();
    let src = root.join("src");
    app.files.visible = std::iter::once(TreeRow {
        entry: Entry {
            id: forge_model::PathId(101),
            name: "src".into(),
            path: src.clone(),
            kind: EntryKind::Directory,
        },
        depth: 0,
        expanded: true,
    })
    .chain((0..35).map(|n| TreeRow {
        entry: Entry {
            id: forge_model::PathId(102 + n),
            name: format!("f{n:02}.txt"),
            path: src.join(format!("f{n:02}.txt")),
            kind: EntryKind::File,
        },
        depth: 1,
        expanded: false,
    }))
    .collect();
    app.files.selected = 0;
    app.files.scroll = 0;
    app.session.focus = PaneFocus::List;
    app.session.layout.list = Rect::new(0, 2, 32, 9);
    app.session.layout.list_inner = Rect::new(0, 2, 32, 9);
    app.session.layout.preview = Rect::new(34, 2, 45, 9);
    let preview_before = app.preview_state.preview.as_ref().map(|p| p.path.clone());
    for _ in 0..4 {
        app.handle_mouse_content(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(app.files.scroll, 12);
    assert_eq!(
        app.files.selected, 0,
        "wheel must not move the selected file"
    );
    assert_eq!(app.session.focus, PaneFocus::List);
    assert!(!app.files.follow_selection);
    assert_eq!(
        app.preview_state.preview.as_ref().map(|p| p.path.clone()),
        preview_before
    );

    // The sticky ancestor of the first visible row should be src.
    assert_eq!(
        forge_ui::find_parent_row(&app.files.visible, app.files.scroll),
        Some(0)
    );
    app.move_sel(1);
    assert!(
        app.files.follow_selection,
        "keyboard selection restores follow mode"
    );
    assert_eq!(app.files.selected, 1);
}

#[tokio::test]
async fn mouse_wheel_scrolls_git_tree_without_changing_selected_diff() {
    let (_dir, mut app) = fixture();
    finish(&mut app).await;
    app.enter_git_sidebar();
    assert!(app.scm_rows().len() > 4);
    app.session.layout.list = Rect::new(0, 2, 40, 8);
    app.session.layout.list_inner = Rect::new(0, 2, 40, 8);
    app.session.layout.preview = Rect::new(42, 2, 55, 8);
    let selected = app.scm.scm_selected;
    let pending = app.preview_state.pending_diff.as_ref().map(|p| p.id);
    app.handle_mouse_content(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 4,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.scm.scroll_scm, 3);
    assert_eq!(app.scm.scm_selected, selected);
    assert!(!app.scm.follow_selection);
    assert_eq!(
        app.preview_state.pending_diff.as_ref().map(|p| p.id),
        pending
    );
    app.move_scm(1);
    assert!(app.scm.follow_selection);
}

#[tokio::test]
async fn tool_list_navigation_separates_selection_detail_and_mouse_scroll() {
    let (_dir, mut app) = fixture();
    let mut panel = Panel::new(PanelKind::Files, "Find file");
    panel.rows = (0..50)
        .map(|i| ToolRow {
            label: format!("file_{i:02}.rs"),
            value: RowValue::File(app.files.tree_root.join(format!("file_{i:02}.rs")), 1),
        })
        .collect();
    panel.list_area = Rect::new(0, 3, 35, 8);
    panel.scroll_limit = 80;
    app.tools.panel = Some(panel);

    app.handle_tools_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 7);
    assert_eq!(app.tools.panel.as_ref().unwrap().scroll, 0);
    app.handle_tools_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 0);
    app.handle_tools_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::CONTROL));
    assert_eq!(app.tools.panel.as_ref().unwrap().scroll, 10);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 0);

    let wheel = |column, row| MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_tools_mouse(wheel(4, 4));
    assert_eq!(app.tools.panel.as_ref().unwrap().list_offset, 3);
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 0);
    assert_eq!(app.tools.panel.as_ref().unwrap().scroll, 10);

    app.handle_tools_mouse(wheel(45, 20));
    assert_eq!(app.tools.panel.as_ref().unwrap().list_offset, 3);
    assert_eq!(app.tools.panel.as_ref().unwrap().scroll, 13);

    app.handle_tools_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 4,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tools.panel.as_ref().unwrap().selected, 4);
    assert_eq!(app.tools.panel.as_ref().unwrap().scroll, 0);
}

#[tokio::test]
async fn tool_branch_default_and_doubleclick_share_safe_confirmation() {
    let (dir, mut app) = fixture();
    git(dir.path(), &["branch", "feature/safe"]);
    finish(&mut app).await;
    app.open_tools(PanelKind::Git(GitQuery::Branches));
    finish(&mut app).await;
    let panel = app.tools.panel.as_ref().unwrap();
    assert!(matches!(&panel.rows[panel.selected].value,
        RowValue::Git(item) if item.id == "main"));
    let branch_index = panel
        .rows
        .iter()
        .position(|row| matches!(&row.value, RowValue::Git(item) if item.id == "feature/safe"))
        .unwrap();
    let area = Rect::new(0, 3, 38, 12);
    app.tools.panel.as_mut().unwrap().list_area = area;
    app.tools.panel.as_mut().unwrap().list_offset = 0; // as a full-size render would clamp
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: area.y + branch_index as u16,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_tools_mouse(click);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Branches))
    ));
    app.handle_tools_mouse(click);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm(_))
    ));
    assert_eq!(
        git(dir.path(), &["branch", "--show-current"]).trim(),
        "main"
    );
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Git(GitQuery::Branches))
    ));
}

#[tokio::test]
async fn search_drafts_remain_per_worktree_and_parent_command_back_is_stable() {
    let (dir, mut app) = fixture();
    finish(&mut app).await;
    app.open_tools(PanelKind::Content);
    let panel = app.tools.panel.as_mut().unwrap();
    panel.input = tui_input::Input::new("needle".into());
    panel.regex = true;
    app.close_tools();
    app.open_tools(PanelKind::Content);
    let panel = app.tools.panel.as_ref().unwrap();
    assert_eq!(panel.input.value(), "needle");
    assert!(panel.regex);

    // Switching to Commands and back retains the whole search session.
    app.open_history_child(PanelKind::Commands);
    app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let panel = app.tools.panel.as_ref().unwrap();
    assert!(matches!(panel.kind, PanelKind::Content));
    assert_eq!(panel.input.value(), "needle");
    app.close_tools();

    let other = tempfile::tempdir().unwrap();
    app.files.tree_root = other.path().to_path_buf();
    app.open_tools(PanelKind::Content);
    assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "");
    assert!(!app.tools.panel.as_ref().unwrap().regex);
    app.close_tools();
    app.files.tree_root = dir.path().to_path_buf();
    app.open_tools(PanelKind::Content);
    assert_eq!(app.tools.panel.as_ref().unwrap().input.value(), "needle");
    assert!(app.tools.panel.as_ref().unwrap().regex);
}

#[tokio::test]
async fn no_git_repository_command_palette_filters_git_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_path_buf());
    app.open_tools(PanelKind::Commands);
    app.tools.panel.as_mut().unwrap().input = tui_input::Input::new("Push".into());
    app.reload_tools();
    let panel = app.tools.panel.as_ref().unwrap();
    assert!(panel.rows.is_empty());
    assert!(panel.message.contains("Git operations need a repository"));
}

#[tokio::test]
async fn scm_section_right_click_has_group_actions_with_safe_discard() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    app.session.activity = ActivityTab::Git;
    let section = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                ScmRow::Section {
                    column: StagingColumn::Unstaged,
                    ..
                }
            )
        })
        .unwrap();
    app.open_staging_section_menu(section, 3, 4);
    let menu = app.session.context_menu.as_ref().unwrap();
    assert_eq!(
        menu.target,
        ContextTarget::StagingSection {
            column: StagingColumn::Unstaged
        }
    );
    assert_eq!(
        menu.items
            .iter()
            .map(|item| item.action)
            .collect::<Vec<_>>(),
        vec![
            ContextAction::DiscardAllChanges,
            ContextAction::StageAllChanges,
            ContextAction::StashUnstagedChanges,
            ContextAction::StashAllChanges,
        ]
    );
    assert!(menu.items.iter().all(|item| item.enabled));

    std::fs::write(root.join("new-untracked"), "preserve me\n").unwrap();
    app.apply_context_action(ContextAction::DiscardAllChanges);
    assert!(matches!(
        app.tools.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm(Operation::Git(GitAction::Discard(_))))
    ));
    assert!(root.join("new-untracked").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn scm_section_stage_all_then_unstage_all_preserves_worktree() {
    let (dir, mut app) = fixture();
    let root = dir.path();
    app.session.activity = ActivityTab::Git;
    std::fs::write(root.join("untracked-file"), "new\n").unwrap();
    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    let section = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                ScmRow::Section {
                    column: StagingColumn::Unstaged,
                    ..
                }
            )
        })
        .unwrap();
    app.open_staging_section_menu(section, 3, 4);
    app.apply_context_action(ContextAction::StageAllChanges);
    assert!(app.scm.pending_write.is_some());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if app.poll_pending_write() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!git(root, &["diff", "--cached", "--name-only"]).is_empty());
    assert!(git(root, &["status", "--porcelain"])
        .lines()
        .all(|line| !line.starts_with("??")));

    app.scm.git_status = Some(app.services.git.status(root).unwrap());
    let staged = app
        .scm_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                ScmRow::Section {
                    column: StagingColumn::Staged,
                    ..
                }
            )
        })
        .unwrap();
    app.open_staging_section_menu(staged, 3, 4);
    assert_eq!(
        app.session.context_menu.as_ref().unwrap().items[0].action,
        ContextAction::UnstageAllChanges
    );
    app.apply_context_action(ContextAction::UnstageAllChanges);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if app.poll_pending_write() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(git(root, &["diff", "--cached", "--name-only"]).is_empty());
    assert!(root.join("untracked-file").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "changed\n"
    );
}

#[tokio::test]
async fn scm_section_right_click_and_keyboard_open_same_menu() {
    let (_dir, mut app) = fixture();
    app.session.activity = ActivityTab::Git;
    app.session.layout.list = Rect::new(0, 0, 45, 25);
    app.session.layout.list_inner = Rect::new(0, 0, 45, 25);
    app.scm.scroll_scm = 0;
    app.handle_mouse_content(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 2,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(
        app.session.context_menu.as_ref().map(|menu| &menu.target),
        Some(ContextTarget::StagingSection {
            column: StagingColumn::Unstaged
        })
    ));
    app.dismiss_menu();
    app.scm.scm_selected = 0;
    app.session.focus = PaneFocus::List;
    app.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT));
    assert!(matches!(
        app.session.context_menu.as_ref().map(|menu| &menu.target),
        Some(ContextTarget::StagingSection {
            column: StagingColumn::Unstaged
        })
    ));
}

//! Filter noisy filesystem watcher notifications before scheduling a refresh.
use std::path::Path;

use std::path::PathBuf;

/// A linked worktree stores .git as a pointer file, not a directory.
/// Resolve both its private HEAD/index and shared refs, without watching the
/// object store recursively or assuming Git metadata is inside the workspace.
#[derive(Clone, Debug)]
pub(super) struct GitWatchPaths {
    dot_git: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl GitWatchPaths {
    pub fn resolve(repo: &Path) -> Option<Self> {
        let dot_git = repo.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git.canonicalize().ok()?
        } else {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            if target.is_empty() {
                return None;
            }
            let target = Path::new(target);
            (if target.is_absolute() {
                target.to_path_buf()
            } else {
                repo.join(target)
            })
            .canonicalize()
            .ok()?
        };
        let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(relative) => {
                let relative = Path::new(relative.trim());
                (if relative.is_absolute() {
                    relative.to_path_buf()
                } else {
                    git_dir.join(relative)
                })
                .canonicalize()
                .ok()?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
            Err(_) => return None,
        };
        Some(Self {
            dot_git,
            git_dir,
            common_dir,
        })
    }

    pub fn watch_directories(&self) -> impl Iterator<Item = PathBuf> {
        [
            self.git_dir.clone(),
            self.common_dir.clone(),
            self.common_dir.join("refs"),
        ]
        .into_iter()
        .filter(|path| path.is_dir())
    }

    pub fn recursive_refs_root(&self, path: &Path) -> bool {
        path == self.common_dir.join("refs")
    }

    pub fn matches_event(&self, event: &notify::Event) -> bool {
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return false;
        }
        event.paths.iter().any(|path| {
            path == &self.dot_git
                || path == &self.git_dir.join("HEAD")
                || path == &self.git_dir.join("index")
                || path == &self.git_dir.join("config.worktree")
                || path == &self.common_dir.join("packed-refs")
                || path == &self.common_dir.join("config")
                || path.starts_with(self.common_dir.join("refs"))
                || path.starts_with(self.git_dir.join("refs"))
        })
    }
}

pub(super) fn relevant(event: &notify::Event, root: &Path) -> bool {
    use notify::EventKind;
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        if !path.starts_with(root) {
            return false;
        }
        // Git objects and transient lockfiles are internal implementation
        // details; HEAD, refs, and index are intentionally still watched.
        let relative = path.strip_prefix(root).unwrap_or(path);
        let mut components = relative.components();
        if components.next().is_some_and(|c| c.as_os_str() == ".git") {
            if matches!(components.next().as_ref().map(|c| c.as_os_str()), Some(name) if name == "objects" || name == "logs") {
                return false;
            }
        }
        !path.file_name().is_some_and(|name| name == "index.lock")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::{event::ModifyKind, Event, EventKind};
    fn event(path: &str) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.into())
    }
    #[test]
    fn resolves_linked_worktree_private_head_and_common_refs_without_objects() {
        let owner = tempfile::tempdir().unwrap();
        let main = owner.path().join("main");
        let linked = owner.path().join("linked");
        std::fs::create_dir(&main).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&main, &["init", "-qb", "main"]);
        git(&main, &["config", "user.name", "Forge Watch Test"]);
        git(&main, &["config", "user.email", "watch@example.invalid"]);
        std::fs::write(main.join("README.md"), "test\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "baseline"]);
        git(
            &main,
            &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        );
        assert!(linked.join(".git").is_file());
        let paths = GitWatchPaths::resolve(&linked).unwrap();
        assert!(
            paths.git_dir.ends_with("worktrees/linked"),
            "{:?}",
            paths.git_dir
        );
        assert!(
            paths.common_dir.ends_with("main/.git"),
            "{:?}",
            paths.common_dir
        );
        assert!(paths.watch_directories().any(|p| p == paths.git_dir));
        assert!(paths.watch_directories().any(|p| p == paths.common_dir));
        assert!(paths.recursive_refs_root(&paths.common_dir.join("refs")));
        assert!(paths.matches_event(&event(paths.git_dir.join("HEAD").to_str().unwrap())));
        assert!(paths.matches_event(&event(
            paths.common_dir.join("refs/heads/linked").to_str().unwrap()
        )));
        assert!(paths.matches_event(&event(
            paths.common_dir.join("packed-refs").to_str().unwrap()
        )));
        assert!(!paths.matches_event(&event(
            paths.common_dir.join("objects/ab/xyz").to_str().unwrap()
        )));
        assert!(!paths.matches_event(&event(paths.git_dir.join("logs/HEAD").to_str().unwrap())));
        assert!(!paths.matches_event(&event(paths.git_dir.join("index.lock").to_str().unwrap())));
        let main_paths = GitWatchPaths::resolve(&main).unwrap();
        assert_eq!(main_paths.git_dir, main_paths.common_dir);
    }

    #[test]
    fn ignores_git_object_churn_but_not_refs_or_external_edit() {
        let root = Path::new("/example/repo");
        assert!(!relevant(&event("/example/repo/.git/objects/ab/123"), root));
        assert!(!relevant(&event("/example/repo/.git/logs/HEAD"), root));
        assert!(!relevant(&event("/example/repo/.git/index.lock"), root));
        assert!(!relevant(&event("/elsewhere/file.txt"), root));
        assert!(relevant(&event("/example/repo/.git/index"), root));
        assert!(relevant(&event("/example/repo/.git/HEAD"), root));
        assert!(relevant(&event("/example/repo/src/main.rs"), root));
    }
}

#[cfg(test)]
mod selection_tests {
    use super::super::*;

    async fn flush_lists(app: &mut App) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                app.poll_pending_list();
                if app.files.pending_lists.is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("directory loads should finish");
    }

    #[tokio::test]
    async fn a_second_status_request_waits_for_the_running_job() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        app.files.pending_lists.clear();
        app.scm.pending_status = None;
        app.scm.status_rerun = false;

        app.request_status();
        let first = app
            .scm
            .pending_status
            .as_ref()
            .expect("first status job")
            .id;
        app.request_status();
        assert!(app.scm.status_rerun, "overlapping request must be deferred");
        assert_eq!(
            app.scm.pending_status.as_ref().unwrap().id,
            first,
            "deferred request must not replace the running job"
        );

        // Finish the first job; the deferred run must start exactly once.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                app.poll_pending_status();
                if app
                    .scm
                    .pending_status
                    .as_ref()
                    .is_some_and(|pending| pending.id != first)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("deferred status job should start");

        assert!(!app.scm.status_rerun);
        tokio::time::timeout(Duration::from_secs(5), async {
            while app.scm.pending_status.is_some() {
                app.poll_pending_status();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("deferred status job should finish");
    }

    async fn flush_preview(app: &mut App) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                app.poll_pending_preview();
                if app.preview_state.pending_preview.is_none() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("file preview should finish");
    }

    #[tokio::test]
    async fn unrelated_filesystem_refresh_keeps_file_preview_and_scroll() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("reading.rs");
        std::fs::write(
            &file,
            (0..300).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == file)
            .unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        app.preview_state.preview_scroll = 125;
        assert!(app.preview_total_lines() >= 250);
        let before = app.preview_state.preview.clone();

        // An unrelated file write and a Git-index refresh can rebuild the
        // whole visible tree without closing the current reader.
        std::fs::write(dir.path().join("unrelated.txt"), "new\n").unwrap();
        app.refresh_external_changes(false);
        flush_lists(&mut app).await;
        assert_eq!(app.selected_entry().unwrap().path, file);
        assert_eq!(app.preview_state.preview_scroll, 125);
        assert!(app.preview_state.pending_preview.is_none());
        assert_eq!(app.preview_state.preview, before);

        // Genuine selection changes still open at line one.
        let other = dir.path().join("unrelated.txt");
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == other)
            .unwrap();
        app.request_preview_for_selection();
        assert_eq!(app.preview_state.preview_scroll, 0);
    }

    #[tokio::test]
    async fn selected_file_external_edit_updates_content_without_scroll_jump() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("src");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("long.rs");
        let old_content = (0..280)
            .map(|i| format!("old line {i}\n"))
            .collect::<String>();
        let new_content = (0..280)
            .map(|i| format!("new line {i}\n"))
            .collect::<String>();
        std::fs::write(&file, old_content).unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.expanded.insert(folder.clone());
        app.request_missing_expanded();
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == file)
            .unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        assert!(app.preview_total_lines() >= 250);
        app.preview_state.preview_scroll = 88;

        std::fs::write(&file, new_content).unwrap();
        app.refresh_external_changes(false);
        app.refresh_preview_if_affected(&[file.clone()]);
        assert!(
            !app.preview_state.preview_loading,
            "keep the previous preview on screen while reloading"
        );
        assert_eq!(app.preview_state.preview_scroll, 88);
        flush_lists(&mut app).await;
        flush_preview(&mut app).await;
        assert_eq!(app.selected_entry().unwrap().path, file);
        assert_eq!(app.preview_state.preview_scroll, 88);
        match &app.preview_state.preview.as_ref().unwrap().content {
            PreviewContent::Text { lines, .. } => assert!(lines[0].contains("new line 0")),
            other => panic!("expected updated text preview, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn irrelevant_watcher_paths_do_not_re_request_preview() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("source.rs");
        std::fs::write(&file, "original\n").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == file)
            .unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        app.preview_state.preview_scroll = 1;
        app.refresh_preview_if_affected(&[
            dir.path().join(".git/index"),
            dir.path().join("other.rs"),
        ]);
        assert!(app.preview_state.pending_preview.is_none());
        assert_eq!(app.preview_state.preview_scroll, 1);
    }

    #[tokio::test]
    async fn scrolling_fetches_every_page_in_a_thousand_line_source() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("very-long.rs");
        std::fs::write(
            &file,
            (0..2400)
                .map(|i| format!("let row_{i} = {i};\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == file)
            .unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        app.session.layout.preview_inner.height = 25;
        let mut iterations = 0;
        loop {
            let more = matches!(
                app.preview_state.preview.as_ref().unwrap().content,
                PreviewContent::Text {
                    truncated: true,
                    ..
                }
            );
            if !more {
                break;
            }
            app.scroll_preview(isize::MAX - 1);
            assert!(
                app.preview_state.pending_page.is_some(),
                "scrolling to the loaded bottom must fetch next page"
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while app.preview_state.pending_page.is_some() {
                    app.poll_pending_page();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            iterations += 1;
            assert!(iterations < 15);
        }
        let PreviewContent::Text {
            lines, truncated, ..
        } = &app.preview_state.preview.as_ref().unwrap().content
        else {
            panic!("source preview not text");
        };
        assert_eq!(lines.len(), 2400);
        assert_eq!(lines[2399], "let row_2399 = 2399;");
        assert!(!truncated);
        assert!(app.preview_state.preview_scroll > 0);
    }

    #[tokio::test]
    async fn folder_selection_keeps_previous_file_pageable() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("subdir");
        std::fs::create_dir(&folder).unwrap();
        let source = dir.path().join("source.rs");
        std::fs::write(&source,
            (0..1000).map(|i| format!("source {i}\n")).collect::<String>()
        ).unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.selected = app.files.visible.iter()
            .position(|row| row.entry.path == source).unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        let first = match &app.preview_state.preview.as_ref().unwrap().content {
            PreviewContent::Text { lines, .. } => lines.len(),
            _ => unreachable!(),
        };
        app.files.selected = app.files.visible.iter()
            .position(|row| row.entry.path == folder).unwrap();
        app.request_preview_for_selection(); // Directory deliberately leaves previous preview intact.
        app.session.layout.preview_inner.height = 25;
        app.scroll_preview(isize::MAX - 1);
        assert!(app.preview_state.pending_page.is_some());
        tokio::time::timeout(Duration::from_secs(5), async {
            while app.preview_state.pending_page.is_some() {
                app.poll_pending_page();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        assert_eq!(app.preview_state.preview.as_ref().unwrap().path, source);
        assert!(matches!(&app.preview_state.preview.as_ref().unwrap().content,
            PreviewContent::Text { lines, .. } if lines.len() > first));
    }

    #[tokio::test]
    async fn in_flight_page_cannot_append_into_a_different_selected_file() {
        let dir = tempfile::tempdir().unwrap();
        let long = dir.path().join("long.rs");
        let other = dir.path().join("other.rs");
        std::fs::write(
            &long,
            (0..2000).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        std::fs::write(&other, "selected other\n").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == long)
            .unwrap();
        app.request_preview_for_selection();
        flush_preview(&mut app).await;
        app.session.layout.preview_inner.height = 18;
        app.scroll_preview(isize::MAX - 1);
        assert!(app.preview_state.pending_page.is_some());
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == other)
            .unwrap();
        app.request_preview_for_selection();
        assert!(app.preview_state.pending_page.is_none());
        flush_preview(&mut app).await;
        assert_eq!(app.preview_state.preview.as_ref().unwrap().path, other);
        assert!(matches!(
            &app.preview_state.preview.as_ref().unwrap().content,
            PreviewContent::Text { lines, .. } if lines == &vec!["selected other".to_owned()]
        ));
    }

    #[tokio::test]
    async fn refresh_keeps_nested_file_selected_after_root_and_children_reload() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("src");
        std::fs::create_dir(&folder).unwrap();
        let target = folder.join("deep.txt");
        std::fs::write(&target, "before").unwrap();
        let mut app = create_workspace_at(ColorMode::Ansi16, dir.path().to_owned());
        flush_lists(&mut app).await;
        app.files.expanded.insert(folder);
        app.request_missing_expanded();
        flush_lists(&mut app).await;
        app.files.selected = app
            .files
            .visible
            .iter()
            .position(|row| row.entry.path == target)
            .unwrap();

        // Both explicit refresh and watcher-driven refresh must retain identity
        // through the temporary state where only the root has been loaded.
        app.request_list();
        flush_lists(&mut app).await;
        assert_eq!(app.selected_entry().unwrap().path, target);

        std::fs::write(&target, "after").unwrap();
        app.refresh_external_changes(false);
        flush_lists(&mut app).await;
        assert_eq!(app.selected_entry().unwrap().path, target);
    }
}

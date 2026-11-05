use super::*;
use std::fs;

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.test"]);
    fs::write(root.join("a.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    dir
}

#[tokio::test]
async fn individual_added_and_deleted_lines_preserve_other_edits() {
    for side in [DiffSide::Unstaged, DiffSide::Staged] {
        for added in [false, true] {
            let dir = fixture();
            let root = dir.path();
            let cli = CliWriteBackend;
            let working = "alpha\nnew beta\ngamma\nextra\ndelta\n";
            fs::write(root.join("a.txt"), working).unwrap();
            if side == DiffSide::Staged {
                git(root, &["add", "a.txt"]);
            }
            let diff = cli.file_diff(root, "a.txt", side).await.unwrap();
            let i = diff
                .lines
                .iter()
                .position(|l| {
                    if added {
                        l.text == "+new beta"
                    } else {
                        l.text == "-beta"
                    }
                })
                .unwrap();
            let patch = diff.line_patch(i).unwrap();
            cli.apply_cached_patch(root, &patch, false)
                .await
                .unwrap_or_else(|e| panic!("{e}\n{patch}"));
            let expected = match (side, added) {
                (DiffSide::Unstaged, false) => "alpha\ngamma\ndelta\n",
                (DiffSide::Unstaged, true) => "alpha\nbeta\nnew beta\ngamma\ndelta\n",
                (DiffSide::Staged, false) => "alpha\nbeta\nnew beta\ngamma\nextra\ndelta\n",
                (DiffSide::Staged, true) => "alpha\ngamma\nextra\ndelta\n",
            };
            assert_eq!(git(root, &["show", ":a.txt"]), expected);
            assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), working);
        }
    }
}

#[tokio::test]
async fn line_patches_handle_new_deleted_quoted_and_no_newline_files() {
    for path in ["space [1].txt", "中文.txt"] {
        let dir = fixture();
        let root = dir.path();
        let cli = CliWriteBackend;
        fs::write(root.join(path), "one\ntwo\nthree").unwrap();
        let diff = cli.file_diff(root, path, DiffSide::Unstaged).await.unwrap();
        let index = diff.lines.iter().position(|l| l.text == "+two").unwrap();
        let patch = diff.line_patch(index).unwrap();
        cli.apply_cached_patch(root, &patch, false)
            .await
            .unwrap_or_else(|e| panic!("{e}\n{patch}"));
        assert_eq!(git(root, &["show", &format!(":{path}")]), "two\n");
        git(root, &["add", "--", path]);
        let diff = cli.file_diff(root, path, DiffSide::Staged).await.unwrap();
        let index = diff.lines.iter().position(|l| l.text == "+three").unwrap();
        let patch = diff.line_patch(index).unwrap();
        cli.apply_cached_patch(root, &patch, false)
            .await
            .unwrap_or_else(|e| panic!("{e}\n{patch}"));
        assert_eq!(git(root, &["show", &format!(":{path}")]), "one\ntwo\n");
        assert_eq!(
            fs::read_to_string(root.join(path)).unwrap(),
            "one\ntwo\nthree"
        );
    }
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    fs::remove_file(root.join("a.txt")).unwrap();
    let diff = cli
        .file_diff(root, "a.txt", DiffSide::Unstaged)
        .await
        .unwrap();
    let index = diff.lines.iter().position(|l| l.text == "-beta").unwrap();
    cli.apply_cached_patch(root, &diff.line_patch(index).unwrap(), false)
        .await
        .unwrap();
    assert_eq!(git(root, &["show", ":a.txt"]), "alpha\ngamma\ndelta\n");
}

#[tokio::test]
async fn history_follows_renames_and_compares_historical_paths() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    git(root, &["mv", "a.txt", "renamed.txt"]);
    git(root, &["commit", "-qm", "rename"]);
    fs::write(root.join("renamed.txt"), "new revision\n").unwrap();
    git(root, &["commit", "-qam", "change"]);
    let (items, _) = cli
        .query(root, GitQuery::History(Some("renamed.txt".into())))
        .await
        .unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items.last().unwrap().path.as_deref(), Some("a.txt"));
    assert_eq!(items[0].path.as_deref(), Some("renamed.txt"));
    let (_, diff) = cli
        .query(
            root,
            GitQuery::Compare {
                old: items[2].id.clone(),
                new: items[0].id.clone(),
                old_path: items[2].path.clone(),
                new_path: items[0].path.clone(),
            },
        )
        .await
        .unwrap();
    assert!(diff.contains("+new revision"), "{diff}");
    let (files, detail) = cli
        .query(root, GitQuery::Commit(items[0].id.clone()))
        .await
        .unwrap();
    assert!(detail.contains("change"));
    assert_eq!(files[0].path.as_deref(), Some("renamed.txt"));
}

#[tokio::test]
async fn branch_stash_worktree_and_history_actions_roundtrip() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    cli.action(root, GitAction::CreateBranch("feature".into()))
        .await
        .unwrap();
    cli.action(
        root,
        GitAction::RenameBranch {
            old: "feature".into(),
            new: "feature-next".into(),
        },
    )
    .await
    .unwrap();
    cli.action(root, GitAction::SwitchBranch("feature-next".into()))
        .await
        .unwrap();
    fs::write(root.join("a.txt"), "feature\n").unwrap();
    git(root, &["commit", "-qam", "feature commit"]);
    cli.action(root, GitAction::Amend("amended".into()))
        .await
        .unwrap();
    let oid = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    cli.action(root, GitAction::SwitchBranch("main".into()))
        .await
        .unwrap();
    cli.action(root, GitAction::CherryPick(oid)).await.unwrap();
    let head = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    cli.action(root, GitAction::Revert(head)).await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "alpha\nbeta\ngamma\ndelta\n"
    );
    fs::write(root.join("untracked.txt"), "saved\n").unwrap();
    cli.action(root, GitAction::SaveStash("message".into()))
        .await
        .unwrap();
    let (stashes, _) = cli.query(root, GitQuery::Stashes).await.unwrap();
    assert_eq!(stashes.len(), 1);
    let id = stashes[0].id.clone();
    let (_, detail) = cli
        .query(root, GitQuery::StashDiff(id.clone()))
        .await
        .unwrap();
    assert!(detail.contains("+saved"));
    cli.action(root, GitAction::ApplyStash(id.clone()))
        .await
        .unwrap();
    assert!(root.join("untracked.txt").exists());
    cli.action(root, GitAction::DropStash(id)).await.unwrap();
    assert!(cli
        .query(root, GitQuery::Stashes)
        .await
        .unwrap()
        .0
        .is_empty());
    let parent = tempfile::tempdir().unwrap();
    let worktree = parent.path().join("agent");
    cli.action(root, GitAction::AddWorktree(worktree.clone()))
        .await
        .unwrap();
    let (worktrees, _) = cli.query(root, GitQuery::Worktrees).await.unwrap();
    assert_eq!(worktrees.len(), 2);
    cli.action(root, GitAction::RemoveWorktree(worktree))
        .await
        .unwrap();
    cli.action(root, GitAction::DeleteBranch("feature-next".into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn discard_hunk_preserves_index_and_other_hunks_and_conflicts_resolve() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    let base = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(root.join("a.txt"), &base).unwrap();
    git(root, &["commit", "-qam", "long base"]);
    let edited = base
        .replace("line 2\n", "changed 2\n")
        .replace("line 40\n", "changed 40\n");
    fs::write(root.join("a.txt"), &edited).unwrap();
    let diff = cli
        .file_diff(root, "a.txt", DiffSide::Unstaged)
        .await
        .unwrap();
    cli.discard_hunk(root, &diff, &diff.hunks()[0])
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        base.replace("line 40\n", "changed 40\n")
    );
    assert!(git(root, &["diff", "--cached"]).is_empty());
    git(root, &["restore", "a.txt"]);
    git(root, &["switch", "-qc", "other"]);
    fs::write(root.join("a.txt"), "theirs\n").unwrap();
    git(root, &["commit", "-qam", "theirs"]);
    git(root, &["switch", "-q", "main"]);
    fs::write(root.join("a.txt"), "ours\n").unwrap();
    git(root, &["commit", "-qam", "ours"]);
    let merge = std::process::Command::new("git")
        .args(["merge", "other"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let (conflicts, _) = cli.query(root, GitQuery::Conflicts).await.unwrap();
    assert_eq!(conflicts.len(), 1);
    cli.action(
        root,
        GitAction::Resolve {
            paths: vec!["a.txt".into()],
            ours: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "theirs\n");
    assert!(cli
        .query(root, GitQuery::Conflicts)
        .await
        .unwrap()
        .0
        .is_empty());
}

#[tokio::test]
async fn remote_status_reports_divergence_and_keeps_pull_errors() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q", "-b", "main"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(root, &["push", "-qu", "origin", "main"]);
    fs::write(root.join("a.txt"), "ahead\n").unwrap();
    git(root, &["commit", "-qam", "ahead"]);
    let (_, status) = cli.query(root, GitQuery::Sync).await.unwrap();
    assert!(status.contains("ahead 1"), "{status}");
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path(),
        &["clone", "-q", remote.path().to_str().unwrap(), "."],
    );
    git(clone.path(), &["config", "user.name", "Test"]);
    git(clone.path(), &["config", "user.email", "test@example.test"]);
    fs::write(clone.path().join("remote.txt"), "remote\n").unwrap();
    git(clone.path(), &["add", "."]);
    git(clone.path(), &["commit", "-qm", "remote"]);
    git(clone.path(), &["push", "-q"]);
    cli.action(root, GitAction::Fetch).await.unwrap();
    let (_, status) = cli.query(root, GitQuery::Sync).await.unwrap();
    assert!(status.contains("ahead 1, behind 1"), "{status}");
    assert!(!status.contains("update: never"));
    let error = cli.pull(root).await.unwrap_err().to_string();
    assert!(error.contains("fast-forward"), "{error}");
}

#[tokio::test]
async fn stale_line_patches_are_rejected_and_deleted_files_can_be_partly_restored() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    fs::write(root.join("a.txt"), "changed\n").unwrap();
    let diff = cli
        .file_diff(root, "a.txt", DiffSide::Unstaged)
        .await
        .unwrap();
    let index = diff
        .lines
        .iter()
        .position(|l| l.kind == DiffLineKind::Add)
        .unwrap();
    fs::write(root.join("a.txt"), "changed again\n").unwrap();
    assert!(cli.apply_line(root, &diff, index).await.is_err());
    assert!(git(root, &["diff", "--cached"]).is_empty());
    fs::remove_file(root.join("a.txt")).unwrap();
    git(root, &["add", "-u"]);
    let diff = cli
        .file_diff(root, "a.txt", DiffSide::Staged)
        .await
        .unwrap();
    let index = diff.lines.iter().position(|l| l.text == "-beta").unwrap();
    cli.apply_line(root, &diff, index).await.unwrap();
    assert_eq!(git(root, &["show", ":a.txt"]), "beta\n");
    assert!(!root.join("a.txt").exists());
}

#[tokio::test]
async fn clean_files_have_no_synthetic_diff_and_deleted_versions_can_be_compared() {
    let dir = fixture();
    let root = dir.path();
    let cli = CliWriteBackend;
    assert!(cli
        .file_diff(root, "a.txt", DiffSide::Unstaged)
        .await
        .unwrap()
        .lines
        .is_empty());
    git(root, &["rm", "-q", "a.txt"]);
    git(root, &["commit", "-qm", "delete"]);
    let (items, _) = cli
        .query(root, GitQuery::History(Some("a.txt".into())))
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    let (_, diff) = cli
        .query(
            root,
            GitQuery::Compare {
                old: items[1].id.clone(),
                new: items[0].id.clone(),
                old_path: items[1].path.clone(),
                new_path: items[0].path.clone(),
            },
        )
        .await
        .unwrap();
    assert!(diff.contains("-alpha"), "{diff}");
    let (older, _) = cli
        .query(
            root,
            GitQuery::HistoryPage {
                path: Some("a.txt".into()),
                skip: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, items[1].id);
}

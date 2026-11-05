use super::*;
use std::path::PathBuf;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub enum GitQuery {
    History(Option<String>),
    HistoryPage {
        path: Option<String>,
        skip: usize,
    },
    Commit(String),
    CommitFile {
        oid: String,
        path: String,
    },
    Compare {
        old: String,
        new: String,
        old_path: Option<String>,
        new_path: Option<String>,
    },
    Branches,
    BranchDiff(String),
    Stashes,
    StashDiff(String),
    Conflicts,
    Worktrees,
    Sync,
}

#[derive(Clone, Debug)]
pub struct GitItem {
    pub id: String,
    pub label: String,
    /// Historical path for a file revision, otherwise an affected file/worktree path.
    pub path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum GitAction {
    CreateBranch(String),
    SwitchBranch(String),
    RenameBranch { old: String, new: String },
    DeleteBranch(String),
    SaveStash(String),
    ApplyStash(String),
    DropStash(String),
    Resolve { paths: Vec<String>, ours: bool },
    MarkResolved(Vec<String>),
    Amend(String),
    CherryPick(String),
    Revert(String),
    Fetch,
    AddWorktree(PathBuf),
    RemoveWorktree(PathBuf),
    Discard(Vec<String>),
    AbortMerge,
    AbortCherryPick,
    AbortRevert,
    ContinueCherryPick,
    ContinueRevert,
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|x| (*x).to_owned()).collect()
}

impl CliWriteBackend {
    pub async fn apply_line(
        &self,
        repo: &Path,
        diff: &FileDiff,
        index: usize,
    ) -> Result<(), GitWriteError> {
        let current = self.file_diff(repo, &diff.path, diff.side).await?;
        if current.lines != diff.lines {
            return Err(GitWriteError::Command {
                op: "line",
                code: -1,
                stderr: "Diff changed; refresh and select the line again".into(),
            });
        }
        let patch = diff.line_patch(index).ok_or(GitWriteError::EmptyPatch)?;
        self.apply_cached_patch(repo, &patch, false).await
    }
    async fn text(&self, repo: &Path, args: Vec<String>) -> Result<String, GitWriteError> {
        let out = run_git(repo, "workbench", args).await?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Structured list payloads use NUL delimiters; paths never become shell code.
    pub async fn query(
        &self,
        repo: &Path,
        query: GitQuery,
    ) -> Result<(Vec<GitItem>, String), GitWriteError> {
        let mut items = Vec::new();
        let detail;
        let (query, skip) = match query {
            GitQuery::HistoryPage { path, skip } => (GitQuery::History(path), skip),
            other => (other, 0),
        };
        match query {
            GitQuery::History(path) => {
                let mut args = strings(&[
                    "--literal-pathspecs",
                    "log",
                    "-200",
                    "--date=short",
                    "--format=%x1e%H%x00%h  %ad  %s  — %an%x00",
                ]);
                args.push(format!("--skip={skip}"));
                if let Some(path) = &path {
                    args.extend(strings(&["--follow", "--name-only", "-z", "--", path]));
                }
                let text = self.text(repo, args).await?;
                for record in text.split('\x1e').skip(1) {
                    let mut fields = record.split('\0');
                    let id = fields.next().unwrap_or_default().trim().to_owned();
                    let label = fields.next().unwrap_or_default().trim().to_owned();
                    let historical = fields
                        .find(|p| !p.trim_matches('\n').is_empty())
                        .map(|p| p.trim_start_matches('\n').to_owned());
                    if !id.is_empty() {
                        items.push(GitItem {
                            id,
                            label,
                            path: if path.is_some() {
                                historical.or_else(|| path.clone())
                            } else {
                                None
                            },
                        });
                    }
                }
                detail = format!("Commits {}–{} · [ newer / ] older · Enter details · Space pin version · D compare pinned → selected · C cherry-pick · R revert",skip+1,skip+items.len());
            }
            GitQuery::Commit(oid) => {
                detail = self
                    .text(
                        repo,
                        strings(&[
                            "show",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--no-color",
                            "--stat",
                            "--format=fuller",
                            &oid,
                            "--",
                        ]),
                    )
                    .await?;
                let files = self
                    .text(
                        repo,
                        strings(&[
                            "diff-tree",
                            "--root",
                            "--no-commit-id",
                            "--name-only",
                            "-r",
                            "-z",
                            &oid,
                            "--",
                        ]),
                    )
                    .await?;
                for path in files.split('\0').filter(|p| !p.is_empty()) {
                    items.push(GitItem {
                        id: oid.clone(),
                        label: path.into(),
                        path: Some(path.into()),
                    });
                }
            }
            GitQuery::CommitFile { oid, path } => {
                detail = self
                    .text(
                        repo,
                        strings(&[
                            "--literal-pathspecs",
                            "show",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--no-color",
                            &oid,
                            "--",
                            &path,
                        ]),
                    )
                    .await?;
            }
            GitQuery::Compare {
                old,
                new,
                old_path,
                new_path,
            } => {
                let old = self
                    .text(
                        repo,
                        strings(&[
                            "rev-parse",
                            "--verify",
                            "--end-of-options",
                            &format!("{old}^{{commit}}"),
                        ]),
                    )
                    .await?
                    .trim()
                    .to_owned();
                let new = self
                    .text(
                        repo,
                        strings(&[
                            "rev-parse",
                            "--verify",
                            "--end-of-options",
                            &format!("{new}^{{commit}}"),
                        ]),
                    )
                    .await?
                    .trim()
                    .to_owned();
                let a = old_path
                    .as_ref()
                    .map_or(old.clone(), |p| format!("{old}:{p}"));
                let b = new_path
                    .as_ref()
                    .map_or(new.clone(), |p| format!("{new}:{p}"));
                let missing = if old_path.is_some() && new_path.is_some() {
                    !run_git_raw(repo, "cat-file", strings(&["cat-file", "-e", &a]))
                        .await?
                        .status
                        .success()
                        || !run_git_raw(repo, "cat-file", strings(&["cat-file", "-e", &b]))
                            .await?
                            .status
                            .success()
                } else {
                    false
                };
                let mut args = strings(&[
                    "--literal-pathspecs",
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                ]);
                if missing {
                    // A deletion revision has no file blob. Compare the two trees at
                    // the historical paths so additions/deletions remain reviewable.
                    args.extend([old, new, "--".into()]);
                    args.extend(old_path);
                    args.extend(new_path);
                } else {
                    args.extend([a, b, "--".into()]);
                }
                detail = self.text(repo, args).await?;
            }
            GitQuery::Branches => {
                let text = self.text(repo, strings(&["for-each-ref", "--format=%(refname:short)%00%(HEAD) %(refname:short)  %(upstream:track)  %(subject)", "refs/heads/"])).await?;
                for line in text.lines() {
                    if let Some((id, label)) = line.split_once('\0') {
                        items.push(GitItem {
                            id: id.into(),
                            label: label.into(),
                            path: None,
                        });
                    }
                }
                detail = "Enter switch · A create · r rename · X delete (merged only) · D diff against HEAD".into();
            }
            GitQuery::BranchDiff(branch) => {
                detail = self
                    .text(
                        repo,
                        strings(&[
                            "diff",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--no-color",
                            "HEAD",
                            &branch,
                            "--",
                        ]),
                    )
                    .await?;
            }
            GitQuery::Stashes => {
                let text = self
                    .text(repo, strings(&["stash", "list", "--format=%H%x00%gd  %gs"]))
                    .await?;
                for line in text.lines() {
                    if let Some((id, label)) = line.split_once('\0') {
                        items.push(GitItem {
                            id: id.into(),
                            label: label.into(),
                            path: None,
                        });
                    }
                }
                detail = "Enter view diff · A save with untracked files · a apply (keeps stash) · X drop".into();
            }
            GitQuery::StashDiff(id) => {
                detail = self
                    .text(
                        repo,
                        strings(&[
                            "stash",
                            "show",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--no-color",
                            "--include-untracked",
                            "-p",
                            &id,
                        ]),
                    )
                    .await?;
            }
            GitQuery::Conflicts => {
                let text = self
                    .text(
                        repo,
                        strings(&["diff", "--name-only", "--diff-filter=U", "-z"]),
                    )
                    .await?;
                for path in text.split('\0').filter(|p| !p.is_empty()) {
                    items.push(GitItem {
                        id: path.into(),
                        label: path.into(),
                        path: Some(path.into()),
                    });
                }
                detail = "Enter preview · e $EDITOR · o use ours · t use theirs · s mark resolved (stage) · : recovery commands".into();
            }
            GitQuery::Worktrees => {
                let text = self
                    .text(repo, strings(&["worktree", "list", "--porcelain", "-z"]))
                    .await?;
                let mut current: Option<GitItem> = None;
                for field in text.split('\0') {
                    if let Some(path) = field.strip_prefix("worktree ") {
                        if let Some(item) = current.take() {
                            items.push(item);
                        }
                        current = Some(GitItem {
                            id: path.into(),
                            label: path.into(),
                            path: Some(path.into()),
                        });
                    } else if let Some(item) = &mut current {
                        if let Some(branch) = field.strip_prefix("branch ") {
                            item.label.push_str(&format!(
                                "  {}",
                                branch.trim_start_matches("refs/heads/")
                            ));
                        }
                    }
                }
                if let Some(item) = current {
                    items.push(item);
                }
                detail =
                    "Enter switch workspace · A create worktree · X remove clean worktree".into();
            }
            GitQuery::Sync => {
                let status = self
                    .text(repo, strings(&["status", "--short", "--branch"]))
                    .await?;
                let fetch = self
                    .text(repo, strings(&["rev-parse", "--git-path", "FETCH_HEAD"]))
                    .await?;
                let fetch_path = PathBuf::from(fetch.trim());
                let fetch_path = if fetch_path.is_absolute() {
                    fetch_path
                } else {
                    repo.join(fetch_path)
                };
                let last = std::fs::metadata(&fetch_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|d| format!("{} seconds ago", d.as_secs()))
                    .unwrap_or_else(|| "never".into());
                detail = format!("{status}\nLast FETCH_HEAD update: {last}\n\nF fetch · p pull --ff-only · P push\nRemote errors remain visible here and in the footer.");
            }
            GitQuery::HistoryPage { .. } => unreachable!(),
        }
        Ok((items, detail))
    }

    pub async fn action(&self, repo: &Path, action: GitAction) -> Result<String, GitWriteError> {
        let summary = match &action {
            GitAction::CreateBranch(name) => format!("Created branch {name}"),
            GitAction::SwitchBranch(name) => format!("Switched to branch {name}"),
            GitAction::RenameBranch { old, new } => format!("Renamed branch {old} to {new}"),
            GitAction::DeleteBranch(name) => format!("Deleted branch {name}"),
            GitAction::SaveStash(_) => "Saved tracked and untracked changes to stash".into(),
            GitAction::ApplyStash(_) => "Applied stash; saved stash kept".into(),
            GitAction::DropStash(_) => "Deleted stash".into(),
            GitAction::Resolve { paths, ours } => format!(
                "Resolved and staged {} files using {}",
                paths.len(),
                if *ours { "ours" } else { "theirs" }
            ),
            GitAction::MarkResolved(paths) => format!("Staged {} resolved files", paths.len()),
            GitAction::Amend(_) => "Amended latest commit with staged changes".into(),
            GitAction::CherryPick(id) => format!("Cherry-picked {id}"),
            GitAction::Revert(id) => format!("Created revert commit for {id}"),
            GitAction::Fetch => "Fetched all remotes".into(),
            GitAction::AddWorktree(path) => format!("Created worktree {}", path.display()),
            GitAction::RemoveWorktree(path) => format!("Removed worktree {}", path.display()),
            GitAction::Discard(paths) => format!(
                "Discarded worktree changes in {} files; staged changes kept",
                paths.len()
            ),
            GitAction::AbortMerge => "Aborted merge".into(),
            GitAction::AbortCherryPick => "Aborted cherry-pick".into(),
            GitAction::AbortRevert => "Aborted revert".into(),
            GitAction::ContinueCherryPick => "Continued cherry-pick".into(),
            GitAction::ContinueRevert => "Continued revert".into(),
        };
        let mut args;
        match action {
            GitAction::CreateBranch(name) => {
                self.text(repo, strings(&["check-ref-format", "--branch", &name]))
                    .await?;
                args = strings(&["branch", "--", &name]);
            }
            GitAction::SwitchBranch(name) => args = strings(&["switch", "--", &name]),
            GitAction::RenameBranch { old, new } => {
                self.text(repo, strings(&["check-ref-format", "--branch", &new]))
                    .await?;
                args = strings(&["branch", "-m", "--", &old, &new]);
            }
            GitAction::DeleteBranch(name) => args = strings(&["branch", "-d", "--", &name]),
            GitAction::SaveStash(message) => {
                args = strings(&["stash", "push", "--include-untracked", "-m", &message])
            }
            GitAction::ApplyStash(id) => args = strings(&["stash", "apply", &id]),
            GitAction::DropStash(id) => {
                // Resolve the pinned object back to its current reflog entry. Never drop a shifted index.
                let text = self
                    .text(repo, strings(&["stash", "list", "--format=%H%x00%gd"]))
                    .await?;
                let reference = text
                    .lines()
                    .find_map(|l| {
                        l.split_once('\0')
                            .filter(|(hash, _)| *hash == id)
                            .map(|(_, r)| r.to_owned())
                    })
                    .ok_or(GitWriteError::NoPaths)?;
                args = strings(&["stash", "drop", &reference]);
            }
            GitAction::Resolve { paths, ours } => {
                args = strings(&[
                    "--literal-pathspecs",
                    "checkout",
                    if ours { "--ours" } else { "--theirs" },
                    "--",
                ]);
                args.extend(paths.clone());
                self.text(repo, args).await?;
                args = strings(&["--literal-pathspecs", "add", "--"]);
                args.extend(paths);
            }
            GitAction::MarkResolved(paths) => {
                args = strings(&["--literal-pathspecs", "add", "--"]);
                args.extend(paths);
            }
            GitAction::Amend(message) => {
                if message.trim().is_empty() {
                    return Err(GitWriteError::EmptyMessage);
                }
                args = strings(&["commit", "--amend", "-m", &message]);
            }
            GitAction::CherryPick(id) => args = strings(&["cherry-pick", &id]),
            GitAction::Revert(id) => args = strings(&["revert", "--no-edit", &id]),
            GitAction::Fetch => args = strings(&["fetch", "--all", "--prune"]),
            GitAction::AddWorktree(path) => {
                args = vec![
                    "worktree".into(),
                    "add".into(),
                    "--".into(),
                    path.to_string_lossy().into(),
                ]
            }
            GitAction::RemoveWorktree(path) => {
                args = vec![
                    "worktree".into(),
                    "remove".into(),
                    "--".into(),
                    path.to_string_lossy().into(),
                ]
            }
            GitAction::Discard(paths) => {
                path_args(&paths.iter().map(Path::new).collect::<Vec<_>>())?;
                args = strings(&["--literal-pathspecs", "restore", "--worktree", "--"]);
                args.extend(paths);
            }
            GitAction::AbortMerge => args = strings(&["merge", "--abort"]),
            GitAction::AbortCherryPick => args = strings(&["cherry-pick", "--abort"]),
            GitAction::AbortRevert => args = strings(&["revert", "--abort"]),
            GitAction::ContinueCherryPick => args = strings(&["cherry-pick", "--continue"]),
            GitAction::ContinueRevert => args = strings(&["revert", "--continue"]),
        }
        let output = run_git_raw(repo, "workbench", args).await?;
        if output.status.success() {
            Ok(summary)
        } else {
            summarize_remote(output, "operation")
        }
    }

    /// Apply an explicitly reviewed patch to the worktree; the index is preserved.
    pub async fn discard_hunk(
        &self,
        repo: &Path,
        diff: &FileDiff,
        hunk: &DiffHunk,
    ) -> Result<(), GitWriteError> {
        if diff.side != DiffSide::Unstaged {
            return Err(GitWriteError::NoHunk);
        }
        let patch = diff.hunk_patch(hunk);
        let mut child = Command::new("git")
            .args(["apply", "--reverse", "--whitespace=nowarn", "-"])
            .current_dir(repo)
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut input) = child.stdin.take() {
            input.write_all(patch.as_bytes()).await?;
        }
        let output = child.wait_with_output().await?;
        summarize_remote(output, "discard-hunk").map(|_| ())
    }
}

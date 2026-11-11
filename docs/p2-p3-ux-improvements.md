# Forge P2/P3 TUI interaction improvements (2026-11-10)

This pass closes the lower-priority findings from the real-terminal UX audit without changing Git repository data or performing any live Push/Commit.

| Area | Behavior |
|---|---|
| Header | Top-right shows changed and staged counts, not a truncated or duplicated upstream string. Bottom left remains the single location for branch and divergence status. |
| Branches | Pressing Enter on the already-current branch does not open a needless Switch confirmation or execute Git. |
| Git context menus | File/folder menus contain file/folder scoped actions only. Commit, Pull, and Push remain in normal Git commands and Sync (S), not right-click actions tied misleadingly to an individual path. |
| Keyboard context menus | Shift-F10 (or terminal Menu key) opens the Files, Git Changes/Staged, folder, or Diff context actions at the focused selection; arrow/j/k selects, Enter executes, Esc dismisses. |
| Worktrees | Each previously visited worktree retains its own expanded directories, selected file path, scroll, file filter, Git expanded folders, and focus. A never-visited worktree starts from the default state. Nothing is persisted outside the running process. |
| Stashes | Z opens list; a applies and keeps stash; I applies with --index, restoring original staged changes while keeping the stash; p pops (applies and drops on success), with conflicts retaining the stash. All require confirmation; Esc cancels. X still drops explicitly. |
| Filename search | Filename-only fuzzy results display the relative path without a fake ':1' content match. Content hits still display the actual line and excerpt. |
| Footer notifications | Success/failure notices emitted through the operation write-status channel expire after about eight seconds, without erasing a later context message. A Git write still running is never silently shown as completed. Persistent branch/sync state is unaffected. |
| History | Commit-history tool already had explicit pagination. The HISTORY accordion in the Git sidebar now independently starts with 200 commits and requests additional 200-commit background graph windows when navigation approaches the oldest loaded entry; never eagerly loads the old 500 on entry. Selected commit/expanded rows are preserved, and exhausted history stops re-fetching. A 5000-commit upper bound prevents unbounded TUI memory use. |
| Already implemented | The Sync panel's no-upstream/sync-info display was already present; no duplicate implementation added. |

## Verification

- Run cargo test --workspace --quiet and cargo build -p forge.
- Run scripts/smoke-stash-advanced.py (I, p, Esc, Git index, reflog).
- Run scripts/smoke-p2p3-navigation.py (real Shift-F10 menu and worktree return).
- Run the sidebar_history_loads_older_graph_pages_only_near_end unit test using a real 212-commit repository.
- Run scripts/smoke-stash-keyboard.py (original stash behavior).
- Run scripts/smoke-git-stage-selection.py, scripts/smoke-git-status-bar.py, scripts/smoke-branches-worktrees-esc.py.
- Run scripts/smoke-kit.py, scripts/smoke-workspace.py and search/preview regressions.

No live repository was modified by these tests; they create isolated temp Git repositories and tmux instances. Worktree navigation snapshots are in memory for this Forge process, not durable across program restarts.

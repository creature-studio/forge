# File operations and Git worktree reliability

This change targets failure integrity and external-state correctness. It does not claim cross-file transactions or forced I/O cancellation.

## Filesystem operations

- **Copy regular files:** open the source and verify its regular-file type before creating output. Copy to a private temporary file in the destination directory, apply permissions and sync, then publish with persist_noclobber. A failed read leaves neither a zero-byte final file nor a temporary artifact.
- **Copy directories:** recursively stage a full private directory adjacent to the target. Remove staging automatically on error; publish the directory only after the entire tree succeeds. Existing destinations are rejected. Directory publication uses std::fs::rename after an existence check, so a concurrent creator of the same empty directory remains a rare no-overwrite race.
- **Symlinks:** replicate the link without following its target, preserving existing behavior.
- **Special files:** FIFO, socket, device and other non-regular entries are rejected for copying, rather than opening a potentially blocking FIFO. Moving them uses normal filesystem rename semantics.
- **Batch Copy/Move/Delete:** validate all selected paths before starting. On error, report the count completed, failed source, underlying error, and count not attempted. Do not imply preceding changes were rolled back. A failed Delete directory might itself have been partially deleted by the OS.

**Remaining limitations:** No system trash/recycle-bin backend, no atomic transaction across multiple files, and no forced cancellation of a blocking filesystem operation. Esc closes the panel, not the underlying work. The current permanent-delete confirmation is unchanged.

## Git metadata and linked worktrees

The watcher resolves either a .git directory or a .git pointer file (gitdir: path) plus its commondir. It watches the current worktree's HEAD/index and common refs (recursive only inside refs), packed-refs and config. Metadata events outside the workspace root now trigger an ordinary Git status refresh. The object store and reflogs are not recursively watched.

This addresses external branch switches in a linked worktree leaving the status bar stale until manual refresh. Watcher failures and unsupported filesystems may still require manual refresh.

## Search result provenance

SearchHit.content distinguishes filename results from content results independently of the preview text. A zero-length regex match (for example ^$ on a blank line) displays its real line number and (empty line) marker. Filename-only hits continue to show a relative path only.

## Verification

Unit tests cover special files, unreadable sources, incomplete directory cleanup, partial batch reports, linked-worktree metadata resolution, and empty regex matches.

Repeatable real TUI scripts:
- scripts/smoke-safe-file-copy.py
- scripts/smoke-fifo-copy.py
- scripts/smoke-search-empty-and-partial-delete.py
- scripts/smoke-linked-worktree-watch.py

All scripts operate on disposable temporary repositories and files.

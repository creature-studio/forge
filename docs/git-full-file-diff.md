# Git Diff: full-file split view

The Git Diff pane now displays the **complete old/new source** (including unchanged
lines) when a bounded complete new-side snapshot can be read. Both sides have
independent line numbers and syntax highlighting; added and removed lines keep
their respective diff backgrounds.

The original three-context-line `FileDiff.lines` patch is **unchanged** and remains
the only source for Stage Hunk, Unstage Hunk, hunk navigation and patch creation.
`FileDiff.full_new_lines` holds a separate, optional complete new-side text
snapshot: working tree for Unstaged, Git Index for Staged, and the commit blob
for Commit Diff. The view fills missing unchanged contexts from that snapshot,
while keeping each original patch line's `src_indices` for navigation. It
verifies new-side patch text matches the fetched snapshot before expanding;
on mismatch it falls back to the original patch-only display.

The snapshot is bounded at 8 MiB, UTF-8 only, and declines NUL-containing
binary data. Oversized or unreadable blobs retain the previous patch-only
fallback rather than risking an unbounded allocation. This is **not** yet a
random-access paging implementation for very large source files.

Verification: `cargo test -p forge-ui full_diff`,
`cargo test -p forge-git-cli`, `cargo test -p forge-app`, and
`python3 scripts/smoke-workspace.py`. The targeted long-file test renders
source line 1062 in a 1400-line file whose only change is at line 1100.

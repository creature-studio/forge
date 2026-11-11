# File source change gutter and inline peek

The Files viewer compares the selected text file's worktree contents with the
Git index. Green line numbers indicate additions, yellow indicates replacements
(removed and added lines in the same change block), and red marks a deletion
anchor. A deletion anchors to the next surviving line, or the last source line
at EOF. An empty file uses its empty row as line 1. Staged-only changes do not
appear, because the baseline is the index.

Click a colored line number or its adjacent separator to toggle that hunk's
read-only peek. Only the first display row of a wrapped source line is marked
and clickable. With Preview focused, **F7** toggles the first hunk at or after
the top visible row, falling back to the first hunk. The preview heading shows
“F7 Peek” when changes are available. This action is listed and can be rebound
in **F4** shortcut settings; existing explicit F7 assignments take precedence.

The small overlay stays inside the source preview, leaving the gutter exposed.
Removed lines are red with `-`, added lines green with `+`, and context gray.
The title contains the hunk header, displayed-line offset and total line count.
Scroll the mouse wheel over the peek to inspect longer hunks. Click its title
bar (including `[×]`), press F7, or press Esc to close it. Scrolling the source
can hide a peek whose anchor leaves the viewport; the overlay becomes visible
again when its anchor returns. Very small panes (under 20 columns or 4 rows)
show gutter marks without a peek until resized.

Source rows are never inserted or reordered. The viewer keeps File mode and
the existing source/wrapped-row mapping, scrolling, text selection, editor,
Git blame and separate Diff page. Peek text is not part of source selection.
Rendered Markdown is excluded, including gutter drawing, mouse hit testing and
heading hints. Markdown's raw **Code** tab participates.

Queries run as cancellable low-priority background jobs. The one-second tick
reads bounded source bytes and the index stamp; unchanged snapshots reuse the
current file's hunk cache without spawning Git. Selection requests and external
refreshes cancel and clear the state. Results are guarded by path, preview
content, and before/after source and index snapshots. A background source save
refreshes the matching preview and clears old selection and blame attribution.

Only regular, complete text previews up to 64 KiB and the existing 400-line
preview limit participate. Untracked files, binary files, truncated previews,
errors, and diffs exceeding 2 MiB have no markers. Queries time out after three
seconds, kill abandoned children, disable optional Git locks, external diff
drivers and textconv, and use literal pathspecs. This feature runs only
`git diff`, without an untracked-file fallback or Git writes. There is no
cross-file cache; only the active bounded diff is retained.

Focused regression tests live in `crates/forge-app/src/inline_diff/tests.rs`:
real temporary Git repositories cover insertions, deletions (including empty
files and EOF), replacements, no-final-newline and patch-like source text,
unchanged-index/worktree checks, cache invalidation, untracked/binary/large
files, CJK wrapping, gutter toggles, close handling, stale results, external
edits, and rendered Markdown exclusion.

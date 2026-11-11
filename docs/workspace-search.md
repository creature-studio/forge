# Workspace search

Ctrl-P builds an asynchronous snapshot of file paths when opened. The snapshot
belongs to the selected workspace subtree, rather than the enclosing Git root.
Its key includes the root, hidden preference, extension, status allowlist, and
effective exclusions. Typing while it builds changes only the pending query;
it does not cancel and restart enumeration. Once ready, queries use the existing
case-insensitive basename subsequence scorer entirely in memory, on a blocking
worker. Ranking considers every indexed file before retaining the best 2,000
rows, with path order breaking ties.

Only one snapshot is cached. Enumeration uses `ignore::WalkParallel` with at most
four workers. It stops at 100,000 paths or approximately 32 MiB of path payload
and path structures; allocator overhead, the vector capacity, and filter options
are additional. A capped snapshot shows a message asking the user to narrow the
workspace. Cache invalidation follows existing external filesystem refresh and
explicit listing refresh hooks, including writes made by Forge. Searches with
status filters also refresh when the new Git status snapshot arrives. Workspace
switches close an active search and drop the snapshot. Closing and reopening
Ctrl-P can reuse a completed snapshot; closing an unfinished build cancels it.
If filesystem notifications are unavailable, explicitly refresh the listing.

Ctrl-P and Ctrl-F both apply a 150 ms trailing-edge debounce to non-empty input.
Typing again restarts the delay and cancels the previous query generation;
initial empty filename results and empty content queries do not wait.
Ctrl-F uses
the existing Rust `regex` engine and parallel `ignore` walker as the offline
fallback: `grep-searcher` and `grep-regex` sources were absent from this
environment's Cargo cache. No dependency or lockfile changes and no external
`rg` executable are required. Literal search remains case sensitive; Ctrl-R
switches to regex. Empty content queries return no results without scanning.

Each worker reuses a buffer bounded to 2 MiB plus one byte and skips oversized,
NUL-containing, and invalid UTF-8 files. Metadata checks avoid reading known
oversized files; a bounded read also catches files that grow after that check.
Matching still operates on logical lines, preserves exact one-based line
numbers (including CRLF files), and limits excerpts to 400 Unicode characters.
At most 2,000 matching lines are accepted under a shared lock. Reaching the cap
stops traversal. The capped subset depends on parallel traversal order; final
rows sort by path and line. Files that disappear or cannot be read are skipped;
an inaccessible search root or invalid regex produces a visible failure.
Index traversal errors are counted and shown in result details.

Both searches respect Git/ignore rules and the hidden preference, never follow
directory symlinks, and always reject `.git` entries. Directory exclusions
`node_modules`, `target`, `dist`, `build`, `.next`, and `coverage` are enforced
in the filesystem layer even if a caller supplies a replacement exclusions set.
Custom comma-separated exclusions augment these defaults.

Query replacement sets a cooperative cancellation flag and aborts the previous
scheduler job. Blocking workers check cancellation between entries and lines
(an individual bounded file read is not interruptible). Each content stream has
a fresh bounded channel and a generation token. Stale streams and completions
cannot publish into a newer query. Workers notify the existing UI completion
signal on the first hit and every 64 hits, so partial results can render before
the scan completes. Previous rows remain visible with `Searching…` until the
first new batch or final reply; selection is preserved by file and line when
possible. Esc cancels work, drops streams, and closes the panel. Opening another
mode starts with empty rows.

## Selecting content results

Content search uses a results pane and a live source context pane. Terminals at
least 95 columns wide show these side by side; narrower terminals stack them.
After a content query completes, the default selection is the first result in
the final path/line-sorted list, not the first hit returned by a parallel worker.
When the user deliberately moves the selection (keyboard or mouse) while results
stream in, the chosen file and line remain selected after sorting.
Every result remains one selectable row: file boundary marks distinguish groups
without adding fake hits or changing mouse indices. The context title shows the
path, selected line, first matching character column, and matching-line count
for that file. Up/Down and list mouse clicks update context; Enter still opens
the exact result line. Wheel scrolling over results selects hits, while the
wheel over context and PgUp/PgDn scroll the source preview.

The selected line is marked, nearby lines carry source line numbers, and matches
use the scanner's case-sensitive literal or regex semantics. Long lines crop by
Unicode display width around their first match, including matches beyond the
400-character result excerpt. During query updates, the results pane says
Updating and highlighting pauses so old hits cannot imply a new-query match.

One file of context is cached. Its bounded read runs on a blocking worker behind
the `tools:context` scheduler key, independently of search and indexing. Selecting
another hit in that file reuses it without disk access or a new search. Selecting
a different file cancels the old job, clears its context immediately, and shows
Loading context with the hit excerpt until completion. Read failures are visible.
Only regular UTF-8 text files without NUL bytes, at most 2 MiB, are accepted;
the read limit also catches growth after metadata inspection. Existing watcher
and explicit refresh hooks invalidate context, and closing search or switching
workspace cancels pending context work. Each preview exposes up to five lines
before the hit and fills the remaining pane with following lines.

## Context preview validation

The context implementation passed `cargo check --offline --workspace`, focused
context loader/state/rendering tests, `cargo test --offline --workspace --
--test-threads=1`, `cargo build --offline -p forge`, the extended
`python3 scripts/smoke-search.py`, targeted rustfmt checks with `skip_children`,
and `git diff --check`. The full serial suite includes existing blame, editor,
file navigation, and search debounce/jump tests. New cases cover rejected files,
Unicode and regex matching, same-file cache reuse, replacement of pending reads,
external invalidation, wide/narrow rendering, and result mouse selection. The
PTY test checks that source lines before and after the match are visible.

## Measurements

Local debug build, warm filesystem cache, 4,000 synthetic files with 100 copies
of a two-line sample per file (about 10 MB). One run, not a cold-disk benchmark:

| Operation | Elapsed |
| --- | ---: |
| Parallel filename index build | 21.8 ms |
| Five successive cached filename queries, total | 24.3 ms |
| Content query reaching the 2,000-hit cap | 16.2 ms |
| Full content scan with no matching lines | 55.2 ms |

These worker timings exclude the content debounce and terminal rendering. They
are not a comparison with the previous implementation or a production latency
guarantee. Reproduce using:

```sh
cargo test --offline -p forge-fs search_timings -- --ignored --nocapture
```

## Validation

Focused Rust tests cover custom/default exclusions, `.git`, Git ignores, hidden
files, regex and literal matching, CRLF line navigation, Unicode excerpt limits,
binary/invalid/oversized files, result caps, cancellation, cache keys and reuse,
watcher invalidation, workspace narrowing, debounce, visible statuses, and stale
query generations. The cache test deletes the source directory between queries
to prove subsequent filtering needs no filesystem enumeration or file reads.

`scripts/smoke-search.py` exercises filename filtering, content line numbers and
exclusions, regex errors/toggling, cancellation during debounce, switching modes,
and watcher-driven cache refresh in a disposable workspace using a real PTY.
Run it after `cargo build --offline -p forge`.

Validation in this shared working tree (2026-11-10):

- `cargo check --offline --workspace`: passed.
- `cargo test --offline --workspace`: 226 passed, zero failed; the timing fixture
  is ignored by default and passed when run explicitly as shown above.
- Targeted `rustfmt` on edited Rust files and
  `cargo fmt -p forge-fs -p forge-app -- --config skip_children=true --check`:
  passed. `git diff --check` passed.
- `cargo build --offline -p forge` and `python3 scripts/smoke-search.py`: passed.

The initial check found that the UI state requires `Sync`; wrapping the stream
receiver in a mutex resolved it. The first smoke harness assertion also failed
because raw ANSI cursor updates split a displayed word across frames. The smoke
test now reconstructs the screen and passes. No unresolved test failure remains.
All smoke filesystem writes occur in a temporary workspace. Existing staged,
unstaged, and untracked work was preserved; no staging, commits, or deployment
were performed in the shared repository.

# Line blame and commit details

Forge's Files viewer attributes the current source line to its Git commit. The
annotation shows the author, relative age, short SHA and subject. Click a source
row to make it current; after scrolling, the first visible row becomes current.
Wrapped source rows, including Markdown Code and CJK continuations, use the
original source line. A full-width source row keeps its text and shows attribution
in a strip below the viewer. New, staged additions and edited lines show
`Uncommitted changes`.

Press **F6** for a floating commit window with the full SHA, author and dates,
full commit message, and file statistics. Within the window:

| Key | Action |
| --- | --- |
| `d` | Read the commit's diff for this line's historical file path |
| `h` | Read the current file's history, following renames |
| `c` | Copy the SHA through Forge's existing clipboard support |
| `F6` | Return to commit details |
| arrows / `j` / `k` / PageUp / PageDown / Home / End | Scroll |
| Esc / `q` | Close and return to the original file, scroll and focus |

The window captures its keyboard and mouse input. It never stages or writes
repository contents. Uncommitted lines open an explanatory window without fake
commit metadata or commit actions. F6 appears in help and the F4 shortcut registry
as **Line blame / commit details** and can be rebound there. Existing explicit
assignments to F6 take priority over the new default.

## Preferences

Use the existing `$XDG_CONFIG_HOME/forge/config.toml`, or
`~/.config/forge/config.toml`. Missing values default to `true`:

```toml
[git.blame]
enabled = true
show_inline = true
show_popup = true
show_uncommitted = true
```

`enabled` disables all attribution work. Inline display and popup access can be
disabled separately. Disabling both skips background attribution work.
`show_uncommitted = false` hides uncommitted attribution and its popup.
Preferences take effect when the workspace next starts. The existing settings
writer preserves this nested table when updating other preferences.

## Bounds and behavior

- Blame runs asynchronously through Forge's job scheduler and the Git facade.
- Eight file results are cached, including failed Git reads. Validation runs at
  most once per second, using bounded file reads, a content fingerprint and gix
  HEAD/index metadata. Cache hits run no Git subprocess. Cursor movement uses
  the cached source-line vector. File edits, HEAD changes and index changes
  invalidate attribution. A read racing a file/HEAD/index change is discarded.
- Only regular text files up to 1 MiB are attributed, for at most the 400 source
  lines loaded by the existing 64 KiB preview. Git blame has a five-second
  deadline. Commit queries also have a five-second deadline; popup display is
  capped at 256 KiB and reports truncation.
- Binary files, directory previews, symlinks and files outside a repository have
  no attribution. Choosing a folder keeps the existing empty/preserved file
  preview behavior; blame never requests a directory preview.
- **Formatted Markdown Preview has no source-offset API in the existing
  renderer. Per-line blame is therefore available in its Code tab (F3).** The
  formatted tab suppresses attribution and F6 explains how to switch, rather
  than showing an incorrect commit for a rendered row.
- The historical path from porcelain blame is used for commit diffs, including
  renamed files and C-quoted filenames. History uses the current path and follows
  renames. The history overlay shows the existing query's first page; use the
  existing H history dialog for pagination and advanced history operations.
- Clipboard failures are shown in the popup and do not close it. Desktop
  clipboard availability depends on the terminal/session, as elsewhere in Forge.

## Verification

Implementation and verification were performed in the original dirty working
tree, which already had many staged, unstaged and untracked changes across app,
UI, Git, configuration, docs and smoke scripts. Existing changes were retained.
No reset, checkout, stash, clean, commit, push or deployment was run against this
working tree. Test Git writes occur only in isolated temporary repositories.

Final checks on 2026-11-10:

| Command | Outcome |
| --- | --- |
| `cargo check --workspace` | PASS, exit 0 |
| `cargo build -p forge` | PASS, exit 0 |
| `cargo test -p forge-app -p forge-config -p forge-git-cli -p forge-git -p forge-git-gix -p forge-fs -p forge-ui -- --test-threads=1` | PASS, exit 0; 198 unit tests, 0 failures; all seven doc-test runs pass (0 tests each) |
| `cargo fmt --check -p forge-app -p forge-config -p forge-git-cli -p forge-git -p forge-git-gix` | PASS, exit 0 |
| `rustfmt --check --edition 2021 --config skip_children=true crates/forge-fs/src/lib.rs crates/forge-ui/src/preview_pane.rs crates/forge-ui/src/markdown.rs crates/forge-ui/src/lib.rs` | PASS, exit 0 |
| `git diff --check` | PASS, exit 0 |
| `cargo fmt --all -- --check` | FAIL, exit 1; existing formatting in three untouched files: `crates/forge-fs/src/preview.rs`, `crates/forge-theme/src/lib.rs`, `crates/forge-ui/src/icons.rs`. Those files have no diff from HEAD and were preserved. |
| `python3 scripts/smoke-blame.py` | PASS, exit 0; six real-PTY scenarios listed below |

Unit totals: `forge-app` 105, `forge-config` 2, `forge-fs` 11, `forge-git` 1,
`forge-git-cli` 14, `forge-git-gix` 5, `forge-ui` 60. Eleven new tests cover
porcelain parsing, C-quoted/renamed paths, staged/unstaged/untracked/unborn files,
large-file bounds, content/HEAD/index cache invalidation, wrapped CJK source
mapping, modal isolation/restoration, tiny-window scrolling and configuration.
Existing directory-selection, empty-preview and editor/history tests also pass.

The PTY smoke script passes all of these scenarios:

1. Author/subject display; full SHA and message; diff/history/copy feedback;
   Esc restores the original source row and screen position.
2. Clicking different source lines resolves the correct committed or uncommitted
   attribution.
3. External edits refresh matching preview content and attribution.
4. Untracked files display `Uncommitted changes`.
5. A long Markdown Code CJK continuation resolves the original commit while the
   following source line resolves a different commit. Formatted Preview does not
   guess source positions.
6. Inline/popup preferences persist across workspace restart and disable display
   and dispatch.

An initial parallel full-suite run had one failure in the existing
`tools::tests::batch_stage_and_unstage_use_marked_paths_and_include_collapsed_children`
test: Git reported an `index.lock` collision inside its temporary repository.
The serial full-suite run passes, including that test. No unrelated staging code
was changed for this result.

The first smoke harness used tmux, but the sandbox denied its Unix socket with
`Operation not permitted`. The delivered smoke script instead uses a direct PTY
and a small terminal screen reader, requiring no tmux or additional packages.
Clipboard dispatch and visible success/failure feedback are tested; actual
desktop clipboard contents cannot be independently verified in this headless
session. The formatted Markdown and bounded-display limitations above remain.

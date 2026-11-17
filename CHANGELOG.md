# Changelog

## [0.3.3] - 2026-11-16

### Added
- Interactive multi-terminal tabs with click-to-switch, close/new controls, Alt+1–8 direct navigation, F12 cycling, and up to eight independent local PTY Shells.
- Terminal scrollback and text selection/copy, bracketed paste support, xterm keyboard and mouse input handling, and a Terminal-specific help guide.
- Theme-aware Git History commit details: structured author/date/message, colored file additions and deletions, and a readable change summary.

### Improved
- Extract a focus-aware footer that follows the active Files, Preview, Git, Terminal, editor, search, or modal context instead of the merely visible panel.
- Transfer keyboard focus correctly across Terminal and workspace mouse clicks; prioritize help modals and provide mouse-close controls.
- Keep the styled commit summary visible while expanding/collapsing commit folders; show a file diff only when a changed file is selected.
- Snapshot visible terminal cells before drawing to reduce time holding the VT parser lock.

### Notes
- All Terminal tabs are local PTY sessions; no tmux persistence integration.
- Keyboard handling supports traditional xterm/VT sequences but not the full Kitty Keyboard Protocol.
- This release tag is prepared locally; publishing requires pushing both master and v0.3.3.

## [0.3.2] - 2026-11-16

### Added
- Workspace Switcher (`Alt+O`) for recent directories and opening any folder, including non-Git workspaces; `O` opens a directory prompt.
- Persist up to 12 recently visited workspace roots; Git Worktrees remain available separately with `W`.
- Shared list viewport and scrollbar controls for Files and Git, with navigation improvements to Help.
- Regression coverage for rapid file switching, stale Git status, directory refresh, watcher overflow, and Terminal bursts.

### Improved
- Extract UI rendering, tool controls, and input handling into focused modules; unify asynchronous request completion and cancel superseded directory listings.
- Prevent Git read-only queries from taking optional index locks while staging or discarding changes.
- Bound filesystem-watch notifications with overflow reconciliation, and harden Terminal input errors, shell exit handling, reaping, and drawing under PTY output.
- Prevent Git snapshots from previous workspace roots from overwriting current repository state.
- Make UI color assertions theme-aware and prevent path operations from silently defaulting to an invalid Git root.

### Notes
- This is a source/version tag; creating it locally does not publish release binaries.
- Existing `W` Git Worktree management is unchanged; `Alt+O` switches regular workspace directories.


## [0.3.1] - 2026-11-16

### Added
- Embedded Terminal powered by portable-pty and vt100: press F8 to open or hide a persistent Shell session in a bottom pane.
- Press F11 to maximize or restore Terminal without restarting the Shell; Ctrl+Shift+Enter also works on terminals that report the combination.
- PTY resize, command input, paste and basic ANSI/VT color/cursor rendering.

### Improved
- Reserve an independent scrollbar column for previews without a Focus Overview ruler, avoiding overlap with code text.
- Improve scrollbar track visibility using the theme muted foreground color.

### Notes
- Terminal starts lazily, uses a capped 2,000-line scrollback and coalesces visible UI refreshes to at most 30 Hz.
- This release provides one local Shell session; tmux integration and multiple Terminal tabs are not included.
- F8 hides the panel but keeps the Shell running; exiting Forge terminates the session.

## [0.3.0] - 2026-11-16

### Added
- Focus Overview: compact, clickable navigation for search matches and Git changes, with current viewport location and a Settings toggle.
- Whole-file Git markers and navigation for paged source files.
- Git Change Peek support for bounded long text files, with refreshed red/green diff styling.

### Improved
- Adaptive selected-line Git Blame with complete attribution below or in F6 details.
- Fix Blame selection resetting on redraw after introducing Focus Overview.
- Reduce redundant Git inspection during idle periods; skip Overview scans when disabled.

### Notes
- Git Change Peek is a floating overlay, **not** a layout-shifting inline diff.
- No miniature code texture is rendered in the Overview ruler.
- Release binaries target Linux glibc (x86_64/aarch64) and macOS (Intel/Apple Silicon); use WSL or source builds on Windows.

## [0.2.0]

- Theme presets, expanded syntax highlighting, commit composer and file/Git workflow improvements.

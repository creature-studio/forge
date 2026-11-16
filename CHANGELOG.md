# Changelog

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

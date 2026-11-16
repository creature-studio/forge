# Keyboard-first position scrollbars

Forge displays a compact one-character vertical scrollbar at the right edge
of the Files, Git and code/Diff preview panes whenever a view contains more
rows than fit in the visible pane. `│` is the track, `┃` shows the current
visible range. The thumb size corresponds to the viewport fraction, and the
position is derived directly from the panel's existing scroll state. There
is no separate scrollbar cursor or extra scrollbar persistence.

The keyboard is sufficient: `j/k` or Up/Down scroll one row, `PageUp` and
`PageDown` move by approximately one screen, and `Home`/`End` navigate to
the beginning/end of the active pane. Tab changes the active pane, so the
same shortcuts work in both the sidebar and preview. The F4 shortcut screen
lists these navigation keys. Mouse wheel scrolling is still supported.

These are read-only position indicators; mouse dragging is not yet enabled.
Horizontal scrollbar and horizontal panning are not implemented here because
the current source preview soft-wraps long lines. An indicator without a
corresponding real horizontal scroll offset would be misleading.

## Code wrapping and horizontal scrolling

Settings → **Code auto wrap** is enabled by default, including for users with
older configuration files. Its `wrap_code` preference persists in
`forge/config.toml`. Disabling it renders each source line on one terminal
row and shows a slim horizontal scrollbar only when columns exceed the
visible width. Code line numbers stay fixed while code pans; Unicode wide
characters are clipped on terminal-cell boundaries. Split Git Diff also
supports synchronized horizontal panning with both line-number gutters fixed.

With the preview focused, use **Alt+Left / Alt+Right** to pan by eight columns
or **Alt+Home / Alt+End** to jump to either edge. Tab switches focus from the
file tree to the preview. Mouse dragging of either scrollbar is not required
and not yet enabled. Turning wrapping back on resets horizontal position.

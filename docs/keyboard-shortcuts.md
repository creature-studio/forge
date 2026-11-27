# Keyboard shortcut conventions

Forge uses explicit modifier keys for workspace actions. Press F4 to see the
effective bindings, including any user overrides. The following are defaults;
the focused editor, search panel, terminal or dialog may own a key instead.

| Action | Default shortcut |
| --- | --- |
| Command palette | Ctrl+k / Ctrl+Shift+p |
| Refresh workspace | Ctrl+r |
| Edit file | Ctrl+e |
| Files: copy / cut / paste files | Ctrl+c / Ctrl+x / Ctrl+v |
| Files: select all visible / range | Ctrl+a / Shift+Up or Down |
| Files: trash / permanent delete | Delete / Shift+Delete |
| Stage / unstage in Git | Alt+a / Alt+u |
| Push / pull | Alt+p / Alt+r |
| Git history | Alt+g |
| File history | Alt+h |
| Commit history dialog | Alt+l |
| External editor | Alt+e |
| Filters | Alt+f |
| Branches | Alt+b |
| Stashes | Alt+z |
| Synchronization | Alt+s |
| Worktrees | Alt+w |
| Diff line actions | Alt+i |
| Select visible files | Alt+v |
| Previous Diff hunk | Alt+n |

F4 permits reassigning non-fixed shortcuts to Ctrl/Alt combinations or F6–F12,
subject to conflicts and reserved host shortcuts. Explicit user bindings take
priority. Plain uppercase workspace aliases such as `B`, `E`, `G` and `P` are
not implicit alternatives to the Alt bindings. Bare `r` does not refresh with
the default bindings; `?`, `,` and plain `s/u` are not the default Help,
Settings and Stage/Unstage shortcuts.

Dialogs retain their own local keys. For example, `r` renames a branch in the
Branches dialog, and `s/u` stage/unstage a line in Diff line actions. In global
content search, Alt+r toggles regex while Ctrl+r refreshes the workspace and
search results by default. In-file search also uses Alt+r to toggle regex.

Terminal focus sends normal keys to the child shell or TUI. Use F8 to hide the
terminal or Ctrl+Shift+Tab to return keyboard focus to the workspace. Modifier
combinations depend on what the outer terminal can deliver.

Case-sensitive editing and text input are unaffected. This is only the
workspace shortcut mapping, not a transformation of typed text.

## Settings entry points

Press **F9** to open Settings directly. The `F9 Settings` footer hint is
visible on the workspace screen, and the command palette also lists Settings.
F5 opens workspace Help. F4 opens the shortcut editor in the workspace and
editor; when the embedded terminal has focus, F4 opens Terminal help instead.


File shortcuts are local to the Files tree; text editing, preview selection,
Git and embedded terminals retain their own input ownership. Ctrl-click
toggles file selection; Shift-click selects a visible range. Paste targets the
selected directory (or the selected file's parent); the root context menu can
paste directly at the workspace root. Conflicts offer skip, keep both, or
confirmed replacement of distinct regular files. Directory replacement is
not supported. Delete moves items to the OS trash when supported and never
falls back to permanent deletion. Shift+Delete requires permanent-delete
confirmation. The clipboard remains scoped to its originating workspace.

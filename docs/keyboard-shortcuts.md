# Keyboard shortcut conventions

Forge now avoids uppercase/lowercase-only distinctions in its **advertised**
workspace shortcuts. A capital letter in an older shortcut was produced by
Shift+letter, which was easy to overlook. The new defaults use explicit
Alt+letter combinations for the secondary action:

| Action | New shortcut | Previous (still supported) |
|---|---|---|
| Push | Alt+p | Shift+p (P) |
| Git history | Alt+g | Shift+g (G) |
| File history | Alt+h | Shift+h (H) |
| Commit history dialog | Alt+l | Shift+l (L) |
| External editor | Alt+e | Shift+e (E) |
| Filters | Alt+f | Shift+f (F) |
| Branches | Alt+b | Shift+b (B) |
| Stashes | Alt+z | Shift+z (Z) |
| Synchronization | Alt+s | Shift+s (S) |
| Worktrees | Alt+w | Shift+w (W) |
| Diff line actions | Alt+i | Shift+i (I) |
| Select visible files | Alt+v | Shift+v (V) |
| Previous Diff hunk | Alt+n | Shift+n (N) |

Plain `p`, `g`, `e`, `v`, `n`, etc. retain their existing actions and
meaning. F4 lists these new defaults and permits reassigning non-fixed
shortcuts to Ctrl/Alt combinations or F5–F12. Legacy uppercase aliases remain
available **unless that action has a user-defined override**; explicit user
bindings always take priority. The terminal must deliver Alt-modified key
sequences; modern Windows Terminal, xterm, and most terminal emulators do.

Case-sensitive editing and text input are unaffected. This is only the
workspace shortcut mapping, not a transformation of typed text.

## Settings entry points

Press **F9** to open Settings directly. The `F9 Settings` footer hint is
visible on the workspace screen, and the command palette also lists Settings.
`Alt+,` is an alternative, and the old bare `,` shortcut remains supported.
`Ctrl+,` is not the default because many terminal keyboard protocols cannot
distinguish Ctrl+comma from comma. F4 still opens the shortcut editor.

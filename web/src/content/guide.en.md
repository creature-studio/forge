---
title: Forge TUI User Guide
description: Real screenshots, installation, file navigation, source preview, Git staging, diffs, history and more.
---

# Forge TUI User Guide

A practical, screenshot-driven guide to the **current Forge v0.4.0 development version**. All screenshots come from a real running Forge terminal session with an isolated sample Git repository, not mockups. **Click any screenshot to open its original full-resolution image.** Check `F4` for your effective key bindings—custom bindings and terminal input capabilities can differ.

> **Start here:** `F1` Files · `F2` Git · `Ctrl+P` Find file · `Ctrl+K` Commands · `F4` Shortcuts · `F5` Help · `F8` Terminal · `F9` Settings. Old shortcuts such as `?`, `,`, and plain `s/u` are no longer the defaults for Help, Settings, and Stage/Unstage.

## 1. Install and launch

On supported Linux and macOS targets, install the latest release:

```bash
curl -fsSL https://creature-studio.github.io/forge/install.sh | sh
```

The installer verifies the downloaded release against matching checksums. Use `FORGE_INSTALL_DIR` for a custom destination or `FORGE_VERSION` to pin a published release. From the project directory:

```bash
cd /path/to/project
forge
```

Run `forge update` for self-update or obtain a package from [GitHub Releases](https://github.com/creature-studio/forge/releases/latest). For source builds, use `cargo build --release -p forge` inside the repository (Rust 1.89+ and Git required). Windows users can run Forge in WSL; a native Windows package is not currently part of the release matrix.

## 2. Workspace layout and navigation

[![Real Forge Files workspace with a source preview](/forge/assets/guide/files.webp)](/forge/assets/guide/files.webp "View full-size screenshot")

The Activity Bar contains Files, Source Control, Search, Terminal and Settings. The collapsible sidebar lists your files or Git changes; the preview area shows the selected file or diff; the footer lists the actions available in the current focus. Use `Tab` / `Shift+Tab` to move focus and `Ctrl+B` to collapse or reopen the sidebar.

In a file tree, use `j/k` or arrow keys to move, `Enter` / `l` to open or expand, and `h` to collapse or move to the parent. `Home`, `End`, `PageUp` and `PageDown` navigate longer lists. Selecting a folder intentionally does not force a file preview. Use the right-click menu or Commands for create, rename, copy and delete operations; destructive actions require confirmation.

## 3. Find files and search content

[![Fuzzy file-name search with Ctrl+P](/forge/assets/guide/file-finder-filtered.webp)](/forge/assets/guide/file-finder-filtered.webp "View full-size screenshot")

`Ctrl+P` opens the fuzzy file finder. Type part of the name, use `↑/↓` to choose a result, then `Enter` to open it. Forge also reveals the corresponding path in the file tree. Use `Esc` to close the finder.

[![Global content search with file results and matching context](/forge/assets/guide/search-global.webp)](/forge/assets/guide/search-global.webp "View full-size screenshot")

For **global content search**, click Search in the Activity Bar, or press `Ctrl+K` and select **Search file contents / regex**. Some terminals can send `Ctrl+Shift+F` directly. Search results list file paths and matching lines; selecting a result previews its context, and `Enter` opens the exact line. Toggle regular expressions with `Alt+R` inside the search panel. `Ctrl+R` refreshes files, Git status and the current search results by default; check `F4` if you have rebound Refresh. File-name search and content search are separate operations.

## 4. Search within the current file

[![Inline find toolbar showing search matches](/forge/assets/guide/search-in-file.webp)](/forge/assets/guide/search-in-file.webp "View full-size screenshot")

Open a code file, focus its preview with `Tab`, and press `Ctrl+F` to search **within that file**. Enter a keyword, then use `Enter` / `Shift+Enter` to move through matches; `Esc` closes the toolbar. `Ctrl+F` behaves according to focus—use the Commands palette for global search if in doubt.

## 5. Code, Markdown and change markers

[![Rendered Markdown preview](/forge/assets/guide/markdown.webp)](/forge/assets/guide/markdown.webp "View full-size screenshot")

For Markdown files, `F3` toggles **Preview** (rendered) and **Code** (source). Code previews support line numbers and syntax highlighting. `F6` opens blame/commit details for supported source contexts. `F7` shows a Git Change Peek near changed lines without pushing the source text down. The optional Focus Overview provides a compact map of noteworthy locations rather than a full code minimap.

## 6. Git changes and full-file diff

[![Source Control sidebar with Changes, Staged and History](/forge/assets/guide/git-status.webp)](/forge/assets/guide/git-status.webp "View full-size screenshot")

Press `F2` to open Source Control. The sidebar separates **CHANGES** (working tree), **STAGED** (Git index) and **HISTORY** (commits). Expand folders with `Enter`, select a changed file and inspect its diff on the right.

[![Git diff with side-by-side change views](/forge/assets/guide/git-diff.webp)](/forge/assets/guide/git-diff.webp "View full-size screenshot")

Forge supports full-file diff context, including unchanged lines where available. Use `Tab` to change focus; `Alt+Left` / `Alt+Right` or the horizontal scrollbar to view long lines. Click `[Open file]` in the diff heading or press `o` to locate the current working-tree file in Files. A single file may appear in both CHANGES and STAGED when it has both indexed and unindexed modifications.

## 7. Stage, unstage and commit

[![Changes and Staged areas after staging one file](/forge/assets/guide/git-stage.webp)](/forge/assets/guide/git-stage.webp "View full-size screenshot")

Select an unstaged file in Git and press `Alt+A` to stage it. Select a staged file and press `Alt+U` to unstage it. With the left pane focused, selection can apply to files, folders or explicitly marked items; in a diff, it can target the current hunk. Verify the staged list before committing. Discarding a change (`x`) is separate and destructive.

[![Commit Composer with staged files, Subject and Body fields](/forge/assets/guide/commit.webp)](/forge/assets/guide/commit.webp "View full-size screenshot")

Press `c` from Git to open the **Commit Composer**. Verify the staged file count, enter a concise subject and optionally a longer body. Use `Tab` to switch fields. `Ctrl+S` / `Ctrl+Enter` leads to the review/confirmation action; `Enter` while editing is not an immediate commit. `Esc` cancels the dialog. Only indexed changes are included in the commit.

## 8. Commit history, blame and branches

[![Commit History with commit hash, author and changed paths](/forge/assets/guide/git-history.webp)](/forge/assets/guide/git-history.webp "View full-size screenshot")

The Git sidebar's **HISTORY** section shows committed revisions, including local commits not yet pushed. Select a commit to see its metadata and file list, or expand its changed-file tree. `Alt+G` focuses repository history, `Alt+L` opens the commit history dialog and `Alt+H` opens file history. Use `F6` for blame details when examining supported source lines.

[![Branches dialog showing the current branch](/forge/assets/guide/branches.webp)](/forge/assets/guide/branches.webp "View full-size screenshot")

`Alt+B` opens **Branches**. The current branch is marked with `*`. The on-screen actions include `A` create, `r` rename, `X` delete a merged branch and `D` compare to HEAD. `Alt+W` opens Worktrees. `Alt+S` opens remote synchronization; clicking the status chip does **not** blindly push. In the right Git context, `Alt+P` pushes and `Alt+R` pulls. Verify your remote, upstream and dirty working tree before synchronizing.

## 9. Stashes

[![Stashes dialog with keyboard action hints](/forge/assets/guide/stashes.webp)](/forge/assets/guide/stashes.webp "View full-size screenshot")

Press `Alt+Z` to manage Git Stashes. Use `A` to save the current work, `Enter` to inspect a stash diff, `a` to Apply while keeping the stash, `p` to Pop, or `X` to Drop. Read the dialog's currently displayed help before making destructive changes. A stash is not a replacement for a committed backup.

## 10. Built-in text editor

[![Forge's built-in text editor](/forge/assets/guide/editor.webp)](/forge/assets/guide/editor.webp "View full-size screenshot")

Select an editable file and press `Ctrl+E`. The lightweight editor supports selection, clipboard actions, line editing and undo/redo. Main keys: `Ctrl+S` Save, `Ctrl+C` Copy, `Ctrl+X` Cut, `Ctrl+V` Paste, `Ctrl+A` Select All, `Ctrl+Z` Undo, `Ctrl+Y` Redo. `Esc` exits or, if modified, opens a decision about saving, discarding or continuing editing. Unsaved edits are **not** silently saved.

## 11. Embedded Terminal and tabs

[![Embedded shell with tabs and contextual key hints](/forge/assets/guide/terminal.webp)](/forge/assets/guide/terminal.webp "View full-size screenshot")

`F8` opens or hides an embedded local Shell without destroying its session. `F11` maximizes/restores it. Up to eight tabs can be opened with `Shift+F8` or `Ctrl+Shift+N` and selected using `Alt+1` through `Alt+8`. `F12` switches to the next tab, while `Ctrl+Shift+W` closes the current one. The bar's `+`, `‹`, `›`, `□`, `?`, and `−` buttons create, switch, maximize, show help and hide; middle-clicking a tab closes it.

When the Shell has focus, **Esc and normal text belong to the Shell**. Use `Ctrl+Shift+Tab` to return focus to Forge. Scroll history with `Shift+PageUp/PageDown` or `Ctrl+Shift+Home/End`. Terminal help is available via its `?` button or `F4` while focused.

## 12. Commands, Help and settings

[![Searchable Commands palette](/forge/assets/guide/commands.webp)](/forge/assets/guide/commands.webp "View full-size screenshot")

`Ctrl+K` opens the Commands palette to discover available operations such as search, file management, Git tools and editing. Filter by name, navigate with arrows and run with `Enter`; `Esc` cancels. `F5` opens the full user help.

[![Appearance and navigation settings](/forge/assets/guide/settings.webp)](/forge/assets/guide/settings.webp "View full-size screenshot")

`F9` opens Settings: Tree Sticky Scroll, Compact Folders, Show hidden files, Color theme, Code auto wrap, Focus Overview, staged/unstaged Git Gutter, and keyboard shortcut configuration. Use `↑/↓` to select and `Enter` or `Space` to apply a setting. Configuration persists across restarts.

[![Keyboard shortcuts management screen](/forge/assets/guide/shortcuts.webp)](/forge/assets/guide/shortcuts.webp "View full-size screenshot")

`F4` opens the shortcut manager. Filter by Files, Git, Editor or Navigation, view effective bindings and reassign non-fixed shortcuts. The contextual footer reflects the active panel. Older one-character shortcuts advertised by earlier guides may no longer be valid defaults.

## 13. Mouse hover and compact layout

[![Activity Bar hover tooltip showing icon explanation and shortcut](/forge/assets/guide/hover.webp)](/forge/assets/guide/hover.webp "View full-size screenshot")

Activity Bar icons, Terminal controls and Git Diff's `[Open file]` display tooltips on mouse hover. Hover is **presentation-only**: movement must never change active panels or run a command. The sidebar divider can be dragged to resize; scrollbars support quick positioning. Narrow terminals switch to a single-pane-oriented workflow so use `Tab` to alternate list and preview. Built-in Unicode icons work without Nerd Font; optional Nerd or ASCII mode is available.

## 14. Quick keyboard reference

| Shortcut | Action |
| --- | --- |
| `F1` / `F2` | Files / Git |
| `Tab` / `Shift+Tab` | Switch panel focus |
| `Ctrl+P` | Find a file by name |
| `Ctrl+F` (preview focus) | Find text in the open file |
| Search icon / `Ctrl+K` | Global content search via Commands |
| `F3` | Markdown Preview / Code |
| `Ctrl+E` | Edit current file |
| `Alt+A` / `Alt+U` | Stage / Unstage |
| `c` | Commit Composer, then `Ctrl+S` to review |
| `F6` / `F7` | Blame / Git Change Peek |
| `Alt+G` / `Alt+H` | Git repository / file history |
| `Alt+B` / `Alt+W` / `Alt+Z` | Branches / Worktrees / Stashes |
| `F8` / `F11` / `Shift+F8` | Terminal / Maximize / New tab |
| `F4` / `F5` / `F9` | Shortcut manager / Help / Settings |
| `Esc` | Close or return (except when Shell owns focus) |

## 15. A complete workflow

1. Launch `forge` in your repository, use `F1` and `Ctrl+P` to find the file.
2. Read the preview, use `Ctrl+F` for in-file search and `Ctrl+E` for small edits.
3. Use `F8` if you need to execute build or test commands in the embedded Shell.
4. Go to Git with `F2`, select a changed file and review its full diff.
5. Stage the verified change with `Alt+A` and confirm the STAGED list.
6. Press `c`, compose a message and use `Ctrl+S` to review the commit.
7. Inspect HISTORY and verify upstream status before any optional push.

For release and compatibility details see [GitHub Releases](https://github.com/creature-studio/forge/releases/latest). For the most precise shortcut information, use `F4` in your own terminal rather than relying on old screenshots.

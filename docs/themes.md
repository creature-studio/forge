# Forge color themes

Open **Settings** from the command palette. Select **Color theme** and press
Enter or Space to cycle through the presets. The selection is immediately
applied and saved in `~/.config/forge/config.toml` (or under
`$XDG_CONFIG_HOME/forge/config.toml`).

| `theme` | Appearance |
|---|---|
| `current` (default) | Existing terminal-based Forge palette |
| `vscode-dark` | VS Code-like charcoal background (`#1e1e1e`) |
| `vscode-black` | Near-black editor background (`#080808`) |
| `github-dark` | Dark blue-black (`#0d1117`) |
| `nord` | Cool gray-blue (`#2e3440`) |

This is a built-in approximation of familiar palettes, not an import of
third-party theme packages. All preset colors are compiled in and require no
additional dependencies. Invalid or missing values safely fall back to the
legacy palette. Existing Rust/Markdown/Syntect syntax scopes derive from the
selected palette.

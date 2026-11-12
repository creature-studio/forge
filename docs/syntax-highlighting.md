# Lightweight script syntax highlighting in Forge

Forge Code Preview retains its Rust/Markdown renderers, and uses Syntect for
script grammars. Unlike Syntect's default feature set, Forge enables only
parsing, yaml-load and the pure-Rust regex-fancy backend: no Oniguruma C
library, no complete default syntax/theme packs.

## Bundled language support

| Language | Extensions |
| --- | --- |
| Python | .py, .pyw |
| Shell | .sh, .bash, .zsh, .fish |
| JavaScript | .js, .jsx, .mjs, .cjs |
| TypeScript | .ts, .tsx, .mts, .cts |
| JSON | .json, .jsonl |
| YAML | .yaml, .yml |
| SQL | .sql |

The bundled grammars are intentionally compact and cover strings, comments,
numbers, keywords and YAML keys. Complex templates, JSX, and interpolated
strings are not fully parsed by these grammars; they can be replaced with
community-maintained Sublime syntax definitions where required.

## Add a syntax plugin

Create ~/.config/forge/syntaxes/ (or $XDG_CONFIG_HOME/forge/syntaxes/ if set),
then save a Sublime Text .sublime-syntax YAML file:

~~~yaml
%YAML 1.2
---
name: My Script
file_extensions: [xyz]
scope: source.myscript
contexts:
  main:
    - match: '\bMAGIC\b'
      scope: keyword.control.myscript
    - match: '#.*$'
      scope: comment.line.myscript
~~~

Restart Forge and open sample.xyz in Code Preview to see MAGIC highlighted.
An external syntax may also override a built-in language using the same
extension; user files take precedence. Only declarative .sublime-syntax
definitions are loaded, not executable plugins. The registry initializes
lazily on the first script preview and is shared by all files.

Malformed custom syntax definitions do not crash the built-in preview:
Forge falls back to known grammars or plain text. Current limitation:
custom grammar parse errors are not surfaced in the UI; validate the grammar
using a Syntect-compatible syntax validator if it does not highlight.

## Memory and performance

- The existing preview reader is capped at **64 KiB and 400 source lines**;
  Forge never parses an entire multi-megabyte source file.
- The renderer uses the existing width-safe TextRowsCache on scroll without
  repeating syntax parsing. Styled lines stream into the same preview cache.
- The small grammar registry is loaded **once per process**, on demand, with
  no default Syntect theme bundle.
- Lines longer than **4096 bytes** fall back to uncolored display, preventing
  pathological regular-expression workloads.
- This is a **bounded preview cache**, not full viewport-only syntax
  virtualization. Its size is constrained by the preview byte/line limits.

## Verification

- cargo test -p forge-ui script_
- cargo test -p forge-ui syntax::tests
- python3 scripts/smoke-script-highlighting.py

The TUI smoke opens multiple language types and a user-supplied .xyz grammar
inside a disposable config directory, compares style colors with plain
text, and reports process RSS before and after script grammar loading.

## Additional built-in languages

Compact grammars also cover Go, C, C++, Java, C#, HTML, CSS/SCSS, TOML,
Dockerfile, Lua, PHP, Ruby, Kotlin, Swift, PowerShell, and XML.
HTML syntax handles `.html`, `.htm`, `.vue`, `.svelte`; this is markup
highlighting, **not** full embedded JavaScript/TypeScript/CSS parsing.
Dockerfile and Dockerfile.* are recognized by basename. `.h` defaults to
C; use `.hpp` for C++ or provide a user grammar override as needed.

These are intentionally lightweight lexical grammars. Full semantic parsing,
JSX/TSX embedded grammars, and editor-style language servers are out of scope.

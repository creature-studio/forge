A terminal panel can look correct in a test buffer and still overwrite its neighboring panel in a real terminal. Forge TUI encountered this with Chinese Markdown and source-code previews: a double-width character at the right edge appeared at the left edge of the next screen row.

This engineering note documents the fix and regression strategy recorded in [Forge's terminal rendering notes](https://github.com/creature-studio/forge/blob/91635771b0c96d31dba88a527019fae15e10b8ab/docs/terminal-rendering.md). Prepared with AI assistance. It describes a specific Ratatui 0.30.1 behavior, not a claim about every terminal or Ratatui release.

## Reproduce the boundary, not just the string

The original Markdown failure was reproduced in a 129-column by 47-row terminal, with a file tree on the left and a README preview on the right. A long mixed Chinese and English paragraph occasionally placed a character such as “的” or “。” in column zero of the next screen row.

Terminal columns are not UTF-8 bytes or Unicode scalar values. A line containing 19 single-column characters followed by a two-column grapheme occupies 21 cells, not 20. If the available width is 20, the incoming grapheme must move to the next visual row before it is drawn.

In the affected Ratatui version, an overflow check could miss the width of the incoming double-width grapheme. That let a wrapped row exceed the available width by one column. At the terminal's right edge, a wide glyph can cause an automatic wrap into column zero. This is why an error in one preview panel could become visible in another.

## Wrap styled graphemes before rendering

Forge's [wrap_line implementation](https://github.com/creature-studio/forge/blob/91635771b0c96d31dba88a527019fae15e10b8ab/crates/forge-ui/src/markdown.rs) takes a styled line and a width in terminal cells, then produces display rows. It iterates styled graphemes, measures cell width, accounts for the next grapheme during overflow checks, and reconstructs styled spans.

The useful invariant is straightforward: a row delivered to the renderer must not occupy more cells than its allocated width. It is not enough to count characters or truncate bytes. The implementation also handles pending words and whitespace; it is not simply a character-by-character hard wrap.

Once the preview has been reflowed, Forge renders the already wrapped rows without applying a second implicit paragraph wrap. Keeping the style information lets the Markdown preview retain formatting while correcting geometry.

## Keep source lines distinct from display rows

A second failure was recorded on November 9 in Code and plain-text previews. Fixing the Markdown renderer alone did not cover these paths.

The recorded fix uses a TextRowsCache to share the safe wrapping logic and map source-file line numbers to visual rows. This distinction matters when a long source line occupies multiple screen rows: scrolling follows visual rows, while jumping to a source line or opening the editor still uses source line numbers.

This is a rendering and navigation correctness issue, not an alteration to the underlying source file.

## Test both memory buffers and real PTYs

The project contains focused unit tests for wide graphemes, styled Markdown, resizing and wrapped-row width. Its regression notes also record dedicated real-terminal smoke tests. A test backend can assert row widths and positions, but cannot by itself establish how a real terminal handles a glyph at its right edge.

To reproduce the documented checks, clone the repository and use the cited revision:

```sh
git checkout 91635771b0c96d31dba88a527019fae15e10b8ab
cargo build -p forge
cargo test -p forge-ui markdown
python3 scripts/smoke-markdown-cjk.py
python3 scripts/smoke-code-cjk-boundary.py
```

The smoke tests require tmux and a UTF-8 locale. The Markdown smoke test uses a temporary Git repository and a 129×47 terminal; the Code smoke test uses 129×38 and exercises scrolling. The historical notes record that the Markdown test reproduced the bug before the fix and passed afterward. These are reproducibility instructions: the tests were not rerun for this article, and this does not establish that every Unicode rendering issue is solved.

See the [Markdown smoke test](https://github.com/creature-studio/forge/blob/91635771b0c96d31dba88a527019fae15e10b8ab/scripts/smoke-markdown-cjk.py) and [Code boundary smoke test](https://github.com/creature-studio/forge/blob/91635771b0c96d31dba88a527019fae15e10b8ab/scripts/smoke-code-cjk-boundary.py) for the exact assertions.

## Takeaway

For a terminal application with multiple panes, terminal cell width is part of the layout contract. Reflow once, preserve styling, distinguish source and visual rows, and test boundary cases in a real PTY. Do not infer terminal correctness solely from a passing in-memory widget test.

Explore the [Forge file manager](/en/features/file-manager/), the [user guide](/en/guide/), or the [engineering article on filesystem event handling](/en/engineering/git-refresh/).

# 终端渲染问题记录

这里记录已经复现并定位、需要随终端渲染依赖升级持续回归的问题。产品功能和交互建议分别保留在其他文档中。

## 2026-11-06：Markdown 中文宽字符越界到文件树（已修复并验证）

**表现**：Files 页选中 `README.md`，右侧使用 Markdown **Preview**；中文段落换行时，偶尔有一个独立的“的”或“。”出现在屏幕第 0 列，覆盖左侧文件树。不是文件内容被修改，也不是单纯的截图残影。

**复现环境**：129×47 的真实终端，Forge 的 Markdown 预览，项目固定使用 Ratatui 0.30.1。选中 `README.md` 后，包含“终端 Git + 文件管理工具……git CLI 执行写入。”的长行可以复现；继续滚动时也应检查侧栏是否受到污染。

**根因**：此版本 Ratatui 的 `Paragraph + Wrap { trim: false }` 在单个新加入的双宽字形恰好越过右边界时，没有完整计入该字形的终端列宽，可能使渲染行多出一列。终端在最右列处理该字形时自动折到下一屏幕行的第 0 列，因而越过右侧预览面板污染文件树。单纯在内存缓冲区检查文字位置不足以覆盖实际 PTY 行为。

**修复**：`crates/forge-ui/src/markdown.rs` 在渲染前按 grapheme 的终端列宽对 Markdown 富文本预换行，保留 span 样式，确保每条待绘制行不超过预览区域宽度；绘制预先分好的行时不再使用 `Paragraph::wrap`。改动仅针对 Markdown Preview，不改变侧栏宽度、导航快捷键或其他面板的渲染。

**回归方法**：

```bash
cargo build -p forge
cargo test -p forge-ui markdown
cargo test -p forge-app
python3 scripts/smoke-markdown-cjk.py
```

专用 PTY 脚本使用临时 Git 仓库、tmux 和 129×47 终端打开 `README.md`，检查顶部以及向下滚动 40 步后没有中文字符渗入文件树。它曾在修复前复现错误，在修复后通过。脚本要求 tmux 和 UTF-8 locale；既有 `smoke-kit.py`、`smoke-workspace.py` 仍用于通用 TUI 回归。

**边界与后续**：这一修复只覆盖 Markdown 预览。其他使用自动换行的纯文本视图在类似宽度条件下仍可能需要单独验证；未来升级 Ratatui 时应重跑该 PTY 测试，不能仅以 `TestBackend` 通过认定真实终端无越界。运行中的旧 Forge 进程需重启才能加载新构建。


## 2026-11-09：Code / 普通文本长行再次污染左侧目录树（已修复）

**表现**：Markdown 文件切换到 Code 后，右侧中英文混排长行，左侧文件树空白位置出现零散字符。与 11 月 6 日 Markdown Preview 的问题同类，但属于另一条渲染路径。

**遗漏原因**：旧修复只在 MarkdownPreview 中做了预先安全换行。Code / 普通文本仍交给 Ratatui Paragraph 自动换行，而且滚动总行数按照文件源行数计算，存在双宽字形越界和滚动错位风险。

**修复**：TextRowsCache 复用 Markdown 的安全换行算法预先生成屏幕行，并缓存源文件行号到显示行的映射。Code / 普通文本的绘制禁用 Paragraph 的二次隐式换行。滚动基于显示行，跳转文件行、打开编辑器基于源文件行。原有行号、缩进、鼠标拖拽复制不受影响；Markdown Preview 和 Git Diff 路径不变。

**回归**：运行 forge-ui 的 code_preview_pre_wraps_long_cjk_rows_and_scrolls_by_visual_row 单元测试，以及 scripts/smoke-code-cjk-boundary.py、scripts/smoke-preview-selection.py。前者覆盖多种宽度与行号映射；后者在 129×38 真实终端持续滚动 80 行，检查文件树不被预览内容污染。升级 Ratatui 或更改终端后仍应重新执行真实 PTY 测试。

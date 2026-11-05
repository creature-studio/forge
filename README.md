# forge

终端 Git + 文件管理工具，使用 ratatui-kit 组件与响应式状态，gix 读取 Git 数据、git CLI 执行写入。

![Forge 终端工作区预览](docs/tui-preview.svg)

## 开发与运行

需要 Rust 1.89+、Git。界面依赖固定为 ratatui-kit 0.10.3、Ratatui 0.30.1、Crossterm 0.29.0。

```bash
cargo check --workspace
cargo build -p forge
cargo run -p forge
cargo test --workspace
# 在临时 Git 仓库中验证真实终端交互（Python 3，Linux/macOS）
python3 scripts/smoke-kit.py
python3 scripts/smoke-workspace.py
```

Files/Git 侧栏支持目录展开、状态着色和虚拟滚动。右侧提供文件预览、语法高亮与左右 diff。支持文件及 hunk 的 stage/unstage、commit、push/pull、右键菜单和鼠标操作。窗口宽度低于 80 列时显示当前焦点面板。

## 架构

| Crate | 职责 |
|-------|------|
| `forge` | CLI 入口 |
| `forge-app` | kit 工作区组件、输入分层与业务操作 |
| `forge-ui` | 文件树、Git 树、语法高亮与 diff 原生绘制桥接 |
| `forge-theme` | 原生绘制器的语义色与终端颜色转换 |
| `forge-fs` | 文件系统服务 |
| `forge-git` | Git 门面 |
| `forge-git-gix` | Git 只读后端 |
| `forge-git-cli` | Git 写入及 diff |
| `forge-jobs` | 可取消优先级任务队列与完成通知 |
| `forge-config` | 配置加载 |
| `forge-model` | 共享领域类型 |

kit 接管布局、全屏渲染、事件分发与退出。工作区通过 `use_state` 和 `ContextProvider` 共享状态，按文件浏览、Git、预览和会话拆分；进程级主题使用 `Atom<Palette>`。提交消息草稿只存在于弹窗局部状态，取消后丢弃。后台任务完成或取消时通知组件树，空闲时不定时轮询。弹窗和右键菜单使用独立输入层。

## 颜色与图标

采用轻量终端样式：背景与正文跟随终端，左右面板仅用一条细线分隔，选中行使用中性灰底，青色只标记焦点。侧栏宽度随窗口调整，底部按当前面板显示快捷键；中文长文件名按终端列宽截断。原生内容支持 ANSI16、ANSI256 和真彩，默认 ANSI16。

```bash
cargo run -p forge -- --force-truecolor
cargo run -p forge -- --force-ansi256
cargo run -p forge -- --force-ansi16
cargo run -p forge -- --demo-colors
FORGE_COLOR=256 cargo run -p forge
```

`FORGE_COLOR` 支持 `16`、`256`、`truecolor`。启动时 stderr 显示 palette 与 icons 模式。

默认 builtin Unicode 图标无需 Nerd Font。`FORGE_ICONS=nerd`（或 `1`）启用 Nerd Font 图标；`FORGE_ICONS=ascii`（或 `off`）使用 ASCII。

## 键位

| Key | Action |
|-----|--------|
| `:` | 搜索命令：文件操作、批量操作和 Git 管理 |
| `Ctrl-P` / `Ctrl-F` | 模糊文件名搜索 / 全局内容搜索；内容搜索内 `Ctrl-R` 切换正则，Enter 跳转 |
| `F1` / `F2` | 切换 Files / Git；也可点击顶部标签 |
| `v` / `V` | 标记当前文件或目录 / 标记所有可见项；Esc 清空。Git 页按行记录路径与 staged/unstaged 侧，同一文件两侧可独立选中 |
| `e` / `E` | 内置文本编辑器 / 使用 `$EDITOR` 打开文件 |
| `F` | 文件过滤：修改、未跟踪、扩展名、隐藏文件、排除目录 |
| `I` | 当前 Diff 的单行操作：选行后 `s/u` 暂存 / 取消暂存 |
| `H` / `L` | 文件历史 / 仓库提交历史；Space 固定版本，`D` 对比，`[/]` 翻页 |
| `B` / `Z` / `S` / `W` | 分支 / Stash / 远程同步 / Worktree 管理 |
| `x` / `Delete` | 丢弃所选文件修改（右侧 Diff 为当前 Hunk）/ 删除文件或目录，均需确认 |
| `j` / `↓`、`k` / `↑` | 当前焦点面板移动或滚动 |
| `Enter` / `l` / `→` | 展开目录、打开文件或 diff |
| `h` / `←` / `Backspace` | 上级目录、折叠目录或跳到父级 |
| `Space` | Git 侧栏展开或折叠目录 |
| `Tab` / `Shift-Tab` | 列表与预览之间切换焦点 |
| `g` | 打开 Git 侧栏 |
| `Esc` | 关闭弹窗/菜单，或从 Git 返回 Files |
| `[` / `]` | 跳到 CHANGES / STAGED |
| `s` / `u` | Git 页左侧列表暂存 / 取消暂存选中的文件或整个文件夹（包含子目录）；焦点在右侧 diff 时仅操作当前 hunk。批量时 `s` 只处理已标记的 CHANGES 侧，`u` 只处理 STAGED 侧 |
| `d` | 在 Git 页显示选中文件的 diff |
| `n` / `N` | 焦点在 diff 时跳到下一个 / 上一个 hunk |
| `c` | 打开提交弹窗；Enter 提交，Esc 取消 |
| `p` / `P` | pull / push |
| `r` | 刷新目录和 Git 状态 |
| `q` / `Ctrl-C` | 退出；提交弹窗内 q 为输入字符 |
| `?` | 打开操作帮助；帮助内 `Esc` / `?` / `q` 关闭，`j/k`、翻页键或滚轮滚动 |

鼠标支持点选、双击目录、右键菜单和滚轮滚动。提交输入支持光标移动、编辑和中文。

Files 页右键提供 New file、New folder、Rename、Copy、Move 和 Delete。右键文件夹会在该目录内新建，右键文件会在同级目录新建，右键空白区域会在工作区根目录新建。删除文件和文件夹都需确认，文件夹删除包含子目录；右键操作只作用于点击项，批量操作通过 `:` 命令面板执行。

选中文件按 `e` 或右键 Edit text，进入内置多行文本编辑器，支持中文、粘贴、鼠标定位和方向键。`Ctrl-S` 保存，`Ctrl-Z/Y` 撤销/重做；`Esc` 退出，有未保存内容时可 `Ctrl-S` 保存并关闭、Enter 放弃或 Esc 继续编辑。支持 2 MiB 以内的 UTF-8 文本，保留 LF/CRLF、末尾换行和文件权限；文件被外部修改时拒绝覆盖并保留草稿。`E` 使用 `$EDITOR`（回退到 `$VISUAL`、`vi`），返回后恢复位置。

Stage / Unstage 统一放在 Git 页。底栏和右键菜单随选中项显示 `Stage file`、`Stage folder` 或当前改动块操作。Git 树中选中 CHANGES 的文件夹按 `s`，暂存该目录及所有子目录的改动；选中 STAGED 的文件夹按 `u`，取消暂存。折叠目录也生效，包括新增、修改和删除的文件，不影响目录外的改动。分组标题仅用于导航。取消暂存保留工作区编辑；commit 提交仓库内所有已暂存文件，pull/push 也作用于整个仓库。

Git 的多选标识由仓库内绝对路径和 `staged` / `unstaged` 侧共同组成，因此同一文件在 CHANGES 与 STAGED 的两行可分别选中，标题计数也分别计算。批量 `s` 只处理明确标记的 CHANGES 侧，批量 `u` 只处理明确标记的 STAGED 侧；目录标记递归匹配子项，但不会连带另一侧。Files 页选择仍是纯路径语义；Files 与 Git 切换会清空多选。

Files 过滤器按选中对象的稳定路径恢复选择：过滤后仍可见则保持同一路径和位置；不可见则回退到最近可见父目录，而不是沿用旧列表索引。关闭过滤保留目录展开状态与当前焦点。

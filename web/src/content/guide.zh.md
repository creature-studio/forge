---
title: Forge 使用指南
description: Forge v0.4.0 中文图文教程，基于真实运行界面、实际快捷键和工具操作。
---

# Forge 使用指南

从打开文件、检查代码到完成一次 Git 提交：这里按实际操作流程介绍 Forge 当前的 Files、Git、搜索、编辑器、终端和工作区工具。**所有配图均来自真实运行的 Forge TUI**，使用独立演示仓库截取，不是产品概念图。**点击截图可查看原始高清尺寸**，手机端也能放大查看代码。适用于 Forge v0.4.0 的当前开发版本；快捷键可在 `F4` 中查看及调整。

> **先记住：** `F1` 文件、`F2` Git、`Ctrl+P` 找文件、`Ctrl+K` 命令面板、`F4` 快捷键、`F5` 帮助、`F8` 终端、`F9` 设置。当前版本不应再用旧文档中的 `?`、`,` 或单字母 `s/u` 作为默认全局快捷键。

## 1. 安装、更新与启动

Linux（x86_64 / aarch64）与 macOS（Intel / Apple Silicon）可使用官方安装脚本：

```bash
curl -fsSL https://creature-studio.github.io/forge/install.sh | sh
```

Windows 10/11 x64 可直接在 PowerShell / Windows Terminal 中安装原生 `forge.exe`（无需 WSL）：

```powershell
irm https://creature-studio.github.io/forge/install.txt | iex
```

安装到当前用户的 `%LOCALAPPDATA%\Programs\Forge\bin`，校验 Release ZIP 的 SHA-256 并添加用户 PATH。请新开终端执行 `forge`；升级时先关闭 Forge，再重复执行安装命令。Windows 暂不支持 `forge update`。需要 Git for Windows 以使用 Git 操作。

安装器下载匹配平台的 Release 压缩包并核对同版本的 SHA256 校验信息，默认安装到 `~/.local/bin`。如果想指定安装位置或固定版本：

```bash
FORGE_INSTALL_DIR="$HOME/bin" sh install.sh
FORGE_VERSION=v0.4.0 sh install.sh
```

上述环境变量也可以与在线安装脚本组合使用；已有本地脚本时从项目根目录执行。安装后，在仓库或普通文件夹中启动：

```bash
cd /path/to/your/project
forge
```

更新可执行 `forge update`，或者到 [GitHub Releases](https://github.com/creature-studio/forge/releases/latest) 手动下载安装。Windows 原生二进制不在当前预编译矩阵中，可通过 WSL 使用。

从源码运行：

```bash
git clone https://github.com/creature-studio/forge.git
cd forge
cargo run -p forge
# 发布版：cargo build --release -p forge
```

## 2. 认识 Forge 工作区

[![Forge Files 工作区，左侧文件树与右侧源码预览](/forge/assets/guide/files.webp)](/forge/assets/guide/files.webp "打开高清截图")

*Files：真实演示仓库的文件树、Git 状态标识和右侧代码预览。*

主界面分为四部分：左侧 **Activity Bar**（文件、Git、搜索、终端、设置）、可折叠的 **Sidebar**、右侧 **Preview**，以及底部随焦点变化的状态与快捷键提示。按 `Tab` / `Shift+Tab` 切换文件列表和预览的键盘焦点；右下角的提示只列出当前操作上下文中的常用能力。

| 操作 | 方式 |
| --- | --- |
| 切到文件浏览 | `F1` 或点击左侧文件图标 |
| 切到 Git 工作区 | `F2` 或点击左侧 Git 图标 |
| 打开终端 | `F8` 或点击左侧 Terminal 图标 |
| 收起／展开 Sidebar | `Ctrl+B` 或点击 Sidebar 标题栏的箭头 |
| 查看全部操作说明 | `F5` |
| 查看与修改键位 | `F4` |

当终端较窄时，Forge 会切换为单面板布局，可用 `Tab` 在文件列表与预览之间切换，不必挤压成两个很窄的栏。鼠标滚轮和可拖动滚动条可以滚动文件列表、预览及部分弹窗。

## 3. 文件浏览、目录与预览

文件树支持展开目录、浏览隐藏文件（在设置中开启）、自动展开被定位文件的上级目录，以及 Git 状态标识。在目录树中选中**文件**时，右侧显示源代码、文本或对应的 Markdown 内容；选中**目录**时不会强制加载文件预览。

- `j` / `↓`：下一项；`k` / `↑`：上一项。
- `Enter` / `l` / `→`：展开目录、打开选中文件；`h` / `←`：折叠或返回上级。
- `PageUp` / `PageDown`、`Home` / `End`：快速定位；滚动位置会尽量保留。
- `Ctrl+E`：对当前支持编辑的文件打开内置编辑器。
- `v`：选择当前项目，方便批量文件操作；支持鼠标右键打开上下文菜单。

目录操作（新建文件、新建目录、重命名、复制、删除等）可通过右键菜单或 `Ctrl+K` 命令面板找到。对于删除、丢弃更改等危险操作，Forge 会要求确认。大型文件采用按需读取/显示，不意味着整个文件一次性都载入屏幕。

## 4. 快速找文件与全局内容搜索

### 按文件名定位：Ctrl+P

[![Forge 文件名模糊搜索](/forge/assets/guide/file-finder-filtered.webp)](/forge/assets/guide/file-finder-filtered.webp "打开高清截图")

按 `Ctrl+P`，输入文件名的一部分（如 `app.ts`），候选项会动态过滤。`↑/↓` 选择，`Enter` 打开；Forge 会展开文件树并定位相应文件。`Esc` 关闭搜索。

### 跨项目搜索：搜索图标或命令面板

[![Forge 全局搜索结果和右侧代码上下文](/forge/assets/guide/search-global.webp)](/forge/assets/guide/search-global.webp "打开高清截图")

点击 Activity Bar 的搜索图标，或者 `Ctrl+K` 搜索并执行 **Search file contents / regex**；若终端能区分 `Ctrl+Shift+F`，也可使用该组合键。输入关键字后左侧展示文件和命中位置，右侧展示上下文。`↑/↓` 选择结果、`Enter` 跳到对应文件及行。搜索框内 `Alt+R` 切换正则表达式；`Ctrl+R` 默认刷新文件、Git 状态和当前搜索结果。如果重绑过刷新操作，请以 `F4` 显示的按键为准。

**区别：** `Ctrl+P` 搜的是文件名；全局搜索匹配的是文件内容。搜索具有后台取消与增量更新能力，大型仓库可通过过滤配置减少不必要的目录扫描。

## 5. 当前文件内查找

[![Forge 当前文件内查找条和命中计数](/forge/assets/guide/search-in-file.webp)](/forge/assets/guide/search-in-file.webp "打开高清截图")

先选中一个代码文件，并通过 `Tab` 把焦点放到右侧预览，再按 `Ctrl+F`。输入关键字后，界面显示命中数量及当前命中位置；`Enter` / `Shift+Enter` 逐项跳转，`Esc` 关闭查找。这里的 `Ctrl+F` 是**文件内查找**，与命令面板中的**全局内容搜索**不同；优先看底部当前焦点提示即可判断。

## 6. 代码阅读、Markdown 与变更标记

[![Forge Markdown 渲染预览](/forge/assets/guide/markdown.webp)](/forge/assets/guide/markdown.webp "打开高清截图")

Markdown 文件有两种模式：**Preview** 展示渲染后的内容，**Code** 展示源文本。`F3` 或预览栏顶部的 Preview / Code 标签可以切换。普通代码文件可以显示行号、语法高亮、变更标记和 Blame 信息。

当光标或鼠标位于修改过的源码行时，`F7` 可显示 **Git Change Peek**，用于查看对应行的变更摘要；`F6` 查看提交归属详情。在已开启的 **Focus Overview** 中，也可以利用右侧概览标记快速定位搜索命中或改动位置。可在 `F9` 设置中调整 Focus Overview、Git Gutter 和自动换行。

## 7. Git Changes 与完整文件 Diff

[![Forge Git 工作区，Changes 与 Staged 区域](/forge/assets/guide/git-status.webp)](/forge/assets/guide/git-status.webp "打开高清截图")

按 `F2` 进入 Source Control。左侧按 **CHANGES（未暂存）**、**STAGED（已暂存）** 和 **HISTORY（历史）** 分区显示。选择更改文件后，右侧展示 Diff；目录可能先显示折叠状态，可用 `Enter` 展开后选择文件。

[![Forge 真实文件差异对比，含修改前后的行号](/forge/assets/guide/git-diff.webp)](/forge/assets/guide/git-diff.webp "打开高清截图")

右侧 Diff 不局限于补丁的几行：在支持的文件和可用上下文中能展示完整文件，未修改行也可随之浏览。普通宽度使用并排对比；较窄布局可以通过焦点切换与滚动阅读。更改行的标记与行号帮助区分旧内容、新内容以及暂存状态。

- `Tab` 切换 Git 列表与右侧 Diff 焦点。
- `n` / `Alt+N`：在适用的 Diff 上下文中跳转变更块；具体绑定以 `F4` 为准。
- `Alt+Left` / `Alt+Right`：横向查看不换行的长代码；也可使用横向滚动条。
- `o` 或点击 Diff 右上角 **[Open file]**：跳回 Files 并定位当前工作区文件，显示它的实际内容（不是仅显示补丁）。
- `F6` / `F7`：查看 Blame 详情 / 当前变更定位提示（在支持的源码预览情境）。

**注意：** 文件在 CHANGES 和 STAGED 中可能同时出现，因为同一文件的部分修改已经暂存、另一部分还在工作区。两侧不是重复数据，而是 Git 工作区与 Index 的不同版本。

## 8. 暂存、取消暂存与选择范围

[![Forge 暂存后，Git 界面分别显示一项未暂存和一项已暂存更改](/forge/assets/guide/git-stage.webp)](/forge/assets/guide/git-stage.webp "打开高清截图")

一次标准暂存操作：在 `F2` 中选择 **CHANGES** 下的文件，按 `Alt+A`；该文件移动到 **STAGED**，顶部计数随之变化。选择 **STAGED** 下的文件再按 `Alt+U`，则取消暂存。

| 当前焦点与对象 | `Alt+A` / `Alt+U` 的作用 |
| --- | --- |
| Git 左侧文件 | 暂存 / 取消暂存所选文件 |
| Git 左侧目录 | 操作目录内相关变更（含子目录） |
| 已标记多项 | 对所选范围进行批量处理，区分 CHANGES 和 STAGED |
| 右侧 Diff 的某个 Hunk | 操作当前可处理的变更块，而不是默认处理整份文件 |

需要先审查再提交时，应确认顶部 **changed / staged** 计数和 STAGED 清单。`x` 丢弃改动是不同操作，具有破坏性，请阅读确认弹窗再执行。**旧版 `s/u` 的单字符快捷键已不应当作新版默认键位。**

## 9. 撰写并确认 Git Commit

[![Forge Commit Composer，显示已暂存文件、Subject 和 Body](/forge/assets/guide/commit.webp)](/forge/assets/guide/commit.webp "打开高清截图")

先暂存需要提交的改动，然后在 Git 界面按 `c` 打开 **Commit Composer**。弹窗会显示已暂存文件数量与清单，并将提交消息拆分为 Subject 和可选的 Body。

1. 检查 STAGED 文件是否正确。如果显示 `0 files`，返回 Git 并先进行暂存。
2. 输入简洁的 **Subject**（界面建议不超过 72 字符）；需要背景说明时在 **Body** 中补充。
3. `Tab` 切换输入字段；在正文中 `Enter` 可换行。
4. 按 `Ctrl+S` 或 `Ctrl+Enter` 进入提交前审查/确认流程；**不是输入时直接按 Enter 就提交**。
5. 如需取消，按 `Esc` 返回，不会创建提交。

提交只使用已经位于 Git Index 中的变更；工作区其他未暂存修改不会因为打开 Composer 就自动包含在提交里。

## 10. Commit History、文件历史与 Blame

[![Forge Git 历史及选中的提交详情](/forge/assets/guide/git-history.webp)](/forge/assets/guide/git-history.webp "打开高清截图")

Git Sidebar 下方的 **HISTORY** 展示近期提交。选择某次提交，右侧可查看提交 Hash、标题、作者、时间和修改文件。按 `Enter` 可按层级展开该提交的文件记录，继续查看对应文件差异。

- `Alt+G`：聚焦仓库历史；`Alt+L`：打开仓库提交历史弹窗。
- `Alt+H`：查看当前文件的历史记录。
- `F6`：在支持的代码预览中查看行的提交归属和详细元数据。
- 点击有改动标识的行号区域，或按 `F7`：使用 Git Change Peek 辅助理解变更。

历史页是**已提交记录**；未 Push 的本地提交也应显示。是否已经同步远端，需要结合分支上的 ahead/behind 标记和底部 Sync 状态判断，不能只依靠提交列表。

## 11. 分支、Worktree 与远端同步

[![Forge Branches 窗口展示当前分支及操作提示](/forge/assets/guide/branches.webp)](/forge/assets/guide/branches.webp "打开高清截图")

在 Git 上下文中按 `Alt+B` 打开 **Branches**：列表中的 `*` 表示当前分支。根据界面底部实时提示，选择分支后可进行切换、创建、重命名和删除等操作；例如 Branches 窗口中有 `A` 创建、`r` 重命名、`X` 删除已合并分支、`D` 与 HEAD 对比。

- `Alt+W`：Worktree 管理；在相关弹窗里进行增删或切换，操作前确认目标路径。
- `Alt+S`：远端同步面板；也可点击底部 Git 分支和同步状态区域。该入口不是点击即 Push。
- `Alt+P` / `Alt+R`：在支持的 Git 焦点上下文中 Push / Pull。先检查上游分支和本地未提交改动；不要在无法确认目标远端时盲目操作。

分支和 Worktree 是两种不同概念：切换分支改变当前检出的分支，Worktree 则允许多个工作目录关联同一个 Git 仓库。多级弹窗支持使用 `Esc` 返回上一级。

## 12. Stash 暂存工作现场

[![Forge Stashes 管理界面及按键提示](/forge/assets/guide/stashes.webp)](/forge/assets/guide/stashes.webp "打开高清截图")

当你要先处理其他分支而不准备提交当前工作区时，可以用 `Alt+Z` 打开 **Stashes**。`A` 保存当前工作；列表选中已有 Stash 后，`Enter` 查看 Diff，`a` Apply（应用但保留记录），`p` Pop（应用并移除），`X` Drop（丢弃记录）。部分场景可选择恢复 staged 状态，具体以弹窗当前提示为准。

> **Stash 不是提交，也不是备份方案。** Pop、Drop、分支切换可能遇到冲突或数据风险；重要改动应先确认哪些文件被收纳，并在变更前保留可恢复路径。

## 13. 内置代码编辑器

[![Forge 内置编辑器，显示代码行号与可编辑区域](/forge/assets/guide/editor.webp)](/forge/assets/guide/editor.webp "打开高清截图")

在 Files 中选中可编辑文件后，按 `Ctrl+E` 进入编辑器。它提供基础文本编辑、插入与删除、光标移动、选择与复制、剪切、粘贴、撤销和重做，面向终端内的轻量修改，而不是完整替代 IDE。

| 编辑器操作 | 快捷键 |
| --- | --- |
| 保存 | `Ctrl+S` |
| 复制选中内容 / 当前行 | `Ctrl+C`（受编辑器焦点和选区约束） |
| 剪切选中内容 / 当前行 | `Ctrl+X` |
| 粘贴 | `Ctrl+V` |
| 全选 | `Ctrl+A` |
| 撤销 / 重做 | `Ctrl+Z` / `Ctrl+Y` |
| 关闭编辑器 | `Esc` |

编辑后按 `Esc`，如果存在未保存修改，会弹出保存、放弃或继续编辑的确认选择；**不要把 Esc 理解为自动保存**。代码预览区的文本选择与编辑器自己的选择是不同功能。在原生终端中，部分 Ctrl 组合键可能被外层终端或 tmux 捕获，请以 Forge 实际收到的输入及底部提示为准。

## 14. 内嵌 Terminal 与多标签

[![Forge 嵌入式 Terminal，文件预览和 Shell 同屏显示](/forge/assets/guide/terminal.webp)](/forge/assets/guide/terminal.webp "打开高清截图")

`F8` 打开底部 Terminal，Shell 在 Forge 中运行。再次按 `F8` 隐藏 Terminal **不会结束 Shell 会话**；重新打开可继续之前的输出。支持最多 8 个本地 Shell 标签页，标签栏右侧按钮 `+`、`‹/›`、`□`、`?`、`−` 分别为新建、切换、最大化、帮助、隐藏。标签右侧 `×` 或鼠标中键可关闭对应会话。

| 操作 | 快捷键 |
| --- | --- |
| 打开 / 隐藏 Terminal | `F8` |
| 新建 Terminal Tab | `Shift+F8` 或 `Ctrl+Shift+N` |
| 最大化 / 还原 | `F11` |
| 按序号切换标签 | `Alt+1`～`Alt+8` |
| 下一标签 | `F12` |
| 关闭当前标签 | `Ctrl+Shift+W` |
| 从 Shell 返回 Forge 焦点 | `Ctrl+Shift+Tab` |
| Terminal 操作说明 | Terminal 中 `F4` 或顶部 `?` |
| 浏览 Shell 输出历史 | `Shift+PageUp / PageDown`；`Ctrl+Shift+Home / End` |

Terminal 有焦点时，普通按键以及 `Esc` 通常先交给 Shell 或 Shell 内部应用，而不是 Forge 的文件树。需要操作文件树时，先按 `Ctrl+Shift+Tab` 或通过鼠标切换焦点。Shell 会话结束与仅隐藏标签是不同操作。

## 15. 命令面板与功能发现

[![Forge 命令面板，列出可搜索操作及快捷键](/forge/assets/guide/commands.webp)](/forge/assets/guide/commands.webp "打开高清截图")

不记得快捷键时按 `Ctrl+K` 打开 **Commands**，输入操作名进行过滤，`↑/↓` 选择，`Enter` 执行，`Esc` 取消。面板包含文件编辑、模糊文件搜索、内容搜索、文件操作和 Git 工具入口；界面还能显示当前筛选信息。

[![Forge 内置 Help 帮助界面](/forge/assets/guide/help.webp)](/forge/assets/guide/help.webp "打开高清截图")

`F5` 打开系统帮助，适合第一次了解功能分区；`F4` 打开可查看和修改的完整快捷键列表。命令面板、帮助页与当前焦点的 Footer 提示各有分工：**查能力用命令面板，查用法用帮助，查实时按键看 Footer。**

## 16. Settings、主题与快捷键

[![Forge 设置面板，显示外观、文件树和 Diff 配置](/forge/assets/guide/settings.webp)](/forge/assets/guide/settings.webp "打开高清截图")

按 `F9` 或点击 Activity Bar 底部齿轮进入设置。`↑/↓` 选择配置项，`Enter` / `Space` 切换或选择，`Esc` 返回。当前设置界面包含：

- **Tree Sticky Scroll**：目录树长列表滚动时吸顶显示父级路径。
- **Compact Folders**：折叠只有单一子目录的层级，减少树结构占用。
- **Show hidden files**：显示 `.env` 等隐藏文件。
- **Color theme**：选择终端主题，包括 VS Code Black 风格。
- **Code auto wrap**：代码自动换行，可根据横向滚动习惯启用。
- **Focus Overview**：显示用于定位关注点的概览标记。
- **Git Gutter: staged / unstaged**：分别控制已暂存、未暂存变更提示。
- **Keyboard shortcuts**：进入 F4 键位管理。

配置会保存并在重启后沿用。`Code auto wrap` 与水平滚动会相互影响；如果需要逐字符对照长行，可暂时关闭自动换行。

[![Forge 快捷键管理器：按功能组筛选并查看绑定](/forge/assets/guide/shortcuts.webp)](/forge/assets/guide/shortcuts.webp "打开高清截图")

在 **Keyboard shortcuts** 中可以按 Files、Git、Editor、Navigation 分类过滤，找到具体动作后进行重绑；固定绑定会以 fixed 等标签标识，不能任意覆盖。已调整的键位以 `F4` 中的当前绑定为准。Forge 为降低终端输入误判，已经减少危险的单字符全局快捷键，不要依赖早期指南中的旧映射。

## 17. 鼠标、Hover 与窄屏布局

[![Forge Activity Bar 的 Hover 说明与快捷键提示](/forge/assets/guide/hover.webp)](/forge/assets/guide/hover.webp "打开高清截图")

鼠标移到 Activity Bar 图标、Terminal 标签栏按钮和 Git Diff 标题栏的 `[Open file]` 时，会弹出统一样式的说明和快捷键提示。**Hover 只显示提示，不会激活面板**；只有明确点击才执行动作。点击和 Hover 复用相同的控件命中区域。

侧栏可以拖动分隔线调整宽度；文件树、代码预览和 Diff 可以用滚轮或滚动条快速定位。过窄的终端会限制同时显示的面板数量或隐藏部分控制按钮，因此尽量使用支持真彩色和鼠标事件的现代终端。无需额外安装 Nerd Font 就能使用内置图标模式；Nerd Font 仅提升特定图标的展示效果。

## 18. 推荐操作流程：从修改到提交

1. 在项目目录运行 `forge`，按 `F1` 进入 Files。
2. `Ctrl+P` 找到目标代码文件；通过预览和 `Ctrl+F` 查找关键逻辑。
3. 用 `Ctrl+E` 小范围修改，保存退出；需要执行命令时按 `F8` 打开 Shell。
4. 按 `F2` 检查 CHANGES，选择文件，在右侧阅读完整 Diff。
5. 确认无误后按 `Alt+A` 暂存；再次检查 STAGED 清单。
6. 按 `c` 打开 Composer，输入提交标题，`Ctrl+S` 进入确认步骤。
7. 在 HISTORY 检查刚创建的本地提交，再根据实际分支状态决定是否需要同步远端。

## 19. 快捷键速查

以下为当前开发版的**常见默认操作**，不同焦点下同一个按键可能对应不同操作；可自定义的键位以 `F4` 为准。

| 模块 | 快捷键 | 作用 |
| --- | --- | --- |
| 导航 | `F1` / `F2` | Files / Git |
| 导航 | `Tab` / `Shift+Tab` | 切换面板焦点 |
| 导航 | `j` / `k`、方向键 | 上下浏览 |
| 导航 | `Enter`、`h` / `l` | 打开、展开、折叠、返回上级 |
| 导航 | `Ctrl+B` | 收起 / 展开 Sidebar |
| 文件 | `Ctrl+P` | 模糊搜索文件名 |
| 搜索 | 搜索图标 / `Ctrl+K` | 全局搜索入口 |
| 搜索 | `Alt+R`（全局搜索框内） | 切换正则表达式 |
| 文件 | `Ctrl+F` | 当前代码预览内查找（需预览焦点） |
| 文件 | `Ctrl+E` | 内置编辑器 |
| 文件 | `F3` | Markdown Preview / Code |
| 代码 | `F6` / `F7` | Blame / Git Change Peek |
| Git | `Alt+A` / `Alt+U` | 暂存 / 取消暂存 |
| Git | `c` | Commit Composer |
| Git | `Alt+G` / `Alt+H` | 仓库历史 / 文件历史 |
| Git | `Alt+B` / `Alt+W` | 分支 / Worktree |
| Git | `Alt+Z` / `Alt+S` | Stash / Sync |
| Git | `Alt+P` / `Alt+R` | Push / Pull（需要正确上下文） |
| 终端 | `F8` / `F11` | 显示 / 最大化终端 |
| 工具 | `Ctrl+K` | 命令面板 |
| 工具 | `F4` / `F5` / `F9` | 快捷键 / 帮助 / 设置 |
| 通用 | `Ctrl+R` | 刷新文件与 Git 状态 |
| 通用 | `Esc` | 退出当前弹窗/返回（终端焦点除外） |

## 20. 常见问题与操作边界

**按了键没反应？** 先查看焦点位置和 Footer。终端内嵌 Shell、文本编辑器、Diff、普通文件树的键位会不同；`Ctrl+Shift` 组合也依赖外层终端是否能正确传递。

**Git Diff 和文件预览为什么内容不同？** Diff 可以对比工作区与 Index、Index 与 HEAD 的不同版本；Files 查看的是当前文件内容。同一文件在 CHANGES 和 STAGED 两边出现是正常的 Git 状态。

**鼠标移动会误跳到其他面板吗？** 正常 Hover 是纯视觉行为，不应触发 Files/Git 切换。如果在特定终端连续按 Esc 后仍能复现，请附终端名称、版本、输入追踪和复现步骤报告问题。

**找不到隐藏文件？** 在 `F9` 打开 **Show hidden files**。Git 忽略文件和搜索结果则需另看过滤规则。

**界面太窄或代码太长？** 可以收起 Sidebar、使用 `Tab` 切换面板、调整 `Code auto wrap`、横向滚动或 Focus Overview。

**安装、版本或系统兼容问题？** 参考 [Release 页面](https://github.com/creature-studio/forge/releases/latest) 和 [项目 README](https://github.com/creature-studio/forge)。

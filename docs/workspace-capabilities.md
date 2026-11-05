# Forge P0 / P1 工作区能力

这 16 项能力已接入 TUI。启动 `target/debug/forge`，按 `:` 打开可搜索的命令面板；输入名称后按 Enter 执行。`?` 帮助中也包含各项操作。界面沿用英文标签。

顶部 Files / Git 标签可点击切换，也支持 `F1` / `F2`。Files 负责浏览、搜索和文件操作，Stage / Unstage 的菜单、快捷键及命令入口仅出现在 Git 页。两页切换时保留各自的选中位置，并清空多选标记。

Files 右键文件或文件夹提供 New file / New folder / Rename / Copy / Move / Delete；文件夹内新建子项、文件同级新建、空白区域在工作区根目录新建。删除先确认，目录删除包含所有子项。右键操作使用点击项，批量操作使用命令面板。

`e` 或右键 Edit text 打开内置多行文本编辑器：支持中文、粘贴、鼠标和键盘定位，`Ctrl-S` 保存，`Ctrl-Z/Y` 撤销/重做。退出有未保存内容时，`Ctrl-S` 保存并关闭、Enter 放弃、Esc 继续编辑。支持不超过 2 MiB 的 UTF-8 文本，保留换行格式、末尾换行和权限；外部修改冲突会保留草稿。`E` 打开外部编辑器。

## P0

| 能力 | 使用方式 |
| --- | --- |
| 1. 快速找文件 | `Ctrl-P`，输入文件名的若干字符，使用模糊子序列匹配；Enter 展开祖先目录并定位文件。 |
| 2. 全局内容搜索 | `Ctrl-F`，输入内容；`Ctrl-R` 切换正则，方向键选结果，Enter 跳转至文件对应行。 |
| 3. 文件及目录操作 | Files 右键菜单或 `:` → New file / New directory / Rename / Copy / Move / Delete。创建和重命名输入工作区相对路径；复制和移动输入目标目录。已有目标不会被覆盖。删除先展示目标清单，再 Enter 确认。 |
| 4. 多选与批量操作 | `v` 标记文件或目录，`V` 标记所有可见文件，`Esc` 清空。标记按绝对路径保存。Git 页左侧列表 `s/u` 批量 Stage / Unstage；`: Copy / Move / Delete` 使用同一组选中路径，自动合并父子目录的重复选择。 |
| 5. 文件 / Hunk / 行级 Git 操作 | Git 页左侧列表 `s/u` 作用于文件、整个目录或多选项；右侧 Diff `s/u` 作用于当前 Hunk；`I` 展示当前 Diff 的改动行，方向键选行后 `s/u` 只操作这一行。索引和 Diff 改变后需重新打开行列表。 |
| 6. 文件历史及任意版本对比 | 选中文件按 `H`，历史跟随重命名。每页 200 个提交，`[` 更新、`]` 更旧；Space 固定第一个版本，移动到另一个版本后 `D` 对比。跨页保留固定版本。Enter 查看提交及涉及文件的 Diff。 |
| 7. 外部编辑器 | `E` 使用 `$EDITOR`，未配置时依次尝试 `$VISUAL`、`vi`。支持带参数和引号的配置，如 `code --wait`。先退出全屏和 raw mode，再启动编辑器；编辑器结束后重新进入 TUI，刷新内容并恢复文件、展开状态、焦点及预览位置。`e` 使用内置文本编辑器。 |
| 8. 文件过滤 | `F` 按 Git 状态（修改、未跟踪）、扩展名过滤，切换隐藏文件，配置排除目录名；过滤也应用于搜索。默认隐藏点文件，排除 `target`、`node_modules`；`.git` 始终隐藏。 |

## P1

| 能力 | 使用方式 |
| --- | --- |
| 9. Discard Changes | 左侧 `x` 丢弃选中的已跟踪文件的工作区改动；右侧 Diff `x` 丢弃当前未暂存 Hunk。先确认目标/补丁。保留索引；未跟踪文件使用 Delete。 |
| 10. Commit 历史 | `L` 展示仓库提交；Enter 查看作者、时间、说明和文件清单，再 Enter 查看该文件 Diff。Space / `D` 对比两个提交；`[/]` 翻页。 |
| 11. 分支管理 | `B` → Enter 切换，`A` 创建，`r` 重命名，`X` 删除，`D` 与 HEAD 对比。删除使用 `branch -d`，Git 会拒绝未合并和正在使用的分支。 |
| 12. Stash | `Z` → `A` 保存（含未跟踪文件），Enter 查看 Diff，`a` 应用并保留 Stash，`X` 删除。应用/删除前确认。用对象 ID 固定目标，避免列表变化后误删其他 Stash。 |
| 13. 冲突解决 | `:` → Conflicts。Enter 预览，`e` 外部编辑器手动合并，`o/t` 选择 ours/theirs 后暂存，`s` 将手动合并标为已解决。覆盖和标记前确认，最终结果由用户审阅。 |
| 14. Amend / Cherry-pick / Revert | `:` → Amend 输入新提交说明，确认后将全部暂存改动纳入并改写 HEAD；历史列表 `C/R` 选择提交后确认。命令面板提供 Cherry-pick / Revert 的 Continue / Abort 及 Abort merge。 |
| 15. 远程同步 | 顶栏显示本地分支与上游 ahead/behind；`S` 展示状态、FETCH_HEAD 更新时间及本次会话 Fetch / Pull / Push 结果，`F` Fetch，`p/P` Pull / Push。Pull 使用 `--ff-only`，分叉或远程错误保留详细输出。 |
| 16. Worktree 管理 | `W` 列表中 Enter 切换当前工作区，`A` 输入路径创建 Worktree（Git 默认按目录名创建分支），`X` 确认删除。当前 Worktree 需先切走；Git 拒绝删除脏或锁定的 Worktree。 |

## 范围与边界

- 搜索在后台执行，快速输入会取消旧搜索；遵守 `.gitignore` / `.ignore`。内容搜索跳过二进制、非 UTF-8 和超过 2 MiB 的文件，每次最多展示 2,000 条命中；文件名搜索最多收集 2,000 个匹配项后排序。
- 文件预览沿用已有大小/行数上限；超过预览范围的搜索结果可用 `e` 打开内置编辑器，超过 2 MiB 或非 UTF-8 文件可用 `E`。外部 vi/vim/nvim/nano/emacs 会收到起始行参数。
- 文件操作限制在当前工作区内，保护工作区根、`.git` 和指向工作区外的父目录符号链接。复制符号链接不会跟随其目标。批量操作先检查顶层目标冲突；若后续出现权限或磁盘错误，显示错误，已完成的项目不会自动回滚。
- 文件名搜索、内容搜索及 Git 列表不运行 shell。Git 文件参数按 literal pathspec 传入，包含空格、中文、`[]` 的路径不会扩大操作范围。非 UTF-8 文件名沿用原项目的有损字符串路径处理。
- 写操作互斥：进行中的文件或 Git 写操作会阻止第二次写入。文件、Hunk、行级取消暂存均保留工作区编辑；Discard 和 Delete 需要独立确认。
- ours/theirs 使用 Git 原生语义；遇到删除/新增型冲突，Git 可能拒绝某一侧，应手动编辑/删除后暂存。没有自动提交合并结果。
- 分支、Stash、Worktree 通过 Git 原生命令处理，仓库状态不允许的操作会展示 Git 错误；不会自动强制删除或覆盖。

## 代码与验证

- `forge-fs/src/workspace.rs`：搜索、过滤选项、路径检查、文件操作。
- `forge-fs/src/text.rs`：UTF-8 读取、原子保存、换行与权限保留、外部修改检测。
- `forge-git-cli/src/workbench.rs`：历史、版本对比、分支、Stash、冲突、远程状态、Worktree、Discard。
- `forge-model/src/lib.rs`：独立改动行补丁构造，支持新增/删除文件和无末尾换行。
- `forge-app/src/tools.rs`：命令面板、输入与确认、多选、后台任务、编辑器退出/恢复。
- `forge-app/src/editor.rs`：内置多行文本编辑、光标定位、保存、撤销/重做与未保存退出确认。
- `forge-app/src/view.rs`：Files / Git 标签、可滚动弹窗、结果列表、多选标记和窄窗口渲染。

验证覆盖标签点击、Files 的 Git 操作隔离、右键文件及文件夹创建/删除/取消、多行中文编辑与粘贴、保存及冲突保留草稿、真实临时 Git 仓库、文件操作边界、搜索结果跳转、写入互斥、单行改动隔离、重命名历史、分支/Stash/Worktree/冲突与远程分叉。真实终端回归脚本为 `scripts/smoke-kit.py` 和 `scripts/smoke-workspace.py`；所有测试均在临时目录中运行。

验证结果：`cargo test --workspace -- --test-threads=1` 共 105 项测试通过，文档测试通过；两套 PTY 回归通过；`cargo build -p forge` 通过。改动相关 crate 的 Clippy 通过（保留现有 `write_with_newline`、`if_same_then_else`、`manual_clamp` 遗留项的允许设置），已暂存及未暂存内容的 `git diff --check` 均通过。

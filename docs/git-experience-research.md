# Forge Git 体验调研：VS Code 与 GitLens

调研日期：2026-11-05。范围：本地未提交改动浏览、文件夹级操作，以及文件/目录历史。正文记录调研时的代码基线与改进方案。

实施进展（2026-11-05）：已实现 Git 树和 Files 树的文件夹 Stage / Unstage，快捷键与右键菜单共用目标解析，递归收集状态快照中的目录改动，覆盖折叠子目录、新增、修改和删除；操作保留目录外的改动。已同步动态底栏、操作范围反馈和 `?` 帮助。后续已补齐搜索、文件操作、多选、行级 Diff、历史/版本比较与 Git 管理，使用方式和边界见 [P0 / P1 工作区能力](workspace-capabilities.md)。文件夹改动汇总预览仍待实施。

## 1. 结论与建议

调研时 Forge 已具备改动目录树、双侧 diff、文件和 hunk 暂存/取消暂存。当时主要缺口是：**目录只承担展示作用，没有贯通改动预览、动作目标解析和批量执行。** 因此第一阶段建议把已有目录树变成可操作的改动入口，最新实施进展见文首。

建议组合两者的设计：

- **VS Code：借鉴当前改动的操作链路。** 按状态分组，将目录解析为该分组下的改动集合，再复用文件级命令；菜单、多选和键盘共用目标解析。
- **GitLens：借鉴范围与历史的组织。** 文件、目录、仓库都可以成为查询范围；先列出变更文件，再按需打开 diff；历史分页，blame 按需加载。
- **实施顺序：** P0 完成目录预览、暂存、取消暂存及刷新一致性；P1 增加筛选、多选、目录丢弃和历史/版本比较；P2 再做 blame 等上下文能力。

这里的“文件夹级 Git 操作”指对指定目录内的改动执行操作。commit、push、pull 仍属于仓库操作，菜单和提交窗口必须明确其范围。

## 2. 调研基线与边界

| 对象 | 固定源码版本 | 本次重点 |
| --- | --- | --- |
| VS Code | `microsoft/vscode@e934a7a384ac95319b46ceb4d9fa78b191625d9d` | 内置 Git 扩展与 Workbench SCM；目录能力跨越这两层 |
| GitLens | `gitkraken/vscode-gitlens@27dd43fe7cd1a76d19419856b8e7253aeab550dd` | 目录节点、staging provider、历史、比较和状态缓存 |
| Forge | 调研时的当前工作区，包含已有未提交变更 | `forge-model`、`forge-git*`、`forge-app`、`forge-ui`、`forge-jobs` |

上游使用调研时取得的 main 提交，并在文末提供固定版本链接；不能据此断言这些代码已进入某个稳定版。Forge 的结论以当前文件内容为准，行号可能随已有开发变化。

验证方式：阅读上述实现及官方文档；在隔离临时仓库使用 Git 2.43.0 验证关键命令语义。未运行 VS Code/GitLens 界面，也未对 Forge 做性能基准。

## 3. Forge 调研基线：已有能力与断点

| 能力 | 当前证据 | 对体验的影响 |
| --- | --- | --- |
| 按目录组织改动 | `forge-ui/src/staging.rs:47,183`：`ScmRow::{Section, Folder, Change}`；目录有递归文件数，两个分组分别维护展开状态 | 目录展示结构已经存在，应继续使用 |
| 文件/hunk 暂存、取消暂存 | `forge-git/src/lib.rs` 与 `forge-git-cli/src/lib.rs` 已有 paths/hunk 接口 | 不需要另建一套目录写后端，主要补范围解析 |
| 目录目标未接入暂存 | `forge-app/src/lib.rs:712`：Git 栏调用 `selected_scm_entry()`，目录返回空；Files 栏显式拒绝目录 | 选中目录后按 `s/u` 无法操作 |
| 目录目标未接入 diff | `forge-app/src/lib.rs:701,1945`：前者只解析文件，后者拒绝目录 | 无法连续审阅一个模块的改动 |
| 目录右键未接入菜单 | `forge-app/src/lib.rs:1799`：Git 栏目录右键直接切换展开；`ContextTarget::Staging` 只记录 column/index | 目录没有可发现的 Git 动作入口 |
| 暂存与工作区状态 | `forge-model/src/lib.rs:231,240`：`ChangeEntry` 有 path/status/side，snapshot 分 staged/unstaged；entries 合并状态供着色 | 双侧模型可保留；不能用合并后的着色状态决定操作 |
| 重命名关系不足 | `forge-git-gix/src/lib.rs:108,145`：重命名源被添加为 Deleted、目标添加为 Renamed；`ChangeEntry` 没有 original_path | 目前的模型不能可靠表达一次重命名及跨目录影响 |
| 选择稳定性 | `forge-app/src/lib.rs:444,1040`：刷新后主要按行号 clamp | 暂存导致列表重排后，可能跳到另一个文件或目录 |
| 外部变更刷新 | 已有启动、手动刷新及写操作成功后的刷新；未发现文件系统 watcher 链路 | 外部编辑、终端执行 Git 后，列表可能需要手动刷新 |
| 写操作调度 | `forge-app/src/lib.rs:763` 等共用 `git:write`；`forge-jobs/src/lib.rs:131` 同 key 会取消前一个任务 | 这是需要实测的风险：读取的“最新请求优先”不能直接当作写命令串行队列 |
| 路径参数 | `forge-git-cli/src/lib.rs:263` 使用 `to_string_lossy()`；命令带 `--`，未设置 literal pathspec | 带模式字符及非 UTF-8 路径需要处理，不能只增加目录前缀匹配 |

关键定位文件：[应用操作链路](../crates/forge-app/src/lib.rs)、[SCM 目录树](../crates/forge-ui/src/staging.rs)、[上下文菜单](../crates/forge-ui/src/context_menu.rs)、[状态模型](../crates/forge-model/src/lib.rs)、[Git CLI](../crates/forge-git-cli/src/lib.rs)、[gix 状态读取](../crates/forge-git-gix/src/lib.rs)、[任务调度](../crates/forge-jobs/src/lib.rs)。

## 4. VS Code：目录动作的核心实现

### 4.1 Git 扩展提供改动，SCM 工作台提供树和动作分发

Git 扩展的 `Repository` 创建 merge、index、workingTree、untracked 四类 resource group。分组可见性和未跟踪文件的呈现受配置控制。SCM 工作台负责把资源组织成目录树，因此只研究 `extensions/git` 会漏掉目录操作的关键部分。[资源分组实现][vscode-groups]、[Source Control API][vscode-api]

目录右键/行内操作由 `scm/resourceFolder/context` 声明，并用 `scmResourceGroup` 决定可用动作：工作区目录可 stage/clean，index 目录可 unstage。Forge 可沿用现有 `ContextAction`，增加文件夹目标与能力判断。[菜单声明][vscode-menus]

### 4.2 目录先变成资源集合，再调用文件命令

`RepositoryPaneActionRunner.runAction()` 先决定使用右键目标还是当前多选，再对目录调用 `ResourceTree.collect()`，将收集到的资源作为命令参数。收集基于资源树，语义不依赖目录当前是否展开。[目录动作分发][vscode-folder]

`git.stage(...resourceStates)` 按资源分组筛选，并处理冲突，再用 `runByRepository()` 调用 `repository.add(resources)`；`git.unstage` 只接受 index 分组，最终调用 `repository.revert(resources)`。单文件和目录共用命令，目录不需要独立的 Git 命令实现。[stage 命令][vscode-stage]、[unstage 命令][vscode-unstage]

对 Forge 的直接启发：**目录操作目标应该来自完整的状态快照，不应从屏幕可见行或实际磁盘递归中推导。** 这样才能包含已删除文件、折叠节点下的文件，并保持 staged/unstaged 语义。

### 4.3 批量执行、首次提交与刷新

底层 add 使用 `git add -A -- <paths>` 并按 CLI 长度分块；取消暂存通常使用 reset，在没有分支的初始化状态使用 `rm --cached -r`。这是 Forge 当前单一 `restore --staged` 路径需要补齐的边界。[CLI add][vscode-add]、[CLI revert][vscode-revert]

刷新同时观察工作区和 Git 元数据变化。自动刷新路径有 idle 判断、1 秒 debounce 和后续节流等待。Forge 应借鉴事件合并与空闲刷新机制，具体间隔需要按 TUI 和仓库规模测量，不直接照搬。[监听设置][vscode-watch]、[刷新调度][vscode-refresh]

## 5. GitLens：目录动作、查询范围与历史

### 5.1 状态文件先聚合，再生成目录树

`StatusFilesNode.getChildren()` 将状态/提交文件按路径聚合为 `StatusFileNode`，再使用 `makeHierarchical()` 和 `FolderNode` 构树，支持 list/tree 布局和 compact 目录。`StatusFileNode` 根据 staged/unstaged 能力生成上下文值；模型另保留 originalPath、indexStatus、workingTreeStatus。[状态文件树][gitlens-status-tree]、[目录节点][gitlens-folder]、[状态文件节点][gitlens-status-file]、[文件模型][gitlens-file-model]

Forge 可以保留现有树，补充稳定标识和目录汇总；compact 路径、列表/树切换属于后续浏览优化。目录的可用动作要根据两侧实际改动集合判断。

### 5.2 GitLens 有明确的目录写接口

GitLens 的 `views.stageDirectory` / `views.unstageDirectory` 取 `FolderNode.relativePath`，交给 staging provider；CLI 实现分别执行 `git add -A -- <directory>`、`git reset -q -- <directory>`。这是直接传目录路径的实现，与 VS Code SCM 收集资源的路径不同。[视图目录命令][gitlens-directory-command]、[staging provider][gitlens-staging]

直接传目录简洁，但执行时 Git 会重新解析目录覆盖范围。Forge 首版建议使用显式文件集合；如果将来提供“按执行时目录现状操作”的命令，需要单独说明该语义。

staging provider 还在 `finally` 中宣告 index 变化，清理 status/diff/tracking 相关缓存。即使多批次操作中途失败，也重新读取真实状态。Forge 的刷新应覆盖失败或部分完成的写操作。[写后失效处理][gitlens-staging]

### 5.3 目录历史与目录比较是两条查询链路

- `FileHistoryNode` 调用 `commits.getLogForPath(..., { isFolder, limit })`，保存查询结果，支持 `hasMore/loadMore`，并按仓库/工作区事件刷新。[历史节点][gitlens-history]
- `diffFolderWithRevision` 先选择目录关联的 commit 或分支/tag，然后调用 `openFolderCompare`。后者先用目录路径查询变更文件，再进入多文件比较。[版本选择命令][gitlens-folder-revision]、[目录比较][gitlens-folder-compare]

适合 Forge 的流程是“目录 → 变更文件列表 → 当前文件 diff”，历史也沿用这个结构。无需在一个预览面板中一次性拼接整目录的全部 patch。

目录历史应明确为“指定路径范围的提交”；单文件重命名追踪与目录历史不是同一保证，首版不承诺自动追踪目录整体移动。

### 5.4 缓存要区分请求参数与状态版本

GitLens 当前 status provider 使用“查询类型 + repoPath + generation”合并同一状态版本的并发读取，新版本不加入旧请求；强制刷新推进 generation。需要保留 rename 信息的目录查询，会读取完整状态再过滤，而不是直接限定 status pathspec。[status provider][gitlens-status-provider]

Forge 可增加仓库状态版本和请求标识，避免旧 status/diff 覆盖新范围。目录预览缓存至少区分 repository、side、path、generation；历史比较还要包含左右 revision。

blame provider 支持渐进读取，按路径、revision 或内容标识缓存，并通过 incremental blame 获取结果。Forge 可以在用户主动进入历史/归属视图时加载，不必在滚动改动树时执行。[blame provider][gitlens-blame]

## 6. 建议的用户行为

### 6.1 看清楚当前范围

Git 视图保留 CHANGES / STAGED，同一文件有两侧改动时分别展示。目录名称旁显示当前分组的递归文件数；选择目录时，右侧先显示该目录的改动概览，按 `d` 进入目录范围审阅。

范围审阅保持现有双侧 diff，标题显示 `src/api · CHANGES · 2/8`，并提供前后文件切换。现有 `n/N` 继续跳 hunk；可新增 `{` / `}` 切换范围内文件，避免占用已有 `[` / `]` 分组跳转。

已有颜色只作辅助；状态字母、完整路径、暂存侧别必须可见。重命名显示 `old → new`；删除文件无需在磁盘中存在也能查看。

### 6.2 文件夹操作矩阵

| 入口 | `s` 暂存 | `u` 取消暂存 | `d` 查看改动 | 右键 |
| --- | --- | --- | --- | --- |
| CHANGES 文件 | 当前文件 | 禁用并提示 | 工作区 vs index | 文件动作 |
| CHANGES 目录 | 该目录下所有可暂存改动 | 禁用并提示 | 当前目录的 CHANGES | 预览、暂存，后续加入丢弃/历史 |
| STAGED 文件 | 禁用并提示 | 当前文件 | index vs HEAD | 文件动作 |
| STAGED 目录 | 禁用并提示 | 该目录下所有 staged 改动 | 当前目录的 STAGED | 预览、取消暂存，后续加入历史 |
| 分组标题 | 对应分组批量操作 | 对应分组批量操作 | 对应分组范围 | 明确显示文件数 |
| Files 目录 | 目录内 CHANGES | 目录内 STAGED | 有两侧时显示概览并选择侧别 | 按两侧计数启用动作 |

`Enter`、左右键与 `Space` 保留目录导航行为；Git 目录右键打开菜单。目录下没有可操作改动时给出简短反馈。冲突单独标识，首版普通目录批量暂存跳过未解决冲突，并显示跳过数；解决冲突使用明确动作。

搜索仅改变展示，不暗中缩小“整个目录”的操作范围；加入搜索后，菜单显示总目标数与过滤可见数。折叠与展开不改变操作目标。

### 6.3 丢弃与提交范围

目录丢弃放在 P1：先展示确切文件集合及数量，分开列出受跟踪改动和未跟踪文件。受跟踪文件丢弃工作区改动时从 index 恢复，保留 staged 内容；未跟踪文件删除是独立动作。不能默认使用整个目录的 `clean -fd` 或 `reset --hard`。[restore 语义][git-restore]

目录菜单可先提供“暂存目录并打开提交窗口”，提交窗口同时列出仓库所有 staged 文件，明确目录外是否已有 staged 项。若后续要实现“只提交该目录”，应独立设计 index 隔离与恢复，不把普通 commit 包装成目录限定提交。

push/pull 始终标注仓库范围。目录筛选不改变它们的作用域。

## 7. 建议的内部改造

### 7.1 统一目标解析

增加共享的范围模型，概念上包含：

- `ChangeScope`：File、Folder、Group；包含 repository 和 side。
- `ChangeId`：repository + side + repo-relative 原始路径；用于选择恢复、菜单目标和预览身份。
- `ResolvedChanges`：状态版本、目标文件、操作路径及跳过原因；preview 和写操作使用同一解析结果。

`resolve_changes(snapshot, scope)` 是纯逻辑，不依赖 `ScmRow` 是否可见：

1. 根据 side 选择 snapshot 集合。
2. 按路径组件匹配目录。`src` 匹配 `src/a.rs`，不匹配 `src-extra/a.rs`；仓库根单独处理。
3. 去重并保留 rename 关系、冲突信息；普通批量动作排除不支持的目标。
4. 根据动作产生精确路径集合和摘要，空集合不调用 Git。

**重命名需要明确扩展范围。** 如果一次 rename 的另一端位于目录外，界面列出两端及目录外影响后再作为一个逻辑变更操作，不能悄悄扩大目录范围。保留 `original_path` 是实现这一点的前提。

路径身份使用原始 `PathBuf/OsString` 或等价表示，展示文本独立生成；非 UTF-8 路径不能通过 lossy 字符串重新构造后执行。

### 7.2 写命令与路径处理

普通暂存使用显式目标集合：`git --literal-pathspecs add -A -- <paths>`。取消暂存可保留 `restore --staged`，增加无 HEAD 分支；临时仓库已验证在 unborn 状态下使用限定路径的 `rm --cached -r -f` 可以仅移除 index 项、保留工作区内容。此分支须确认没有 HEAD，且只处理解析后的 staged 文件。[Git 全局选项][git-options]、[add 选项][git-add]、[VS Code 初始化处理][vscode-revert]

大量路径优先使用命令支持的 `--pathspec-from-file=- --pathspec-file-nul`，同时保持 literal pathspec 语义；不支持的命令按参数字节长度分块。不能将 NUL 分隔误当成关闭 Git 路径模式匹配。[add 文档][git-add]、[restore 文档][git-restore]

写操作按实际仓库/index 串行执行，首版可在有写任务时禁用重复写入口并显示进度。读取可以取消或丢弃旧结果；已开始的写操作必须等待结果并读取真实状态。遇到 `index.lock` 等错误显示原因，保留刷新能力。

执行前确认快照版本，必要时刷新后重新解析范围；开始执行后固定目标文件集合。固定集合不等于冻结文件内容，Git 仍读取执行时内容。丢弃等破坏性操作需要在确认后检查内容/状态是否变化，变化时重新展示目标。

若使用分块，反馈成功、失败和未执行数量；无论成功或失败均重新读取状态与相关 diff。

### 7.3 预览、选择与刷新

- 目录预览维护 `scope + file_ids + current_file`，继续按文件复用 `FileDiff` 和现有 hunk 操作；目录摘要本身不提供 hunk 操作。
- 新状态按 `ChangeId` 恢复选择，找不到时选择同目录下一项，再退回目录或分组；保留目录展开和范围。取消暂存后文件移到另一侧时，按既定规则移动或保持原侧，不能只沿用旧行号。
- diff 结果校验 repo、side、path、generation、request id，防止旧预览覆盖新选择；当前 `poll_pending_diff()` 读取后丢弃 path/side，建议显式校验。
- 观察工作区和真实 Git 元数据目录，合并连续事件；worktree 的 `.git` 可能是文件，不能假定是目录。手动刷新继续保留。
- 目录摘要从状态快照聚合，不逐目录执行 status；按 snapshot/展开状态缓存树投影，避免每次绘制、移动和命中测试都重建整棵树。

### 7.4 模块落点

| 模块 | 建议调整 |
| --- | --- |
| `forge-model` | 范围、稳定身份、rename 源路径、目录汇总及状态版本；保留着色与操作状态的区别 |
| `forge-git-gix` | 保留重命名两端和冲突信息；继续集中生成全仓状态快照 |
| `forge-git` | 共享范围解析/操作计划入口，统一 Files 与 Git 入口的语义 |
| `forge-git-cli` | literal paths、无 HEAD 取消暂存、路径分块/NUL 输入；P1 增加历史和 revision compare |
| `forge-app` | 统一键盘/鼠标/菜单目标；目录范围预览、稳定选择、写队列和版本校验 |
| `forge-ui/staging.rs` | 目录计数、范围提示、树投影缓存；后续加入过滤和 compact |
| `forge-ui/context_menu.rs` | 目标从易变化的 index 扩展为稳定文件/目录/分组身份，按能力生成菜单 |
| `forge-jobs` | 区分可取消读取与需要完成/串行的写任务；增加可合并刷新通知 |

## 8. 实施顺序和验收

### P0：完成“看目录 → 审阅 → 批量暂存/取消暂存”

1. 先补路径身份、rename 信息、范围解析及写任务互斥，确保目标可靠。
2. 贯通 Files/Git 目录、分组标题的 `s/u` 与右键菜单，显示数量及跳过原因。
3. 加入目录范围预览与文件间跳转，复用现有 diff/hunk 渲染。
4. 用稳定身份恢复选择，加入刷新事件与结果版本校验。

验收：选中折叠目录也可操作全部合格后代；仅影响对应 side 和明示目标；删除文件可操作；首次提交前可取消暂存；目录外 staged 项保留；外部 Git 操作后列表更新；反复快速按键不导致写任务互相取消。

### P1：降低大量改动的浏览成本，补齐 GitLens 风格的目录上下文

增加路径搜索、状态过滤、多选、紧凑目录、目录丢弃；增加文件/目录分页历史、提交详情和指定 revision 比较。新增文件计数和行数统计分开：行数统计按需加载，避免初始化时对每个文件启动 diff。

验收：搜索能找到深层改动；过滤不改变目录动作的已声明范围；多选文件与目录去重；目录历史可分页且只展示指定路径范围；版本比较先列变更文件并按需加载内容。

### P2：按需补充归属信息

增加最近修改者、文件 blame、选中行的提交详情；根据实际使用再决定 commit graph、stash、worktree 管理等功能。

### 测试重点

| 层次 | 必须覆盖的场景 |
| --- | --- |
| 目标解析 | 嵌套/折叠目录、`src` 与 `src-extra`、根目录、同文件两侧都有改动、多个重叠选区、空范围 |
| 真实 Git 集成 | 修改/新增/删除/未跟踪/忽略、partial stage、首次提交、重命名及跨目录 rename、冲突 |
| 路径 | 空格、中文、换行、`[]*?`、冒号开头、前导连字符；Unix 非 UTF-8 路径；其他平台在对应 CI 验证 |
| 状态一致性 | 连续 stage/unstage、后台编辑、外部 commit/checkout、失败后刷新、旧 diff 晚到、index 锁定 |
| TUI | 目录右键打开菜单；键盘与菜单目标一致；刷新后选择及展开保留；窄终端范围提示可理解 |
| 大目录 | 数千改动时滚动与输入保持响应；树构建和 Git 调用次数有记录，之后再设定性能阈值 |

实施时复用现有 `scripts/smoke-kit.py` 验证真实终端交互，补充有行为价值的临时仓库集成用例；本次调研没有运行工作区编译与测试。

## 9. 已完成的隔离命令验证

以下结果来自临时仓库，不涉及当前工作区文件与 index。

| 验证 | 观察结果 | 方案含义 |
| --- | --- | --- |
| 用显式路径暂存目录内修改、删除和新增 | 三项正确进入 index；`src-extra` 的修改与 ignored 文件未进入 | 可以复用 paths API，不需要磁盘递归 |
| 对上述文件取消暂存 | 修改内容、删除状态与新增文件内容均保留 | unstage 只改变 index |
| `git add -- 'literal[1].txt'` | 同时暂存了 `literal1.txt` 和 `literal[1].txt` | `--` 不会关闭 Git pathspec 模式解释 |
| 同命令加全局 `--literal-pathspecs` | 仅暂存 `literal[1].txt` | literal 路径处理应成为统一能力 |
| 快照后创建 `src/late.txt`，按快照显式路径执行 | late 文件未被暂存 | 目标集合可与展示保持一致 |
| 相同条件下直接 `git add -A -- src` | late 文件被暂存 | 直接目录路径会扩大到执行时现状 |
| 无 HEAD 时执行 `restore --staged` | 退出码 128 | 需要 unborn 分支 |
| 无 HEAD、index/工作区版本不同，执行限定路径的 `rm --cached -r -f` | index 清空，较新的工作区内容保留 | 该 fallback 可用于首次提交前取消暂存，仍需正式集成测试 |

尚未实测：Forge 当前写任务取消的实际子进程行为、跨平台 watcher、子模块及大仓性能。相关项属于实施验收要求，不作为已证实缺陷或性能结论。

## 10. 上游代码使用边界

本方案采用设计与行为分析，按 Forge 的 Rust 分层独立实现。GitLens 根 LICENSE 将通常目录与名为 `plus` 的目录区分，后者适用 LICENSE.plus；如后续直接复用代码，应按具体文件核对许可，不能将整个仓库都按 MIT 处理。[GitLens LICENSE][gitlens-license]、[LICENSE.plus][gitlens-license-plus]

## 来源索引

文中的上游源码链接均固定到本次调研提交；Git/产品文档反映调研时可访问内容。

[vscode-api]: https://code.visualstudio.com/api/extension-guides/scm-provider
[vscode-groups]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/repository.ts#L1020-L1045
[vscode-menus]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/package.json
[vscode-folder]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/src/vs/workbench/contrib/scm/browser/scmViewPane.ts#L467-L485
[vscode-stage]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/commands.ts#L1522-L1581
[vscode-unstage]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/commands.ts#L2033-L2056
[vscode-add]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/git.ts#L2011-L2027
[vscode-revert]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/git.ts#L2393-L2420
[vscode-watch]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/repository.ts#L930-L955
[vscode-refresh]: https://github.com/microsoft/vscode/blob/e934a7a384ac95319b46ceb4d9fa78b191625d9d/extensions/git/src/repository.ts#L3339-L3370
[gitlens-status-tree]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/views/nodes/statusFilesNode.ts#L41-L96
[gitlens-folder]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/views/nodes/folderNode.ts
[gitlens-status-file]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/views/nodes/statusFileNode.ts
[gitlens-file-model]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/packages/git/src/models/file.ts#L9-L18
[gitlens-directory-command]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/views/viewCommands.ts#L1184-L1300
[gitlens-staging]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/packages/git-cli/src/providers/staging.ts
[gitlens-history]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/views/nodes/fileHistoryNode.ts
[gitlens-folder-revision]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/commands/diffFolderWithRevision.ts
[gitlens-folder-compare]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/src/git/actions/commit.ts#L643-L684
[gitlens-status-provider]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/packages/git-cli/src/providers/status.ts
[gitlens-blame]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/packages/git-cli/src/providers/blame.ts
[git-options]: https://git-scm.com/docs/git
[git-add]: https://git-scm.com/docs/git-add
[git-restore]: https://git-scm.com/docs/git-restore
[gitlens-license]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/LICENSE
[gitlens-license-plus]: https://github.com/gitkraken/vscode-gitlens/blob/27dd43fe7cd1a76d19419856b8e7253aeab550dd/LICENSE.plus

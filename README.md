# forge

高性能终端 Git + 文件管理一体工具（Ratatui / 异步 I/O；gix 读、git CLI 写）。

## Crate 地图

| Crate | 职责 |
|-------|------|
| `forge` | 可执行入口 |
| `forge-app` | 状态机 / 事件循环 |
| `forge-ui` | Ratatui 布局与组件 |
| `forge-theme` | 主题色板（Grok-inspired dark） |
| `forge-fs` | 目录遍历 / 监视 / 预览 / 文件操作 |
| `forge-git` | Git 门面 |
| `forge-git-gix` | 只读（gix） |
| `forge-git-cli` | 写入 / diff（git CLI） |
| `forge-jobs` | 可取消优先级任务队列 |
| `forge-config` | 配置加载 |
| `forge-model` | 共享领域类型（无外部重依赖） |

依赖方向：`forge` → `forge-app` → `{ui,fs,git,jobs,config,model}`；`forge-git` → `{gix,cli,model}`；领域 crate 只依赖 `forge-model`。

## 开发 / 运行

```bash
cargo check
cargo build -p forge
cargo run -p forge
```

当前能力：双栏文件管理 + 底部 staging 工作台 + Grok 风深色主题；Git status 着色、文件级 diff、stage/unstage/commit；虚拟滚动与鼠标。

## 颜色 / 真彩

**现象：** 纯白字黑底、选中无抬升、目录不变蓝 → 终端丢掉了 `Color::Rgb`（`\e[38;2;…m`），不是主题没接上。

**默认策略：** `COLORTERM` 未声明真彩时，forge 走 **ANSI 256**（`Color::Indexed`），保证有颜色。声明了真彩才发 24-bit RGB。

```bash
# 看当前模式（启动时 stderr 会打印 forge: color mode = …）
cargo run -p forge

# 强制真彩（终端必须真的支持）
COLORTERM=truecolor cargo run -p forge
# 或
cargo run -p forge -- --force-truecolor

# 强制 256 色（兼容性最好）
cargo run -p forge -- --force-ansi256
# 或
FORGE_COLOR=256 cargo run -p forge
```

自检：启动后应看到青色 `forge`、目录图标 + 偏蓝目录名、选中整行青底。底栏 status 含 `color:ansi256` 或 `color:truecolor`；stderr 含 `icons=builtin` / `nerd` / `ascii`。

## 图标

默认 **builtin**（内置 BMP Unicode：`▸▾` / `□▣` / `·` / `⎇`），常见字体即可显示，无需 Nerd Font。Nerd 图标可选；终端无法内嵌字体，PUA 字形缺字会变成方框。

```bash
# 默认：builtin（无需 Nerd Font）
cargo run -p forge

# 可选：Nerd Font 图标（需终端装 JetBrainsMono Nerd Font 等）
FORGE_ICONS=nerd cargo run -p forge
# 或 FORGE_ICONS=1

# 纯 ASCII
FORGE_ICONS=ascii cargo run -p forge
```

## 键位

| Key | Action |
|-----|--------|
| `j` / `↓` · `k` / `↑` | 当前焦点区移动 / 滚动 |
| `Enter` / `l` / `→` | 进入目录；Git 侧栏展开文件夹或打开 diff |
| `h` / `←` / `Backspace` | 上级目录；Git 侧栏折叠文件夹 / 跳到父级 |
| `Space` | Git 侧栏：展开/折叠当前文件夹 |
| `Tab` / `Shift-Tab` | 焦点：列表 → 预览 → staging |
| `g` | 聚焦 staging（CHANGES/STAGED 目录树） |
| `[` / `]` | 跳到 CHANGES / STAGED 并展开首个变更路径 |
| `s` | stage（unstaged 选中项，或列表中的文件） |
| `u` | unstage（staged 选中项，或列表中的文件） |
| `d` | 对选中变更 / 文件显示 diff（预览区） |
| `c` | 打开 commit 消息输入（Enter 提交，Esc 取消） |
| `r` | 刷新列表 + status |
| `q` / `Ctrl-C` | 退出 |

鼠标：点选列表 / staging；双击进目录；滚轮滚动当前区。

## 视觉（Modern Minimal）

- Charcoal 面板 + Edit cyan accent（与 Files 模式一致）
- 选中行：柔和 slate-cyan 抬升（非亮蓝块）；add/del 背景降饱和，避免大红大绿
- Git 侧栏与 Files 同套图标：默认 builtin Unicode；`FORGE_ICONS=nerd`/`1` 用 Nerd Font；`ascii`/`off` 纯 ASCII
- 左右对比 diff；CHANGES / STAGED 按路径分段可展开目录树

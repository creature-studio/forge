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

自检：启动后应看到青色 `forge`、目录图标 + 偏蓝目录名、选中整行青底。底栏 status 含 `color:ansi256` 或 `color:truecolor`；stderr 含 `icons=nerd` 或 `icons=ascii`。

## 图标（Nerd Font）

文件树 / staging / 顶栏分支默认使用 **Nerd Font** 图标（JetBrainsMono Nerd Font、Cascadia Code NF、MesloLGS NF 等均可）。若终端字体缺字形会显示成方框或 `?`，可切到 ASCII：

```bash
# 默认：Nerd 图标
cargo run -p forge

# 关闭图标（纯 ASCII 回退）
FORGE_ICONS=ascii cargo run -p forge
# 或
FORGE_ICONS=off cargo run -p forge
```

`FORGE_ICONS=nerd|on|1` 强制开启；未设置时默认 Nerd。

## 键位

| Key | Action |
|-----|--------|
| `j` / `↓` · `k` / `↑` | 当前焦点区移动 / 滚动 |
| `Enter` / `l` | 进入目录；staging 下打开 diff |
| `h` / `Backspace` | 上级目录（或从 staging/preview 回列表） |
| `Tab` / `Shift-Tab` | 焦点：列表 → 预览 → staging |
| `g` | 聚焦 staging |
| `[` / `]` | staging 列：unstaged / staged |
| `s` | stage（unstaged 选中项，或列表中的文件） |
| `u` | unstage（staged 选中项，或列表中的文件） |
| `d` | 对选中变更 / 文件显示 diff（预览区） |
| `c` | 打开 commit 消息输入（Enter 提交，Esc 取消） |
| `r` | 刷新列表 + status |
| `q` / `Ctrl-C` | 退出 |

鼠标：点选列表 / staging；双击进目录；滚轮滚动当前区。

## 视觉（Grok dark）

- 近黑背景、柔和次级文字、克制边框
- 冷白/青 accent；选中为浅抬升而非刺眼反色
- Git / diff 色为降饱和 amber / sage / dusty rose（非霓虹）

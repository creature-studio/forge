# Forge performance sample / 本地性能样例

This page records the **local evidence** behind the homepage's performance numbers. It is **not** a cross-platform benchmark or a guaranteed resource limit.

## Observed sample (Forge v0.3.3)

| Measurement | Local observation |
| --- | --- |
| Time until root file list appears | ≈39–40 ms (median of three launches) |
| Idle resident memory (RSS) | ≈14 MiB (median of three samples, release build) |
| Release executable size | 17,231,256 bytes ≈ 17.2 MB |

The run used a local Linux x86_64 machine, an already compiled `target/release/forge` binary, an **already warm filesystem cache** and a disposable, clean Git repository containing `README.md` and `src/` with **1,200 tracked files**. The source directory was **not expanded** at measurement time. The measured startup interval is from `tmux new-session` to the first terminal capture showing the populated root list, and it includes tmux process startup. RSS was read from `/proc/<pid>/status` after approximately half a second of idle time. Tests were repeated three times and medians reported. These values describe **this one sample only** and do not characterize worst-case performance, large expanded trees, peak memory, cold-cache startup, or other devices.

## Reproduce

```bash
cargo build --release -p forge
python3 scripts/benchmark-homepage.py
```

Requirements: Linux, Python 3, Git and tmux. The script creates a disposable sample repository in a temporary directory, launches Forge in an isolated tmux socket, reads its ready screen and RSS, closes sessions and cleans up. It doesn't write to your own repository or configuration.

## Architecture behind the characteristics

- Native compiled Rust TUI, not a packaged web app or Electron renderer.
- Background jobs and bounded concurrency keep file previews and Git operations off the main draw path.
- Lazy previews and shallow folder watchers avoid recursively reading or watching every unexpanded dependency tree when first opening a workspace.
- A single terminal workspace integrates navigation, source preview, Git changes, history, staging and local Shell sessions.

Performance varies with repository size and Git status, syntax-highlighted files, machine speed, filesystem, terminal, cache state, enabled features and build profile. If you need comparable figures for a benchmark suite, record cold and warm launches, percentiles, peak RSS, and the environment explicitly.

---

## 中文说明

首页展示的启动速度、RSS 内存和二进制体积来自**一台 Linux x86_64 机器上的本地样例**，不是所有机器均可达到的性能承诺。样例仓库的 `src/` 下有 1,200 个已跟踪文件，但测试时尚未展开该目录，文件系统缓存已预热。使用 Release 可执行文件，重复 3 次并取中位数：根目录首次显示约 39–40 毫秒、空闲 RSS 约 14 MiB、二进制约 17.2 MB。启动时间包含 tmux 新建会话的时间；RSS 于启动后大约半秒采样。不同环境、冷启动、大文件和展开后的目录可能有明显差异。可运行上面的 Python 脚本复核。

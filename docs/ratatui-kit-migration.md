# ratatui-kit 迁移记录

Forge 已改用 ratatui-kit 0.10.3、Ratatui 0.30.1 和 Crossterm 0.29.0；最低 Rust 版本为 1.89。运行方式保持 `cargo run -p forge`。

## 实现

- kit 接管声明式布局、全屏渲染、响应式更新和输入分层；采用语义 Palette、Modal 和 Input。参考 NSH TUI 的中性表面与细分隔线，工作区移除双框，使用紧凑侧栏、单列分隔线、独立标题和上下文快捷键；选中行使用灰底，青色仅标记焦点。
- 工作区按文件浏览、Git、预览和会话拆分状态，服务上下文持有原有文件系统、Git 和任务调度器。
- 工作区业务状态由组件作用域的 `use_state` 和 `ContextProvider` 持有，生命周期随工作区结束。进程级主题使用 `Atom<Palette>`；提交草稿使用弹窗内部的 `use_state`，取消后销毁。这一作用域选择遵循 Atom 教程“使用满足需求的最小作用域”的建议。
- 文件树、Git 树、语法高亮、diff 与右键菜单作为原生内容组件桥接，继续使用现有虚拟化、命中检测和 hunk 算法。旧的 `UiShell`、`ShellState` 及手写事件循环已删除。
- 调度器新增 `completion_notifier()`，完成和取消时唤醒组件。就绪任务的结果检查只在通知到达时发生，空闲时无定时轮询。
- 保留 CLI 入口、颜色模式、图标配置、快捷键和鼠标操作；窄窗口仅显示当前焦点面板。

## 验证

- `cargo test --workspace --locked`：63 项测试通过。
- `cargo build -p forge --locked`：通过。
- `cargo clippy --workspace --all-targets --locked`：通过，仍有原有绘制及业务代码的 lint 警告。
- `git diff --check` 与本次修改 Rust 文件的 rustfmt 检查：通过。
- 离屏渲染验证单分隔线双栏、整行灰色选中项、窄屏、中文文件名、错误提示、上下文快捷键、提交弹窗、菜单及极小尺寸；状态测试验证过期预览不会覆盖当前选择。
- `python3 scripts/smoke-kit.py`：在临时 Git 仓库和真实 PTY 中验证 stage/unstage、提交、取消草稿、Git hook 失败、弹窗快捷键隔离、缩放、鼠标、空闲无输出和终端模式恢复。
- 依赖树确认仅一套 Ratatui 0.30.1 / Crossterm 0.29.0。

## 检查边界

`cargo fmt --all -- --check` 仍报告未修改文件的既有格式差异，涉及文件系统、Git 后端、领域类型、主题及部分旧 UI 模块。本次只格式化修改过的 Rust 文件。

push/pull 继续复用原后端，本次未连接实际远端验证。原生内容的语法和 diff 色由 kit 语义 Palette 映射，保留内容专用的颜色 token。

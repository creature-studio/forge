# 发布与自更新

Forge 的二进制通过 GitHub Release 分发，安装脚本和 `forge update` 都只依赖 release 里的固定文件名。

## 发布一个版本

1. 修改 `Cargo.toml` 中 `[workspace.package] version`（例如 `0.1.0` → `0.2.0`），提交并推送 `master`。
2. 打同名 tag 并推送：

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. `.github/workflows/release.yml` 被 tag 触发，四个平台并行构建：

   | tag 中的平台 | Runner | 产物 |
   |---|---|---|
   | `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `forge-x86_64-unknown-linux-gnu.tar.gz` |
   | `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `forge-aarch64-unknown-linux-gnu.tar.gz` |
   | `x86_64-apple-darwin` | `macos-15-intel` | `forge-x86_64-apple-darwin.tar.gz` |
   | `aarch64-apple-darwin` | `macos-15` | `forge-aarch64-apple-darwin.tar.gz` |

4. 汇总 job 生成 `SHA256SUMS`，把四个压缩包、`SHA256SUMS` 和 `install.sh` 一起发布到 Release，tag 自动成为 latest。

构建使用 `cargo build --release --locked`。如果 tag 与 `Cargo.toml` 里的版本号不一致，工作流会在构建前失败，避免出现 `forge update` 永远判定有新版本的情况。重跑同一个 tag（`workflow_dispatch` 填 tag）会用 `gh release upload --clobber` 覆盖已有资产。

## 安装

```bash
curl -fsSL https://creature-studio.github.io/forge/install.sh | sh
```

脚本也可以从 release 资产直接取：`https://github.com/creature-studio/forge/releases/latest/download/install.sh`。

行为：

- 用 `uname` 判断平台，映射到上表的 target。
- 未指定 `FORGE_VERSION` 时，从 `releases/latest` 的重定向地址解析 tag。
- 下载 `forge-<target>.tar.gz` 与 `SHA256SUMS`，用 `sha256sum` 或 `shasum -a 256` 校验。
- 解压后安装到 `FORGE_INSTALL_DIR`，默认 `${XDG_BIN_HOME:-$HOME/.local/bin}`，权限 `0755`。
- 不在 `PATH` 里时打印添加提示，不做 `sudo`，不改动 shell 配置。

## `forge update`

`crates/forge/src/update.rs` 只用标准库加 `curl`、`tar`、`sha256sum`/`shasum`，没有新增 Cargo 依赖：

- `forge update --check` 只比较版本，不写文件。
- `forge update` 拉取最新 tag，与编译进二进制的 `CARGO_PKG_VERSION` 比较，只在有新版本时替换。
- 下载、校验、解压后先复制成同目录的 `.forge-update-<pid>-<nonce>`，再用 `fs::rename` 原子替换正在运行的可执行文件。
- 目标目录没有写权限时直接报错，不会留下半截文件。
- `--force` 可在版本相同时重新安装，用于修复损坏的二进制。

musl 与非 Linux/macOS 平台直接退出，提示改用源码构建，因为 release 只提供 glibc 二进制。

## 本地验证

```bash
cargo test -p forge                    # 版本解析与排序
cargo build -p forge
target/debug/forge update --check      # 需要已发布至少一个 release

# 不联网验证 install.sh：伪造一个本地 release 目录，把 base 指向 file://
mkdir -p /tmp/fakerel/stage
printf '#!/bin/sh\necho stub\n' > /tmp/fakerel/stage/forge && chmod +x /tmp/fakerel/stage/forge
tar -czf /tmp/fakerel/forge-x86_64-unknown-linux-gnu.tar.gz -C /tmp/fakerel/stage forge
(cd /tmp/fakerel && sha256sum *.tar.gz > SHA256SUMS)
sed 's#^base=.*#base="file:///tmp/fakerel"#' install.sh > /tmp/install-test.sh
FORGE_VERSION=v0.1.0 FORGE_INSTALL_DIR=/tmp/forgebin sh /tmp/install-test.sh
```

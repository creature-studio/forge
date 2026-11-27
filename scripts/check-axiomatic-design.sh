#!/usr/bin/env bash
# Linux/WSL acceptance. All interactive tests use disposable workspaces and
# private tmux sockets; no interactive user session or user config is changed.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v tmux >/dev/null
command -v python3 >/dev/null
command -v timeout >/dev/null
scratch=$(mktemp -d "${TMPDIR:-/tmp}/forge-ad-check.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export XDG_CONFIG_HOME="$scratch/config" XDG_DATA_HOME="$scratch/data" XDG_STATE_HOME="$scratch/state" XDG_CACHE_HOME="$scratch/cache"
export DISPLAY= WAYLAND_DISPLAY= FORGE_ICONS=builtin
git diff --check
cargo test --workspace --quiet >"$scratch/unit.log" 2>&1 || { cat "$scratch/unit.log"; exit 1; }
awk '/^test result:/{passed+=$4; failed+=$6; ignored+=$8} END {printf "Unit: %d passed, %d failed, %d ignored\n",passed,failed,ignored}' "$scratch/unit.log"
cargo build -p forge --quiet
echo 'Build: PASS'
for script in smoke-embedded-terminal smoke-axiomatic-design smoke-git-stage-selection smoke-open-file-reveal smoke-compact-folders smoke-preview-selection; do
    echo "Running $script"
    if timeout 90s python3 "scripts/$script.py" >"$scratch/$script.log" 2>&1; then
        grep '^PASS' "$scratch/$script.log"
    else
        cat "$scratch/$script.log"
        exit 1
    fi
done
echo 'Axiomatic design regression gate: PASS'

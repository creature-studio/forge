#!/usr/bin/env python3
"""Real-terminal Git branch / ahead / behind widget and safe Sync click."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time
import unicodedata

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-git-status-") as temp:
    root = Path(temp, "repo")
    remote = Path(temp, "origin.git")
    root.mkdir()
    def run(*args, cwd=root):
        proc = sp.run(args, cwd=cwd, capture_output=True, text=True)
        if proc.returncode != 0:
            raise AssertionError(f"{args}: {proc.stderr}")
        return proc.stdout
    run("git", "init", "--bare", "-q", "--initial-branch=dev", str(remote), cwd=Path(temp))
    run("git", "init", "-q", "-b", "dev")
    run("git", "config", "user.name", "Forge Status UI Test")
    run("git", "config", "user.email", "status@example.invalid")
    (root / "README.md").write_text("initial\n")
    run("git", "add", ".")
    run("git", "commit", "-qm", "initial")
    run("git", "remote", "add", "origin", str(remote))
    run("git", "push", "-q", "-u", "origin", "dev")
    (root / "README.md").write_text("local ahead\n")
    run("git", "commit", "-qam", "local ahead")
    assert run("git", "status", "--short", "--branch").startswith("## dev...origin/dev [ahead 1]")
    sock = f"forge-status-ui-{os.getpid()}"
    def tm(*args):
        result = sp.run(["tmux", "-u", "-L", sock, *args], capture_output=True, text=True,
                        env={**os.environ, "LC_ALL": "C.UTF-8",
                             "XDG_CONFIG_HOME": str(Path(temp, "cfg"))})
        if result.returncode:
            raise AssertionError(f"tmux {args}: {result.stderr}")
        return result.stdout
    def screen():
        return tm("capture-pane", "-pt", "ui").splitlines()
    def wait_for(fragment, seconds=12):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            rows = screen()
            for y, line in enumerate(rows):
                if fragment in line:
                    return rows, y, line
            time.sleep(.15)
        raise AssertionError(f"Could not find {fragment!r}:\n" + "\n".join(screen()))
    def cell_width(s):
        return sum(0 if unicodedata.combining(ch) else
                   2 if unicodedata.east_asian_width(ch) in ("F", "W") else 1
                   for ch in s)
    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "110", "-y", "28", "-c", str(root), str(BIN))
        rows, y, line = wait_for("↓0 ↑1")
        assert "dev" in line and "⎇" in line and "↻" in line, line
        assert y >= 24, f"Git status must be in the footer, y={y}: {line}"
        print("PASS: footer shows ⎇ dev ↓0 ↑1 ↻ from real local Git upstream metadata")

        # Click the status chip and ensure Sync opens instead of triggering
        # a push to the temporary remote.
        x = cell_width(line[:line.index("dev")]) + 2
        tm("send-keys", "-l", "-t", "ui", f"\x1b[<0;{x+1};{y+1}M")
        tm("send-keys", "-l", "-t", "ui", f"\x1b[<0;{x+1};{y+1}m")
        wait_for("Remote synchronization")
        assert run("git", "rev-list", "--count", "origin/dev..dev").strip() == "1"
        print("PASS: clicking branch indicator opens Remote synchronization; no implicit Push")
        tm("send-keys", "-t", "ui", "Escape")
        tm("send-keys", "-t", "ui", "q")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

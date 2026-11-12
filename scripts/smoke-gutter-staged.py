#!/usr/bin/env python3
"""Real PTY regression: staged-only and mixed Git source gutters/peeks."""
from pathlib import Path
import os
import subprocess
import tempfile
import time

helper = Path(__file__).with_name("smoke-blame.py").resolve()
namespace = {"__file__": str(helper)}
exec(compile(helper.read_text().split("\nwith tempfile.TemporaryDirectory", 1)[0],
             str(helper), "exec"), namespace)
Terminal = namespace["Terminal"]

with tempfile.TemporaryDirectory(prefix="forge-staged-gutter-") as tmp:
    root = Path(tmp, "repo")
    root.mkdir()
    source = root / "a.txt"
    source.write_text("alpha\nbeta\ngamma\ndelta\n")
    def git(*args):
        return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True).stdout
    git("init", "-q", "-b", "main")
    git("add", "a.txt")
    git("-c", "user.name=Forge", "-c", "user.email=forge@example.invalid", "commit", "-qm", "base")
    source.write_text("alpha\nSTAGED\ngamma\ndelta\n")
    git("add", "a.txt")
    terminal = Terminal(root, {**os.environ, "XDG_CONFIG_HOME": str(Path(tmp, "cfg")),
                              "FORGE_ICONS": "builtin", "DISPLAY": "", "WAYLAND_DISPLAY": ""})
    def wait(text):
        stop = time.monotonic() + 9
        while time.monotonic() < stop:
            capture = terminal.capture()
            if text in capture:
                return capture
            time.sleep(.12)
        raise AssertionError(f"Missing {text!r}:\n{terminal.capture()}")
    def send(data):
        terminal.send(data)
    try:
        wait("F7 Peek")
        send(b"\t")  # Move focus from Files tree to the preview before F7.
        send(b"\x1b[18~")  # Source change peek
        wait("Staged")
        wait("+STAGED")
        send(b"\x1b")
        source.write_text("alpha\nSTAGED\ngamma\nWORKTREE\n")
        wait("WORKTREE")
        send(b"\x1b[18~")
        wait("Unstaged")
        wait("+WORKTREE")
        send(b"\x1b")
        print("PASS: staged-only and mixed staged/unstaged Git gutter peek in real PTY")
    finally:
        terminal.close()

#!/usr/bin/env python3
"""Real TUI: s stages a selected file, follows it to STAGED, u returns to CHANGES."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-stage-select-") as temp:
    root = Path(temp, "repo")
    root.mkdir()
    def git(*args):
        r = sp.run(["git", *args], cwd=root, capture_output=True, text=True)
        if r.returncode:
            raise AssertionError(f"git {args}: {r.stderr}")
        return r.stdout.strip()

    git("init", "-qb", "main")
    git("config", "user.name", "Forge Stage Test")
    git("config", "user.email", "stage@example.invalid")
    (root / "a.txt").write_text("base a\n")
    (root / "b.txt").write_text("base b\n")
    git("add", ".")
    git("commit", "-qm", "baseline")
    (root / "a.txt").write_text("changed a\n")
    (root / "b.txt").write_text("changed b\n")
    git("add", "b.txt")  # STAGED group must already exist.

    sock = f"forge-stage-select-{os.getpid()}"
    def tm(*args):
        r = sp.run(["tmux", "-u", "-L", sock, *args], capture_output=True, text=True,
                   env={**os.environ, "LC_ALL": "C.UTF-8",
                        "XDG_CONFIG_HOME": str(Path(temp, "cfg"))})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def screen():
        return tm("capture-pane", "-pt", "ui").splitlines()
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.2)
    def selected_file(side):
        lines = screen()
        try:
            section_y = next(i for i, s in enumerate(lines) if f"{side} (" in s)
            after_y = next(i for i, s in enumerate(lines)
                           if i > section_y and (("STAGED (" in s) if side == "CHANGES"
                                                else ("HISTORY (" in s)))
        except StopIteration:
            return False
        displayed = any("a.txt" in row for row in lines[section_y+1:after_y])
        side = "unstaged" if side == "CHANGES" else "staged"
        # The selected-row arrow is theme-dependent, and status messages
        # briefly replace the selected-row footer after a Git write. The
        # selected file's newly rendered diff is a stronger invariant.
        diff = "\n".join(lines[:9])
        return displayed and f"Diff  {side} · a.txt" in diff

    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "108", "-y", "30", "-c", str(root), str(BIN), "--force-ansi16")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and "a.txt" not in "\n".join(screen()):
            time.sleep(.2)
        send("F2")
        send("Home")
        send("j")  # CHANGES heading -> a.txt
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and not selected_file("CHANGES"):
            time.sleep(.15)
        assert selected_file("CHANGES"), "Could not select CHANGES/a.txt\n"+"\n".join(screen())
        send("s")
        deadline = time.monotonic() + 9
        while time.monotonic() < deadline:
            if "a.txt" in git("diff", "--cached", "--name-only") and selected_file("STAGED"):
                break
            time.sleep(.15)
        assert selected_file("STAGED"), (
            "Stage succeeded but selected file was lost when it moved to STAGED:\n"
            + "\n".join(screen()))
        print("PASS: s moves a.txt to STAGED and keeps the same file selected")

        send("u")
        deadline = time.monotonic() + 9
        while time.monotonic() < deadline:
            if "a.txt" not in git("diff", "--cached", "--name-only") and selected_file("CHANGES"):
                break
            time.sleep(.15)
        assert selected_file("CHANGES"), (
            "Unstage succeeded but did not return focus to CHANGES/a.txt:\n"
            + "\n".join(screen()))
        assert git("diff", "--cached", "--name-only") == "b.txt", "Other staged file changed"
        print("PASS: u restores a.txt to CHANGES without touching other staged files")
        send("q")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

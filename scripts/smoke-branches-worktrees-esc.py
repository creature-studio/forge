#!/usr/bin/env python3
"""Actual Forge TUI: nested Branches/Worktrees Esc keeps list and Git data."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-esc-nav-") as home:
    repo = Path(home, "repo")
    repo.mkdir()
    def git(*args):
        r = sp.run(["git", *args], cwd=repo, text=True, capture_output=True)
        if r.returncode:
            raise AssertionError(f"git {args}: {r.stderr}")
        return r.stdout.strip()
    git("init", "-qb", "main")
    git("config", "user.name", "Forge Escape Smoke")
    git("config", "user.email", "esc@example.invalid")
    (repo / "README.md").write_text("baseline\n")
    git("add", ".")
    git("commit", "-qm", "baseline")
    git("branch", "feature/demo")
    linked = Path(home, "linked")
    git("worktree", "add", "-q", "-b", "feature/linked", str(linked))
    sock = f"forge-esc-nav-{os.getpid()}"
    def tm(*args):
        r = sp.run(["tmux", "-u", "-L", sock, *args], capture_output=True,
                   text=True, env={**os.environ, "LC_ALL":"C.UTF-8",
                         "XDG_CONFIG_HOME": str(Path(home, "cfg"))})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def screen():
        return tm("capture-pane", "-pt", "ui")
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.24)
    def wait_for(fragment, timeout=7):
        deadline=time.monotonic()+timeout
        while time.monotonic() < deadline:
            pane=screen()
            if fragment in pane:
                return pane
            time.sleep(.15)
        raise AssertionError(f"Missing {fragment!r}:\n{screen()}")
    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "112", "-y", "34", "-c", str(repo), str(BINARY))
        wait_for("Files")
        send("B")
        wait_for("Branches")
        # Branches now defaults to current branch, which is intentionally a
        # no-op on Enter. Select a different branch to test the dialog.
        send("Home")
        send("Enter")
        wait_for("Switch this repository")
        send("Escape")
        wait_for("Branches")
        assert "Switch this repository" not in screen()
        print("PASS: Branches → switch confirmation → Esc returns to Branches")
        send("r")
        wait_for("Rename branch")
        send("Escape")
        wait_for("Branches")
        send("A")
        wait_for("Create branch")
        send("Escape")
        wait_for("Branches")
        print("PASS: Branch rename/create prompts return to Branches")
        send("X")
        wait_for("Delete branch")
        send("Escape")
        wait_for("Branches")
        assert "feature/demo" in git("branch", "--list"), "Esc deleted branch"
        print("PASS: Branch delete confirmation cancelled with no Git change")
        send("D")
        wait_for("Git diff", timeout=6)
        send("Escape")
        wait_for("Branches")
        send("Escape")

        send("W")
        wait_for("Worktrees")
        send("A")
        wait_for("Create worktree")
        send("Escape")
        wait_for("Worktrees")
        # The primary worktree sorts before the linked worktree; choose the
        # other entry using Down if needed. The panel can be navigated by keys.
        send("j")
        send("X")
        wait_for("Remove worktree")
        send("Escape")
        wait_for("Worktrees")
        assert linked.is_dir(), "Cancelled Worktree deletion removed files"
        assert str(linked) in git("worktree", "list", "--porcelain")
        print("PASS: Worktree create/remove dialogs return to Worktrees; deletion cancelled")
        send("Escape")
        send("q")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

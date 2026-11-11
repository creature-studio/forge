#!/usr/bin/env python3
"""Keyboard-only stash save/diff/back/apply/drop, with real temporary Git repo."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-stash-keyboard-") as d:
    root = Path(d, "repo")
    root.mkdir()
    def git(*args):
        result = sp.run(["git", *args], cwd=root, capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(f"git {args}: {result.stderr}")
        return result.stdout.strip()
    git("init", "-q", "-b", "main")
    git("config", "user.name", "Forge Stash Keyboard Test")
    git("config", "user.email", "forge-stash@example.invalid")
    tracked = root / "code.rs"
    untracked = root / "new-file.md"
    tracked.write_text("fn main() {}\n")
    git("add", ".")
    git("commit", "-qm", "initial")
    tracked.write_text("fn main() { println!(\"STASH_MARKER\"); }\n")
    untracked.write_text("Untracked STASH_MARKER\n")

    sock = f"forge-stash-keys-{os.getpid()}"
    def tm(*args):
        r = sp.run(["tmux", "-u", "-L", sock, *args], capture_output=True, text=True,
                   env={**os.environ, "LC_ALL": "C.UTF-8",
                        "XDG_CONFIG_HOME": str(Path(d, "cfg"))})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.17)
    def screen():
        time.sleep(.12)
        return tm("capture-pane", "-pt", "ui")
    def wait_for(check, title, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if check():
                return
            time.sleep(.18)
        raise AssertionError(f"Timeout {title}\n{screen()}\nGit status:\n{git('status', '--short')}")
    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "114", "-y", "32", "-c", str(root), str(BIN))
        wait_for(lambda: "Files" in screen(), "startup")
        send("Z")
        wait_for(lambda: "Stashes" in screen(), "stash panel")
        wait_for(lambda: "No results" in screen(), "empty stash list")
        send("A")
        wait_for(lambda: "Save stash" in screen(), "save message prompt")
        send("Escape")
        wait_for(lambda: "Stashes" in screen() and "No results" in screen(),
                 "cancel input returns stash list")
        send("A")
        wait_for(lambda: "Save stash" in screen(), "save prompt second time")
        tm("send-keys", "-l", "-t", "ui", "keyboard stash")
        send("Enter")
        wait_for(lambda: "Save all tracked and untracked" in screen(), "save confirmation")
        send("Escape")
        wait_for(lambda: "Save stash" in screen() and "keyboard stash" in screen(),
                 "cancel confirm returns edited message")
        send("Enter")
        wait_for(lambda: "Save all tracked and untracked" in screen(),
                 "save confirmation repeated")
        assert tracked.read_text() != "fn main() {}\n"
        send("Enter")
        wait_for(lambda: bool(git("stash", "list")), "stash save", timeout=12)
        wait_for(lambda: tracked.read_text() == "fn main() {}\n" and not untracked.exists(),
                 "stash worktree cleanup")
        assert len(git("stash", "list").splitlines()) == 1
        wait_for(lambda: "Stashes" in screen() and "keyboard stash" in screen(), "return stash list")
        print("PASS: Z → A → type message → Enter → confirmation saves tracked + untracked")

        send("Enter")
        wait_for(lambda: "Stash diff" in screen() and "STASH_MARKER" in screen(),
                 "inspect stash diff")
        send("PageDown")
        send("Escape")
        wait_for(lambda: "Stashes" in screen() and "keyboard stash" in screen(),
                 "Esc returns stash list")
        print("PASS: Enter reads stash diff, PgDown scrolls, Esc returns selected list")

        send("a")
        wait_for(lambda: "Apply" in screen() and "stash is kept" in screen(), "apply confirmation")
        send("Enter")
        wait_for(lambda: "STASH_MARKER" in tracked.read_text() and untracked.exists(),
                 "applied tracked and untracked")
        assert len(git("stash", "list").splitlines()) == 1, "Apply unexpectedly dropped stash"
        wait_for(lambda: "Stashes" in screen() and "keyboard stash" in screen(),
                 "return stash list after apply")
        print("PASS: a → confirm applies stash while retaining saved entry")

        send("X")
        wait_for(lambda: "Permanently drop" in screen(), "drop confirmation")
        send("Escape")
        assert git("stash", "list"), "Esc canceled drop but stash disappeared"
        wait_for(lambda: "Stashes" in screen(), "drop cancel returns list")
        send("X")
        wait_for(lambda: "Permanently drop" in screen(), "drop confirmation repeated")
        send("Enter")
        wait_for(lambda: not git("stash", "list"), "confirmed drop", timeout=12)
        assert untracked.exists() and "STASH_MARKER" in tracked.read_text()
        print("PASS: X → Esc cancels; X → Enter drops only stash, keeps worktree")
        send("Escape")
        send("q")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

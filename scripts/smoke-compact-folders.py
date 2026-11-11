#!/usr/bin/env python3
"""Compact Folders on Files, Git and History, with persisted Settings toggle."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-compact-ui-") as d:
    root = Path(d, "repo")
    root.mkdir()
    config = Path(d, "cfg")
    sock = f"forge-compact-{os.getpid()}"
    paths = [
        "manager/frontend/src/views/login.vue",
        "manager/frontend/src/views/permission.ts",
        "manager/server/zenk-admin/src/main/java/com/nist/zenk/web/controller/system/IdentitySsoController.java",
        "manager/server/zenk-admin/src/test/java/TestLogin.java",
    ]
    def git(*args):
        return sp.run(["git", *args], cwd=root, check=True, capture_output=True, text=True).stdout
    git("init", "-q", "-b", "main")
    git("config", "user.name", "Compact UI Test")
    git("config", "user.email", "compact@example.invalid")
    for path in paths:
        file = root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("original\n")
    git("add", ".")
    git("commit", "-qm", "initial")
    for path in paths:
        (root / path).write_text("modified in history\n")
    git("add", ".")
    git("commit", "-qm", "commit with nested directories")
    for path in paths:
        (root / path).write_text("working tree changes\n")

    def tm(*args):
        out = sp.run(["tmux", "-L", sock, *args], text=True, capture_output=True,
            env={**os.environ, "XDG_CONFIG_HOME": str(config), "FORGE_ICONS": "builtin"})
        if out.returncode:
            raise AssertionError(f"tmux {args}: {out.stderr}")
        return out.stdout
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.25)
    def snap():
        time.sleep(.35)
        return tm("capture-pane", "-pt", "ui")
    def left():
        return "\n".join(line[:49] for line in snap().splitlines())

    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui", "-x", "145",
           "-y", "40", "-c", str(root), str(BINARY))
        time.sleep(1.3)
        send("F2")
        send("Enter")  # expand manager (CHANGES)
        send("j")
        send("Enter")  # frontend/src/views and its files expand in one action
        changes = left()
        print("GIT CHANGES TREE:\n" + changes[:1500])
        assert "frontend / src / views" in changes, "changes chain was not merged"
        assert "server / zenk-admin / src" in changes or "server" in changes

        send("G")
        send("Tab") # Return focus from commit detail preview to History tree.
        send("Enter")  # expand selected commit's files
        time.sleep(.9)
        history = left()
        print("GIT HISTORY TREE (commit expanded):\n" + history[:1900])
        assert "HISTORY" in history
        assert "manager (4)" in history, "first commit was not expanded"
        send("j")
        send("Enter")  # manager; reveals frontend and server branches
        send("j")
        send("Enter")  # one action: frontend / src / views and its files
        time.sleep(.3)
        expanded = left()
        print("HISTORY FOLDERS:\n" + expanded[:1700])
        assert "frontend / src / views" in expanded, "History folder chain did not compact"
        assert "login.vue" in expanded, "History changed file disappeared"
        send("j")
        send("Enter")  # open real file diff through compact History ancestry
        time.sleep(.5)
        diff_screen = snap()
        assert "login.vue" in diff_screen, "Commit diff lost original filename"
        assert "modified in history" in diff_screen, "Commit file diff was not loaded"


        send("F1")
        send("Home")
        send("Enter")  # manager: frontend and server
        send("j")
        send("Enter")  # one action: frontend / src / views and its files
        time.sleep(.4)  # filesystem listings may be async
        files = left()
        print("FILES TREE:\n" + files[:1250])
        assert "frontend / src / views" in files, "Files folder chain did not compact"
        assert "login.vue" in files, "File path lost after compaction"
        send("F2")



        # Settings toggles immediately and persists across restart.
        send(",")
        settings = snap()
        assert "Compact Folders" in settings
        send("Down")
        send("Space")
        assert "Compact Folders" in snap()
        config_file = config / "forge" / "config.toml"
        assert "compact_folders = false" in config_file.read_text()
        send("Escape")
        uncompact = left()
        assert "frontend / src / views" not in uncompact, "compact mode did not disable"
        send(",")
        send("Down")
        send("Space")
        send("Escape")
        assert "compact_folders = true" in config_file.read_text()
        print("PASS: Git compact paths and persisted Settings On/Off")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

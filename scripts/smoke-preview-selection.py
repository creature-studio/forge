#!/usr/bin/env python3
"""Real-terminal mouse selection + Ctrl-C in Markdown Preview, Code and .rs source."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-preview-selection-") as d:
    root = Path(d, "repo")
    root.mkdir()
    (root / "README.md").write_text(
        "# AlphaPreview\n\nA paragraph with 中文说明.\n\n"
        "| Field | Description |\n|---|---|\n| userId | 中文用户 |\n", encoding="utf-8")
    (root / "sample.rs").write_text("fn main() {\n    let flag = true;\n    println!(\"CodeSelection\");\n}\n")
    def git(*args):
        sp.run(["git", *args], cwd=root, check=True, capture_output=True)
    git("init", "-q", "-b", "main")
    git("add", ".")
    git("-c", "user.name=Forge Preview Test", "-c", "user.email=preview@example.invalid",
        "commit", "-qm", "baseline")
    with (root / "sample.rs").open("a") as stream:
        stream.write("// DiffDragMarker\n")
    sock = f"forge-preview-sel-{os.getpid()}"
    cfg = Path(d, "cfg")
    def tm(*args):
        result = sp.run(["tmux", "-L", sock, *args], text=True, capture_output=True,
                        env={**os.environ, "XDG_CONFIG_HOME": str(cfg),
                             "DISPLAY": "", "WAYLAND_DISPLAY": "", "FORGE_ICONS": "builtin"})
        if result.returncode:
            raise AssertionError(f"tmux {' '.join(args)}: {result.stderr}")
        return result.stdout
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.2)
    def raw(seq):
        tm("send-keys", "-l", "-t", "ui", seq)
        time.sleep(.28)
    def screen():
        time.sleep(.27)
        return tm("capture-pane", "-p", "-t", "ui").splitlines()
    def wait_for(fragment, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            rows = screen()
            if any(fragment in row for row in rows):
                return rows
        raise AssertionError(f"Missing {fragment!r} from screen:\n" + "\n".join(screen()))
    def select(text):
        rows = wait_for(text)
        y = next(i for i, line in enumerate(rows) if text in line)
        x = rows[y].find(text)
        # SGR press/drag/release. Mouse coordinates in SGR are 1-based.
        def event(button, at_x, at_y, up=False):
            raw(f"\x1b[<{button};{at_x + 1};{at_y + 1}{'m' if up else 'M'}")
        # A file preview can finish rendering while the asynchronous Git
        # status refreshes. Let the final view settle before dragging, and
        # retry the gesture once if tmux swallowed the initial mouse packet.
        time.sleep(.35)
        for _ in range(2):
            event(0, x, y)
            event(32, x + min(len(text), 8), y)
            event(0, x + min(len(text), 8), y, up=True)
            if "Preview selection ready" in "\n".join(screen()):
                break
        else:
            raise AssertionError("Preview did not accept mouse drag:\n" + "\n".join(screen()))
        send("C-c")
        updated = "\n".join(screen())
        assert "characters" in updated and (
            "clipboard" in updated or "OSC 52" in updated
        ), updated
    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "114", "-y", "30", "-c", str(root), str(BINARY))
        wait_for("README.md")
        send("Home")
        wait_for("AlphaPreview")
        select("AlphaPreview")
        print("PASS: Markdown Preview mouse drag + Ctrl-C")

        send("F3")
        wait_for("# AlphaPreview")
        select("# AlphaPreview")
        print("PASS: Markdown Code mouse drag + Ctrl-C")

        rows = screen()
        y = next(i for i, row in enumerate(rows) if "sample.rs" in row)
        x = rows[y].index("sample.rs") + 2
        # Click the file directly: keyboard Home while Preview has focus
        # scrolls the document, not the sidebar.
        raw(f"\x1b[<0;{x+1};{y+1}M")
        raw(f"\x1b[<0;{x+1};{y+1}m")
        wait_for("CodeSelection")
        select("CodeSelection")
        print("PASS: Rust code preview mouse drag + Ctrl-C")
        send("Escape")
        assert "Preview selection cleared" in "\n".join(screen())
        send("F2")
        wait_for("CHANGES")
        rows = wait_for("sample.rs")
        y = next(i for i, row in enumerate(rows) if "sample.rs" in row and i > 2)
        x = rows[y].find("sample.rs") + 2
        raw(f"\x1b[<0;{x+1};{y+1}M")
        raw(f"\x1b[<0;{x+1};{y+1}m")
        wait_for("DiffDragMarker")
        select("DiffDragMarker")
        print("PASS: Git Diff preview mouse drag + Ctrl-C")
        send("q")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

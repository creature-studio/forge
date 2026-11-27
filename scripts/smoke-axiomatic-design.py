#!/usr/bin/env python3
"""Real-terminal integration: editor, shared search scope, watcher and preview.
A private tmux server supplies the terminal protocol; no user sessions are used.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


def smoke():
    with tempfile.TemporaryDirectory(prefix="forge-ad-smoke-") as directory:
        base = Path(directory)
        root = base / "repo"
        root.mkdir()
        note = root / "a.txt"
        note.write_text("alpha\nbeta\n")
        (root / "src").mkdir()
        (root / "src" / "deep.rs").write_text("line one\nneedle42\nline three\n")
        for folder in ["target", "node_modules"]:
            (root / folder).mkdir()
            (root / folder / "excluded.rs").write_text("needle42\n")

        def git(*args):
            return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()

        git("init", "-qb", "main")
        git("config", "user.name", "Forge Test")
        git("config", "user.email", "forge@example.invalid")
        git("add", "a.txt", "src")
        git("commit", "-qm", "baseline")
        socket = f"forge-ad-smoke-{os.getpid()}"
        env = {**os.environ, "XDG_CONFIG_HOME": str(base / "cfg"),
               "DISPLAY": "", "WAYLAND_DISPLAY": "", "FORGE_ICONS": "builtin"}

        def tm(*args, data=None):
            result = subprocess.run(["tmux", "-L", socket, *args], env=env,
                                    input=data, capture_output=True)
            if result.returncode:
                raise AssertionError(result.stderr.decode(errors="replace"))
            return result.stdout.decode(errors="replace")

        def screen():
            return tm("capture-pane", "-pt", "ui")

        def key(*keys):
            tm("send-keys", "-t", "ui", *keys)
            time.sleep(.18)

        def literal(text):
            tm("send-keys", "-t", "ui", "-l", text)
            time.sleep(.18)

        def wait(predicate, label):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                if predicate():
                    return
                time.sleep(.12)
            raise AssertionError(label + "\n" + screen())

        def see(fragment):
            wait(lambda: fragment in screen(), f"missing {fragment!r}")

        def paste(value):
            tm("load-buffer", "-b", "ad-paste", "-", data=value.encode())
            tm("paste-buffer", "-d", "-p", "-b", "ad-paste", "-t", "ui")
            time.sleep(.25)

        try:
            tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
               "-x", "130", "-y", "38", "-c", str(root), str(BINARY))
            see("a.txt")
            key("End")
            see("alpha")
            key("C-e")
            see("Copy selection")
            key("C-a")
            paste("replacement\ntext")
            key("C-s")
            wait(lambda: note.read_text() == "replacement\ntext", "editor save")
            key("C-z", "C-s")
            wait(lambda: note.read_text() == "alpha\nbeta\n", "atomic paste undo")
            key("C-y", "C-s")
            wait(lambda: note.read_text() == "replacement\ntext", "atomic paste redo")
            key("Escape")
            see("EXPLORER")
            print("PASS: editor save, bracketed paste undo/redo and return to workspace", flush=True)

            note.write_text("external-change-visible\n")
            see("external-change-visible")
            (root / "freshly-created.rs").write_text("fresh\n")
            see("freshly-created.rs")
            key("F1", "C-p")
            literal("freshly")
            see("freshly-created.rs")
            key("Escape")
            key("C-p")
            literal("deeprs")
            see("deep.rs")
            literal("zzzz")
            see("No results")
            key("Escape")
            # Use the Search activity so the host need not distinguish
            # Ctrl+Shift+F from Ctrl+F (the in-file Find shortcut).
            rows = screen().splitlines()
            y = next(i for i, row in enumerate(rows) if "⌕" in row[:5])
            x = rows[y].index("⌕")
            literal(f"\x1b[<0;{x+1};{y+1}M")
            literal(f"\x1b[<0;{x+1};{y+1}m")
            see("Search contents")
            literal("needle")
            see("deep.rs:2")
            see("line one")
            see("line three")
            assert "excluded.rs" not in screen()
            key("Enter")
            see("needle42")
            assert git("diff", "--cached", "--name-only") == ""
            assert note.read_text() == "external-change-visible\n"
            print("PASS: external changes, filename/content search, exclusions, reveal and Git isolation", flush=True)
        finally:
            subprocess.run(["tmux", "-L", socket, "kill-server"], env=env, capture_output=True)


if __name__ == "__main__":
    smoke()

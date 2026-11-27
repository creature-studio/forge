#!/usr/bin/env python3
"""Exercise real embedded PTY tab isolation and input routing in a disposable workspace."""
import os
from pathlib import Path
import subprocess
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


def smoke():
    with tempfile.TemporaryDirectory(prefix="forge-terminal-smoke-") as directory:
        base = Path(directory)
        root = base / "workspace"
        root.mkdir()
        (root / "a.txt").write_text("preview-marker\n")
        cfg = base / "config" / "forge"
        cfg.mkdir(parents=True)
        (cfg / "config.toml").write_text(
            '[terminal]\ndefault_profile = "test"\n'
            '[terminal.profiles.test]\ncommand = "/bin/sh"\nargs = ["-i"]\n'
        )
        socket = f"forge-terminal-smoke-{os.getpid()}"
        env = {**os.environ, "XDG_CONFIG_HOME": str(cfg.parent), "SHELL": "/bin/sh",
               "DISPLAY": "", "WAYLAND_DISPLAY": "", "FORGE_ICONS": "builtin"}

        def tm(*args):
            return subprocess.check_output(
                ["tmux", "-L", socket, *args], env=env, stderr=subprocess.STDOUT, text=True
            )

        def key(*args):
            tm("send-keys", "-t", "ui", *args)
            time.sleep(.2)

        def shell(command):
            tm("send-keys", "-t", "ui", "-l", command)
            key("Enter")

        def click(x, y):
            for suffix in ("M", "m"):
                tm("send-keys", "-t", "ui", "-l", f"\x1b[<0;{x+1};{y+1}{suffix}")
                time.sleep(.12)

        def paste(text):
            tm("set-buffer", "-b", "focus-paste", text)
            tm("paste-buffer", "-d", "-p", "-b", "focus-paste", "-t", "ui")
            time.sleep(.25)

        def click_text(text, last=False):
            rows = [(i, row.index(text)) for i, row in enumerate(screen().splitlines()) if text in row]
            assert rows, screen()
            y, x = rows[-1 if last else 0]
            click(x, y)

        def screen():
            return tm("capture-pane", "-pt", "ui")

        def wait(predicate, description):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                if predicate():
                    return
                time.sleep(.12)
            raise AssertionError(description + "\n" + screen())

        def marker(name, value):
            wait(lambda: (root / name).is_file() and (root / name).read_text() == value,
                 f"missing {name}={value}")

        try:
            tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
               "-x", "120", "-y", "40", "-c", str(root), str(BINARY))
            wait(lambda: "preview-marker" in screen(), "initial preview")
            key("F8")
            time.sleep(.6)
            shell("PS1='FOCUS_PROMPT> '")
            wait(lambda: "FOCUS_PROMPT>" in screen(), "shell prompt")
            click_text("preview-marker")
            key("C-f")
            wait(lambda: "Find in file" in screen(), "Find opened")
            paste("preview-marker")
            def find_query():
                rows = screen().splitlines()
                y = next(i for i, row in enumerate(rows) if "Find in file" in row)
                return rows[y+1]
            wait(lambda: "preview-marker" in find_query(), "Find owns paste")
            click_text("FOCUS_PROMPT>", last=True)
            paste("printf focus-ok > focus.txt")
            key("Enter")
            marker("focus.txt", "focus-ok")
            assert "preview-marker" in find_query(), screen()
            assert "printf focus-ok" not in find_query(), screen()
            # Click the actual query field, not a preview line or terminal prompt.
            rows = screen().splitlines()
            y = next(i for i, row in enumerate(rows) if "Find in file" in row) + 1
            click(rows[y].index("preview-marker") + 2, y)
            key("BSpace")
            wait(lambda: "preview-marke" in find_query() and "preview-marker" not in find_query(), "Find regained keyboard")
            key("Escape")
            wait(lambda: "Find in file" not in screen(), "Find dismissed")
            click_text("FOCUS_PROMPT>", last=True)
            print("PASS: Find and PTY pointer focus handoff, isolated paste and keyboard ownership", flush=True)
            shell("SESSION=first; printf '%s' \"$SESSION\" > first.txt")
            marker("first.txt", "first")
            key("S-F8")
            time.sleep(.6)
            shell("SESSION=second; printf '%s' \"$SESSION\" > second.txt")
            marker("second.txt", "second")
            key("F12")
            shell("printf '%s' \"$SESSION\" > switched.txt")
            marker("switched.txt", "first")
            key("F12")
            shell("printf '%s' \"$SESSION\" > back.txt")
            marker("back.txt", "second")
            key("F11")
            shell("printf '%s' \"$SESSION\" > maximized.txt")
            marker("maximized.txt", "second")
            key("F11")
            key("F8")
            wait(lambda: "preview-marker" in screen(), "hidden terminal restored workspace")
            key("F8")
            shell("printf started > sleeping.txt; sleep 30 && printf wrong > unexpected.txt")
            marker("sleeping.txt", "started")
            key("C-c")
            shell("printf '%s' \"$SESSION\" > interrupted.txt")
            marker("interrupted.txt", "second")
            assert not (root / "unexpected.txt").exists()
            assert (root / "a.txt").read_text() == "preview-marker\n"
            print("PASS: real PTY input, two tabs, retained shell state, maximize, hide/show, Ctrl+C")
        finally:
            subprocess.run(["tmux", "-L", socket, "kill-server"], env=env, capture_output=True)


if __name__ == "__main__":
    smoke()

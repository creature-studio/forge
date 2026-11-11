#!/usr/bin/env python3
"""Direct PTY source gutter / peek regression; Git writes stay in a temp repo.

Run after cargo build --offline -p forge. Reuses the terminal emulator from the
blame smoke script, loading only its helper definitions, before its test body.
"""
from pathlib import Path
import os
import subprocess
import tempfile
import time

helper = Path(__file__).with_name("smoke-blame.py").resolve()
scope = {"__file__": str(helper)}
exec(compile(helper.read_text().split("\nwith tempfile.TemporaryDirectory", 1)[0],
             str(helper), "exec"), scope)
Terminal = scope["Terminal"]

with tempfile.TemporaryDirectory(prefix="forge-inline-peek-") as tmp:
    root = Path(tmp, "repo")
    root.mkdir()
    file = root / "a.txt"
    file.write_text("old source\nsecond source\nthird source\n")
    markdown = root / "m.md"
    markdown.write_text("# BeforeMarkdown\nsecond md\n")

    def git(*args):
        return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True).stdout

    git("init", "-q", "-b", "main")
    git("add", "a.txt", "m.md")
    git("-c", "user.name=Peek Test", "-c", "user.email=peek@example.invalid",
        "commit", "-qm", "baseline")
    file.write_text("new source\nsecond source\nthird source\n")
    markdown.write_text("# AfterMarkdown\nsecond md\n")
    terminal = Terminal(root, {**os.environ, "XDG_CONFIG_HOME": str(Path(tmp, "cfg")),
                              "FORGE_ICONS": "builtin", "DISPLAY": "", "WAYLAND_DISPLAY": ""})

    def wait(text):
        end = time.monotonic() + 7
        while time.monotonic() < end:
            screen = terminal.capture()
            if text in screen:
                return screen
            time.sleep(.1)
        raise AssertionError(f"Missing {text!r}:\n{terminal.capture()}")

    def send(seq):
        terminal.send(seq)

    def click(x, y):
        send(f"\x1b[<0;{x+1};{y+1}M".encode())
        send(f"\x1b[<0;{x+1};{y+1}m".encode())

    try:
        time.sleep(.6)
        send(b"\x1b[H")
        # Let initial list/status jobs and the Home preview request settle.
        time.sleep(1.5)
        rows = wait("F7 Peek").splitlines()
        y = next(y for y, row in enumerate(rows) if "1│new source" in row)
        x = rows[y].index("1│new source")
        click(x, y)
        wait("-old source")
        wait("+new source")
        wait("[×] Esc")
        send(b"\x1b[18~")  # F7
        text = wait("1│new source")
        assert "-old source" not in text
        assert "1│new source" in text.splitlines()[y], "peek changed source position"
        send(b"\x1b[18~")
        wait("-old source")
        send(b"\x1b")
        assert "-old source" not in wait("1│new source")
        send(b"\x1b[17~")  # F6
        wait("Commit details")
        wait("Uncommitted changes")
        send(b"\x1b")
        wait("1│new source")
        file.write_text("external source\nsecond source\nthird source\n")
        wait("external source")
        wait("F7 Peek")
        send(b"\x1b[18~")
        wait("+external source")
        send(b"\x1b")
        send(b"\t")
        send(b"\x1b[B")
        text = wait("AfterMarkdown")
        assert "F7 Peek" not in text, "rendered Markdown has source gutter hint"
        send(b"\t")
        send(b"\x1b[18~")
        assert "[×] Esc" not in terminal.capture(), "rendered Markdown opened a peek"
        send(b"\x1bOR")  # F3
        wait("1│# AfterMarkdown")
        wait("F7 Peek")
        send(b"\x1b[18~")
        wait("-# BeforeMarkdown")
        wait("+# AfterMarkdown")
        print("PASS: PTY click, F7/Esc, stable source position, F6 blame, external edit, Markdown Preview excluded and Code peek")
    finally:
        terminal.close()

#!/usr/bin/env python3
"""Real-terminal blame/popup regression. All Git writes use a temporary repo.

Run after cargo build -p forge: python3 scripts/smoke-blame.py
Uses a direct PTY (no socket server); configuration and desktop clipboard are isolated.
"""
import os
import codecs
import fcntl
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess as sp
import tempfile
import termios
import time
import unicodedata

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


class Terminal:
    """Small VT screen for the cursor/erase sequences emitted by Ratatui."""
    def __init__(self, root, env):
        self.width, self.height = 118, 32
        self.rows = [[" "] * self.width for _ in range(self.height)]
        self.x = self.y = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.chdir(root)
            os.execve(str(BINARY), [str(BINARY)], {**env, "TERM": "xterm-256color"})
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", self.height, self.width, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def feed(self, text):
        self.pending += text
        while self.pending:
            if self.pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                self.pending = self.pending[match.end():]
                params, _, final = match.groups()
                nums = [int(v or 0) for v in params.lstrip("?<=>").split(";")]
                a = nums[0] or 1
                if final in "Hf":
                    self.y = min(self.height - 1, a - 1)
                    self.x = min(self.width - 1, (nums[1] if len(nums) > 1 else 1) - 1)
                elif final == "G": self.x = min(self.width - 1, a - 1)
                elif final == "d": self.y = min(self.height - 1, a - 1)
                elif final == "A": self.y = max(0, self.y - a)
                elif final == "B": self.y = min(self.height - 1, self.y + a)
                elif final == "C": self.x = min(self.width - 1, self.x + a)
                elif final == "D": self.x = max(0, self.x - a)
                elif final == "J" and nums[0] in (2, 3):
                    self.rows = [[" "] * self.width for _ in range(self.height)]
                elif final == "K":
                    lo, hi = (0, self.x + 1) if nums[0] == 1 else (0, self.width) if nums[0] == 2 else (self.x, self.width)
                    self.rows[self.y][lo:hi] = [" "] * (hi - lo)
                elif final == "n" and nums[0] == 6:
                    os.write(self.master, f"\x1b[{self.y+1};{self.x+1}R".encode())
                continue
            if self.pending.startswith("\x1b]"):
                end = re.search(r"\x07|\x1b\\", self.pending)
                if not end: return
                self.pending = self.pending[end.end():]
                continue
            ch, self.pending = self.pending[0], self.pending[1:]
            if ch == "\x1b":
                if not self.pending:
                    self.pending = ch
                    return
                self.pending = self.pending[1:]
            elif ch == "\r": self.x = 0
            elif ch == "\n": self.y = min(self.height - 1, self.y + 1)
            elif ch == "\b": self.x = max(0, self.x - 1)
            elif not unicodedata.category(ch).startswith("C"):
                width = 0 if unicodedata.combining(ch) else 2 if unicodedata.east_asian_width(ch) in "WF" else 1
                if width and self.x < self.width:
                    self.rows[self.y][self.x] = ch
                    if width == 2 and self.x + 1 < self.width: self.rows[self.y][self.x + 1] = ""
                    self.x += width

    def capture(self):
        while select.select([self.master], [], [], .03)[0]:
            try: chunk = os.read(self.master, 65536)
            except OSError: break
            if not chunk: break
            self.feed(self.decoder.decode(chunk))
        return "\n".join("".join(row) for row in self.rows)

    def send(self, data):
        os.write(self.master, data)
        time.sleep(.18)
        self.capture()

    def close(self):
        try: os.kill(self.pid, signal.SIGTERM)
        except ProcessLookupError: pass
        os.waitpid(self.pid, 0)
        os.close(self.master)

with tempfile.TemporaryDirectory(prefix="forge-blame-") as temp:
    root = Path(temp, "repo")
    root.mkdir()
    (root / "folder").mkdir()
    file = root / "a.txt"
    file.write_text("base source\nsecond source\nthird source\n" +
                    "".join(f"source line {i} 中文\n" for i in range(4, 90)))
    markdown = root / "m.md"
    markdown.write_text("# " + "中文内容" * 30 + "\nMarkdownSecond\n")
    config = Path(temp, "config")
    env = {**os.environ, "XDG_CONFIG_HOME": str(config), "DISPLAY": "",
           "WAYLAND_DISPLAY": "", "FORGE_ICONS": "builtin"}

    def git(*args):
        return sp.run(["git", *args], cwd=root, env=env, check=True,
                      capture_output=True, text=True).stdout.strip()

    git("init", "-q", "-b", "main")
    git("config", "user.name", "Blame Author")
    git("config", "user.email", "blame@example.invalid")
    git("add", "a.txt", "m.md")
    git("commit", "-qm", "original subject", "-m", "Full message body for blame popup.")
    first = git("rev-parse", "HEAD")
    file.write_text(file.read_text().replace("second source", "second version"))
    markdown.write_text(markdown.read_text().replace("MarkdownSecond", "MarkdownChanged"))
    git("config", "user.name", "Second Author")
    git("add", "a.txt", "m.md")
    git("commit", "-qm", "second subject")
    second = git("rev-parse", "HEAD")
    file.write_text(file.read_text().replace("third source", "working change"))
    (root / "z.txt").write_text("untracked source\n")
    terminal = Terminal(root, env)

    def send(*keys):
        codes = {"Down": b"\x1b[B", "Home": b"\x1b[H", "End": b"\x1b[F",
                 "Escape": b"\x1b", "F3": b"\x1bOR", "F6": b"\x1b[17~"}
        for key in keys:
            terminal.send(codes.get(key, key.encode()))

    def screen():
        return terminal.capture()

    def wait(fragment, timeout=7):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            text = screen()
            if fragment in text:
                return text
            time.sleep(.08)
        raise AssertionError(f"Missing {fragment!r}:\n{screen()}")

    def click_source(fragment):
        rows = wait(fragment).splitlines()
        y = next(i for i, row in enumerate(rows) if fragment in row)
        x = rows[y].index(fragment)
        for code in (f"\x1b[<0;{x+1};{y+1}M", f"\x1b[<0;{x+1};{y+1}m"):
            terminal.send(code.encode())

    try:
        wait("folder")
        time.sleep(.5)
        assert "base source" not in screen(), "directory selection must start with empty preview"
        send("Down")
        wait("base source")
        wait("Blame Author")
        before = screen().splitlines()
        original_y = next(i for i, row in enumerate(before) if "base source" in row)
        send("F6")
        wait("Commit details")
        wait(first)
        wait("Full message body for blame popup.")
        send("d")
        wait("Commit diff")
        wait("+base source")
        send("h")
        wait("File history")
        wait("second subject")
        send("F6")
        wait("Full message body for blame popup.")
        send("c")
        text = screen()
        assert "copied" in text or "clipboard unavailable" in text, text
        send("Escape")
        wait("base source")
        assert "Commit details" not in screen()
        assert "base source" in screen().splitlines()[original_y], "popup moved preview"
        print("PASS: author/subject, full commit details, diff/history/copy, Esc position")

        click_source("working change")
        wait("Uncommitted changes")
        send("F6")
        wait("Commit details")
        wait("Uncommitted changes")
        send("Escape")
        click_source("second version")
        wait("Second Author")
        send("F6")
        wait(second)
        send("Escape")
        print("PASS: clicked line selects correct commit; working edits are uncommitted")

        file.write_text(file.read_text().replace("base source", "external edit"))
        wait("external edit")
        send("Home")
        wait("Uncommitted changes")
        print("PASS: external edit refreshes matching preview and blame")

        send("Escape", "End")
        wait("untracked source")
        wait("Uncommitted changes")
        print("PASS: untracked file attribution")

        send("Home", "Down", "Down")
        wait("MarkdownChanged")
        send("F6")
        assert "Commit details" not in screen(), "formatted Markdown must not guess source positions"
        send("F3")
        wait("1│#")
        click_source("MarkdownChanged")
        send("F6")
        wait(second)
        send("Escape")
        rows = screen().splitlines()
        y = next(i for i, row in enumerate(rows) if "1│#" in row)
        x = rows[y].index("#") + 2
        # Select a continuation of source line 1, not a numbered source row.
        for code in (f"\x1b[<0;{x+1};{y+2}M", f"\x1b[<0;{x+1};{y+2}m"):
            terminal.send(code.encode())
        send("F6")
        wait(first)
        send("Escape")
        print("PASS: Markdown Code CJK continuation and next source line resolve different commits")

        # Restart under disabled preferences to verify persistence and dispatch.
        config.joinpath("forge").mkdir(parents=True, exist_ok=True)
        config.joinpath("forge/config.toml").write_text(
            "[git.blame]\nenabled = true\nshow_inline = false\nshow_popup = false\n")
        terminal.close()
        terminal = Terminal(root, env)
        time.sleep(.6)
        send("Down")
        wait("external edit")
        send("F6")
        text = screen()
        assert "Commit details" not in text and "Uncommitted changes" not in text, text
        print("PASS: configured inline/popup switches")
    finally:
        terminal.close()

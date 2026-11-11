#!/usr/bin/env python3
"""Regression: Markdown CJK preview must stay inside the right pane at 129x47.

Runs the real `target/debug/forge` in a PTY through tmux, so terminal
wide-character wrapping is emulated exactly like a user's terminal. The pre-fix
bug emitted a double-width `。` in the preview's last column; the terminal then
wrapped it to column 0 of the next screen row, overwriting the left file tree.

Run after `cargo build -p forge`: python3 scripts/smoke-markdown-cjk.py
Uses only the Python standard library and a temporary Git repository.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unicodedata

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"
README = Path(__file__).resolve().parents[1] / "README.md"
COLUMNS, ROWS = 129, 47
# Layout at 129 columns: sidebar = (129 / 4).clamp(24, 40) = 32, then a divider.
SIDEBAR_COLUMNS = 32


def display_width(ch):
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in "WF" else 1


def left_columns(row, columns=SIDEBAR_COLUMNS):
    """Return the display cells that start inside the left file-tree region."""
    out = []
    used = 0
    for ch in row:
        if used >= columns:
            break
        out.append(ch)
        used += display_width(ch)
    return "".join(out)


def wide_chars(text):
    return [ch for ch in text if display_width(ch) == 2]


class Session:
    def __init__(self, directory):
        self.socket = f"forge-cjk-{os.getpid()}"
        self.target = "s"
        env = {**os.environ, "LC_ALL": "C.UTF-8", "LANG": "C.UTF-8"}
        self.proc = subprocess.Popen(
            [
                "tmux", "-u", "-L", self.socket, "new-session", "-d",
                "-s", self.target, "-x", str(COLUMNS), "-y", str(ROWS),
                f"cd {directory} && exec {BINARY} --force-truecolor",
            ],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if self._tmux("list-sessions", check=False).returncode == 0:
                return
            time.sleep(0.05)
        raise AssertionError("tmux session did not start")

    def _tmux(self, *args, check=True):
        result = subprocess.run(
            ["tmux", "-L", self.socket, *args],
            capture_output=True,
            text=True,
            check=False,
        )
        if check and result.returncode != 0:
            raise AssertionError(
                f"tmux {' '.join(args)} failed: {result.stderr or result.stdout}"
            )
        return result

    def send(self, *keys):
        self._tmux("send-keys", "-t", self.target, *keys)

    def capture(self):
        return self._tmux("capture-pane", "-p", "-t", self.target).stdout.splitlines()

    def wait_for(self, predicate, label, timeout=8):
        deadline = time.monotonic() + timeout
        screen = []
        while time.monotonic() < deadline:
            screen = self.capture()
            if predicate(screen):
                return screen
            time.sleep(0.1)
        raise AssertionError(label + "\n" + "\n".join(screen))

    def close(self):
        self._tmux("kill-server", check=False)
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def assert_sidebar_clean(screen, label):
    for index, row in enumerate(screen):
        left = left_columns(row)
        leaked = wide_chars(left)
        assert not leaked, (
            f"{label}: full-width glyph {leaked!r} leaked into the file tree at row {index}:\n"
            f"{left!r}\n" + "\n".join(screen)
        )


def main():
    assert BINARY.is_file(), f"missing {BINARY}; run `cargo build -p forge` first"
    assert shutil.which("tmux"), "tmux is required for the real-terminal regression"
    with tempfile.TemporaryDirectory(prefix="forge-cjk-smoke-") as directory:
        root = Path(directory)
        shutil.copyfile(README, root / "README.md")
        (root / "tests").mkdir()
        (root / "tests" / "placeholder.txt").write_text("placeholder\n")
        for command in (
            ["git", "init", "-q"],
            ["git", "add", "-A"],
            ["git", "-c", "user.name=Forge Test", "-c", "user.email=forge@example.test",
             "commit", "-qm", "baseline"],
        ):
            subprocess.run(command, cwd=directory, check=True)

        session = Session(directory)
        try:
            session.wait_for(
                lambda screen: any("README.md" in row for row in screen),
                "file tree never listed README.md",
            )

            # Directories sort first; move down until README.md is previewed.
            for _ in range(10):
                screen = session.capture()
                if any("Preview  README.md" in row for row in screen):
                    break
                session.send("j")
                time.sleep(0.3)
            else:
                raise AssertionError("could not select README.md\n" + "\n".join(screen))

            screen = session.wait_for(
                lambda rows: any("终端 Git +" in row for row in rows),
                "README markdown preview never rendered",
            )
            assert_sidebar_clean(screen, "top of README")
            first_paragraph = next(row for row in screen if "终端 Git +" in row)
            assert "执行写入" not in first_paragraph, (
                "the CJK paragraph was not wrapped before 执行写入。:\n" + first_paragraph
            )
            assert any("执行写入。" in row for row in screen), (
                "wrapped continuation 执行写入。 is missing from the preview\n"
                + "\n".join(screen)
            )

            # Scroll past 30 preview rows and keep checking every captured frame.
            session.send("Tab")
            time.sleep(0.3)
            for index in range(1, 41):
                session.send("j")
                if index % 5 == 0:
                    time.sleep(0.15)
                    assert_sidebar_clean(session.capture(), f"scroll row {index}")
            time.sleep(0.3)
            screen = session.capture()
            assert_sidebar_clean(screen, "scrolled past 30 rows")
            assert any(wide_chars(row) for row in screen), (
                "preview stopped rendering CJK while scrolling\n" + "\n".join(screen)
            )
            print(
                "PASS: README Markdown preview at 129x47 wraps CJK inside the preview pane; "
                "file tree stayed clean at top and after 40 scroll steps"
            )
        finally:
            session.close()


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise

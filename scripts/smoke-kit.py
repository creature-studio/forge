#!/usr/bin/env python3
"""Exercise the real kit runtime in a PTY and an isolated Git repository.

Run after `cargo build -p forge`: python3 scripts/smoke-kit.py
Uses only the Python standard library; never touches the user's repository.
"""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time


BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


def smoke():
    with tempfile.TemporaryDirectory(prefix="forge-kit-smoke-") as directory:
        root = Path(directory)

        def git(*args):
            return subprocess.check_output(["git", "-C", directory, *args], stderr=subprocess.STDOUT).decode().strip()

        git("init", "-q")
        git("config", "user.name", "Forge Test")
        git("config", "user.email", "forge@example.test")
        baseline = "".join(f"line {i}\n" for i in range(50))
        changed = baseline.replace("line 2\n", "changed first\n").replace("line 40\n", "changed second\n")
        (root / "a.txt").write_text(baseline)
        git("add", ".")
        git("commit", "-qm", "baseline")
        (root / "a.txt").write_text(changed)
        (root / "中文.txt").write_text("中文预览\n")
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(directory)
            os.execve(str(BINARY), [str(BINARY), "--force-truecolor"], {**os.environ, "TERM": "xterm-256color"})

        transcript = bytearray()

        def drain(duration=0.3):
            stop = time.monotonic() + duration
            data = bytearray()
            while time.monotonic() < stop:
                if select.select([master], [], [], max(0, stop-time.monotonic()))[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    data.extend(chunk)
            transcript.extend(data)
            assert b"panicked at" not in transcript, transcript.decode(errors="replace")
            return data

        def send(keys):
            os.write(master, keys)
            return drain(0.5)

        def wait_for(predicate, label):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                if predicate():
                    return
                drain(0.1)
            raise AssertionError(label + "\n" + transcript.decode(errors="replace")[-5000:])

        def resize(columns, rows):
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
            os.kill(pid, signal.SIGWINCH)
            drain()

        try:
            resize(120, 30)
            drain(1)
            assert b"Files" in transcript
            send(b"g")
            assert b"Git" in transcript
            send(b"s")
            wait_for(lambda: "a.txt" in git("diff", "--cached", "--name-only"), "stage failed")
            assert git("show", ":a.txt") == changed.strip(), "sidebar staged only a hunk"
            send(b"]u")
            wait_for(lambda: not git("diff", "--cached", "--name-only"), "unstage failed")

            # Help owns input; closing it restores the preview focus.
            send(b"[\t")
            send(b"?")
            assert b"Git help" in transcript
            send(b"scp")
            assert not git("diff", "--cached", "--name-only")
            send(b"q")
            send(b"s")
            wait_for(lambda: "a.txt" in git("diff", "--cached", "--name-only"), "hunk stage failed")
            index = git("show", ":a.txt")
            assert "changed first" in index and "changed second" not in index
            send(b"\x1b")
            send(b"]\tu")
            wait_for(lambda: not git("diff", "--cached", "--name-only"), "hunk unstage failed")
            send(b"\x1b")
            send(b"[s")
            wait_for(lambda: "a.txt" in git("diff", "--cached", "--name-only"), "restage failed")

            # q/g/s/r are draft characters while the modal is open.
            send(b"c")
            send(b"qgsr draft")
            assert git("log", "-1", "--format=%s") == "baseline"
            send(b"\x1b")
            send(b"c")
            send(b"kit smoke ? commit\r")
            wait_for(lambda: git("log", "-1", "--format=%s") == "kit smoke ? commit", "commit failed, help intercepted draft, or cancelled draft leaked")

            (root / "a.txt").write_text("rejected commit\n")
            send(b"r")
            send(b"[s")
            wait_for(lambda: "a.txt" in git("diff", "--cached", "--name-only"), "stage before failed commit")
            hook = root / ".git/hooks/pre-commit"
            hook.write_text("#!/bin/sh\necho fixture-rejection >&2\nexit 1\n")
            hook.chmod(0o755)
            send(b"cblocked commit\r")
            wait_for(lambda: b"failed:" in transcript, "Git failure was not displayed")
            assert git("log", "-1", "--format=%s") == "kit smoke ? commit"
            assert "a.txt" in git("diff", "--cached", "--name-only")
            hook.unlink()

            # Empty commit is kept editable, and can be cancelled after an error.
            send(b"c\r")
            assert b"empty commit message" in transcript
            send(b"\x1b")

            # Collapsed folders expose scope in the footer and include nested files.
            (root / "folder/nested").mkdir(parents=True)
            (root / "folder/a.txt").write_text("folder change\n")
            (root / "folder/nested/b.txt").write_text("nested change\n")
            (root / "folder-sibling").mkdir()
            (root / "folder-sibling/keep.txt").write_text("leave unstaged\n")
            send(b"r")
            send(b"\x1b[Hj")  # Home selects Changes; j selects its first folder.
            menu = send(b"\x1b[<2;5;4M\x1b[<2;5;4m")
            assert b"Stage folder" in menu, "right-click did not open folder actions"
            send(b"\r")
            wait_for(lambda: "folder/nested/b.txt" in git("diff", "--cached", "--name-only"), "folder stage failed")
            staged = git("diff", "--cached", "--name-only").splitlines()
            assert "folder/a.txt" in staged and "folder-sibling/keep.txt" not in staged
            assert b"Stage folder" in transcript
            send(b"]k")
            send(b"u")
            wait_for(lambda: "folder/nested/b.txt" not in git("diff", "--cached", "--name-only"), "folder unstage failed")
            assert (root / "folder/nested/b.txt").read_text() == "nested change\n"

            # History accordion: G focuses the newest commit, Enter expands it, Esc leaves.
            mark = len(transcript)
            send(b"G")
            # The status bar redraws incrementally, so match a stable fragment.
            wait_for(
                lambda: b"changed files" in transcript[mark:],
                "G did not focus the newest commit in the sidebar",
            )
            send(b"\r")
            wait_for(
                lambda: b"collapses files" in transcript[mark:],
                "commit did not expand inside the sidebar",
            )
            send(b"\x1b")
            drain(0.5)
            mark = len(transcript)
            send(b"?")
            # The overlay draws its title in pieces; the footer proves help opened.
            wait_for(lambda: b"Close help" in transcript[mark:], "history kept keyboard focus after Esc")
            send(b"\x1b")

            resize(45, 12)
            send(b"\t")
            resize(120, 30)
            # SGR mouse: select the sidebar, scroll, open a context menu, dismiss.
            send(b"\x1b[<0;5;5M\x1b[<0;5;5m")
            send(b"\x1b[<65;5;5M")
            send(b"\x1b[<2;5;5M\x1b[<2;5;5m")
            send(b"\x1b")
            send(b"r")
            drain(1)
            assert not drain(0.3), "runtime keeps producing output while idle"
            send(b"?")
            send(b"\x03")  # Ctrl-C closes help without quitting
            assert os.waitpid(pid, os.WNOHANG)[0] == 0
            send(b"q")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                result, status = os.waitpid(pid, os.WNOHANG)
                if result:
                    assert os.waitstatus_to_exitcode(status) == 0
                    assert termios.tcgetattr(master)[3] & termios.ICANON
                    assert b"\x1b[?1000l" in transcript
                    print("PASS: file/folder/hunk stage/unstage, help isolation, commit/cancel/failure/draft input, resize, mouse, idle, terminal restoration")
                    return
                drain(0.1)
            raise AssertionError("quit did not exit")
        finally:
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except ProcessLookupError:
                pass
            os.close(master)


if __name__ == "__main__":
    smoke()

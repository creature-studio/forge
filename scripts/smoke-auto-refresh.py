#!/usr/bin/env python3
"""Exercise external file creation and preview refresh in a real Forge PTY."""
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

with tempfile.TemporaryDirectory(prefix="forge-watch-") as temp:
    root = Path(temp)
    (root / "a.txt").write_text("before-change\n")
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.name", "Test"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.email", "test@example.test"], cwd=root, check=True)
    subprocess.run(["git", "add", "."], cwd=root, check=True)
    subprocess.run(["git", "commit", "-qm", "baseline"], cwd=root, check=True)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(root)
        os.execve(str(BINARY), [str(BINARY)], {**os.environ, "TERM": "xterm-256color",
                                                    "DISPLAY": "", "WAYLAND_DISPLAY": ""})
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
    transcript = bytearray()
    def drain(seconds=.2):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([fd], [], [], max(0, end-time.monotonic()))[0]:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                transcript.extend(data)
        assert b"panicked at" not in transcript

    def until(text, offset, label):
        end = time.monotonic() + 8
        while time.monotonic() < end:
            drain(.1)
            if text in transcript[offset:]:
                return
        raise AssertionError(label + ": " + repr(transcript[-1500:]))

    try:
        drain(1)
        assert b"a.txt" in transcript, "initial file missing"
        cut = len(transcript)
        (root / "external-created.txt").write_text("created\n")
        until(b"external-created.txt", cut, "external create not refreshed")
        cut = len(transcript)
        (root / "a.txt").write_text("external-change-visible\n")
        until(b"external-change-visible", cut, "external content not refreshed")
        # A Ctrl-C outside the editor must not kill the TUI.
        os.write(fd, b"\x03")
        drain(.3)
        assert os.waitpid(pid, os.WNOHANG)[0] == 0
        os.write(fd, b"q")
        drain(.5)
        done, status = os.waitpid(pid, 0)
        assert done == pid and os.waitstatus_to_exitcode(status) == 0
        pid = 0
        print("PASS: external file added, external preview changed, Ctrl-C safe, q exits")
    finally:
        if pid:
            try:
                os.kill(pid, signal.SIGTERM)
                os.waitpid(pid, 0)
            except ProcessLookupError:
                pass
        os.close(fd)

#!/usr/bin/env python3
"""Search-only PTY smoke in a disposable workspace. Run after cargo build -p forge."""
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import tempfile
import termios
import time
import unicodedata

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


def render_screen(transcript):
    """Reconstruct cursor updates; raw ANSI diffs can split words across frames."""
    rows = [[" "] * 130 for _ in range(32)]
    x = y = 0
    tokens = re.split(r"(\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*(?:\x07|\x1b\\)|\x1b.)",
                      transcript.decode(errors="replace"))
    for token in tokens:
        if token.startswith("\x1b["):
            final = token[-1]
            nums = [int(n or 0) for n in token[2:-1].lstrip("?<=>").split(";")]
            count = nums[0] or 1
            if final in "Hf":
                y = min(31, max(0, count - 1))
                x = min(129, max(0, (nums[1] if len(nums) > 1 else 1) - 1))
            elif final == "G": x = min(129, count - 1)
            elif final == "d": y = min(31, count - 1)
            elif final == "A": y = max(0, y - count)
            elif final == "B": y = min(31, y + count)
            elif final == "C": x = min(129, x + count)
            elif final == "D": x = max(0, x - count)
            elif final == "J" and nums[0] in (2, 3):
                rows = [[" "] * 130 for _ in range(32)]
            elif final == "K":
                lo, hi = (0, x + 1) if nums[0] == 1 else (0, 130) if nums[0] == 2 else (x, 130)
                rows[y][lo:hi] = [" "] * (hi - lo)
        elif not token.startswith("\x1b"):
            for char in token:
                if char == "\r": x = 0
                elif char == "\n": y = min(31, y + 1)
                elif not unicodedata.category(char).startswith("C") and x < 130:
                    rows[y][x] = char
                    x += 2 if unicodedata.east_asian_width(char) in "WF" else 1
    return "\n".join("".join(row) for row in rows).encode()


def smoke():
    with tempfile.TemporaryDirectory(prefix="forge-search-") as directory:
        root = Path(directory)
        (root / "src").mkdir()
        (root / "src/deep.rs").write_text("line one\nneedle42\nline three\n")
        for folder in ["node_modules", "target", "dist", "build", ".next", "coverage", ".git"]:
            (root / folder).mkdir()
            (root / folder / "excluded.rs").write_text("needle42\n")
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(directory)
            os.execve(str(BINARY), [str(BINARY), "--force-ansi16"],
                      {**os.environ, "TERM": "xterm-256color", "XDG_CONFIG_HOME": str(root / "config")})

        transcript = bytearray()

        def drain(seconds=0.3):
            data = bytearray()
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    data.extend(chunk)
            transcript.extend(data)
            assert b"panicked at" not in transcript
            return render_screen(transcript)

        def send(keys, seconds=0.4):
            os.write(master, keys)
            return drain(seconds)

        def expect(data, text):
            assert text in data, repr(text) + " absent:\n" + data.decode(errors="replace")[-6000:]

        try:
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 130, 0, 0))
            os.kill(pid, signal.SIGWINCH)
            drain(1)
            expect(send(b"\x10deeprs"), b"deep.rs")
            expect(send(b"x"), b"No results")
            expect(send(b"\x7f"), b"deep.rs")
            send(b"\x1b")
            results = send(b"\x06needle")
            expect(results, b"deep.rs:2")
            expect(results, b"1 results")
            expect(results, b"line one")
            expect(results, b"line three")
            expect(results, b"1 matches")
            assert b"excluded.rs" not in results
            send(b"\r")
            # Reopening search now restores its query and regex mode.
            expect(send(b"\x06"), b"> needle")
            expect(send(b"\x7f" * 32 + b"needle\\d+\x12", 1.4), b"1 results")
            send(b"\x1b")
            expect(send(b"\x06"), b"Search contents")
            expect(send(b"\x7f" * 32 + b"[", 1.2), b"Failed:")
            expect(send(b"\x12"), b"No results")
            send(b"\x1b")
            # Cancel during the debounce and immediately open the other mode.
            send(b"\x06" + b"\x7f" * 32 + b"needle", 0.02)
            send(b"\x1b", 0.05)
            expect(send(b"\x10" + b"\x7f" * 32 + b"deep"), b"deep.rs")
            send(b"\x1b")
            (root / "freshly-created.rs").write_text("fresh\n")
            drain(1)
            expect(send(b"\x10" + b"\x7f" * 32 + b"freshly"), b"freshly-created.rs")
            send(b"\x1b")
            send(b"\x03")
            print("PASS: filename cache, no results, live content context/line numbers, regex/errors, Esc/races, watcher refresh")
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
            os.close(master)


if __name__ == "__main__":
    smoke()

#!/usr/bin/env python3
"""Real PTY regression: clipboard, negotiated paste, and atomic paste undo."""
import argparse
import os
from pathlib import Path
import pty
import select
import signal
import tempfile
import time
import fcntl
import termios
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/forge')
BINARY = parser.parse_args().binary.resolve()
with tempfile.TemporaryDirectory(prefix='forge-editor-') as directory:
    path = Path(directory) / 'a.txt'
    path.write_text('alpha\nbeta\n')
    pid, master = pty.fork()
    if pid == 0:
        os.chdir(directory)
        os.execve(str(BINARY), [str(BINARY)], {**os.environ, 'TERM': 'xterm-256color', 'DISPLAY': '', 'WAYLAND_DISPLAY': ''})
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
    transcript = bytearray()
    def drain(seconds=.3):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([master], [], [], max(0, end-time.monotonic()))[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                transcript.extend(data)
        assert b'panicked at' not in transcript
    def send(data):
        pending = memoryview(data)
        while pending:
            written = os.write(master, pending)
            pending = pending[written:]
        drain()
        assert os.waitpid(pid, os.WNOHANG)[0] == 0, 'Forge exited during editing'
    def wait_file(expected):
        end = time.monotonic() + 5
        while time.monotonic() < end:
            if path.read_text() == expected:
                return
            drain(.1)
        raise AssertionError((path.read_text(), expected, transcript[-3000:]))
    try:
        drain(1)
        assert b'\x1b[?2004h' in transcript, 'Forge did not enable bracketed paste; terminal paste will arrive as individual keys'
        send(b'\x05')
        drain(.5)
        assert b'Copy selection / line' in transcript, 'editor did not load'
        # Editor input deliberately avoids a redundant reactive repaint. Modal
        # transitions must still appear on the event loop's immediate frame.
        send(b'\x1bOS')  # F4.
        assert b'Rebind' in transcript, 'shortcut modal did not open'
        send(b'\x1b')
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
        drain(.2)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        drain(.2)
        send(b'\x03')  # empty selection: copy is not quit
        send(b'\x18')  # cut entire first line
        send(b'\x13')
        wait_file('beta\n')
        send(b'\x16')  # paste cut line back at line start
        send(b'\x13')
        wait_file('alpha\nbeta\n')
        send(b'\x01')  # select all
        send(b'\x03')  # copy, not runtime auto-exit
        send(b'\x1b[3~')  # delete selection
        send(b'\x16')
        send(b'\x13')
        wait_file('alpha\nbeta\n')
        send(b'\x01')
        send(b'\x1b[200~replacement\ntext\x1b[201~')
        send(b'\x13')
        wait_file('replacement\ntext')
        # A large Unicode paste, including CRLF and tabs, must replace the
        # selection as one edit even when the PTY splits its input reads.
        replacement = ('整段粘贴🙂\tcode\r\n' * 1000) + '末尾'
        send(b'\x01')
        send(b'\x1b[200~' + replacement.encode() + b'\x1b[201~')
        send(b'\x13')
        wait_file(replacement.replace('\r\n', '\n'))
        send(b'\x1a')  # Ctrl-Z undoes the entire paste and restores the selection.
        send(b'\x13')
        wait_file('replacement\ntext')
        send(b'\x19')  # Ctrl-Y redoes the entire paste.
        send(b'\x13')
        wait_file(replacement.replace('\r\n', '\n'))
        send(b'\x1b')
        os.write(master, b'q')
        drain(.5)
        result = os.waitpid(pid, os.WNOHANG)
        assert result[0] == pid and os.waitstatus_to_exitcode(result[1]) == 0, result
        pid = 0
        assert transcript.rfind(b'\x1b[?2004l') > transcript.rfind(b'\x1b[?2004h'), 'bracketed paste was not disabled on exit'
        print('PASS: PTY clipboard, paste negotiation, large Unicode paste, atomic undo/redo, save and terminal restoration')
    finally:
        if pid:
            try:
                os.kill(pid, signal.SIGTERM)
                os.waitpid(pid, 0)
            except ProcessLookupError:
                pass
        os.close(master)

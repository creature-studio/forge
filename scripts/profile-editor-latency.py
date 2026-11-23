#!/usr/bin/env python3
"""Measure key-to-terminal-output latency through Forge's real PTY event loop."""
import argparse
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import statistics
import struct
import tempfile
import termios
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/forge')
parser.add_argument('--samples', type=int, default=30)
parser.add_argument('--burst', type=int, default=120)
parser.add_argument('--rate', type=float, default=120)
args = parser.parse_args()
if args.samples < 1 or args.burst < 1 or args.rate <= 0:
    parser.error('samples, burst and rate must be positive')
binary = args.binary.resolve()


def measure(extension, content):
    with tempfile.TemporaryDirectory(prefix='forge-key-latency-') as directory:
        path = Path(directory) / f'a.{extension}'
        path.write_text(content)
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(directory)
            os.execve(str(binary), [str(binary)], {
                **os.environ, 'TERM': 'xterm-256color',
                'DISPLAY': '', 'WAYLAND_DISPLAY': '',
            })
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 48, 160, 0, 0))

        def drain(duration):
            end = time.monotonic() + duration
            data = bytearray()
            while time.monotonic() < end:
                if select.select([master], [], [], max(0, end - time.monotonic()))[0]:
                    data.extend(os.read(master, 65536))
            assert b'panicked at' not in data, data[-2000:]
            return data

        def wait_for(marker):
            end = time.monotonic() + 5
            data = bytearray()
            while marker not in data:
                assert time.monotonic() < end, (marker, data[-2000:])
                if select.select([master], [], [], .1)[0]:
                    data.extend(os.read(master, 65536))
            assert b'panicked at' not in data, data[-2000:]
            return data

        try:
            drain(.8)
            os.write(master, b'\x05')  # Ctrl+E.
            wait_for(b'Copy selection / line')
            drain(.1)
            typed = []
            for position, movement in [('start', b''), ('end', b'\x1b[1;5F')]:
                if movement:
                    os.write(master, movement)
                    drain(.2)
                samples = []
                output_bytes = 0
                characters = []
                for index in range(args.samples):
                    # A fresh marker prevents a delayed syntax repaint of an
                    # earlier character from being mistaken for this key.
                    character = chr(0x4E00 + len(typed) * args.samples + index)
                    marker = character.encode()
                    start = time.perf_counter()
                    os.write(master, marker)
                    data = wait_for(marker)
                    samples.append((time.perf_counter() - start) * 1000)
                    characters.append(character)
                    output_bytes += len(data)
                    drain(.002)
                ordered = sorted(samples)
                print(f'{extension:>3} {position}: median={statistics.median(samples):.2f} ms '
                      f'p95={ordered[min(len(ordered)-1, int(len(ordered)*.95))]:.2f} ms '
                      f'max={max(samples):.2f} ms output={output_bytes / len(samples):.0f} bytes/key', flush=True)
                typed.append(''.join(characters))
            start = time.perf_counter()
            for index in range(args.burst):
                os.write(master, b'Y')
                drain(max(0, start + (index + 1) / args.rate - time.perf_counter()))
            last_key = time.perf_counter()
            os.write(master, b'\x13')  # Save to check every measured key was handled.
            expected = typed[0] + content + typed[1] + 'Y' * args.burst
            deadline = time.monotonic() + 5
            while path.read_text() != expected:
                assert time.monotonic() < deadline, 'typing dropped, reordered or accumulated input'
                drain(.002)
            print(f'{extension:>3} burst: {args.burst} keys at {args.rate:g}/s, '
                  f'queue + save={(time.perf_counter() - last_key) * 1000:.2f} ms; all input preserved', flush=True)
        finally:
            os.kill(pid, signal.SIGTERM)
            os.waitpid(pid, 0)
            os.close(master)


measure('txt', 'alpha\nbeta\n' * 5000)
measure('ts', 'const item = { value: 123, label: "sample" };\n' * 5000)
measure('rs', 'let item = Some("sample"); // comment\n' * 5000)

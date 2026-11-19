#!/usr/bin/env python3
"""Repeatable local Forge release-startup/RSS sample, not a universal benchmark.

Requires Linux, tmux, Git and an existing target/release/forge.
Run: python3 scripts/benchmark-homepage.py
"""
from __future__ import annotations
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/forge'
RUNS = 3


def cmd(*args: str, cwd: Path | None = None, env: dict | None = None) -> str:
    result = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, check=True)
    return result.stdout


def bench(root: Path, index: int) -> tuple[float, float]:
    socket = f'forge-home-benchmark-{os.getpid()}-{index}'
    env = {**os.environ, 'XDG_CONFIG_HOME': str(root / 'configuration')}
    tmux = ('tmux', '-L', socket)
    start = time.perf_counter()
    cmd(*tmux, '-f', '/dev/null', 'new-session', '-d', '-s', 'app', '-x', '110', '-y', '32',
        '-c', str(root), str(BINARY), env=env)
    try:
        elapsed = None
        for _ in range(150):
            frame = cmd(*tmux, 'capture-pane', '-pt', 'app', env=env)
            if 'README.md' in frame and 'listing ' not in frame:
                elapsed = time.perf_counter() - start
                break
            time.sleep(.025)
        if elapsed is None:
            raise RuntimeError('Forge root listing did not appear within the capture window')
        time.sleep(.5)
        pid = int(cmd(*tmux, 'display-message', '-p', '-t', 'app', '#{pane_pid}', env=env).strip())
        status = Path(f'/proc/{pid}/status').read_text().splitlines()
        rss_kib = int(next(line.split()[1] for line in status if line.startswith('VmRSS:')))
        return elapsed, rss_kib / 1024
    finally:
        subprocess.run([*tmux, 'kill-server'], env=env, capture_output=True)


def main() -> None:
    if not BINARY.is_file():
        raise SystemExit('Release binary missing. Build first: cargo build --release -p forge')
    with tempfile.TemporaryDirectory(prefix='forge-home-benchmark-') as temp:
        root = Path(temp)
        source = root / 'src'
        source.mkdir()
        (root / 'README.md').write_text('# Forge benchmark\n')
        for i in range(1200):
            (source / f'component_{i:04}.ts').write_text('export const value = 1;\n')
        cmd('git', 'init', '-q', '-b', 'main', cwd=root)
        cmd('git', 'add', '.', cwd=root)
        cmd('git', '-c', 'user.name=Benchmark', '-c', 'user.email=benchmark@example.invalid',
            'commit', '-qm', 'initial', cwd=root)
        samples = [bench(root, n) for n in range(RUNS)]
        print('Linux x86_64 · 1,200 tracked files in unopened src/ · warm filesystem cache')
        for n, (startup, rss) in enumerate(samples, 1):
            print(f'Run {n}: root visible {startup*1000:.0f} ms · idle RSS {rss:.1f} MiB')
        print(f'Median: {statistics.median(x for x, _ in samples)*1000:.0f} ms · '
              f'{statistics.median(y for _, y in samples):.1f} MiB')
        print(f'Release binary: {BINARY.stat().st_size / 1e6:.1f} MB')
        print('Single-machine observation only; values vary across hosts and workloads.')


if __name__ == '__main__':
    main()

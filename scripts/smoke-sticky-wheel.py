#!/usr/bin/env python3
"""End-to-end sticky tree wheel behavior on Files and Git, with Settings toggle."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/forge'

with tempfile.TemporaryDirectory(prefix='forge-sticky-wheel-') as folder:
    root = Path(folder, 'repo')
    root.mkdir()
    cfg = Path(folder, 'cfg')
    # Sticky Scroll alone should pin each ancestor independently; Compact
    # Folders has its own coverage and is disabled for this narrow test.
    ui_config = cfg / "forge" / "config.toml"
    ui_config.parent.mkdir(parents=True, exist_ok=True)
    ui_config.write_text("compact_folders = false\n")

    sock = f'forge-sticky-wheel-{os.getpid()}'
    def git(*args):
        sp.run(['git', *args], cwd=root, check=True, capture_output=True)
    git('init', '-q', '-b', 'main')
    git('config', 'user.name', 'Sticky Wheel Test')
    git('config', 'user.email', 'sticky@example.invalid')
    for n in range(45):
        path = root / 'src' / 'nested' / f'file_{n:03d}.py'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'original {n}\n')
    git('add', '.')
    git('commit', '-qm', 'Base')
    for n in range(45):
        (root / 'src' / 'nested' / f'file_{n:03d}.py').write_text(f'changed {n}\n')
    def tm(*args, check=True):
        result = sp.run(['tmux', '-L', sock, *args], text=True, capture_output=True,
            env={**os.environ, 'XDG_CONFIG_HOME': str(cfg), 'FORGE_ICONS': 'builtin'})
        if check and result.returncode:
            raise AssertionError(f'tmux {args}: {result.stderr}')
        return result.stdout
    def send(*keys):
        tm('send-keys', '-t', 'ui', *keys)
        time.sleep(.18)
    def wheel(n=1):
        for _ in range(n):
            tm('send-keys', '-l', '-t', 'ui', '\x1b[<65;6;8M')
            time.sleep(.12)
    def left():
        time.sleep(.25)
        return [line.split('│')[0] for line in tm('capture-pane', '-pt', 'ui').splitlines()[2:11]]
    def toggle():
        send(':')
        for char in 'sticky':
            send(char)
        assert 'Tree Sticky Scroll' in tm('capture-pane', '-pt', 'ui')
        send('Enter')
    try:
        tm('-f', '/dev/null', 'new-session', '-d', '-s', 'ui', '-x', '100', '-y', '19',
            '-c', str(root), str(BINARY))
        time.sleep(1.2)
        send('Enter')
        send('j')
        send('Enter')
        send('Home')  # Keep selection on src while wheel scrolls its children.
        wheel(7)
        a = left()
        assert 'src' in a[0] and 'nested' in a[1], a
        assert 'file_019' in '\n'.join(a), a
        assert '›' in a[0], 'wheel should preserve selected src row'
        toggle()
        off = left()
        assert 'src' not in off[0] and 'nested' not in off[0], off
        toggle()
        on_again = left()
        assert 'src' in on_again[0] and 'nested' in on_again[1], on_again
        print('PASS: Files wheel keeps selection while sticky headings pin; Settings Off/On restores legacy view')

        send('F2')
        time.sleep(.6)
        wheel(7)
        git_top = '\n'.join(left()[:5])
        assert 'CHANGES' in git_top and 'src' in git_top and 'nested' in git_top, git_top
        send('Home')
        after_home = '\n'.join(left()[:3])
        assert 'CHANGES' in after_home and 'file_019' not in after_home
        print('PASS: Git wheel pins ancestor hierarchy; Home restores keyboard-follow selection')
    finally:
        sp.run(['tmux', '-L', sock, 'kill-server'], capture_output=True)

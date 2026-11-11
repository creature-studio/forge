#!/usr/bin/env python3
"""Tree sticky headers: view-only, togglable, persisted and context-safe."""
import os
from pathlib import Path
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/forge'

with tempfile.TemporaryDirectory(prefix='forge-sticky-') as d:
    root = Path(d, 'repo')
    root.mkdir()
    config = Path(d, 'cfg')
    # Sticky Scroll alone should pin each ancestor independently; Compact
    # Folders has its own coverage and is disabled for this narrow test.
    ui_config = config / "forge" / "config.toml"
    ui_config.parent.mkdir(parents=True, exist_ok=True)
    ui_config.write_text("compact_folders = false\n")

    sock = f'forge-sticky-{os.getpid()}'
    def git(*args):
        sp.run(['git', *args], cwd=root, check=True, capture_output=True)
    git('init', '-q', '-b', 'main')
    git('config', 'user.name', 'Sticky Test')
    git('config', 'user.email', 'sticky@example.invalid')
    for n in range(44):
        p = root / 'src' / 'nested' / f'file_{n:03}.txt'
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(f'first {n}\n')
    git('add', '.')
    git('commit', '-m', 'Base')
    for n in range(44):
        (root / 'src' / 'nested' / f'file_{n:03}.txt').write_text(f'updated {n}\n')
    git('add', '.')
    def tm(*args, check=True):
        r = sp.run(['tmux', '-L', sock, *args], text=True, capture_output=True,
                   env={**os.environ, 'XDG_CONFIG_HOME': str(config)})
        if check and r.returncode:
            raise AssertionError(f'tmux {args}: {r.stderr}')
        return r.stdout
    def start(name):
        tm('-f', '/dev/null', 'new-session', '-d', '-s', name, '-x', '108', '-y', '24',
           '-c', str(root), str(BINARY))
        time.sleep(1.25)
    def send(name, *keys):
        tm('send-keys', '-t', name, *keys)
        time.sleep(.17)
    def screen(name):
        time.sleep(.22)
        return tm('capture-pane', '-pt', name)
    def left_lines(name, lines=7):
        # Only the tree, never the file path in the right preview.
        return [line.split('│')[0] for line in screen(name).splitlines()[2:2+lines]]
    def scroll_files(name):
        send(name, 'Enter')  # src
        send(name, 'j')
        send(name, 'Enter')  # nested
        for _ in range(27):
            send(name, 'j')
    def toggle(name):
        send(name, ':')
        for c in 'sticky':
            send(name, c)
        page = screen(name)
        assert 'Tree Sticky Scroll' in page, page
        send(name, 'Enter')

    try:
        start('one')
        scroll_files('one')
        left_on = left_lines('one')
        assert 'src' in left_on[0] and 'nested' in left_on[1], left_on
        toggle('one')
        left_off = left_lines('one')
        assert 'src' not in left_off[0] and 'nested' not in left_off[0], left_off
        cfg_file = config / 'forge' / 'config.toml'
        assert 'tree_sticky_scroll = false' in cfg_file.read_text()
        send('one', 'q')
        assert sp.run(['tmux', '-L', sock, 'has-session', '-t', 'one'], capture_output=True).returncode != 0
        start('two')
        scroll_files('two')
        assert 'src' not in left_lines('two')[0], left_lines('two')
        toggle('two')
        assert 'tree_sticky_scroll = true' in cfg_file.read_text()
        assert 'src' in left_lines('two')[0] and 'nested' in left_lines('two')[1]
        # With many staged changes, the Git pane also pins the correct group
        # and ancestors instead of scrolling them out of context.
        send('two', 'F2')
        time.sleep(.55)
        for _ in range(27):
            send('two', 'j')
        git_top = '\n'.join(left_lines('two', 5))
        assert 'STAGED' in git_top and 'src' in git_top and 'nested' in git_top, git_top
        send('two', 'q')
        print('PASS: Files and Git sticky ancestors; toggle off/on; persisted across restart')
    finally:
        sp.run(['tmux', '-L', sock, 'kill-server'], capture_output=True)

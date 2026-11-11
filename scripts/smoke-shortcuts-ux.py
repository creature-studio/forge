#!/usr/bin/env python3
"""Verify command alias, context-grouped F4 screen, and Ctrl-C in the commit modal."""
from pathlib import Path
import os
import subprocess
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/forge'

with tempfile.TemporaryDirectory(prefix='forge-shortcuts-ux-') as folder:
    root = Path(folder) / 'repo'
    root.mkdir()
    config = Path(folder) / 'config'
    def git(*args):
        subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)
    git('init', '-q', '-b', 'main')
    git('config', 'user.name', 'Forge UX Test')
    git('config', 'user.email', 'ux@example.invalid')
    (root / 'a.txt').write_text('original\n')
    git('add', '.')
    git('commit', '-m', 'Initial')
    (root / 'a.txt').write_text('changed\n')
    git('add', 'a.txt')

    sock = f'forge-keyux-{os.getpid()}'
    def tm(*args):
        r = subprocess.run(['tmux', '-L', sock, *args], text=True, capture_output=True,
                           env={**os.environ, 'XDG_CONFIG_HOME': str(config)})
        if r.returncode:
            raise RuntimeError(f'tmux {args}: {r.stderr}')
        return r.stdout
    def press(*keys, target='ux'):
        tm('send-keys', '-t', target, *keys)
        time.sleep(.32)
    def screen(target='ux'):
        time.sleep(.25)
        return tm('capture-pane', '-pt', target)
    try:
        tm('-f', '/dev/null', 'new-session', '-d', '-s', 'ux', '-x', '120', '-y', '34',
           '-c', str(root), str(BINARY))
        time.sleep(1)
        press('C-k')
        assert '┌ Commands' in screen(), 'Ctrl-K did not open command palette'
        press('Escape')
        press('F4')
        assert 'Keyboard shortcuts' in screen()
        assert 'Navigation' in screen()
        press('Tab')
        assert 'Edit file' in screen(), 'Files group missing'
        press('Tab')
        assert 'Stage selection / hunk' in screen(), 'Git group missing'
        press('Tab')
        assert 'Copy selection / line' in screen(), 'Editor group missing'
        press('Tab')
        page = screen()
        assert 'Quit workspace' in page and 'fixed' in page, 'fixed navigation not visible'
        press('Escape')
        press('F2')
        press('c')
        assert 'Commit' in screen(), 'commit dialog did not open'
        press('C-c')
        assert 'Commit' in screen(), 'Ctrl-C unexpectedly dismissed commit or exited'
        press('Escape')
        press('q')
        exited = subprocess.run(['tmux', '-L', sock, 'has-session', '-t', 'ux'],
                                capture_output=True)
        assert exited.returncode != 0, 'q did not exit the workspace'

        # Older configurations could already use Ctrl-K. Their custom binding
        # must win over the newly introduced command-palette convenience alias.
        cfg = config / 'forge' / 'keybindings.conf'
        cfg.parent.mkdir(parents=True, exist_ok=True)
        cfg.write_text('stage=Ctrl+k\n')
        tm('new-session', '-d', '-s', 'ux2', '-x', '120', '-y', '34',
           '-c', str(root), str(BINARY))
        time.sleep(.8)
        press('C-k', target='ux2')
        assert '┌ Commands' not in screen('ux2'), 'Ctrl-K stole a legacy custom binding'
        press('F4', target='ux2')
        page = screen('ux2')
        assert 'assigned elsewhere' in page, 'legacy Ctrl-K shadowing not explained'
        assert 'Bindings not loaded' not in page
        press('Escape', target='ux2')
        press('q', target='ux2')
        print('PASS: Ctrl-K palette, grouped F4, fixed navigation, Ctrl-C safe, legacy Ctrl-K migration')
    finally:
        subprocess.run(['tmux', '-L', sock, 'kill-server'], capture_output=True)

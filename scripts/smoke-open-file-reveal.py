#!/usr/bin/env python3
"""Git Open file unfolds Files tree, scrolls to the selected file, keeps tree on narrow terminals."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/forge'

with tempfile.TemporaryDirectory(prefix='forge-open-reveal-') as folder:
    root = Path(folder, 'repo')
    root.mkdir()
    cfg = Path(folder, 'config')
    sock = f'forge-open-reveal-{os.getpid()}'
    def git(*args):
        result = sp.run(['git', *args], cwd=root, check=True, capture_output=True, text=True)
        return result.stdout
    git('init', '-q', '-b', 'main')
    git('config', 'user.name', 'Forge Reveal UX')
    git('config', 'user.email', 'reveal@example.invalid')
    for index in range(55):
        (root/f'a{index:02d}.txt').write_text('unchanged\n')
    target = root/'z_source'/'deep'/'leaf'/'target.py'
    target.parent.mkdir(parents=True)
    target.write_text('before\n')
    git('add', '.')
    git('commit', '-qm', 'initial')
    target.write_text('after change\n')
    def tm(*args):
        result = sp.run(['tmux','-L',sock,*args],text=True,capture_output=True,
             env={**os.environ,'XDG_CONFIG_HOME': str(cfg), 'FORGE_ICONS': 'builtin'})
        if result.returncode:
            raise AssertionError(f'tmux {args}: {result.stderr}')
        return result.stdout
    def send(*keys):
        tm('send-keys','-t','ux',*keys)
        time.sleep(.35)
    def snapshot():
        time.sleep(.45)
        return tm('capture-pane','-pt','ux')
    try:
        tm('-f','/dev/null','new-session','-d','-s','ux','-x','116','-y','23',
           '-c',str(root),str(BINARY))
        time.sleep(1.5)
        send('F2')
        time.sleep(.4)
        git_screen = snapshot()
        assert 'target.py' in git_screen, 'changed file is missing from Git tree'
        send('o')
        file_screen = snapshot()
        left = [line[:34] for line in file_screen.splitlines()]
        assert 'z_source' in '\n'.join(left), left
        assert 'deep' in '\n'.join(left) or '…' in '\n'.join(left), left
        assert 'leaf' in '\n'.join(left), left
        assert 'target.py' in '\n'.join(left), left
        assert 'after change' in file_screen, 'right preview is not the opened file'
        print('PASS: Git o reveals deep Files tree and scrolls target into viewport')

        send('F2')
        tm('resize-window', '-t','ux','-x','70','-y','23')
        time.sleep(.5)
        send('o')
        narrow = snapshot()
        assert 'target.py' in narrow, narrow
        assert 'z_source' in narrow and 'leaf' in narrow and ('deep' in narrow or '…' in narrow), narrow
        assert 'F1  Files' in narrow or 'F1 Files' in narrow, narrow
        print('PASS: narrow viewport retains Files tree focus and expanded ancestors')
        send('q')
    finally:
        sp.run(['tmux', '-L', sock, 'kill-server'],capture_output=True)

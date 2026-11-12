#!/usr/bin/env python3
"""End-to-end Commit Composer test; only commits inside a temporary Git repository."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

helper = Path(__file__).with_name('smoke-blame.py').resolve()
namespace = {'__file__': str(helper)}
exec(compile(helper.read_text().split('\nwith tempfile.TemporaryDirectory', 1)[0], str(helper), 'exec'), namespace)
Terminal = namespace['Terminal']

with tempfile.TemporaryDirectory(prefix='forge-commit-composer-') as tmp:
    root = Path(tmp, 'repo'); root.mkdir()
    (root / 'alpha.rs').write_text('fn alpha() {}\n')
    (root / 'beta.rs').write_text('fn beta() {}\n')
    def git(*args):
        return sp.run(['git', *args], cwd=root, capture_output=True, check=True).stdout.decode()
    git('init', '-q', '-b', 'main')
    git('add', '.')
    git('-c','user.name=Test','-c','user.email=forge@example.invalid','commit','-qm','initial')
    (root / 'alpha.rs').write_text('fn alpha() { println!("ok"); }\n')
    (root / 'beta.rs').write_text('fn beta() { println!("ok"); }\n')
    git('add', 'alpha.rs', 'beta.rs')
    env = {**os.environ, 'XDG_CONFIG_HOME': str(Path(tmp, 'config')),
           'FORGE_ICONS': 'builtin', 'DISPLAY':'', 'WAYLAND_DISPLAY':'',
           'GIT_AUTHOR_NAME':'Test', 'GIT_AUTHOR_EMAIL':'forge@example.invalid',
           'GIT_COMMITTER_NAME':'Test', 'GIT_COMMITTER_EMAIL':'forge@example.invalid'}
    term = Terminal(root, env)
    def wait(s, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            screen = term.capture()
            if s in screen: return screen
            time.sleep(.14)
        raise AssertionError(f'expected {s!r}:\n{term.capture()}')
    try:
        wait('Git');time.sleep(.6)
        term.send(b'c')
        screen = wait('Commit Composer')
        assert '2 files' in screen and 'alpha.rs' in screen and 'beta.rs' in screen, screen
        term.send(b'\x1b')
        wait('commit cancelled')
        assert git('rev-list', '--count', 'HEAD').strip() == '1'
        term.send(b'c')
        wait('Commit Composer')
        term.send(b'feat: improve commit editor')
        term.send(b'\t')
        term.send(b'Multiline body\r\nSecond line')
        term.send(b'\x13')  # Ctrl+S -> review, not commit
        wait('Confirm Commit')
        assert git('rev-list', '--count', 'HEAD').strip() == '1', 'review must not commit'
        term.send(b'\x1b')  # Back to editing, text preserved
        wait('Commit Composer')
        term.send(b'\x13')
        wait('Confirm Commit')
        term.send(b'\r')  # explicit confirmation
        deadline = time.monotonic()+12
        while time.monotonic() < deadline:
            if git('rev-list', '--count', 'HEAD').strip() == '2': break
            time.sleep(.2)
        else: raise AssertionError('commit did not complete: '+term.capture())
        assert git('log','-1','--pretty=%B').strip() == 'feat: improve commit editor\n\nMultiline body\nSecond line'
        assert git('diff','--cached').strip() == ''
        print('PASS: commit composer preview, cancellation, multiline body, explicit review/confirmation, two staged files')
    finally:
        term.close()

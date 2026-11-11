#!/usr/bin/env python3
"""Settings as its own modal; changes persist and do not corrupt preferences."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BINARY = Path(__file__).resolve().parents[1] / 'target/debug/forge'

with tempfile.TemporaryDirectory(prefix='forge-settings-') as folder:
    root=Path(folder,'project');root.mkdir()
    (root/'a.txt').write_text('visible\n')
    (root/'.secret.txt').write_text('hidden\n')
    cfg=Path(folder,'config')
    path=cfg/'forge'/'config.toml';path.parent.mkdir(parents=True)
    path.write_text('theme = "classic"\nother_option = "preserved"\n')
    sock=f'forge-settings-{os.getpid()}'
    def tm(*args,check=True):
        r=sp.run(['tmux','-L',sock,*args],capture_output=True,text=True,
            env={**os.environ,'XDG_CONFIG_HOME':str(cfg),'FORGE_ICONS':'builtin'})
        if check and r.returncode:raise AssertionError(f'tmux {args}: {r.stderr}')
        return r.stdout
    def send(*keys):
        tm('send-keys','-t','ux',*keys);time.sleep(.27)
    def screen():
        time.sleep(.18)
        return tm('capture-pane','-pt','ux')
    try:
        tm('-f','/dev/null','new-session','-d','-s','ux','-x','105','-y','31',
           '-c',str(root),str(BINARY))
        time.sleep(1.2)
        assert '.secret.txt' not in screen()
        send(',')
        s=screen()
        print('SETTINGS SCREEN:\n'+'\n'.join(s.splitlines()[7:24]))
        assert 'Settings' in s and 'Tree Sticky Scroll' in s and 'Show hidden files' in s
        assert '[ On ]' in s and '[ Off ]' in s and 'Keyboard shortcuts' in s
        send('Down')
        assert 'Compact Folders' in screen()
        send('Down')
        send('Space')
        assert 'Show hidden files' in screen()
        assert 'show_hidden_files = true' in path.read_text()
        assert 'other_option = "preserved"' in path.read_text()
        send('Escape')
        time.sleep(.5)
        assert '.secret.txt' in screen(), 'hidden file did not appear after settings toggle'

        send(',')
        send('Enter') # sticky toggle
        assert 'Tree Sticky Scroll' in screen()
        assert 'tree_sticky_scroll = false' in path.read_text()
        send('Down');send('Down');send('Down');send('Enter')
        assert 'Keyboard shortcuts' in screen() and 'CONTEXT' in screen()
        send('Escape')
        send('q')
        assert sp.run(['tmux','-L',sock,'has-session','-t','ux'],capture_output=True).returncode != 0
        print('PASS: separate Settings UI, hidden-files toggle, sticky-scroll toggle, F4 navigation, unknown TOML preserved')
    finally:
        sp.run(['tmux','-L',sock,'kill-server'],capture_output=True)

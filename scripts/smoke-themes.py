#!/usr/bin/env python3
"""Verify every theme starts a real TUI in a disposable configuration directory."""
import os
import re
import subprocess as sp
import tempfile
import time
from pathlib import Path

binary = Path(__file__).resolve().parents[1] / 'target/debug/forge'
with tempfile.TemporaryDirectory(prefix='forge-theme-smoke-') as temp:
    root = Path(temp)
    for theme in ('current','vscode-dark','vscode-black','github-dark','nord'):
        config = root / theme / 'forge'
        config.mkdir(parents=True)
        (config / 'config.toml').write_text(f'theme = "{theme}"\n')
        socket = 'forge-theme-%s-%d' % (theme, os.getpid())
        env = {**os.environ, 'XDG_CONFIG_HOME':str(config.parent),'TERM':'xterm-256color', 'COLORTERM':'truecolor', 'LC_ALL':'C.UTF-8'}
        try:
            sp.run(['tmux','-L',socket,'-f','/dev/null','new-session','-d','-x','118','-y','30','-c',str(root),str(binary)],check=True,capture_output=True,env=env)
            time.sleep(.7)
            result=sp.run(['tmux','-L',socket,'capture-pane','-e','-p'],capture_output=True,text=True,check=True,env=env).stdout
            assert 'Files' in result or 'Preview' in result or 'Forge' in result, (theme, result[:200])
            ansi = set(re.findall(r'\x1b\[[0-9;:]*m',result))
            print(f'{theme}: TUI started; {len(ansi)} terminal style sequences')
        finally:
            sp.run(['tmux','-L',socket,'kill-server'],capture_output=True,env=env)

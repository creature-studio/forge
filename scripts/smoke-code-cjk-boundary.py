#!/usr/bin/env python3
"""Code-mode long CJK lines must never spill into the left tree (real tmux PTY)."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time
import unicodedata

BIN = Path(__file__).resolve().parents[1] / 'target/debug/forge'
WIDTH, HEIGHT = 129, 38
SIDEBAR = 32

def cell_width(c):
    return 0 if unicodedata.combining(c) else 2 if unicodedata.east_asian_width(c) in 'WF' else 1

def left(line):
    out = ''; n = 0
    for c in line:
        if n >= SIDEBAR: break
        n += cell_width(c)
        out += c
    return out

with tempfile.TemporaryDirectory(prefix='forge-code-cjk-') as folder:
    root = Path(folder, 'repo')
    root.mkdir()
    # Code gutter is five display cells, leaving 91 in a 96-cell preview.
    lines = ['# 中文测试']
    for i in range(130):
        lines.append(f'{i:03d} ' + ('a' * 86) + '中文标记尾巴')
    (root / 'README.md').write_text('\n'.join(lines) + '\n')
    (root / 'docs').mkdir()
    (root / 'src').mkdir()
    sock = f'forge-code-cjk-{os.getpid()}'
    def tm(*args):
        r = sp.run(['tmux', '-u', '-L', sock, *args], text=True, capture_output=True,
            env={**os.environ, 'LC_ALL': 'C.UTF-8', 'LANG': 'C.UTF-8',
                 'XDG_CONFIG_HOME': str(Path(folder,'config'))})
        if r.returncode: raise AssertionError(f'{args}: {r.stderr}')
        return r.stdout
    def send(*keys):
        tm('send-keys', '-t', 'ui', *keys); time.sleep(.06)
    def check(label):
        result = tm('capture-pane', '-pt', 'ui').splitlines()
        for i, row in enumerate(result):
            bleed = ''.join(c for c in left(row) if cell_width(c) == 2)
            if bleed:
                raise AssertionError(f'{label}: overflow into Files at screen row {i}: {left(row)!r}\n' + '\n'.join(result[:20]))
        return result
    try:
        tm('-f','/dev/null','new-session','-d','-s','ui','-x',str(WIDTH),
           '-y',str(HEIGHT),'-c',str(root),str(BIN),'--force-truecolor')
        time.sleep(1.0)
        send('End') # Files: select README.md after two folder entries
        time.sleep(.5)
        send('F3')
        time.sleep(.35)
        before = check('opening Code')
        assert any('1│' in row for row in before), 'Code mode was not opened'
        send('Tab')  # F3 doesn't transfer focus from Files to the preview.
        # Scroll across wrapped CJK rows. Assert that scrolling really works:
        # earlier versions counted source lines, and this test accidentally
        # moved the selected file rather than the preview.
        for k in range(80):
            send('j')
            if k % 4 == 0:
                rows = check(f'code scroll {k}')
                for y in range(6, HEIGHT - 3):
                    assert not left(rows[y]).strip(), (
                        f'extra Code glyphs rendered in Files column {y}: {left(rows[y])!r}'
                    )
        after = check('code scroll end')
        assert before[3:] != after[3:], 'Code viewport did not actually scroll'
        assert any('Preview  line' in row for row in after[-3:]), 'preview focus or scroll was lost'
        print('PASS: no CJK glyph leaks into left Files tree in Code mode (129x38)')
    finally:
        sp.run(['tmux','-L',sock,'kill-server'],capture_output=True)

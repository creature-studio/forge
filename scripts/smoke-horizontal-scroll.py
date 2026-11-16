#!/usr/bin/env python3
"""Real-terminal code no-wrap and horizontal keyboard panning regression."""
import os
from pathlib import Path
import tempfile
import time

helper = Path(__file__).with_name('smoke-blame.py').resolve()
namespace = {'__file__': str(helper)}
exec(compile(helper.read_text().split('\nwith tempfile.TemporaryDirectory',1)[0], str(helper), 'exec'), namespace)
Terminal = namespace['Terminal']

with tempfile.TemporaryDirectory(prefix='forge-horizontal-') as tmp:
    root=Path(tmp,'repo'); root.mkdir()
    (root/'alpha.rs').write_text('fn sample() { let value = "' + 'a'*90 + 'ENDMARKER' + '"; }\n')
    config=Path(tmp,'cfg'); settings=config/'forge'/'config.toml'; settings.parent.mkdir(parents=True)
    settings.write_text('wrap_code = false\n')
    env={**os.environ,'XDG_CONFIG_HOME':str(config),'FORGE_ICONS':'builtin',
         'DISPLAY':'','WAYLAND_DISPLAY':''}
    term=Terminal(root,env)
    def wait_for(needle, timeout=9):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            result=term.capture()
            if needle in result:return result
            time.sleep(.15)
        raise AssertionError(f'Cannot find {needle}:\n{term.capture()}')
    try:
        wait_for('alpha.rs');time.sleep(.6)
        before=term.capture()
        assert 'ENDMARKER' not in before, before
        term.send(b'\t')  # Focus preview.
        for _ in range(10):term.send(b'\x1b[1;3C')  # Alt+Right, 8 cols per press.
        after=wait_for('ENDMARKER')
        assert '1│' in after, 'source line numbers must remain fixed'
        term.send(b'\x1b[1;3H')  # Alt+Home, return to left edge.
        time.sleep(.3)
        assert 'ENDMARKER' not in term.capture(), 'Alt+Home did not reset horizontal position'
        print('PASS: real PTY no-wrap, horizontal Alt+Right, fixed line number and Alt+Home')
    finally:
        term.close()

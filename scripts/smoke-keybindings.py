#!/usr/bin/env python3
"""Real runtime: Ctrl-E, F4 rebind UI, persisted binding after process restart."""
import os
from pathlib import Path
import pty
import select
import signal
import tempfile
import time
import fcntl
import termios
import struct
BINARY=Path(__file__).resolve().parents[1]/'target/debug/forge'
with tempfile.TemporaryDirectory(prefix='forge-keys-') as folder:
    root=Path(folder)/'workspace';root.mkdir();(root/'a.txt').write_text('original')
    config=Path(folder)/'config'
    def run(first):
        pid,fd=pty.fork()
        if pid==0:
            os.chdir(root)
            os.execve(str(BINARY),[str(BINARY)],{**os.environ,'TERM':'xterm-256color','XDG_CONFIG_HOME':str(config)})
        fcntl.ioctl(fd,termios.TIOCSWINSZ,struct.pack('HHHH',35,120,0,0))
        transcript=bytearray()
        def drain(seconds=.4):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([fd],[],[],max(0,end-time.monotonic()))[0]:
                    try:data=os.read(fd,65536)
                    except OSError:break
                    if not data:break
                    transcript.extend(data)
            assert b'panicked at' not in transcript
        def send(data):
            os.write(fd,data);drain()
            assert os.waitpid(pid,os.WNOHANG)[0]==0,'unexpected runtime exit'
        try:
            drain(1)
            if first:
                send(b'\x05');drain(.5)
                assert b'Copy selection / line' in transcript,'Ctrl-E did not open editor'
                send(b'\x1bOS') # F4 opens over editor
                assert b'CONTEXT' in transcript, transcript[-3500:]
                send(b'\x1b');send(b'\x1b') # close settings, then editor
                send(b'\x1bOS')
                send(b'Edit file');send(b'\r');send(b'\x10') # Ctrl-P conflicts
                assert b'Already bound' in transcript
                send(b'\x1b[17~') # F6 bind
                assert 'edit=F6' in (config/'forge/keybindings.conf').read_text()
                send(b'\x1b')
            previous=len(transcript)
            send(b'\x1b[17~');drain(.5)
            assert b'Copy selection / line' in transcript[previous:],'F6 binding did not open editor'
            send(b'\x1b')
            os.write(fd,b'q');drain()
            done,status=os.waitpid(pid,0);pid=0
            assert os.waitstatus_to_exitcode(status)==0
            assert (root/'a.txt').read_text()=='original'
        finally:
            if pid:
                try:os.kill(pid,signal.SIGTERM);os.waitpid(pid,0)
                except ProcessLookupError:pass
            os.close(fd)
    run(True);run(False)
print('PASS: Ctrl-E edit, F4 settings, conflict rejection, live rebind, and binding reload after restart')

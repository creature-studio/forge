#!/usr/bin/env python3
"""Real Forge TUI advanced stash: restore staged and confirmed Pop."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-stash-advanced-") as d:
    repo=Path(d,"repo");repo.mkdir()
    def git(*args):
        p=sp.run(["git",*args],cwd=repo,capture_output=True,text=True)
        if p.returncode: raise AssertionError(f"git {args}: {p.stderr}")
        return p.stdout.strip()
    git("init","-qb","main")
    git("config","user.name","Forge Smoke")
    git("config","user.email","stash@example.invalid")
    f=repo/"file.txt";f.write_text("base\n")
    git("add",".");git("commit","-qm","base")
    f.write_text("staged content\n")
    git("add","file.txt")
    git("stash","push","-m","advanced stash")
    sock=f"forge-stash-advanced-{os.getpid()}"
    def tm(*args):
        p=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
            env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":str(Path(d,"cfg"))})
        if p.returncode: raise AssertionError(f"tmux {args}: {p.stderr}")
        return p.stdout
    def send(*keys):
        tm("send-keys","-t","ui",*keys);time.sleep(.18)
    def screen(): return tm("capture-pane","-pt","ui")
    def wait_for(s,timeout=9):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            pane=screen()
            if s in pane:return pane
            time.sleep(.13)
        raise AssertionError(f"Missing {s!r}:\n{pane}")
    try:
        tm("-f","/dev/null","new-session","-d","-s","ui","-x","120","-y","32","-c",str(repo),str(BIN))
        wait_for("Files")
        send("Z");wait_for("advanced stash")
        send("I");wait_for("original staged")
        send("Escape");wait_for("advanced stash")
        assert not git("diff","--cached","--name-only")
        print("PASS: I confirmation Esc cancels restore-index without mutation")
        send("I");wait_for("original staged");send("Enter")
        until=time.monotonic()+8
        while time.monotonic()<until and git("diff","--cached","--name-only")!="file.txt":
            time.sleep(.17)
        assert git("diff","--cached","--name-only")=="file.txt",screen()
        assert "advanced stash" in git("stash","list")
        print("PASS: I applies stash with original staged state and keeps stash")
        git("reset","-q","--hard","HEAD")
        wait_for("advanced stash")
        send("p");wait_for("Permanently removes") if False else wait_for("permanently removes")
        send("Escape")
        assert "advanced stash" in git("stash","list")
        print("PASS: Pop confirmation Esc retains stash")
        wait_for("advanced stash")
        send("p");wait_for("permanently removes");send("Enter")
        until=time.monotonic()+9
        while time.monotonic()<until and git("stash","list"):
            time.sleep(.18)
        assert not git("stash","list"),screen()
        assert f.read_text()=="staged content\n"
        print("PASS: confirmed p pops stash, removes entry and restores file")
    finally:
        sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

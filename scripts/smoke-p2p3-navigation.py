#!/usr/bin/env python3
"""Real tmux TUI regression for keyboard context menus and worktree return."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-p2p3-nav-") as d:
    root=Path(d,"repo");root.mkdir()
    def git(*args):
        r=sp.run(["git",*args],cwd=root,capture_output=True,text=True)
        if r.returncode:raise AssertionError(f"git {args}: {r.stderr}")
        return r.stdout.strip()
    git("init","-qb","main")
    git("config","user.name","Forge UX Smoke")
    git("config","user.email","ux@example.invalid")
    (root/"docs").mkdir()
    (root/"docs"/"guide.md").write_text("# MAIN_GUIDE\n\nThe current guide\n")
    (root/"src").mkdir()
    (root/"src"/"main.rs").write_text("fn main() {}\n")
    git("add",".");git("commit","-qm","initial")
    other=Path(d,"linked")
    git("worktree","add","-q","-b","test/linked",str(other))
    sock=f"forge-p2p3-nav-{os.getpid()}"
    def tm(*args):
        r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
                 env={**os.environ,"LC_ALL":"C.UTF-8",
                      "XDG_CONFIG_HOME":str(Path(d,"config"))})
        if r.returncode:raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def screen():return tm("capture-pane","-pt","ui")
    def send(*keys):
        tm("send-keys","-t","ui",*keys);time.sleep(.19)
    def wait_for(text,seconds=8):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            v=screen()
            if text in v:return v
            time.sleep(.15)
        raise AssertionError(f"Missing {text!r}:\n{screen()}")
    try:
        tm("-f","/dev/null","new-session","-d","-s","ui","-x","112","-y","32",
           "-c",str(root),str(BIN))
        wait_for("Files")
        send("Home");send("Enter")   # docs directory expands
        send("j")
        wait_for("MAIN_GUIDE")
        send("S-F10")
        v=wait_for("New file",seconds=4)
        assert "Rename" in v
        send("Escape")
        print("PASS: Shift-F10 opens Files context actions and Esc closes")
        send("W")
        wait_for("Worktrees")
        send("j");send("Enter")
        wait_for(str(other))
        send("W");wait_for("Worktrees")
        send("Home");send("Enter")
        wait_for(str(root))
        wait_for("MAIN_GUIDE")
        print("PASS: switching away/back preserves docs expansion, selected guide and its preview")
        send("q")
    finally:
        sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

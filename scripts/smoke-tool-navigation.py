#!/usr/bin/env python3
"""Actual tmux/PTY regression for panel navigation, selection, mouse and history stack."""
from pathlib import Path
import os,re,subprocess as sp,tempfile,time

BIN=Path(__file__).resolve().parents[1]/"target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-tools-nav-") as d:
    root=Path(d,"repo");root.mkdir()
    def git(*args,cwd=root):
        r=sp.run(["git",*args],cwd=cwd,text=True,capture_output=True)
        if r.returncode:raise AssertionError(f"git {args}: {r.stderr}")
        return r.stdout.strip()
    git("init","-qb","main")
    git("config","user.name","Forge Tools UX")
    git("config","user.email","ux@example.invalid")
    for n in range(57):
        (root/f"note_{n:02}.txt").write_text(f"needle for test {n}\nother line\n")
    git("add",".");git("commit","-qm","base")
    for n in range(45):git("branch",f"feature/b-{n:02}")
    sock=f"forge-tool-nav-{os.getpid()}"
    def tm(*args):
        r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
            env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":d+"/cfg"})
        if r.returncode:raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def send(*keys,delay=.20):
        tm("send-keys","-t","ui",*keys);time.sleep(delay)
    def raw(text):
        tm("send-keys","-l","-t","ui",text);time.sleep(.2)
    def pane():return tm("capture-pane","-pt","ui")
    def until(text,seconds=9):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            s=pane()
            if text in s:return s
            time.sleep(.15)
        raise AssertionError(f"Missing {text!r}:\n{pane()}")
    def selected(s):
        return next((line.strip() for line in s.splitlines()
            if "│› " in line or "│›" in line), "")
    def click(x,y):
        raw(f"\x1b[<0;{x+1};{y+1}M")
        raw(f"\x1b[<0;{x+1};{y+1}m")
    try:
        tm("-f","/dev/null","new-session","-d","-s","ui","-x","116","-y","34","-c",str(root),str(BIN))
        until("Files")
        send("B");s=until("feature/b-44")
        assert "* main" in selected(s),f"initial selected branch should be current:\n{s}"
        send("Home");first=selected(pane())
        assert "feature/b-00" in first,first
        send("PageDown");paged=selected(pane())
        assert paged!=first and "feature/" in paged,paged
        send("End");assert "* main" in selected(pane())
        print("PASS: Branches defaults current; Home / End / PageDown navigate long list")

        send(":");until("Commands");send("Escape")
        assert "┌ Branches" in pane(),"Commands Esc should return Branches"
        send("F4");until("Keyboard shortcuts")
        send("Escape");assert "┌ Branches" in pane(),"F4 help should return Branches"
        print("PASS: : Commands / Esc and F4 / Esc return to original Branches panel")

        send("Home")
        s=pane()
        row=next(i for i,line in enumerate(s.splitlines()) if "feature/b-00" in line)
        click(10,row);click(10,row)
        s=until("Switch this repository to branch feature/b-00")
        send("Escape")
        assert git("branch","--show-current")=="main"
        assert "┌ Branches" in pane()
        print("PASS: double-click opens switch confirmation, Esc protects Git branch")

        send("Escape")
        send("C-f");raw("needle")
        until("57 results")
        time.sleep(.55)
        def selected_hit():
            return next((m.group(1) for row in pane().splitlines()
                if (m:=re.search(r"│+›[^\n]*?(\d{2})\.txt",row))),None)
        before=selected_hit()
        send("PageDown");after=selected_hit()
        assert before=="00" and after is not None and after!=before,(before,after,pane())
        send("PageUp");again=selected_hit()
        assert again==before,(before,after,again,pane())
        send("C-r");until("Search contents · regex")
        send("Escape");send("C-f")
        s=until("Search contents · regex")
        assert "> needle" in s,s
        print("PASS: Content PgUp/PgDn moves results, query and regex survive reopening")
        send("Escape")

        send("C-p");raw("note_")
        until("note_00.txt")
        send("PageDown")
        assert "note_" in selected(pane())
        send("Escape");send("C-p")
        assert "> note_" in pane()
        send("Escape")
        print("PASS: File search PageDown navigates; filename query survives reopening")

        send("L");until("Commit history");send("Space")
        send("D");until("Git diff")
        send("Escape")
        assert "Commit history" in pane()
        assert "Pinned" in pane()
        print("PASS: History compare Esc restores history and pinned reference")
        send("Escape")

        tm("resize-window","-t","ui","-x","48","-y","17")
        send("Z");until("Stashes")
        s=pane()
        assert "┌ Stashes " in s and "Restore st" not in s.splitlines()[2]
        assert "p Pop" in s or "X Drop" in s,s
        print("PASS: narrow Stash panel has short title and discoverable actions")
    finally:
        sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

with tempfile.TemporaryDirectory(prefix="forge-no-git-commands-") as d:
    sock=f"forge-tool-plain-{os.getpid()}"
    def tm2(*args):
        r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
            env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":d+"/cfg"})
        if r.returncode:raise AssertionError(r.stderr)
        return r.stdout
    try:
        tm2("-f","/dev/null","new-session","-d","-s","ui","-x","100","-y","29","-c",d,str(BIN))
        time.sleep(.65)
        tm2("send-keys","-t","ui",":")
        tm2("send-keys","-l","-t","ui","Push")
        time.sleep(.65)
        result=tm2("capture-pane","-pt","ui")
        assert "No matching commands" in result and "fatal:" not in result,result
        print("PASS: non-Git workspace hides Git-only Push instead of failing on Enter")
    finally:sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

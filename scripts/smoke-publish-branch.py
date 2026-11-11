#!/usr/bin/env python3
"""Real TUI: publish a new local Git branch with remote picker + confirmation."""
from pathlib import Path
import os
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-publish-branch-") as d:
    home = Path(d)
    repo = home / "repo"
    repo.mkdir()
    origin, backup = home / "origin.git", home / "backup.git"
    def git(*args, cwd=repo, check=True):
        r = sp.run(["git", *args], cwd=cwd, capture_output=True, text=True)
        if r.returncode and check:
            raise AssertionError(f"git {args}: {r.stderr}")
        return r.stdout.strip()
    git("init", "-q", "-b", "dev")
    git("config", "user.email", "publish@example.invalid")
    git("config", "user.name", "Forge Publish Test")
    (repo / "README.md").write_text("# Publish demo\n")
    git("add", ".")
    git("commit", "-qm", "baseline")
    git("init", "--bare", "-q", str(origin), cwd=home)
    git("init", "--bare", "-q", str(backup), cwd=home)
    git("remote", "add", "origin", str(origin))
    git("remote", "add", "backup", str(backup))
    git("switch", "-qc", "feature/publish-test")
    sock = f"forge-publish-{os.getpid()}"

    def tm(*args):
        r = sp.run(["tmux", "-u", "-L", sock, *args], capture_output=True, text=True,
            env={**os.environ, "LC_ALL":"C.UTF-8",
                 "XDG_CONFIG_HOME":str(home / "config")})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def send(*keys):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(.18)
    def screen():
        return tm("capture-pane", "-pt", "ui")
    def wait_for(value, timeout=10):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            pane = screen()
            if value in pane:
                return pane
            time.sleep(.14)
        raise AssertionError(f"Missing {value!r}:\n{screen()}")
    def remote_heads(path):
        return git("--git-dir", str(path), "for-each-ref", "--format=%(refname)",
                   "refs/heads", cwd=home)
    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui", "-x", "116",
           "-y", "34", "-c", str(repo), str(BIN))
        wait_for("Files")
        send("S")
        wait_for("Remote synchronization")
        wait_for("P Publish Branch")
        send("P")
        wait_for("Publish Branch")
        panel = wait_for("Remote: origin")
        assert "Remote: backup" in panel, panel
        assert not remote_heads(backup) and not remote_heads(origin)
        print("PASS: S → P discovers two remotes for branch with no upstream")

        # Select backup explicitly; the origin remote must remain untouched.
        if panel.index("Remote: backup") < panel.index("Remote: origin"):
            send("Enter")
            chosen = "backup"
        else:
            send("j")
            send("Enter")
            chosen = "backup"
        wait_for("Publish local branch feature/publish-test")
        assert "No force-push" in screen()
        send("Escape")
        wait_for("Publish Branch")
        assert not remote_heads(backup) and not remote_heads(origin), "Esc inadvertently published"
        print("PASS: preview and Esc cancellation preserve both remote repositories")

        send("Enter")
        wait_for("Publish local branch feature/publish-test")
        send("Enter")
        deadline=time.monotonic()+12
        while time.monotonic()<deadline:
            if git("config","--get","branch.feature/publish-test.remote",check=False)==chosen:
                break
            time.sleep(.15)
        assert git("config","--get","branch.feature/publish-test.remote")==chosen, screen()
        assert git("config","--get","branch.feature/publish-test.merge")=="refs/heads/feature/publish-test"
        ref="refs/heads/feature/publish-test"
        assert ref in remote_heads(backup) and not remote_heads(origin)
        assert git("rev-parse","HEAD")==git("--git-dir",str(backup),"rev-parse",ref,cwd=home)
        wait_for("Remote synchronization")
        wait_for("feature/publish-test...backup/feature/publish-test")
        print("PASS: confirmed Publish pushes only chosen backup, sets upstream and refreshes Sync")

        send("P")
        time.sleep(.5)
        assert "Publish Branch" not in screen(), "tracked branch incorrectly reenters publish picker"
        print("PASS: subsequent P follows ordinary Push without reopening Publish picker")
        send("q")
    finally:
        sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

    # Another real TUI session in a separate repository, this time with no
    # remote at all. The picker must explain why nothing can be published.
    unconfigured = home / "unconfigured"
    unconfigured.mkdir()
    git("init", "-qb", "topic", cwd=unconfigured)
    git("config", "user.name", "Forge Publish Test", cwd=unconfigured)
    git("config", "user.email", "publish@example.invalid", cwd=unconfigured)
    (unconfigured / "x.txt").write_text("test\n")
    git("add", ".", cwd=unconfigured)
    git("commit", "-qm", "base", cwd=unconfigured)
    sock2 = f"forge-no-remotes-{os.getpid()}"
    def tm2(*args):
        r = sp.run(["tmux", "-u", "-L", sock2, *args],
            capture_output=True, text=True,
            env={**os.environ, "LC_ALL":"C.UTF-8",
                 "XDG_CONFIG_HOME":str(home / "empty-config")})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def wait2(value, timeout=10):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            pane = tm2("capture-pane", "-pt", "ui")
            if value in pane:
                return pane
            time.sleep(.15)
        raise AssertionError(f"Missing {value!r}:\n{pane}")
    try:
        tm2("-f","/dev/null","new-session","-d","-s","ui",
            "-x","112","-y","32","-c",str(unconfigured),str(BIN))
        wait2("Files")
        tm2("send-keys","-t","ui","S")
        wait2("P Publish Branch")
        tm2("send-keys","-t","ui","P")
        pane = wait2("No remotes configured")
        assert "git remote add" in pane, pane
        tm2("send-keys","-t","ui","Enter")
        time.sleep(.3)
        assert not git("remote", cwd=unconfigured), "Picker must not create a remote"
        assert not git("config", "--get", "branch.topic.remote",
                       cwd=unconfigured, check=False)
        print("PASS: no-remote picker explains setup and cannot accidentally publish")

        # The Git sidebar's direct P shortcut follows the same safe flow.
        tm2("send-keys", "-t", "ui", "Escape")
        wait2("Remote synchronization")
        tm2("send-keys", "-t", "ui", "Escape")
        time.sleep(.18)
        tm2("send-keys", "-t", "ui", "F2")
        time.sleep(.25)
        tm2("send-keys", "-t", "ui", "P")
        wait2("Publish Branch")
        print("PASS: Git sidebar direct P opens Publish Branch for untracked branch")
    finally:
        sp.run(["tmux","-L",sock2,"kill-server"],capture_output=True)

    # A branch with no commits cannot publish: explain the prerequisite
    # instead of starting a failed remote write.
    unborn = home / "unborn"
    unborn.mkdir()
    git("init", "-qb", "new-branch", cwd=unborn)
    git("remote", "add", "origin", str(origin), cwd=unborn)
    sock3 = f"forge-unborn-{os.getpid()}"
    def tm3(*args):
        r=sp.run(["tmux","-u","-L",sock3,*args],
            capture_output=True, text=True,
            env={**os.environ,"LC_ALL":"C.UTF-8",
                 "XDG_CONFIG_HOME":str(home/"unborn-config")})
        if r.returncode:
            raise AssertionError(f"tmux {args}: {r.stderr}")
        return r.stdout
    def wait3(value, timeout=10):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            pane=tm3("capture-pane","-pt","ui")
            if value in pane:
                return pane
            time.sleep(.15)
        raise AssertionError(f"Missing {value!r}:\n{pane}")
    try:
        tm3("-f","/dev/null","new-session","-d","-s","ui",
            "-x","110","-y","32","-c",str(unborn),str(BIN))
        wait3("Files")
        tm3("send-keys","-t","ui","S")
        wait3("Remote synchronization")
        wait3("No commits yet")
        tm3("send-keys","-t","ui","P")
        wait3("Create the first commit before publishing")
        assert not remote_heads(origin), "Unborn branch attempted a remote write"
        print("PASS: unborn branch explains first-commit requirement without a remote write")
    finally:
        sp.run(["tmux","-L",sock3,"kill-server"],capture_output=True)

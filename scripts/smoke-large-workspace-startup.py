#!/usr/bin/env python3
"""Large project initial Files listing must finish without recursive watcher startup."""
from pathlib import Path
import os
import subprocess
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"

with tempfile.TemporaryDirectory(prefix="forge-big-startup-") as temp:
    root = Path(temp, "repo")
    root.mkdir()
    (root / "README.md").write_text("# Large workspace\n")
    (root / ".gitignore").write_text("node_modules/\n")
    source = root / "src"
    source.mkdir()
    for i in range(600):
        (source / f"file_{i:04}.rs").write_text("fn main() {}\n")
    packages = root / "node_modules"
    packages.mkdir()
    for i in range(1800):
        (packages / f"package-{i:04}" / "dist" / "types").mkdir(parents=True)
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    sock = f"forge-big-{os.getpid()}"
    def tm(*args):
        p = subprocess.run(["tmux", "-u", "-L", sock, *args], capture_output=True, text=True,
            env={**os.environ, "LC_ALL": "C.UTF-8", "XDG_CONFIG_HOME": str(Path(temp, "cfg"))})
        if p.returncode:
            raise AssertionError(f"tmux {args}: {p.stderr}")
        return p.stdout
    try:
        start = time.monotonic()
        tm("-f", "/dev/null", "new-session", "-d", "-s", "app",
           "-x", "100", "-y", "32", "-c", str(root), str(BIN))
        deadline = start + 6
        while time.monotonic() < deadline:
            pane = tm("capture-pane", "-pt", "app")
            if "src" in pane and "README.md" in pane and "listing " not in pane:
                elapsed = time.monotonic() - start
                # Don't fail based on tiny timing differences, but prevent
                # a recursively-watched dependency tree from blocking startup.
                print(f"PASS: large workspace root visible after {elapsed:.2f}s "
                      "(1800 ignored dependency packages; root listing not blocked)")
                break
            time.sleep(0.12)
        else:
            raise AssertionError("Initial directory listing stuck in a large repo:\n" + pane)
        # Let the asynchronous watcher hook register its shallow root watch
        # before simulating an external edit.
        time.sleep(1.3)
        (root / "new-from-editor.txt").write_text("new\n")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if "new-from-editor." in tm("capture-pane", "-pt", "app"):
                print("PASS: shallow root watcher refreshes external file creation")
                break
            time.sleep(0.13)
        else:
            raise AssertionError("Root watcher lost external refresh:\n" + tm("capture-pane", "-pt", "app"))

        # Loaded expanded folders retain their own shallow watch, even
        # though the 1800 unopened node_modules packages are unwatched.
        tm("send-keys", "-t", "app", "Enter")
        time.sleep(1.5)
        (source / "AAA-added-externally.rs").write_text("fn example() {}\n")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if "AAA-added" in tm("capture-pane", "-pt", "app"):
                print("PASS: expanded src folder watches external changes without recursive scan")
                break
            time.sleep(.13)
        else:
            raise AssertionError("Expanded-folder watcher missed change:\n" +
                                 tm("capture-pane", "-pt", "app"))
    finally:
        subprocess.run(["tmux", "-L", sock, "kill-server"], capture_output=True)

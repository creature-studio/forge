#!/usr/bin/env python3
"""Exercise workspace tools and editor suspension in a real PTY, in a temporary repo."""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/forge"


def smoke():
    with tempfile.TemporaryDirectory(prefix="forge-workspace-") as directory:
        root = Path(directory)

        def git(*args):
            return subprocess.check_output(["git", "-C", directory, *args], stderr=subprocess.STDOUT).decode()

        git("init", "-q", "-b", "main")
        git("config", "user.name", "Test")
        git("config", "user.email", "test@example.test")
        (root / "src").mkdir()
        (root / "a.rs").write_text("first\nsecond\nthird\n")
        (root / "src/deep.rs").write_text("line one\nlet needle = 42;\n")
        git("add", ".")
        git("commit", "-qm", "base")
        (root / "a.rs").write_text("first\nnew second\nthird\nextra\n")
        editor = root / "editor fixture.py"
        editor.write_text("import pathlib,sys\np=pathlib.Path(sys.argv[-1])\np.write_text(p.read_text()+'editor returned\\n')\n")
        pid, master = pty.fork()
        if pid == 0:
            os.chdir(directory)
            os.execve(str(BINARY), [str(BINARY), "--force-ansi16"], {**os.environ, "TERM": "xterm-256color", "EDITOR": f"python3 '{editor}'"})
        transcript = bytearray()

        def drain(seconds=0.4):
            stop = time.monotonic() + seconds
            data = bytearray()
            while time.monotonic() < stop:
                if select.select([master], [], [], max(0, stop - time.monotonic()))[0]:
                    try:
                        part = os.read(master, 65536)
                    except OSError:
                        break
                    if not part:
                        break
                    data.extend(part)
            transcript.extend(data)
            assert b"panicked at" not in transcript, transcript.decode(errors="replace")[-4000:]
            return data

        def send(keys):
            os.write(master, keys)
            return drain(0.5)

        def wait_for(predicate, label):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                if predicate():
                    return
                drain(0.1)
            raise AssertionError(label + "\n" + transcript.decode(errors="replace")[-6000:])

        def command(query):
            send(b":" + query.encode() + b"\r")

        try:
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
            os.kill(pid, signal.SIGWINCH)
            drain(1)
            assert b"Files" in transcript and b"Git" in transcript
            send(b"\x1b[<0;17;2M")  # Visible Git tab.
            assert b"Git" in transcript
            send(b"\x1b[<0;4;2M")  # Visible Files tab.
            send(b"su")
            assert not git("diff", "--cached"), "Files shortcuts changed the index"

            # Root blank-space menu: create a folder, then create a file inside it.
            send(b"\x1b[<2;4;23M")
            send(b"\x1b[B\r")  # New folder.
            send(b"menu-dir\r")
            wait_for(lambda: (root / "menu-dir").is_dir(), "right-click new folder failed")
            send(b"\x1b")
            send(b"\x1b[<2;4;3M")  # First content row, after heading and its gap.
            send(b"\x1b[B\r")  # New file inside folder.
            send(b"note.txt\r")
            wait_for(lambda: (root / "menu-dir/note.txt").is_file(), "folder context new file failed")
            send(b"\x1b")
            send(b"\x1b[<2;4;3M")
            send(b"\x1b[A\x1b[A\r")  # Wrap to Delete folder, above Copy path.
            assert (root / "menu-dir/note.txt").exists(), "folder deleted before confirmation"
            send(b"\x1b")
            assert (root / "menu-dir/note.txt").exists(), "cancelled folder delete changed files"
            send(b"\x1b[<2;4;3M")
            send(b"\x1b[A\x1b[A\r")
            send(b"\r")
            wait_for(lambda: not (root / "menu-dir").exists(), "folder context delete failed")
            send(b"\x1b")

            send(b"\x10deeprs")  # Ctrl-P, filename subsequence.
            assert b"deep.rs" in transcript
            send(b"\r")
            send(b"\x05")
            send("qgsu你好\r".encode())
            send(b"\x1b[200~pasted\ntext\n\x1b[201~")
            send(b"\x13")  # Ctrl-S in the built-in editor.
            wait_for(lambda: (root / "src/deep.rs").read_text().startswith("qgsu你好\npasted\ntext\n"), "built-in multiline editor save failed")
            assert not git("diff", "--cached"), "editor input triggered Git actions"
            send(b"\x1b")
            saved = (root / "src/deep.rs").read_text()
            send(b"\x05")
            send(b"unsaved")
            send(b"\x1b")  # Dirty close confirmation.
            send(b"\x1b")  # Keep editing.
            send(b"\x1b")
            send(b"d")  # Explicitly discard the unsaved draft.
            assert (root / "src/deep.rs").read_text() == saved
            send(b"E")
            wait_for(lambda: "editor returned" in (root / "src/deep.rs").read_text(), "$EDITOR did not receive the selected file")
            drain(1)
            send(b"\x06needle")  # Ctrl-F, content search and line jump.
            assert b"contents" in transcript and b"needle" in transcript
            send(b"\r")
            send(b"\x1b")

            command("New file")
            send(b"\x15created.txt\r")  # Ctrl-U clears the suggested directory.
            wait_for(lambda: (root / "created.txt").exists(), "new file failed")
            send(b"\x1b")
            send(b"\x10created")
            send(b"\r")
            send(b"\x1b")
            send(b"\x1b[3~")  # Delete opens confirmation.
            assert (root / "created.txt").exists(), "delete ran before confirmation"
            send(b"\x1b")
            assert (root / "created.txt").exists(), "cancelled delete changed files"
            send(b"\x1b[3~\r")
            wait_for(lambda: not (root / "created.txt").exists(), "confirmed deletion failed")
            send(b"\x1b")

            send(b"g[")
            send(b"I")
            assert b"Diff" in transcript and b"lines" in transcript
            send(b"s")  # First changed line is only '-second'.
            wait_for(lambda: "second\n" not in git("show", ":a.rs"), "single-line staging failed")
            assert "new second" not in git("show", ":a.rs"), "line action staged its neighbor"
            send(b"\x1b")
            send(b"L")
            assert b"history" in transcript
            send(b"\r")
            assert b"details" in transcript
            send(b"\x1b")  # Commit details returns to the history list.
            send(b"\x1b")  # Then close history before opening another tool.
            send(b"B")
            assert b"Branches" in transcript
            send(b"\x1b")
            send(b"Z")
            assert b"Stashes" in transcript
            send(b"\x1b")
            send(b"W")
            assert b"Worktrees" in transcript
            send(b"\x1b")
            send(b"S")
            assert b"synchronization" in transcript
            send(b"\x1b")
            send(b"q")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    assert os.waitstatus_to_exitcode(status) == 0
                    break
                drain(0.1)
            else:
                raise AssertionError("forge failed to exit")
            assert not (termios.tcgetattr(master)[3] & termios.ICANON) == 0, "terminal stayed raw"
            print("PASS: Files/Git tabs, context file/folder create/delete/cancel, built-in multiline text save/discard/paste, fuzzy/content search, external editor return, line stage, history, branches, stashes, worktrees, sync, terminal restoration")
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError):
                pass
            os.close(master)


if __name__ == "__main__":
    smoke()

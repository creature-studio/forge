from pathlib import Path
import os,subprocess as sp,tempfile,time
B="/home/fxh/tools/forge/target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-audit-fifo-copy-") as d:
 root=Path(d,"repo");root.mkdir()
 src=root/"source";src.mkdir()
 (src/"a.txt").write_text("valid")
 os.mkfifo(src/"z_pipe")
 (root/"dest").mkdir()
 sock=f"forge-fifo-audit-{os.getpid()}"
 def tm(*args,check=True):
  r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
   env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":d+"/cfg"})
  if check and r.returncode:raise RuntimeError(f"tmux {args}: {r.stderr}")
  return r.stdout
 def send(*keys,delay=.1):tm("send-keys","-t","ui",*keys);time.sleep(delay)
 def screen():return tm("capture-pane","-pt","ui")
 try:
  tm("-f","/dev/null","new-session","-d","-s","ui","-x","114","-y","30","-c",str(root),B)
  time.sleep(.6)
  send("Home");send("Down");send(":");send("-l","Copy");send("Enter")
  if "Copy items" not in screen(): raise AssertionError("copy dialog did not open")
  send("C-a");send("Delete");send("-l","dest");send("Enter",delay=.25)
  print("after copy started:",[x.strip() for x in screen().splitlines() if "Working" in x or "Operation in progress" in x])
  time.sleep(2.0)
  mid=screen()
  dest=root/"dest"/"source"
  print("after 2s still Working:", "Operation in progress" in mid or "Working" in mid)
  assert not dest.exists(), "FIFO copy should not publish incomplete destination folder"
  assert not any(f.name.startswith(".forge-copy-") for f in (root/"dest").iterdir())
  print("PASS: FIFO copy fails without publishing incomplete directory or staging junk")
  send("Escape",delay=.05);send("q",delay=.1)
  time.sleep(.2)
  proc=tm("list-panes","-t","ui","-F","#{pane_pid}",check=False)
  print("after Esc and q session process status:",repr(proc.strip()),"dest remains:",dest.exists())
 finally:
  sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

from pathlib import Path
import os,subprocess as sp,tempfile,time
B=Path(__file__).resolve().parents[1] / "target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-audit-partial-copy-") as d:
 root=Path(d,"repo");root.mkdir()
 (root/"a_ok.txt").write_text("good content")
 src=root/"z_locked.txt";src.write_text("TOP_SECRET")
 src.chmod(0)
 (root/"dest").mkdir()
 sock=f"forge-copy-audit-{os.getpid()}"
 def tm(*args):
  r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
   env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":d+"/cfg"})
  if r.returncode:raise RuntimeError(f"{args}: {r.stderr}")
  return r.stdout
 def send(*keys,sleep=.2):tm("send-keys","-t","ui",*keys);time.sleep(sleep)
 def screen():return tm("capture-pane","-pt","ui")
 try:
  tm("-f","/dev/null","new-session","-d","-s","ui","-x","116","-y","32","-c",str(root),str(B))
  time.sleep(.8)
  send("Home");send("Down");send("v")
  send("Down");send("v")
  print("selected footer:",[x.strip() for x in screen().splitlines() if "selected" in x.lower()][-3:])
  send(":");send("-l","Copy");time.sleep(.35)
  before=screen()
  print("command matched:", [x.strip() for x in before.splitlines() if "Copy selected items" in x][:2])
  send("Enter")
  prompt=screen()
  print("prompt:", [x.strip() for x in prompt.splitlines() if "Copy items" in x or "Destination" in x][:3])
  send("C-a");send("Delete");send("-l","dest");send("Enter",sleep=1.1)
  after=screen()
  print("result:", [x.strip() for x in after.splitlines() if "Failed:" in x or "Operation failed" in x or "Completed" in x][:4])
  print("SOURCE z_locked exists:",src.exists(),"ROOT a_ok:",(root/"a_ok.txt").exists())
  assert "1 of 2 succeeded" in after, after
  assert (root/"dest/a_ok.txt").read_text()=="good content"
  assert not (root/"dest/z_locked.txt").exists(), "unreadable source must never publish 0-byte copy"
  assert not any(f.name.startswith(".forge-copy-") for f in (root/"dest").iterdir())
  print("PASS: partial batch copy reports exact count, keeps valid file, no broken target or staging junk")
 finally:
  sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)
  src.chmod(0o600)

from pathlib import Path
import os, subprocess as sp, tempfile, time

B=Path(__file__).resolve().parents[1] / "target/debug/forge"
def scenario(kind):
 with tempfile.TemporaryDirectory(prefix=f"forge-audit-{kind}-") as temp:
  root=Path(temp,"repo");root.mkdir()
  if kind=="search":
   (root/"src").mkdir()
   (root/"src/a.txt").write_text("alpha\n\nbeta\n")
  else:
   (root/"a.txt").write_text("delete-me")
   protected=root/"z_protected";protected.mkdir()
   (protected/"keep.txt").write_text("keep-me")
   protected.chmod(0o500)
  socket=f"forge-audit-{kind}-{os.getpid()}"
  def tm(*args):
   r=sp.run(["tmux","-u","-L",socket,*args],capture_output=True,text=True,
    env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":temp+"/cfg"})
   if r.returncode:raise RuntimeError(f"tmux {args} -> {r.stderr}")
   return r.stdout
  def screen():return tm("capture-pane","-pt","ui")
  def send(*keys,sleep=.3):
   tm("send-keys","-t","ui",*keys);time.sleep(sleep)
  try:
   tm("-f","/dev/null","new-session","-d","-s","ui","-x","130","-y","32","-c",str(root),str(B))
   time.sleep(.8)
   if kind=="search":
    send("C-f")
    send("-l","^$")
    send("C-r",sleep=1.2)
    data=screen()
    print("SEARCH PANEL EXCERPT")
    for row in data.splitlines():
     if "src/a.txt" in row or "results" in row or "regex" in row:
      print(row[:115])
    assert "a.txt:2  (empty line)" in data, data
    print("PASS: empty-line regex hit retains content line 2")
   else:
    send("V")
    send("Delete",sleep=.35)
    data=screen()
    print("DELETE PROMPT EXCERPT")
    for row in data.splitlines():
     if "Permanently" in row or "items" in row or "a.txt" in row or "z_protected" in row:
      print(row[:115])
    send("Enter",sleep=1.3)
    data=screen()
    assert "1 of 2 succeeded" in data, data
    assert not (root/"a.txt").exists() and protected.exists()
    print("PASS: partial deletion reports completed count and failed item")
    for row in data.splitlines():
     if "Failed:" in row or "Operation failed" in row or "Permission denied" in row:
      print(row[:116])
  finally:
   sp.run(["tmux","-L",socket,"kill-server"],capture_output=True)
   if kind=="delete":
    protected.chmod(0o700)
scenario("search")
scenario("delete")

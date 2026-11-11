from pathlib import Path
import os,subprocess as sp,tempfile,time
B=str(Path(__file__).resolve().parents[1] / "target/debug/forge")
with tempfile.TemporaryDirectory(prefix="forge-audit-linked-watch-") as d:
 root=Path(d,"main");root.mkdir()
 def git(*args,cwd=root):
  r=sp.run(["git",*args],cwd=cwd,capture_output=True,text=True)
  if r.returncode:raise RuntimeError(f"git {args} {r.stderr}")
  return r.stdout.strip()
 git("init","-qb","main");git("config","user.name","Audit")
 git("config","user.email","audit@example.invalid")
 (root/"README.md").write_text("same commit in both branches\n")
 git("add",".");git("commit","-qm","initial")
 linked=Path(d,"linked")
 git("worktree","add","-q","-b","feature/old",str(linked))
 print("Linked .git is_file:",(linked/".git").is_file(), "linked HEAD:",git("branch","--show-current",cwd=linked))
 sock=f"forge-linked-audit-{os.getpid()}"
 def tm(*args):
  r=sp.run(["tmux","-u","-L",sock,*args],capture_output=True,text=True,
   env={**os.environ,"LC_ALL":"C.UTF-8","XDG_CONFIG_HOME":d+"/cfg"})
  if r.returncode:raise RuntimeError(f"tmux {args}: {r.stderr}")
  return r.stdout
 def pane():return tm("capture-pane","-pt","ui")
 def branch_lines(s):
  return [line.strip() for line in s.splitlines() if "feature/" in line or "↻" in line or "⎇" in line]
 try:
  tm("-f","/dev/null","new-session","-d","-s","ui","-x","118","-y","32","-c",str(linked),B)
  time.sleep(1.2)
  print("Before:",branch_lines(pane())[-3:])
  git("switch","-qc","feature/new",cwd=linked)
  deadline=time.monotonic()+8
  while time.monotonic()<deadline:
   mid=pane()
   if "feature/new" in mid and "feature/old" not in mid:
    break
   time.sleep(.2)
  assert "feature/new" in mid and "feature/old" not in mid, (
   "Worktree metadata changed but Forge failed to auto-refresh:\n"+mid
  )
  print("PASS: linked Git worktree external branch switch auto-refreshes without pressing r")
 finally:
  sp.run(["tmux","-L",sock,"kill-server"],capture_output=True)

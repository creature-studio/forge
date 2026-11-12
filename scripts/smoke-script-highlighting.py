#!/usr/bin/env python3
"""Real Forge tmux smoke: multi-language colors and user-installed .sublime-syntax plugin."""
from pathlib import Path
import os
import re
import subprocess as sp
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/forge"
with tempfile.TemporaryDirectory(prefix="forge-script-highlight-") as d:
    repo = Path(d, "workspace")
    repo.mkdir()
    (repo / "app.js").write_text('const enabled = true; // js-file\n')
    (repo / "00-plain.txt").write_text('const enabled = true; // plain-file\n')
    (repo / "demo.py").write_text('def hello(): # python-file\n    return "world" # comment\n')
    (repo / "script.sh").write_text("if true; then echo shell-file; fi # shell-file\n")
    (repo / "sample.ts").write_text("const count: number = 123; // ts-file\n")
    (repo / "data.json").write_text('{"enabled": true, "message": "json-file"}\n')
    (repo / "schema.sql").write_text('SELECT * FROM users WHERE id = 42; -- sql-file\n')
    (repo / "config.yaml").write_text('enabled: true # yaml-file\n')
    (repo / "large.py").write_text("def first_line(): # large-file-first\n" + "".join(f"    print({i}) # row-{i:06d}\n" for i in range(120000)))
    (repo / "main.go").write_text('package main // go-file\nfunc main() { return }\n')
    (repo / "native.cpp").write_text('class Code {}; // cpp-file\n')
    (repo / "Sample.java").write_text('public class Sample {} // java-file\n')
    (repo / "page.html").write_text('<div class="hello">html-file</div>\n')
    (repo / "layout.xml").write_text('<entry key="xml">xml-file</entry>\n')
    (repo / "theme.css").write_text('body { display: flex; } /* css-file */\n')
    (repo / "settings.toml").write_text('enabled = true # toml-file\n')
    (repo / "Dockerfile").write_text('FROM alpine:latest # docker-file\nRUN echo hello\n')
    (repo / "plugin.lua").write_text('local enabled = true -- lua-file\n')
    (repo / "example.xyz").write_text('MAGIC is a custom keyword # custom-file\n')
    plugin = Path(d, "config", "forge", "syntaxes")
    plugin.mkdir(parents=True)
    (plugin / "custom.sublime-syntax").write_text(
        "%YAML 1.2\n---\nname: Forge Custom Syntax\n"
        "file_extensions: [xyz]\nscope: source.custom\ncontexts:\n"
        "  main:\n    - match: '\\bMAGIC\\b'\n"
        "      scope: keyword.control.custom\n"
    )
    sock = f"forge-script-syntax-{os.getpid()}"
    env = {**os.environ, "LC_ALL": "C.UTF-8", "XDG_CONFIG_HOME": str(Path(d, "config"))}

    def tm(*args):
        result = sp.run(["tmux", "-u", "-L", sock, *args],
                        capture_output=True, text=True, env=env)
        if result.returncode:
            raise AssertionError(f"tmux {args}: {result.stderr}")
        return result.stdout

    def send(*keys, delay=.22):
        tm("send-keys", "-t", "ui", *keys)
        time.sleep(delay)

    def styled_screen():
        return tm("capture-pane", "-e", "-p", "-t", "ui")

    def open_file(name):
        send("C-p")
        # Find file remembers the previous query by design. Clear it before
        # testing a different extension; Ctrl-A moves the caret, not selects.
        send("End")
        tm("send-keys", "-t", "ui", *(["BSpace"] * 64))
        tm("send-keys", "-l", "-t", "ui", name)
        time.sleep(1.2)
        send("Enter", delay=.7)

    def code_row(marker):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            lines = styled_screen().splitlines()
            for row in lines:
                visible = re.sub(r"\x1b\[[0-9;:]*m", "", row)
                if marker in visible and "│" in visible:
                    return row
            time.sleep(.17)
        raise AssertionError(f"missing loaded preview {marker!r}: {styled_screen()}")

    try:
        tm("-f", "/dev/null", "new-session", "-d", "-s", "ui",
           "-x", "122", "-y", "33", "-c", str(repo), str(BIN))
        time.sleep(.65)
        pid = tm("display-message", "-p", "-t", "ui", "#{pane_pid}").strip()

        def rss_mb():
            status = Path("/proc", pid, "status").read_text()
            found = re.search(r"^VmRSS:\s+(\d+)\s+kB", status, flags=re.MULTILINE)
            return int(found.group(1)) / 1024 if found else 0.0

        baseline_rss = rss_mb()
        print(f"Forge RSS before syntax grammar loading: {baseline_rss:.1f} MiB")
        results = {}
        for filename, fragment in [
            ("00-plain.txt", "plain-file"),
            ("app.js", "js-file"),
            ("demo.py", "python-file"),
            ("schema.sql", "sql-file"),
            ("script.sh", "shell-file"),
            ("sample.ts", "ts-file"),
            ("data.json", "json-file"),
            ("config.yaml", "yaml-file"),
            ("main.go", "go-file"),
            ("native.cpp", "cpp-file"),
            ("Sample.java", "java-file"),
            ("page.html", "html-file"),
            ("layout.xml", "xml-file"),
            ("theme.css", "css-file"),
            ("settings.toml", "toml-file"),
            ("Dockerfile", "docker-file"),
            ("plugin.lua", "lua-file"),
            ("example.xyz", "custom-file"),
        ]:
            open_file(filename)
            row = code_row(fragment)
            styles = re.findall(r"\x1b\[[0-9;:]*m", row)
            results[filename] = len(styles)
            print(f"{filename}: {len(styles)} terminal style changes")

        assert results["app.js"] > results["00-plain.txt"], results
        assert results["demo.py"] > 4, results
        assert results["schema.sql"] > 4, results
        assert results["script.sh"] > 4, results
        assert results["sample.ts"] > 4, results
        assert results["data.json"] > 4, results
        for language in ("main.go", "native.cpp", "Sample.java", "page.html", "layout.xml", "theme.css", "settings.toml", "Dockerfile", "plugin.lua"):
            assert results[language] > 4, (language, results)
        assert results["example.xyz"] > results["00-plain.txt"], results
        print("PASS: highlighted scripts and installed .xyz syntax plugin are visible in real TUI")
        # A multi-megabyte source must not be fully parsed or cached.
        started = time.monotonic()
        open_file("large.py")
        code_row("large-file-first")
        large_open_seconds = time.monotonic() - started
        rss_open = rss_mb()
        for _ in range(12):
            send("PgDn", delay=.06)
        scroll_screen = styled_screen()
        assert "large.py" in re.sub(r"\x1b\[[0-9;:]*m", "", scroll_screen), "large preview disappeared on scroll"
        rss_scroll = rss_mb()
        print(f"Large file ({(repo / 'large.py').stat().st_size / 1048576:.1f} MiB) opened in {large_open_seconds:.2f}s, RSS on open {rss_open:.1f} MiB, after scrolling {rss_scroll:.1f} MiB")
        assert rss_scroll - baseline_rss < 32, "large file caused excessive resident memory growth"
        final_rss = rss_mb()
        print(f"Forge RSS after syntax previews: {final_rss:.1f} MiB")
        print(f"RSS change from initial plain preview: {final_rss-baseline_rss:+.1f} MiB")
    finally:
        sp.run(["tmux", "-L", sock, "kill-server"], capture_output=True, env=env)

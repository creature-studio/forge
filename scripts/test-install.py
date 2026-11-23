#!/usr/bin/env python3
"""Exercise the real installer with local release assets and no network access."""

import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"
PAYLOAD = b"#!/bin/sh\nprintf 'forge test binary\\n'\n"
MOCK_DOWNLOAD = r'''
import os
from pathlib import Path
import sys

args = sys.argv[1:]
url = args[-1]
root = Path(os.environ["TEST_RELEASE"])
with (root / "requests").open("a") as log:
    log.write(url + "\n")
if url.endswith("/releases/latest"):
    print(os.environ["TEST_LATEST"])
elif url.startswith("https://github.com/creature-studio/forge/releases/download/v9.8.7/"):
    flag = "-o" if "-o" in args else "-O"
    destination = Path(args[args.index(flag) + 1])
    destination.write_bytes((root / url.rsplit("/", 1)[-1]).read_bytes())
else:
    sys.exit("unexpected URL: " + url)
'''


class InstallerTests(unittest.TestCase):
    def run_installer(self, downloader, *, version=None, latest=None, corrupt=False):
        with tempfile.TemporaryDirectory(prefix="forge-install-test-") as directory:
            root = Path(directory)
            tools = root / "tools"
            tools.mkdir()
            # Deliberately exclude both real downloaders from PATH.
            for name in (
                "tar", "gzip", "awk", "cut", "head", "sed", "tr", "mkdir", "mktemp",
                "rm", "cp", "chmod", "install", "sha256sum", "shasum",
            ):
                target = shutil.which(name)
                if target:
                    (tools / name).symlink_to(target)
            uname = tools / "uname"
            uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
            uname.chmod(0o755)
            mock = tools / downloader
            mock.write_text(f"#!{sys.executable}\n" + MOCK_DOWNLOAD)
            mock.chmod(0o755)

            asset = "forge-x86_64-unknown-linux-gnu.tar.gz"
            with tarfile.open(root / asset, "w:gz") as archive:
                info = tarfile.TarInfo("forge")
                info.size = len(PAYLOAD)
                info.mode = 0o755
                archive.addfile(info, io.BytesIO(PAYLOAD))
            digest = hashlib.sha256((root / asset).read_bytes()).hexdigest()
            (root / "SHA256SUMS").write_text(f"{'0' * 64 if corrupt else digest}  {asset}\n")
            installed = root / "bin" / "forge"
            installed.parent.mkdir()
            installed.write_bytes(b"existing installation\n")

            env = os.environ.copy()
            env.pop("FORGE_VERSION", None)
            env.update({
                "PATH": str(tools),
                "HOME": str(root),
                "TMPDIR": str(root),
                "FORGE_INSTALL_DIR": str(installed.parent),
                "TEST_RELEASE": str(root),
                "TEST_LATEST": latest if latest is not None else (
                    "https://github.com/creature-studio/forge/releases/tag/v9.8.7"
                    if downloader == "curl" else json.dumps({"tag_name": "v9.8.7"})
                ),
            })
            if version is not None:
                env["FORGE_VERSION"] = version
            result = subprocess.run(
                ["/bin/sh", str(INSTALLER)], env=env, text=True,
                capture_output=True, timeout=15,
            )
            requests = (root / "requests").read_text().splitlines()
            return result, installed.read_bytes(), os.access(installed, os.X_OK), requests

    def test_latest_installs_with_either_downloader(self):
        for downloader in ("curl", "wget"):
            with self.subTest(downloader=downloader):
                result, installed, executable, _ = self.run_installer(downloader)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(installed, PAYLOAD)
                self.assertTrue(executable)
                self.assertIn("Installed forge v9.8.7", result.stdout)

    def test_explicit_version_skips_latest_lookup(self):
        for downloader in ("curl", "wget"):
            for version in ("v9.8.7", "9.8.7"):
                with self.subTest(downloader=downloader, version=version):
                    result, installed, _, requests = self.run_installer(downloader, version=version)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(installed, PAYLOAD)
                    self.assertEqual(len(requests), 2)
                    self.assertTrue(all("/releases/download/v9.8.7/" in url for url in requests))

    def test_invalid_latest_preserves_existing_installation(self):
        for downloader in ("curl", "wget"):
            for latest in ("", '{"message":"Not Found"}'):
                with self.subTest(downloader=downloader, latest=latest):
                    result, installed, _, requests = self.run_installer(downloader, latest=latest)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("could not determine the latest Forge release", result.stderr)
                    self.assertEqual(installed, b"existing installation\n")
                    self.assertEqual(len(requests), 1)

    def test_bad_checksum_preserves_existing_installation(self):
        for downloader in ("curl", "wget"):
            with self.subTest(downloader=downloader):
                result, installed, _, _ = self.run_installer(downloader, corrupt=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("checksum mismatch", result.stderr)
                self.assertEqual(installed, b"existing installation\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)

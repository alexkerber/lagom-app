#!/usr/bin/env python3
"""Exercise the Android build orchestration without compiling or accessing devices."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class AndroidBuildTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        (self.repo / "scripts").mkdir()
        shutil.copyfile(Path(__file__).with_name("build-android.sh"), self.repo / "scripts/build-android.sh")
        self.sdk = self.repo / "sdk"
        self.sdk.mkdir()
        self.bin = self.repo / "bin"
        self.bin.mkdir()
        cargo = self.bin / "cargo"
        cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open("calls.jsonl", "a") as log:
    log.write(json.dumps([os.environ["ANDROID_NDK_HOME"], args]) + "\\n")
if args[0] == "ndk":
    for abi in ("arm64-v8a", "x86_64"):
        library = pathlib.Path(args[args.index("-o") + 1], abi, "liblagom_ffi.so")
        library.parent.mkdir(parents=True, exist_ok=True)
        library.write_bytes(b"fixture")
else:
    binding = pathlib.Path(args[args.index("--out-dir") + 1], "app/lagommail/core/lagom_ffi.kt")
    binding.parent.mkdir(parents=True, exist_ok=True)
    binding.write_text("// fixture")
''')
        cargo.chmod(0o755)

    def run_builder(self):
        env = dict(os.environ, ANDROID_HOME=str(self.sdk), PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}")
        return subprocess.run(["bash", "scripts/build-android.sh"], cwd=self.repo, env=env, capture_output=True, text=True)

    def test_newest_numeric_ndk_and_both_abis(self):
        for version in ("9.1.0", "10.2.0", "10.1.99", "preview"):
            (self.sdk / "ndk" / version).mkdir(parents=True)
        result = self.run_builder()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in (self.repo / "calls.jsonl").read_text().splitlines()]
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(Path(ndk).name == "10.2.0" for ndk, _ in calls))
        args = calls[0][1]
        self.assertEqual([args[i + 1] for i, value in enumerate(args) if value == "-t"], ["arm64-v8a", "x86_64"])
        self.assertEqual(args[args.index("--platform") + 1], "29")
        self.assertIn("--locked", args)
        self.assertEqual(calls[1][1][0], "run")
        self.assertIn("kotlin", calls[1][1])

    def test_missing_ndk_fails_before_cargo(self):
        result = self.run_builder()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Install an Android NDK", result.stderr)
        self.assertFalse((self.repo / "calls.jsonl").exists())


if __name__ == "__main__":
    unittest.main()

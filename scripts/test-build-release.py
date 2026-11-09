#!/usr/bin/env python3
"""Exercise the release wrapper with disposable config and a fake Cargo."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class BuildRelease(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        (self.repo / "scripts").mkdir()
        self.script = self.repo / "scripts/build-release.sh"
        shutil.copyfile(Path(__file__).with_name("build-release.sh"), self.script)
        cargo = self.repo / "cargo"
        cargo.write_text('''#!/usr/bin/env bash
set -eu
touch "$FIXTURE_REPO/cargo-ran"
[[ "$*" == 'build --release -p lagom' ]]
[[ "$PWD" == "$FIXTURE_REPO" ]]
[[ "$LAGOM_GOOGLE_CLIENT_ID" == 'fixture-google' ]]
[[ "$LAGOM_GOOGLE_CLIENT_SECRET" == 'fixture-secret' ]]
[[ "$LAGOM_MICROSOFT_CLIENT_ID" == 'fixture-microsoft' ]]
exit "${FIXTURE_EXIT:-0}"
''')
        cargo.chmod(0o755)
        self.env = dict(os.environ, PATH=f"{self.repo}:{os.environ['PATH']}", FIXTURE_REPO=str(self.repo))
        for key in list(self.env):
            if key.startswith("LAGOM_"):
                self.env.pop(key)

    def config(self, path):
        path.write_text("LAGOM_GOOGLE_CLIENT_ID='fixture-google'\n"
                        "LAGOM_GOOGLE_CLIENT_SECRET='fixture-secret'\n"
                        "LAGOM_MICROSOFT_CLIENT_ID='fixture-microsoft'\n"
                        "echo fixture-secret\n")

    def run_script(self, *args, trace=False):
        result = subprocess.run(["bash", *(["-x"] if trace else []), str(self.script), *args],
                                cwd=self.repo / "scripts", env=self.env, text=True, capture_output=True)
        self.assertNotIn("fixture-secret", result.stdout + result.stderr)
        self.assertNotIn("fixture-google", result.stdout + result.stderr)
        self.assertNotIn("fixture-microsoft", result.stdout + result.stderr)
        return result

    def test_default_config_is_exported_without_logging(self):
        self.config(self.repo / ".env.oauth")
        self.assertEqual(self.run_script(trace=True).returncode, 0)

    def test_explicit_relative_path_with_spaces_and_cargo_failure(self):
        self.config(self.repo / "local config.env")
        self.env["FIXTURE_EXIT"] = "17"
        self.assertEqual(self.run_script("../local config.env").returncode, 17)

    def test_missing_config_fails_before_cargo(self):
        self.assertIn("OAuth env file not found", self.run_script().stderr)
        self.assertEqual(self.run_script().returncode, 1)

    def test_invalid_config_and_extra_arguments_fail(self):
        (self.repo / ".env.oauth").write_text("return 1\n")
        self.assertEqual(self.run_script().returncode, 1)
        self.assertEqual(self.run_script("one", "two").returncode, 2)

    def test_bad_assignment_before_valid_lines_fails_before_cargo(self):
        config = self.repo / ".env.oauth"
        self.config(config)
        config.write_text(config.read_text().replace("LAGOM_GOOGLE_CLIENT_ID=", "LAGOM_GOOGLE_CLIENT_ID = "))
        result = self.run_script()
        self.assertFalse((self.repo / "cargo-ran").exists(), "invalid config must stop before Cargo")
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

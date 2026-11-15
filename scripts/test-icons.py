#!/usr/bin/env python3
"""Exercise icon discovery, byte copying, safe pruning and read-only drift checks."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class IconsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.desktop = self.repo / "apps/desktop/assets/icons"
        self.sources = self.repo / "apps/mobile/apple/Sources"
        self.ios = self.repo / "apps/mobile/apple/Assets.xcassets/Icons"
        self.android = self.repo / "apps/mobile/android/app/src/main/res/drawable"
        for path in (self.desktop, self.sources, self.ios, self.android, self.repo / "scripts"):
            path.mkdir(parents=True)
        for name in ("ios", "android"):
            shutil.copyfile(Path(__file__).with_name(f"{name}-icons.py"),
                            self.repo / f"scripts/{name}-icons.py")
        self.svg = b'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M0,0h16" stroke="black"/></svg>\r\n'

    def run_icons(self, platform="ios", check=False):
        return subprocess.run([sys.executable, f"scripts/{platform}-icons.py"] + (["--check"] if check else []),
                              cwd=self.repo, capture_output=True, text=True)

    def snapshot(self):
        return {p.relative_to(self.repo): p.read_bytes() for p in self.repo.rglob("*") if p.is_file()}

    def test_discovery_and_exact_assets(self):
        (self.sources / "Views.swift").write_text('''
Icon("mail", label: "Not an icon")
Icon(expanded ? "chevron" :
    "chevron-right")
HouseButton(title: "Not an icon", icon: unread ? "read" :
    "unread")
var icon: String {
    switch self {
    case .sent: "sent"
    case .drafts: "compose"
    }
}
Icon(page.icon)
func wrapper(icon: String) { Icon(icon) }
struct Label { let icon: String; let title = "Not an icon" }
// Icon("ignored")
/* Button(icon: "ignored-too") */
let url = "https://example.com"
''')
        names = {"mail", "chevron", "chevron-right", "read", "unread", "sent", "compose"}
        for name in names:
            (self.desktop / f"{name}.svg").write_bytes(self.svg)
        self.assertEqual(self.run_icons().returncode, 0)
        self.assertEqual({p.stem for p in self.ios.iterdir()}, names)
        for name in names:
            imageset = self.ios / f"{name}.imageset"
            self.assertEqual((imageset / f"{name}.svg").read_bytes(), self.svg)
            metadata = json.loads((imageset / "Contents.json").read_text())
            self.assertEqual(metadata["images"], [{"filename": f"{name}.svg", "idiom": "universal"}])
            self.assertEqual(metadata["properties"], {"preserves-vector-representation": True,
                                                      "template-rendering-intent": "template"})
        self.assertEqual(self.run_icons(check=True).returncode, 0)

    def test_missing_svg_fails_before_writing(self):
        (self.sources / "View.swift").write_text('Icon("mail")\nButton(icon: "missing")')
        (self.desktop / "mail.svg").write_bytes(self.svg)
        before = self.snapshot()
        for check in (False, True):
            result = self.run_icons(check=check)
            self.assertEqual(result.returncode, 1)
            self.assertIn("Missing desktop SVGs", result.stderr)
            self.assertIn("missing", result.stderr)
            self.assertEqual(self.snapshot(), before)

    def test_ios_drift_and_pruning(self):
        (self.sources / "View.swift").write_text('Icon("mail")')
        (self.desktop / "mail.svg").write_bytes(self.svg)
        for name in ("unused", "custom"):
            imageset = self.ios / f"{name}.imageset"
            imageset.mkdir()
            (imageset / "keep.txt").write_text(name)
        (self.desktop / "unused.svg").write_bytes(self.svg)
        before = self.snapshot()
        result = self.run_icons(check=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Unused:", result.stdout)
        self.assertEqual(self.snapshot(), before)
        self.assertIn("Removing:", self.run_icons().stdout)
        self.assertFalse((self.ios / "unused.imageset").exists())
        self.assertTrue((self.ios / "custom.imageset/keep.txt").exists())
        for filename in ("mail.svg", "Contents.json"):
            path = self.ios / "mail.imageset" / filename
            path.write_bytes(b"stale")
            before = self.snapshot()
            self.assertEqual(self.run_icons(check=True).returncode, 1)
            self.assertEqual(self.snapshot(), before)
            self.assertEqual(self.run_icons().returncode, 0)
            self.assertEqual(self.run_icons(check=True).returncode, 0)

    def test_android_missing_and_stale(self):
        (self.desktop / "cloud-synced.svg").write_bytes(self.svg)
        before = self.snapshot()
        self.assertEqual(self.run_icons("android", check=True).returncode, 1)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.run_icons("android").returncode, 0)
        path = self.android / "ic_cloud_synced.xml"
        self.assertIn('android:pathData="M0,0h16"', path.read_text())
        self.assertEqual(self.run_icons("android", check=True).returncode, 0)
        path.write_bytes(b"stale")
        before = self.snapshot()
        self.assertEqual(self.run_icons("android", check=True).returncode, 1)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.run_icons("android").returncode, 0)
        self.assertEqual(self.run_icons("android", check=True).returncode, 0)


if __name__ == "__main__":
    unittest.main()

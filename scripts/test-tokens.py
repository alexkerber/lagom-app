#!/usr/bin/env python3
"""Check token propagation, preserved platform values and safe block-only writes."""
import contextlib
import copy
import importlib.util
import io
from pathlib import Path
import tempfile
import tomllib
import unittest

spec = importlib.util.spec_from_file_location("tokens", Path(__file__).with_name("tokens.py"))
tokens = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tokens)


class Tokens(unittest.TestCase):
    def setUp(self):
        self.source = (tokens.ROOT / "design/tokens.toml").read_bytes()
        self.data = tomllib.loads(self.source.decode())

    def test_existing_mobile_values_and_platform_spring_units(self):
        for platform in ("ios", "android"):
            sizes = tokens.mobile_sizes(self.data, platform)
            self.assertEqual([sizes[key] for key in tokens.MOBILE_SIZES], [13, 14, 15, 16, 17, 23, 30, 56])
            self.assertEqual(tokens.mobile_colors(self.data, platform, "dark")["sel_icon"], 0x94ADDC)
        self.assertEqual(tokens.mobile_sizes(self.data, "android")["display"], 30)
        rendered = tokens.generated(self.data)
        self.assertIn("SPRING: Duration = Duration::from_millis(280)", rendered[tokens.DESKTOP])
        self.assertIn("ZETA: f32 = 0.85", rendered[tokens.DESKTOP])
        self.assertIn("OMEGA: f32 = 5.5", rendered[tokens.DESKTOP])
        self.assertIn("Animation.spring(duration: 0.28, bounce: 0.15)", rendered[tokens.IOS])
        self.assertIn("const val damping = 0.75f", rendered[tokens.ANDROID])
        self.assertIn("const val stiffness = 700f", rendered[tokens.ANDROID])
        # One face list for every platform: family names, PostScript names, resource names and tracking.
        self.assertIn('pub const DISPLAY: &str = "Hauora";', rendered[tokens.DESKTOP])
        self.assertIn("DISPLAY_TRACKING: i16 = -45;", rendered[tokens.DESKTOP])
        self.assertIn('static let label = "GeistMono-Medium"', rendered[tokens.IOS])
        self.assertIn("Font(R.font.hauora_bold, FontWeight.Bold)", rendered[tokens.ANDROID])
        self.assertIn('pub const TEXT: &str = "Hauora";', rendered[tokens.DESKTOP])
        self.assertIn('static let text = "Hauora"', rendered[tokens.IOS])
        self.assertIn("TextFace = FontFamily(Font(R.font.hauora_regular, FontWeight.Normal), Font(R.font.hauora_medium, FontWeight.Medium)", rendered[tokens.ANDROID])
        self.assertIn("--display-tracking: -0.045em;", rendered[tokens.SITE])

    def test_shared_edits_propagate_and_overrides_win(self):
        changed = copy.deepcopy(self.data)
        changed["colors"]["light"]["bg"] = 0x123456
        changed["radii"]["control"] = 9
        changed["type"]["mobile_factor"] = 1.2
        changed["motion"]["ms"]["calm"] = 175
        output = tokens.generated(changed)
        for path, block in output.items():
            self.assertIn("123456", block)
            if path not in tokens.SITE_PAGES:
                self.assertIn("175", block)
        self.assertIn("--radius: 9px", output[tokens.SITE])
        self.assertIn("CONTROL: f32 = 9.0", output[tokens.DESKTOP])
        self.assertIn("control: CGFloat = 9", output[tokens.IOS])
        self.assertIn("control = 9.dp", output[tokens.ANDROID])
        for platform in ("ios", "android"):
            self.assertEqual(tokens.mobile_sizes(changed, platform)["subject"], 24)
            self.assertEqual(tokens.mobile_sizes(changed, platform)["small"], 13)

    def test_check_is_read_only_generation_is_idempotent_and_preserves_surroundings(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            root = Path(directory)
            (root / "design").mkdir()
            (root / "design/tokens.toml").write_bytes(self.source)
            before, after = "// hand-written café\r\n", "\n// keep this too\r\n"
            stale = {path: (before + "\nstale\n".join(tokens.markers(path)) + after).encode() for path in tokens.generated(self.data)}
            original = stale[tokens.DESKTOP]
            for path in tokens.generated(self.data):
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_bytes(stale[path])
            self.assertEqual(tokens.generate(root, check=True), 1)
            for path in tokens.generated(self.data):
                self.assertEqual((root / path).read_bytes(), stale[path])
            self.assertEqual(tokens.generate(root), 0)
            self.assertEqual(tokens.generate(root, check=True), 0)
            snapshots = {path: (root / path).read_bytes() for path in tokens.generated(self.data)}
            self.assertEqual(tokens.generate(root), 0)
            for path, content in snapshots.items():
                self.assertTrue(content.startswith(before.encode()))
                self.assertTrue(content.endswith(after.encode()))
                self.assertEqual((root / path).read_bytes(), content)
            # A broken marker in the last file must not partially update earlier files.
            (root / tokens.DESKTOP).write_bytes(original)
            (root / tokens.ANDROID).write_text("no markers\n")
            with self.assertRaises(ValueError):
                tokens.generate(root)
            self.assertEqual((root / tokens.DESKTOP).read_bytes(), original)

    def test_bad_markers_fail_closed(self):
        for source in ("", tokens.END + "\n" + tokens.BEGIN,
                       "\n" + tokens.BEGIN + "\n" + tokens.BEGIN + "\n" + tokens.END + "\n",
                       "\n" + tokens.BEGIN + " inline\n" + tokens.END + "\n"):
            with self.subTest(source=source), self.assertRaises(ValueError):
                tokens.replace_block(source, "new")

    def test_windows_line_endings(self):
        source = "// before\r\n" + tokens.BEGIN + "\r\nold\r\n" + tokens.END + "\r\n// after\r\n"
        updated = tokens.replace_block(source, "first\nsecond")
        self.assertEqual(updated, source.replace("old", "first\r\nsecond"))
        self.assertEqual(tokens.replace_block(updated, "first\nsecond"), updated)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Generate native design tokens from design/tokens.toml; --check never writes."""
import argparse
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BEGIN = "// BEGIN GENERATED TOKENS (scripts/tokens.py)"
END = "// END GENERATED TOKENS"
DESKTOP = Path("apps/desktop/src/theme.rs")
IOS = Path("apps/mobile/apple/Sources/Theme.swift")
ANDROID = Path("apps/mobile/android/app/src/main/java/app/lagommail/android/Theme.kt")
SITE = Path("site/index.html")
SITE_PAGES = (SITE, Path("site/privacy.html"), Path("site/support.html"))
MOBILE_COLORS = {
    "bg": ("background", "bg"), "subtle": ("subtle", "subtle"),
    "raised": ("raised", "raised"), "ink": ("ink", "ink"),
    "ink2": ("secondary", "ink2"), "line": ("line", "line"),
    "sel": ("selected", "sel"), "sel_focus": ("selection", "selFocus"),
    "sel_icon": ("selectionIcon", "selIcon"), "accent": ("accent", "accent"),
    "accent_ink": ("accentInk", "accentInk"), "done": ("done", "done"),
    "later": ("later", "later"), "danger": ("danger", "danger"),
    "sidebar": ("sidebar", "sidebar"),
}
MOBILE_SIZES = ("small", "snippet", "base", "body", "title", "subject")


def number(value):
    return f"{value:g}"


def mobile_sizes(tokens, platform):
    sizes = {key: value * tokens["type"]["mobile_factor"] for key, value in tokens["type"]["sizes"].items()}
    sizes.update(tokens["overrides"][platform]["type_sizes"])
    return sizes


def mobile_colors(tokens, platform, mode):
    colors = dict(tokens["colors"][mode])
    accent = tokens["colors"]["light"]["accent"]
    colors["sel_focus"] = accent
    # The native desktop keeps this half lift in floating point; mobile uses RGB bytes.
    colors["sel_icon"] = accent if mode == "light" else sum(
        ((accent >> shift & 255) + 256) // 2 << shift for shift in (16, 8, 0)
    )
    colors.update(tokens["overrides"][platform]["colors"].get(mode, {}))
    return colors


def rust(tokens):
    colors = tokens["colors"]
    out = ["impl Accent {", "    /// (light, dark)", "    fn hex(self) -> (u32, u32) {", "        match self {"]
    for variant, key in (("Dusk", "accent"), ("PineGreen", "done"), ("Gold", "later")):
        out.append(f"            Accent::{variant} => (0x{colors['light'][key]:06X}, 0x{colors['dark'][key]:06X}),")
    out += ["        }", "    }", "}", "", "impl Theme {"]
    for index, mode in enumerate(("light", "dark")):
        c = colors[mode]
        out += [f"    pub fn {mode}(accent: Accent) -> Self {{", "        Self {", f"            dark: {str(mode == 'dark').lower()},"]
        for key in ("bg", "subtle", "raised", "ink", "ink2", "line", "sel"):
            out.append(f"            {key}: rgb(0x{c[key]:06X}),")
        out += ["            sel_focus: rgb(accent.selection()),", "            sel_icon: rgb(accent.selection()),",
                f"            accent: rgb(accent.hex().{index}),", f"            accent_ink: rgb(0x{c['accent_ink']:06X}),",
                f"            swipe_done: rgb(Accent::PineGreen.hex().{index}),", f"            swipe_later: rgb(Accent::Gold.hex().{index}),",
                f"            danger: rgb(0x{c['danger']:06X}),",
                f"            press: Rgba {{ a: {c['press_alpha']}, ..rgb(0x{c['ink']:06X}) }},", "        }", "    }"]
    out += ["}", "", "fn accent_fill(value: i32) -> Option<u32> {", "    Some(match value {"]
    for index, key in enumerate(("graphite", "red", "orange", "yellow", "green", "blue", "purple", "pink"), -1):
        out.append(f"        {index} => 0x{tokens['system_selection'][key]:06X}, // {key}")
    out += ["        _ => return None,", "    })", "}", ""]
    for mode in ("light", "dark"):
        out.append(f"pub const SIDEBAR_{mode.upper()}: u32 = 0x{colors[mode]['sidebar']:06X};")
    for module, values in (("size", tokens["type"]["sizes"]), ("radius", tokens["radii"])):
        out += ["", f"pub mod {module} {{"]
        out += [f"    pub const {key.upper()}: f32 = {float(value)};" for key, value in values.items()]
        out.append("}")
    out += ["", f"pub const LEADING: f32 = {tokens['type']['leading']};",
            f"pub const READING_LEADING: f32 = {tokens['type']['reading_leading']};",
            "", "pub mod motion {", "    use std::time::Duration;"]
    out += [f"    pub const {key.upper()}: Duration = Duration::from_millis({value});" for key, value in tokens["motion"]["ms"].items()]
    out += [f"    pub const ZETA: f32 = {tokens['motion']['spring']['damping']};",
            f"    pub const OMEGA: f32 = {tokens['motion']['spring']['omega']};", "}"]
    return "\n".join(out)


def swift(tokens):
    light, dark = (mobile_colors(tokens, "ios", mode) for mode in ("light", "dark"))
    out = ["extension Theme {"]
    out += [f"    static let {names[0]} = color(0x{light[key]:06X}, 0x{dark[key]:06X})" for key, names in MOBILE_COLORS.items()]
    out += ["", "    enum Radius {"]
    out += [f"        static let {key}: CGFloat = {number(value)}" for key, value in tokens["radii"].items()]
    out += ["    }", "}", "", "extension TextSize {", "    var points: CGFloat {", "        switch self {"]
    sizes = mobile_sizes(tokens, "ios")
    out += [f"        case .{key}: {number(sizes[key])}" for key in MOBILE_SIZES]
    ms = tokens["motion"]["ms"]
    out += ["        }", "    }", "}", "", "extension Motion {",
            f"    static let spring = Animation.spring(duration: {number(ms['spring'] / 1000)}, bounce: {tokens['overrides']['ios']['spring']['bounce']})",
            f"    static let fade = Animation.easeOut(duration: {number(ms['fade'] / 1000)})",
            f"    static let calm = Duration.milliseconds({ms['calm']})",
            f"    static let calmSeconds: Double = {number(ms['calm'] / 1000)}", "}"]
    return "\n".join(out)


def kotlin(tokens):
    out = []
    for mode in ("light", "dark"):
        colors = mobile_colors(tokens, "android", mode)
        out.append(f"internal val {mode.title()}Colors = LagomColors(")
        out += [f"    {names[1]} = Color(0xFF{colors[key]:06X})," for key, names in MOBILE_COLORS.items()]
        out += [")", ""]
    out.append("internal object TextSize {")
    sizes = mobile_sizes(tokens, "android")
    out += [f"    val {key} = {number(sizes[key])}.sp" for key in (*MOBILE_SIZES, "display")]
    out += ["}", "", "internal object Radius {"]
    out += [f"    val {key} = {number(value)}.dp" for key, value in tokens["radii"].items()]
    out += ["}", ""]
    typography = tokens["type"] | tokens["overrides"]["android"]["type"]
    out += [f"internal const val Leading = {typography['leading']}f",
            f"internal const val ReadingLeading = {typography['reading_leading']}f", "", "internal object MotionTokens {"]
    out += [f"    const val {key} = {tokens['motion']['ms'][key]}" for key in ("calm", "fade", "empty", "pulse")]
    spring = tokens["motion"]["spring"] | tokens["overrides"]["android"]["spring"]
    out += [f"    const val damping = {spring['damping']}f", f"    const val stiffness = {number(spring['stiffness'])}f", "}"]
    return "\n".join(out)


def css(tokens):
    # The site is light only; its one colour is the app icon, so the focus ring stays ink, not accent.
    c = tokens["colors"]["light"]
    hexes = {name: f"#{c[key]:06X}" for name, key in (("bg", "bg"), ("card", "subtle"), ("ink", "ink"), ("ink2", "ink2"), ("line", "line"))}
    return "      " + " ".join(f"--{name}: {value};" for name, value in hexes.items()) + f"\n      --accent: {hexes['ink']}; --radius: {number(tokens['radii']['control'])}px;"


def generated(tokens):
    return {DESKTOP: rust(tokens), IOS: swift(tokens), ANDROID: kotlin(tokens)} | dict.fromkeys(SITE_PAGES, css(tokens))


def markers(path):
    """CSS has no line comments, so the site wraps the same markers in /* */."""
    return (f"/* {BEGIN} */", f"/* {END} */") if Path(path).suffix == ".html" else (BEGIN, END)


def replace_block(source, block, begin=BEGIN, end=END):
    if source.count(begin) != 1 or source.count(end) != 1:
        raise ValueError("expected exactly one pair of generated-token markers")
    before, rest = source.split(begin)
    old, after = rest.split(end)
    newline = "\r\n" if old.startswith("\r\n") else "\n"
    if not before.endswith("\n") or not old.startswith(newline) or not old.endswith(newline) or not after.startswith(newline):
        raise ValueError("generated-token markers must be on their own lines")
    return before + begin + newline + block.replace("\n", newline) + newline + end + after


def generate(root, check=False):
    with (root / "design/tokens.toml").open("rb") as source:
        tokens = tomllib.load(source)
    updates = {}
    # Validate every marker before writing any file. Bytes preserve hand-written line endings.
    for path, block in generated(tokens).items():
        source = (root / path).read_bytes().decode("utf-8")
        try:
            updated = replace_block(source, block, *markers(path))
        except ValueError as error:
            raise ValueError(f"{path}: {error}") from error
        if source != updated:
            updates[path] = updated
    for path, updated in updates.items():
        print(f"{'out of date' if check else 'updated'}: {path}")
        if not check:
            (root / path).write_bytes(updated.encode("utf-8"))
    return 1 if check and updates else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="exit 1 if generated tokens differ; do not write")
    args = parser.parse_args()
    try:
        return generate(ROOT, args.check)
    except (OSError, ValueError, KeyError) as error:
        print(f"tokens: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())

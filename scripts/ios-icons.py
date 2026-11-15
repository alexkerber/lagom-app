#!/usr/bin/env python3
"""Copy desktop SVGs used by Swift into iOS imagesets. Run from the repository root.

Icon names belong in Icon(...), icon: arguments, or var icon: String properties,
including ternaries and switches. Keep names literal so missing SVGs can be checked.
"""
import argparse
import pathlib
import re
import shutil

SRC = pathlib.Path("apps/desktop/assets/icons")
SOURCES = pathlib.Path("apps/mobile/apple/Sources")
OUT = pathlib.Path("apps/mobile/apple/Assets.xcassets/Icons")
STRING = r'"(?:\\.|[^"\\])*"'


def used_icons():
    names = set()
    for source in sorted(SOURCES.rglob("*.swift")):
        # Preserve strings when stripping comments (including URLs inside strings).
        code = re.sub(STRING + r"|//[^\n]*|/\*.*?\*/",
                      lambda m: m[0] if m[0].startswith('"') else " ",
                      source.read_text(encoding="utf-8"), flags=re.S)
        # shortcut: literal expressions and one nested switch; extend when Swift adds other icon sources.
        uses = re.finditer(
            r"\bvar\s+icon\s*:\s*String\s*\{((?:[^{}]|\{[^{}]*\})*)\}"
            r"|\bIcon\s*\(\s*([^,)]*)"
            r"|\bicon\s*:(?!\s*String\b)\s*([^,)]*)", code)
        for use in uses:
            expression = next(group for group in use.groups() if group is not None)
            for literal in re.findall(STRING, expression):
                name = literal[1:-1]
                if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", name):
                    raise SystemExit(f"{source}: icon name must be a literal desktop SVG name: {literal}")
                names.add(name)
    if not names:
        raise SystemExit(f"No icon names found in {SOURCES}")
    missing = sorted(name for name in names if not (SRC / f"{name}.svg").is_file())
    if missing:
        raise SystemExit(f"Missing desktop SVGs in {SRC}: {', '.join(missing)}")
    return names


def contents(name):
    return ('{\n'
            f'  "images" : [ {{ "filename" : "{name}.svg", "idiom" : "universal" }} ],\n'
            '  "info" : { "author" : "xcode", "version" : 1 },\n'
            '  "properties" : { "preserves-vector-representation" : true, "template-rendering-intent" : "template" }\n'
            '}\n').encode("utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="report stale assets without writing")
    args = parser.parse_args()
    names = used_icons()  # Validate all names before changing any assets.
    stale = False
    for name in sorted(names):
        imageset = OUT / f"{name}.imageset"
        for filename, expected in ((f"{name}.svg", (SRC / f"{name}.svg").read_bytes()),
                                   ("Contents.json", contents(name))):
            path = imageset / filename
            if not path.is_file() or path.read_bytes() != expected:
                stale = True
                print(f"{'Out of date' if args.check else 'Writing'}: {path}")
                if not args.check:
                    imageset.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(expected)
    for imageset in sorted(OUT.glob("*.imageset")):
        if imageset.stem not in names and (SRC / f"{imageset.stem}.svg").is_file():
            stale = True
            print(f"{'Unused' if args.check else 'Removing'}: {imageset}")
            if not args.check:
                shutil.rmtree(imageset)
    if args.check and stale:
        raise SystemExit("Run python3 scripts/ios-icons.py to update iOS icons.")


if __name__ == "__main__":
    main()

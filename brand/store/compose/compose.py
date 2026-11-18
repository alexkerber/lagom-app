#!/usr/bin/env python3
"""Store screenshots: a caption over a device mockup on a brand background, rendered by headless Chrome.

    python3 compose.py slides.json OUT_DIR

slides.json is a list of {"name", "size": [w, h], "caption", "sub"?, "image", "style": "calm"|"glow"|"night",
"scale"? (share of the canvas height the image may take, default 0.74)}. Image paths are absolute or relative
to slides.json. Fonts come from site/fonts, so the captions match lagommail.app.
"""
import json
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FONTS = ROOT / "site" / "fonts"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

# Background, ink and secondary ink per style. calm and night are the site's tokens; glow is the app icon.
STYLES = {
    "calm": ("#F5F5F3", "#111110", "#5F5F5A"),
    "night": ("radial-gradient(120% 80% at 50% 100%, #2A2F45 0%, #161618 60%)", "#F5F5F3", "#A3A39C"),
    "glow": ("linear-gradient(180deg, #3F63D6 0%, #8F86D9 38%, #F0B9A0 72%, #F6CD5B 100%)", "#FFFFFF", "rgba(255,255,255,.82)"),
}


def page(slide, base):
    w, h = slide["size"]
    bg, ink, ink2 = STYLES[slide.get("style", "calm")]
    image = pathlib.Path(slide["image"])
    image = image if image.is_absolute() else base / image
    unit = min(w, h) / 100  # type scales with the short side, so phone, tablet and Mac read alike
    scale = slide.get("scale", 0.74)
    sub = f'<p class="sub">{slide["sub"]}</p>' if slide.get("sub") else ""
    return f"""<!doctype html><html><head><meta charset="utf-8"><style>
@font-face {{ font-family: Hauora; font-weight: 800; src: url("{(FONTS / 'hauora-sans-800.woff2').as_uri()}"); }}
@font-face {{ font-family: Hauora; font-weight: 500; src: url("{(FONTS / 'hauora-sans-500.woff2').as_uri()}"); }}
html, body {{ margin: 0; width: {w}px; height: {h}px; overflow: hidden; }}
body {{ background: {bg}; color: {ink}; font-family: Hauora; display: flex; flex-direction: column; align-items: center; }}
h1 {{ font-weight: 800; font-size: {unit * 8.4}px; line-height: 1.04; letter-spacing: -0.02em; text-align: center;
     margin: {h * 0.06}px {unit * 7}px 0; max-width: {w * 0.86}px; }}
.sub {{ font-weight: 500; font-size: {unit * 3.6}px; line-height: 1.3; color: {ink2}; text-align: center; margin: {unit * 2.4}px {unit * 8}px 0; }}
.shot {{ flex: 1; width: 100%; display: flex; align-items: flex-end; justify-content: center; min-height: 0; }}
.shot img {{ max-width: 94%; max-height: {h * scale}px; object-fit: contain; display: block; }}
</style></head><body><h1>{slide["caption"]}</h1>{sub}<div class="shot"><img src="{image.as_uri()}"></div></body></html>"""


def main():
    spec = pathlib.Path(sys.argv[1]).resolve()
    out = pathlib.Path(sys.argv[2]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for slide in json.loads(spec.read_text()):
            w, h = slide["size"]
            html = pathlib.Path(tmp) / f'{slide["name"]}.html'
            html.write_text(page(slide, spec.parent))
            png = out / f'{slide["name"]}.png'
            subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
                            "--allow-file-access-from-files", f"--window-size={w},{h}", f"--screenshot={png}", html.as_uri()],
                           check=True, capture_output=True, timeout=120)
            print(png)


if __name__ == "__main__":
    main()

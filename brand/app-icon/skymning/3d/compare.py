"""Build compare.png: reference (cropped to the plate) above our render, at 512, 128 and 32 px.

usage: python3 compare.py <reference.png> <icon.png> <out.png>   (needs rsvg-convert)
"""
import os
import shutil
import subprocess
import sys
import tempfile

ref, ours, out = sys.argv[1:4]
# reference plate spans x 316..1704, y 336..1724 of its 2048 px frame; crop so the
# plate fills 824/1024 of the tile, the same as our icon
side = 1388 * 1024 / 824
cx, cy = 1010, 1030


# librsvg only loads files next to the SVG, so stage everything in one folder
tmp = tempfile.mkdtemp()
shutil.copy(ref, os.path.join(tmp, "r.png"))
shutil.copy(ours, os.path.join(tmp, "o.png"))
r, o = "r.png", "o.png"
sizes, gap = (512, 128, 32), 24
W = sum(sizes) + gap * (len(sizes) + 1) + 90
H = 512 * 2 + gap * 3
parts = []
for row, (label, img, crop) in enumerate((("reference", r, True), ("render", o, False))):
    x, y = gap + 90, gap + row * (512 + gap)
    parts.append(f'<text x="{gap}" y="{y + 30}" font-family="Helvetica" font-size="16" fill="#444">{label}</text>')
    for s in sizes:
        if crop:
            parts.append(f'<svg x="{x}" y="{y}" width="{s}" height="{s}" viewBox="{cx - side / 2} {cy - side / 2} {side} {side}">'
                         f'<image href="{r}" width="2048" height="2048"/></svg>')
        else:
            parts.append(f'<image x="{x}" y="{y}" width="{s}" height="{s}" href="{img}"/>')
        x += s + gap
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">'
       f'<rect width="100%" height="100%" fill="#D2D4DD"/>' + "".join(parts) + "</svg>")
open(os.path.join(tmp, "c.svg"), "w").write(svg)
subprocess.run(["rsvg-convert", "-o", os.path.abspath(out), os.path.join(tmp, "c.svg")], check=True)
shutil.rmtree(tmp)

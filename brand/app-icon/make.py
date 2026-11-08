import os, subprocess
FALU, FALU_D = "#B03A2E", "#EB6A55"
PAPER, INK, INK_BG = "#F4EFE6", "#111110", "#1A1917"
def a(dark):  # level vial
    bg = INK_BG if dark else FALU; vial = FALU_D if dark else PAPER; bub = INK_BG if dark else FALU; tick = INK_BG if dark else INK
    return f'''<rect width="1024" height="1024" fill="{bg}"/>
<rect x="152" y="382" width="720" height="260" rx="130" fill="{vial}"/>
<circle cx="512" cy="512" r="84" fill="{bub}"/>
<rect x="378" y="422" width="28" height="180" rx="14" fill="{tick}"/>
<rect x="618" y="422" width="28" height="180" rx="14" fill="{tick}"/>'''
def b(dark):  # l on a level line
    bg = INK_BG if dark else PAPER; ink = PAPER if dark else INK; acc = FALU_D if dark else FALU
    return f'''<rect width="1024" height="1024" fill="{bg}"/>
<rect x="432" y="176" width="160" height="580" rx="80" fill="{ink}"/>
<rect x="432" y="640" width="160" height="116" fill="{ink}"/>
<rect x="192" y="756" width="640" height="104" rx="52" fill="{acc}"/>'''
def c(dark):  # tumbler filled to the line
    bg = INK_BG if dark else PAPER; ink = PAPER if dark else INK; acc = FALU_D if dark else FALU
    glass = "M284 208 L740 208 L684 816 L340 816 Z"
    return f'''<rect width="1024" height="1024" fill="{bg}"/>
<defs><clipPath id="g{int(dark)}"><path d="{glass}"/></clipPath></defs>
<rect x="0" y="512" width="1024" height="512" fill="{acc}" clip-path="url(#g{int(dark)})"/>
<path d="{glass}" fill="none" stroke="{ink}" stroke-width="64" stroke-linejoin="round"/>'''
icons = {"A-vattenpass": a, "B-l-pa-linjen": b, "C-laget-om": c}
def svg(body, rx=True):
    clip = '<clipPath id="sq"><rect width="1024" height="1024" rx="229"/></clipPath>' if rx else ''
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><defs>{clip}</defs><g {"clip-path=\"url(#sq)\"" if rx else ""}>{body}</g></svg>'
for name, f in icons.items():
    for dark in (False, True):
        tag = "dark" if dark else "light"
        open(f"{name}-{tag}.svg", "w").write(svg(f(dark), rx=False))   # full-bleed master (OS masks it)
# contact sheet: per concept, light + dark at 256, 64, 32, 16 on paper and ink strips
W, row = 1600, 340
parts = []
y = 40
labels = {"A-vattenpass": "A  Vattenpass", "B-l-pa-linjen": "B  l på linjen", "C-laget-om": "C  Laget om"}
for name, f in icons.items():
    parts.append(f'<text x="40" y="{y+20}" font-family="Helvetica" font-weight="700" font-size="28" fill="{INK}">{labels[name]}</text>')
    x = 40
    for dark in (False, True):
        for size in (256, 64, 32, 16):
            inner = svg(f(dark)).replace('<svg ', f'<svg x="{x}" y="{y+50}" width="{size}" height="{size}" ', 1).replace('id="sq"', f'id="sq{name[0]}{int(dark)}{size}"').replace('url(#sq)', f'url(#sq{name[0]}{int(dark)}{size})')
            parts.append(inner)
            x += size + 28
        x += 60
    y += row
sheet = f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{y}" viewBox="0 0 {W} {y}"><rect width="{W}" height="{y}" fill="#E9E4DA"/>{"".join(parts)}</svg>'
open("sheet.svg", "w").write(sheet)

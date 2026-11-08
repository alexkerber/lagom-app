#!/bin/bash
# Rebuild platform assets from the layered Skymning SVG masters on macOS.
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p brand/app-icon/export
mkdir -p target
scratch=$(mktemp -d "$PWD/target/icon-export.XXXXXX")
trap 'rm -f "$scratch"/*; rmdir "$scratch"' EXIT
python3 - "$scratch" <<'PYTHON'
import copy
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ET.register_namespace('', 'http://www.w3.org/2000/svg')
ns = '{http://www.w3.org/2000/svg}'
scratch = Path(sys.argv[1])
for variant, filename in [('light', 'icon.svg'), ('dark', 'icon-dark.svg'), ('tinted', 'icon-tinted.svg')]:
    root = ET.parse(Path('brand/app-icon/skymning') / filename).getroot()
    defs, *layers = list(root)
    background, foreground = layers[:2], layers[2:]
    assert defs.tag == ns + 'defs'
    assert [e.get('fill') for e in background] == ['url(#plate)', 'url(#sheen)']
    assert all(e.get('width') == '824' for e in background)
    for e in background:
        e.attrib.update(x='0', y='0', width='1024', height='1024', rx='0')
    def write(name, elements):
        output = ET.Element(root.tag, root.attrib)
        output.extend([copy.deepcopy(defs), *copy.deepcopy(elements)])
        ET.ElementTree(output).write(scratch / (name + '.svg'))
    write('ios-' + variant, background + foreground)
    if variant == 'light':
        write('android-background', background)
        write('foreground', foreground)
        silhouette = [copy.deepcopy(e) for e in foreground if e.get('fill') == 'url(#glass)']
        assert [e.tag for e in silhouette] == [ns + 'rect', ns + 'path']
        for e in silhouette:
            for key in ('stroke', 'stroke-opacity', 'stroke-width'):
                e.attrib.pop(key, None)
            e.set('fill', '#fff')
        write('monochrome', silhouette)
PYTHON
rsvg-convert -o "$scratch/mac.png" brand/app-icon/skymning/icon.svg
for asset in ios-light ios-dark ios-tinted android-background foreground monochrome; do
    rsvg-convert -o "$scratch/$asset.png" "$scratch/$asset.svg"
done
cat > "$scratch/export.swift" <<'SWIFT'
import AppKit
import ImageIO
import UniformTypeIdentifiers
let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let dest = root.appendingPathComponent("brand/app-icon/export")
let scratch = URL(fileURLWithPath: CommandLine.arguments[1])
func source(_ name: String) -> NSImage {
    NSImage(contentsOf: scratch.appendingPathComponent(name + ".png"))!
}
let light = source("mac")
let subject = source("foreground")
// Include the glow's nonzero alpha when fitting inside the circular safe zone.
let probe = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
    bytesPerRow: 4096, space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
probe.draw(subject.cgImage(forProposedRect: nil, context: nil, hints: nil)!,
    in: CGRect(x: 0, y: 0, width: 1024, height: 1024))
let pixels = probe.data!.assumingMemoryBound(to: UInt8.self)
var radius: Double = 1
for y in 0..<1024 { for x in 0..<1024 where pixels[(y * 1024 + x) * 4 + 3] > 0 {
    radius = max(radius, hypot(Double(x) - 511.5, Double(y) - 511.5))
}}
let adaptiveScale = CGFloat((1024 * 0.33 - 3) / radius)
assert(adaptiveScale > 0 && adaptiveScale <= 1)
func color(_ hex: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 255)/255, green: CGFloat((hex >> 8) & 255)/255, blue: CGFloat(hex & 255)/255, alpha: 1)
}
func png(_ image: NSImage, _ size: Int, _ name: String, background: NSColor? = nil, subjectScale: CGFloat = 1) throws {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let alpha: CGImageAlphaInfo = background == nil ? .premultipliedLast : .noneSkipLast
    let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
        bytesPerRow: size*4, space: space, bitmapInfo: alpha.rawValue)!
    context.interpolationQuality = .high
    let rect = CGRect(x: 0, y: 0, width: size, height: size)
    if let background { context.setFillColor(background.cgColor); context.fill(rect) }
    let source = image.cgImage(forProposedRect: nil, context: nil, hints: nil)!
    let inset = CGFloat(size) * (1-subjectScale)/2
    context.draw(source, in: rect.insetBy(dx: inset, dy: inset))
    let result = context.makeImage()!
    assert(result.width == size && result.alphaInfo == alpha)
    let output = CGImageDestinationCreateWithURL(dest.appendingPathComponent(name) as CFURL,
        UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(output, result, nil)
    assert(CGImageDestinationFinalize(output))
}
try FileManager.default.createDirectory(at: dest.appendingPathComponent("AppIcon.iconset"), withIntermediateDirectories: true)
for size in [16,32,128,256,512] {
    try png(light,size,"AppIcon.iconset/icon_\(size)x\(size).png")
    try png(light,size*2,"AppIcon.iconset/icon_\(size)x\(size)@2x.png")
}
for variant in ["light", "dark", "tinted"] {
    try png(source("ios-" + variant),1024,"ios-" + variant + ".png",background:color(0))
}
try png(subject,432,"android-foreground.png",subjectScale:adaptiveScale)
try png(source("monochrome"),432,"android-monochrome.png",subjectScale:adaptiveScale)
try png(source("android-background"),432,"android-background.png",background:color(0))
try png(source("ios-light"),512,"play-store-512.png",background:color(0))
try png(light,512,"lagom-512.png")
try png(light,32,"favicon-32.png")
try png(source("ios-light"),180,"apple-touch-icon-180.png",background:color(0))
for size in [16,24,32,48,64,128,256] { try png(light,size,"windows-\(size).png") }
SWIFT
swift "$scratch/export.swift" "$scratch"
iconutil -c icns brand/app-icon/export/AppIcon.iconset -o brand/app-icon/export/AppIcon.icns
python3 - <<'PY'
import struct
from pathlib import Path
folder=Path('brand/app-icon/export')
sizes=[16,24,32,48,64,128,256]
images=[(folder/f'windows-{s}.png').read_bytes() for s in sizes]
offset=6+16*len(sizes)
entries=[]
for size,data in zip(sizes,images):
    entries.append(struct.pack('<BBBBHHII',size%256,size%256,0,0,1,32,len(data),offset))
    offset+=len(data)
(folder/'lagom.ico').write_bytes(struct.pack('<HHH',0,1,len(sizes))+b''.join(entries)+b''.join(images))
assert (folder/'lagom.ico').read_bytes()[:6]==struct.pack('<HHH',0,1,7)
for size in sizes: (folder/f'windows-{size}.png').unlink()
PY
cp brand/app-icon/export/favicon-32.png site/favicon-32.png
cp brand/app-icon/export/apple-touch-icon-180.png site/apple-touch-icon-180.png

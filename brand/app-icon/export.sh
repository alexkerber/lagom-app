#!/bin/bash
# macOS: render with Blender first, then run this script to rebuild every platform asset.
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p brand/app-icon/export
scratch=$(mktemp -d)
trap 'rm -f "$scratch"/*; rmdir "$scratch"' EXIT
cat > "$scratch/export.swift" <<'SWIFT'
import AppKit
import ImageIO
import UniformTypeIdentifiers
let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let dest = root.appendingPathComponent("brand/app-icon/export")
let light = NSImage(contentsOf: root.appendingPathComponent("brand/app-icon/3d/icon-light.png"))!
let subject = NSImage(contentsOf: root.appendingPathComponent("brand/app-icon/3d/icon.png"))!
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
try png(subject,1024,"ios-light.png",background:color(0xEFEDE7))
try png(subject,1024,"ios-dark.png",background:color(0x303A35))
// Extra inset keeps the diagonal silhouette inside the circular 66% safe zone.
try png(subject,432,"android-foreground.png",subjectScale:0.85)
try png(subject,512,"play-store-512.png",background:color(0xEFEDE7))
try png(light,512,"lagom-512.png")
try png(light,32,"favicon-32.png")
try png(subject,180,"apple-touch-icon-180.png",background:color(0xEFEDE7))
for size in [16,24,32,48,64,128,256] { try png(light,size,"windows-\(size).png") }
SWIFT
swift "$scratch/export.swift"
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

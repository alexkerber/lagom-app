#!/bin/bash
# Rebuild platform assets from the layered Skymning Blender renders on macOS.
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p brand/app-icon/export
mkdir -p target
scratch=$(mktemp -d "$PWD/target/icon-export.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
cat > "$scratch/export.swift" <<'SWIFT'
import AppKit
import ImageIO
import UniformTypeIdentifiers
let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let dest = root.appendingPathComponent("brand/app-icon/export")
let scratch = URL(fileURLWithPath: CommandLine.arguments[1])
func source(_ name: String) -> NSImage {
    NSImage(contentsOf: root.appendingPathComponent("brand/app-icon/skymning/3d/" + name + ".png"))!
}
let light = source("icon-1024")
let subject = source("layer-envelope")
// Match the 72 dp visible area of Android's 108 dp adaptive canvas.
// Faint shadow tails may extend outside the 66 dp safe circle.
let probe = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
    bytesPerRow: 4096, space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
probe.draw(subject.cgImage(forProposedRect: nil, context: nil, hints: nil)!,
    in: CGRect(x: 0, y: 0, width: 1024, height: 1024))
let pixels = probe.data!.assumingMemoryBound(to: UInt8.self)
var radius: Double = 1
for y in 0..<1024 { for x in 0..<1024 where pixels[(y * 1024 + x) * 4 + 3] >= 32 {
    radius = max(radius, hypot(Double(x) - 511.5, Double(y) - 511.5))
}}
let adaptiveScale: CGFloat = 72.0 / 108.0
assert(radius * Double(adaptiveScale) < 1024 * (33.0 / 108.0) - 3)
func color(_ hex: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 255)/255, green: CGFloat((hex >> 8) & 255)/255, blue: CGFloat(hex & 255)/255, alpha: 1)
}
func composite(dark: Bool) -> NSImage {
    let context = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
        bytesPerRow: 4096, space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    let rect = CGRect(x: 0, y: 0, width: 1024, height: 1024)
    if dark {
        let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
            colors: [color(0x0C1A35).cgColor, color(0x2B1E26).cgColor] as CFArray,
            locations: [0, 1])!
        context.drawLinearGradient(gradient, start: CGPoint(x: 512, y: 1024),
            end: CGPoint(x: 512, y: 0), options: [])
    } else {
        context.draw(source("layer-bg").cgImage(forProposedRect: nil, context: nil, hints: nil)!, in: rect)
    }
    context.draw(subject.cgImage(forProposedRect: nil, context: nil, hints: nil)!, in: rect)
    return NSImage(cgImage: context.makeImage()!, size: NSSize(width: 1024, height: 1024))
}
// Preserve the render's alpha coverage while replacing its colour with white.
for i in stride(from: 0, to: 1024 * 1024 * 4, by: 4) {
    pixels[i] = pixels[i+3]; pixels[i+1] = pixels[i+3]; pixels[i+2] = pixels[i+3]
}
let silhouette = NSImage(cgImage: probe.makeImage()!, size: NSSize(width: 1024, height: 1024))
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
try png(composite(dark: false),1024,"ios-light.png",background:color(0))
try png(composite(dark: true),1024,"ios-dark.png",background:color(0))
try png(silhouette,1024,"ios-tinted.png")
try png(subject,432,"android-foreground.png",subjectScale:adaptiveScale)
try png(silhouette,432,"android-monochrome.png",subjectScale:adaptiveScale)
try png(source("layer-bg"),432,"android-background.png",background:color(0))
try png(light,512,"play-store-512.png")
try png(light,512,"lagom-512.png")
try png(light,32,"favicon-32.png")
try png(light,180,"apple-touch-icon-180.png")
for size in [16,24,32,48,64,128,256] { try png(light,size,"windows-\(size).png") }
SWIFT
# Keep the Swift interpreter's temp files and module cache inside the scratch folder.
TMPDIR="$scratch" swift -module-cache-path "$scratch/module-cache" "$scratch/export.swift" "$scratch"
# Lossless recompression keeps the source renders and duplicated platform sizes small.
python3 - <<'PNG'
from pathlib import Path
import struct
import zlib

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

for path in [*Path('brand/app-icon/export').rglob('*.png'),
             *Path('brand/app-icon/skymning/3d').glob('*.png')]:
    original = path.read_bytes()
    assert original[:8] == b'\x89PNG\r\n\x1a\n'
    chunks, compressed, pos = [], b'', 8
    while pos < len(original):
        size = struct.unpack_from('>I', original, pos)[0]
        kind = original[pos+4:pos+8]
        data = original[pos+8:pos+8+size]
        assert zlib.crc32(kind + data) == struct.unpack_from('>I', original, pos+8+size)[0]
        chunks.append((kind, data))
        if kind == b'IDAT': compressed += data
        pos += size + 12
    raw = zlib.decompress(compressed)
    packed = zlib.compress(raw, 9)
    assert zlib.decompress(packed) == raw
    result, written = original[:8], False
    for kind, data in chunks:
        if kind == b'IDAT':
            if written: continue
            data, written = packed, True
        result += chunk(kind, data)
    if len(result) < len(original): path.write_bytes(result)
PNG
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

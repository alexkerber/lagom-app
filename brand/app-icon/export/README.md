# App icon exports

Approved source: `../skymning/3d/icon-1024.png`, rendered by `skymning.py`.
Rebuild exports from the repository root with `bash brand/app-icon/export.sh`.
Requires macOS, Swift/AppKit, iconutil and Python 3. Temporary files stay under
ignored `target/`. No Blender render is required when exporting existing layers.
PNG data is recompressed losslessly, including source renders; pixels are unchanged.

macOS (`AppIcon.icns` and `AppIcon.iconset`), Windows (`lagom.ico`), Linux and
the desktop runtime (`lagom-512.png`), favicon, Apple touch icon and Play Store
use the approved composite, including its rounded plate and transparency.
The website copies are also updated by the exporter.

`ios-light.png` composites `layer-envelope.png` over the full-bleed `layer-bg.png`.
`ios-dark.png` uses the same envelope over a #0C1A35 → #2B1E26 gradient: night navy to a warm dark.
Both are opaque 1024 px squares without baked-in rounding.
`ios-tinted.png` is a white silhouette derived from the envelope layer's alpha,
on transparency, as requested; it is a tint input, not an opaque App Store icon.

Android layers are 432 px: `android-background.png` uses the full-bleed background;
`android-foreground.png` scales the envelope and glow by 72/108, making the envelope about 37% of
the canvas width. The envelope fits the 66 dp diameter safe circle on a 108 dp
canvas; faint shadow tails may extend outside that circle. `android-monochrome.png` uses the same placement
and alpha coverage with white colour on transparency.

The iconset covers 16–1024 px including @2x. ICO sizes: 16, 24, 32, 48, 64, 128, 256.

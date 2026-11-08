# App icon exports

Source: `../skymning/icon.svg`, with `icon-dark.svg` and `icon-tinted.svg` variants.
Rebuild from the repository root with `bash brand/app-icon/export.sh`.
Requires macOS, Swift/AppKit, iconutil, Python 3 and rsvg-convert (Homebrew librsvg).
Temporary files stay under the ignored `target/` directory.

The first two SVG rectangles are the background; subsequent shapes are the
separate envelope, shadow and glow. The exporter checks this structure before
extracting layers. iOS light/dark/tinted, Play Store and Apple touch icons have
opaque full-bleed backgrounds, with no baked-in rounding. macOS, Windows,
Linux and favicon retain the rounded tile from the light master.

Android adaptive layers are 432 × 432: `android-background.png` contains the
full-bleed gradient; `android-foreground.png` contains the envelope and glow.
All visible foreground pixels fit inside the central 66% diameter circle.
`android-monochrome.png` uses the same placement with a white envelope
silhouette on transparency. The background gradient runs from #4A74C6 through
#5860BC to #935FCB, with the master's subtle white sheen.

`AppIcon.iconset` contains 16–1024 px assets including @2x; `lagom.ico` contains
16, 24, 32, 48, 64, 128 and 256 px images. The desktop embeds `lagom-512.png`.
The exporter also updates the two matching assets in `site/`.
Historical Blender and concept sources remain in `../3d/` and `../concepts/`;
they are not inputs to this export.

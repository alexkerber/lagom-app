# Hero icon studies

- **A — Vattenpass:** luminous emerald liquid and a centered air pocket in a clear vial: exactly enough, exactly aligned.
- **B — Glaskuvert:** a frosted glass envelope with an amber-lit flap seam: incoming work presented as a warm invitation.
- **C — Stillpunkt:** one quiet pearl inside an open rose glass orbit: one thing in focus, with room around it.

## Layer plan

| Concept | Background | Foreground layers | Total |
| --- | --- | --- | --- |
| A | Midnight blue | Optical glass with integrated marks; emerald liquid with an air cavity; luminous air interface | 4 |
| B | Deep plum | Frosted envelope body; warm interior; translucent flap | 4 |
| C | Deep aubergine | Open rose glass orbit; glass pearl; warm pearl core | 4 |

The Blender source tags each visible object with `icon_layer` and asserts a maximum of four visible objects including the background. Studio lights and camera are not artwork layers. No downloaded textures or environments are used.

The small-size silhouette is the primary design; refractive highlights provide depth. Later native adaptations can preserve the vial/bubble, envelope/seam, and open-ring/pearl silhouettes. These files are rendered concept previews, not tested Icon Composer or Android resources.

## Rebuild

From the repository root, run `brand/app-icon/concepts/render-hero.sh`. It uses Blender 5.2, Cycles with 512 samples and adaptive sampling, Metal where available, up to 16 light bounces, and denoising. `swift` creates the single-hue 128 px previews; `rsvg-convert` builds the board. Everything runs without opening windows.

For one image, run Blender with the same script and append `A`, `B`, or `C` after the output directory. The board shows 256, 128 and 32 px images on light and dark Dock-like backgrounds plus one 128 px blue-gray tint preview per concept. Tint previews are luminance-based, not native platform exports.

## Reference research

[Michael Flarup's Liquid Glass discussion](https://www.flarup.email/p/through-the-liquid-glass) informed the emphasis on dimensionality without sacrificing legibility. [Apple's Icon Composer](https://developer.apple.com/icon-composer/) and [WWDC26 group lab](https://developer.apple.com/videos/play/wwdc2026/8012/) informed the small layer count and separation of artwork from lighting/material effects. The three concepts and their geometry were created for this study; no reference artwork is included.

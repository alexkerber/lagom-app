# App icon exports

Source: `../3d/icon.py`. From the repository root, render with:

```sh
/Applications/Blender.app/Contents/MacOS/Blender -b -P brand/app-icon/3d/icon.py -- brand/app-icon/3d/icon.png
bash brand/app-icon/export.sh
```

Export requires macOS with Swift/AppKit, iconutil and Python 3. No downloaded assets or Python packages are used.

Android adaptive background: `#EFEDE7`.

The transparent 432 px foreground keeps the subject inside the central 66% safe zone. iOS and Play Store exports are opaque, full-bleed squares; the platform supplies its own mask. macOS, Windows, Linux and favicon use the light rounded plate. The 32 px source preview also uses the light plate.

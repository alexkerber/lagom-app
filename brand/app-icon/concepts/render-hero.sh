#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
/Applications/Blender.app/Contents/MacOS/Blender -b -P brand/app-icon/concepts/render-hero.py -- brand/app-icon/concepts
swift brand/app-icon/concepts/hero-tint.swift brand/app-icon/concepts
rsvg-convert brand/app-icon/concepts/hero-board.svg -o brand/app-icon/concepts/hero-board.png

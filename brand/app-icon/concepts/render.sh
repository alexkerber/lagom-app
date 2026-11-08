#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
for i in 1 2 3; do
  rsvg-convert -w 1024 -h 1024 "astra-$i.svg" -o "astra-$i.png"
done
rsvg-convert astra-board.svg -o astra-board.png

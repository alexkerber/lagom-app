#!/usr/bin/env bash
# Prints a folder with Sparkle.framework and bin/ (sign_update, generate_keys) from the pinned
# Sparkle release. The only source is the project's GitHub releases. The checksum is checked on
# every run, also for a cached tarball. Bump both lines together.
set -euo pipefail
version=2.10.1
sha256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
target=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/target
tarball=$target/Sparkle-$version.tar.xz
mkdir -p "$target"
if [[ ! -f $tarball ]]; then
    curl -fsSL --proto '=https' -o "$tarball.part" \
        "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz"
    mv "$tarball.part" "$tarball"
fi
if ! echo "$sha256  $tarball" | shasum -a 256 -c - >/dev/null; then
    rm -f "$tarball"
    echo "Sparkle $version does not match its pinned checksum; deleted the download." >&2
    exit 1
fi
dir=$target/sparkle
rm -rf "$dir"
mkdir -p "$dir"
tar -xJf "$tarball" -C "$dir"
echo "$dir"

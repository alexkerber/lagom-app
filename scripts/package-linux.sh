#!/usr/bin/env bash
# Build lagom for Linux and pack it as a .deb and an AppImage in target/linux.
# Usage: scripts/package-linux.sh [version]
# Needs Debian or Ubuntu (dpkg-deb, dpkg-shlibdeps), curl and the build packages from
# .github/workflows/ci.yml. Downloads a pinned, hash-checked linuxdeploy for the AppImage.
# LAGOM_SKIP_BUILD=1 packs an existing release binary; LAGOM_DEB_MAINTAINER sets Maintainer.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
version=${1:-0.1.0}
if [[ ! $version =~ ^[0-9][0-9A-Za-z.+~]*$ ]]; then
    echo "Version must start with a digit, without a leading v: $version" >&2
    exit 2
fi
id=app.lagommail.desktop
out=$repo/target/linux
bin=${CARGO_TARGET_DIR:-$repo/target}/release/lagom
# shortcut: placeholder until lagom has a release address; set LAGOM_DEB_MAINTAINER for real releases.
maintainer=${LAGOM_DEB_MAINTAINER:-lagom <https://github.com/alexkerber/lagom-app/issues>}

if [[ ${LAGOM_SKIP_BUILD:-0} != 1 ]]; then
    if [[ -f .env.oauth ]]; then
        scripts/build-release.sh
    else
        echo 'warning: no .env.oauth, so Google and Microsoft sign-in are off in this build' >&2
        cargo build --release --locked -p lagom
    fi
fi

# One /usr tree for both packages.
rm -rf "$out"
root=$out/root
install -Dm755 -s "$bin" "$root/usr/bin/lagom"
install -Dm644 "packaging/linux/$id.desktop" "$root/usr/share/applications/$id.desktop"
for size in 16x16:16x16 32x32:32x32 64x64:32x32@2x 128x128:128x128 256x256:256x256 512x512:512x512; do
    install -Dm644 "brand/app-icon/export/AppIcon.iconset/icon_${size#*:}.png" \
        "$root/usr/share/icons/hicolor/${size%%:*}/apps/$id.png"
done
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$root/usr/share/applications/$id.desktop"
fi

# .deb, with the D-Bus service that lets a notification click start lagom.
arch=$(dpkg --print-architecture)
deb=$out/deb
cp -a "$root" "$deb"
install -Dm644 "packaging/linux/$id.service" "$deb/usr/share/dbus-1/services/$id.service"
# dpkg-shlibdeps reads debian/control in its working directory.
mkdir -p "$out/shlibs/debian"
: > "$out/shlibs/debian/control"
depends=$(cd "$out/shlibs" && dpkg-shlibdeps -O "$deb/usr/bin/lagom" | sed -n 's/^shlibs:Depends=//p')
size=$(du -sk "$deb" | cut -f1)
mkdir -p "$deb/DEBIAN"
# GPUI loads Vulkan and Wayland at run time, so dpkg-shlibdeps cannot see them.
cat > "$deb/DEBIAN/control" <<CONTROL
Package: lagom
Version: $version
Architecture: $arch
Maintainer: $maintainer
Installed-Size: $size
Depends: $depends, libvulkan1, libwayland-client0, libwayland-cursor0
Recommends: mesa-vulkan-drivers | vulkan-icd
Section: mail
Priority: optional
Homepage: https://github.com/alexkerber/lagom-app
Description: A fast, minimal, keyboard-first email client
 Mail, calendar and tasks. Made in Sweden. Open source.
CONTROL
dpkg-deb --build --root-owner-group "$deb" "$out/lagom_${version}_$arch.deb"

# AppImage. linuxdeploy bundles the libraries a desktop may lack and skips the base system ones.
# The runtime is the AppImage's own start code, so pin it too instead of appimagetool's latest.
machine=$(uname -m)
case $machine in
    x86_64)
        tool_sum=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
        runtime_sum=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64)
        tool_sum=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff
        runtime_sum=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
    *) echo "No pinned AppImage tools for $machine" >&2; exit 1 ;;
esac
# fetch <file> <url> <sha256>: downloads once into target/, checks the hash on every run.
fetch() {
    if ! echo "$3  $1" | sha256sum -c --status - 2>/dev/null; then
        curl -fsSL -o "$1" "$2"
        echo "$3  $1" | sha256sum -c -
    fi
}
tool=$repo/target/linuxdeploy-$machine.AppImage
runtime=$repo/target/appimage-runtime-$machine
fetch "$tool" "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-$machine.AppImage" "$tool_sum"
fetch "$runtime" "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$machine" "$runtime_sum"
chmod +x "$tool"
appdir=$out/AppDir
cp -a "$root" "$appdir"
rm -r "$appdir/usr/share/applications"
# CI runners and containers have no FUSE.
(cd "$out" && APPIMAGE_EXTRACT_AND_RUN=1 LINUXDEPLOY_OUTPUT_VERSION=$version LDAI_RUNTIME_FILE=$runtime \
    LDAI_OUTPUT="lagom-$version-$machine.AppImage" "$tool" --appdir "$appdir" \
    --desktop-file "$repo/packaging/linux/$id.desktop" \
    --icon-file "$root/usr/share/icons/hicolor/512x512/apps/$id.png" --output appimage)
ls -1 "$out"/*.deb "$out"/*.AppImage

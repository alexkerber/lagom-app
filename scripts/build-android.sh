#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
# Choose the newest installed NDK by its numeric revision, independent of directory order.
ANDROID_NDK_HOME=$(python3 - "$ANDROID_HOME" <<'PY'
import pathlib, sys
ndks = pathlib.Path(sys.argv[1], "ndk")
if not ndks.is_dir():
    sys.exit("Install an Android NDK in the SDK first")
versions = [p for p in ndks.iterdir() if p.is_dir() and all(n.isdigit() for n in p.name.split('.'))]
if not versions:
    sys.exit("Install an Android NDK in the SDK's ndk directory first")
print(max(versions, key=lambda p: tuple(map(int, p.name.split('.')))))
PY
)
export ANDROID_NDK_HOME
export CARGO_TARGET_DIR="$repo/target"
app="$repo/apps/mobile/android/app"
cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o "$app/src/main/jniLibs" \
    build --locked --release -p lagom-ffi --lib
cargo run --locked -p lagom-ffi --features bindgen --bin uniffi-bindgen -- \
    generate --library "$app/src/main/jniLibs/arm64-v8a/liblagom_ffi.so" \
    --language kotlin --out-dir "$app/build/generated/uniffi"
for abi in arm64-v8a x86_64; do test -s "$app/src/main/jniLibs/$abi/liblagom_ffi.so"; done
test -s "$app/build/generated/uniffi/app/lagommail/core/lagom_ffi.kt"

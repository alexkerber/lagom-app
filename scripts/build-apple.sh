#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
export IPHONEOS_DEPLOYMENT_TARGET=18.0
# Cargo and Xcode share only artifacts made by this script, never a simulator device.
export CARGO_TARGET_DIR="$repo/target"
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
    cargo build --locked --release -p lagom-ffi --lib --target "$target"
done
generated="$repo/apps/mobile/apple/Generated"
stage=$(mktemp -d "$repo/target/apple.XXXXXX")
mkdir -p "$generated" "$stage/headers"
cargo run --locked -p lagom-ffi --features bindgen --bin uniffi-bindgen -- \
    generate --library "$repo/target/aarch64-apple-ios/release/liblagom_ffi.a" \
    --language swift --out-dir "$stage/bindings"
cp "$stage/bindings/LagomFFI.h" "$stage/headers/"
cp "$stage/bindings/LagomFFI.modulemap" "$stage/headers/module.modulemap"
xcodebuild -create-xcframework \
    -library "$repo/target/aarch64-apple-ios/release/liblagom_ffi.a" -headers "$stage/headers" \
    -library "$repo/target/aarch64-apple-ios-sim/release/liblagom_ffi.a" -headers "$stage/headers" \
    -output "$stage/LagomFFI.xcframework"
ditto "$stage/LagomFFI.xcframework" "$generated/LagomFFI.xcframework"
cp "$stage/bindings/LagomCore.swift" "$generated/"

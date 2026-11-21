#!/bin/bash
# Unpack the Rust library and Swift bindings that CI's `rust` job built (scripts/build-apple.sh)
# into Generated/, so release.sh can archive with LAGOM_PREBUILT=1 and run no cargo.
# It checks the tar against the hash from the job that made it, allows only the files that
# build-apple.sh writes (plain files and folders, from the listing before extraction), and checks
# that the module map and header can make the compiler read no other file. Nothing in the tar runs.
# Usage: apps/mobile/apple/unpack-ffi.sh <lagom-ffi.tar> <sha256>
set -euo pipefail
usage="Usage: $0 <lagom-ffi.tar> <sha256>"
tar=${1:?$usage}
sha=${2:?$usage}
cd "$(dirname "$0")"
fail() { echo "The FFI artifact: $*" >&2; exit 1; }
[[ $sha =~ ^[0-9a-f]{64}$ ]] || fail "not a SHA-256: $sha"
echo "$sha  $tar" | shasum -a 256 -c -

# The listing in variables: `tar | grep -q` under pipefail can fail on SIGPIPE and skip a check.
names=$(tar -tf "$tar")
types=$(tar -tvf "$tar" | cut -c1 | sort -u | tr -d '\n')
[[ $types == - || $types == -d ]] || fail "holds links or special files (types: $types)"
slice='ios-arm64(-simulator)?'
allowed="^(LagomCore\.swift|LagomFFI\.xcframework/?|LagomFFI\.xcframework/Info\.plist|LagomFFI\.xcframework/$slice/?|LagomFFI\.xcframework/$slice/liblagom_ffi\.a|LagomFFI\.xcframework/$slice/Headers/?|LagomFFI\.xcframework/$slice/Headers/(LagomFFI\.h|module\.modulemap))$"
if grep -Ev "$allowed" <<<"$names"; then fail "holds files that build-apple.sh does not write"; fi
[[ -z $(sort <<<"$names" | uniq -d) ]] || fail "lists a file twice"

rm -rf Generated
mkdir Generated
tar -xf "$tar" -C Generated
[[ -z $(find Generated ! -type f ! -type d) && -z $(find Generated -type f -links +1) ]] || fail "holds links or special files"
[[ -f Generated/LagomCore.swift ]] || fail "lacks LagomCore.swift"

# The module map uniffi writes. Anything else could name a header anywhere on the runner.
modulemap='module LagomFFI {
    header "LagomFFI.h"
    export *
    use "Darwin"
    use "_Builtin_stdbool"
    use "_Builtin_stdint"
}'
# Exactly the two slices, each with the library and its headers at the plain relative paths.
plist=Generated/LagomFFI.xcframework/Info.plist
count=$(plutil -extract AvailableLibraries raw -o - "$plist")
[[ $count == 2 ]] || fail "Info.plist lists $count libraries, not 2"
ids=()
for i in 0 1; do
  id=$(plutil -extract "AvailableLibraries.$i.LibraryIdentifier" raw -o - "$plist")
  ids+=("$id")
  [[ $(plutil -extract "AvailableLibraries.$i.LibraryPath" raw -o - "$plist") == liblagom_ffi.a ]] || fail "$id: LibraryPath"
  [[ $(plutil -extract "AvailableLibraries.$i.HeadersPath" raw -o - "$plist") == Headers ]] || fail "$id: HeadersPath"
done
[[ $(printf '%s\n' "${ids[@]}" | sort | tr '\n' ' ') == "ios-arm64 ios-arm64-simulator " ]] || fail "slices ${ids[*]}"
device=Generated/LagomFFI.xcframework/ios-arm64/Headers
for id in "${ids[@]}"; do
  headers=Generated/LagomFFI.xcframework/$id/Headers
  [[ -f Generated/LagomFFI.xcframework/$id/liblagom_ffi.a && -f $headers/LagomFFI.h ]] || fail "$id: missing files"
  [[ $(<"$headers/module.modulemap") == "$modulemap" ]] || fail "$id: unexpected module.modulemap"
  cmp -s "$headers/LagomFFI.h" "$device/LagomFFI.h" || fail "$id: LagomFFI.h differs between the slices"
done
# The header may include the three system headers uniffi uses, and no other file. With no line
# splice, block comment, digraph (%:), trigraph or token pasting, a directive is exactly a line
# that starts with "#", so the allow-list below sees every directive the compiler sees.
header=$device/LagomFFI.h
if grep -En '\\[[:space:]]*$|/\*|%:|\?\?|##|_Pragma|__has_embed|__has_include' "$header"; then
  fail "LagomFFI.h uses a line splice, block comment, digraph, trigraph, token pasting, _Pragma or include test"
fi
if grep -En '^[[:space:]]*#' "$header" \
  | grep -Ev '^[0-9]+:[[:space:]]*#(include <(stdbool|stddef|stdint)\.h>|pragma once|(define|undef|ifdef|ifndef) [A-Za-z_][A-Za-z0-9_]*( .*)?|(else|endif)( //.*)?|error .*)[[:space:]]*$'; then
  fail "LagomFFI.h has a directive outside the allow-list"
fi

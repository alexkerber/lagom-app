#!/usr/bin/env bash
# Usage: scripts/upload-release.sh <tag> <file>...   (or --test: check stable_name)
# Uploads to the tag's draft release. Only .github/workflows/release-draft.yml creates the draft,
# so parallel platform builds cannot make two; this waits up to 15 minutes for it.
set -euo pipefail
# The file name without the tag's version: v0.1.30-beta.8 and lagom-0.1.30-beta.8-linux-x86_64.AppImage
# give lagom-linux-x86_64.AppImage; lagom-0.1.30.dmg gives lagom.dmg. A name without the version fails.
stable_name() {
    local version=${1#v} base=$2
    [[ $base == "lagom-$version"[-.]* ]] || { echo "::error::$base does not start with lagom-$version" >&2; return 1; }
    echo "lagom${base#lagom-"$version"}"
}
[[ ${1-} != --test ]] || {
    for case in "v0.1.30-beta.8 lagom-0.1.30-beta.8-linux-x86_64.AppImage lagom-linux-x86_64.AppImage" \
                "v0.1.30-beta.8 lagom-0.1.30-beta.8.dmg lagom.dmg" \
                "v0.1.30 lagom-0.1.30-windows-x64-setup.exe lagom-windows-x64-setup.exe" \
                "v1.2.3-preview.1 lagom-1.2.3-preview.1-android.apk lagom-android.apk"; do
        read -r tag from want <<<"$case"
        [[ $(stable_name "$tag" "$from") == "$want" ]] || { echo "stable_name $from: want $want" >&2; exit 1; }
    done
    ! stable_name v0.1.31 lagom-0.1.30.dmg 2>/dev/null || { echo "a mismatched version must fail" >&2; exit 1; }
    echo ok; exit 0
}
tag=$1
shift
for _ in $(seq 90); do
    if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>/dev/null); then
        # A rerun must not swap files under a release people already download.
        [[ $draft == true ]] || { echo "::error::$tag is published; upload by hand if you mean it." >&2; exit 1; }
        # Each file also goes up under a name without the version, so the site can link to
        # releases/latest/download/<name> once and never again (lagom-windows-x64-setup.exe).
        stable=$(mktemp -d)
        trap 'rm -rf "$stable"' EXIT
        for file in "$@"; do
            name=$(stable_name "$tag" "$(basename "$file")")
            cp "$file" "$stable/$name"
        done
        gh release upload "$tag" "$@" "$stable"/* --clobber
        exit
    fi
    sleep 10
done
echo "::error::No draft release for $tag after 15 minutes; check the Release draft workflow." >&2
exit 1

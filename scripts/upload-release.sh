#!/usr/bin/env bash
# Usage: scripts/upload-release.sh <tag> <file>...   (or --test: check stable_name)
# Uploads to the tag's draft release. Only .github/workflows/release-draft.yml creates the draft,
# so parallel platform builds cannot make two; this waits up to 15 minutes for it.
set -euo pipefail
# lagom-0.1.30-beta.8-linux-x86_64.AppImage -> lagom-linux-x86_64.AppImage; lagom-0.1.30.dmg -> lagom.dmg
stable_name() {
    sed -E 's/^lagom-[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)(\.[0-9]+)*)?([-.])/lagom\4/' <<<"$1"
}
[[ ${1-} != --test ]] || {
    for pair in "lagom-0.1.30-beta.8-linux-x86_64.AppImage lagom-linux-x86_64.AppImage" \
                "lagom-0.1.30-beta.8.dmg lagom.dmg" "lagom-0.1.30-windows-x64-setup.exe lagom-windows-x64-setup.exe" \
                "lagom-1.2.3-rc.1-android.apk lagom-android.apk"; do
        read -r from want <<<"$pair"
        [[ $(stable_name "$from") == "$want" ]] || { echo "stable_name $from: $(stable_name "$from"), want $want" >&2; exit 1; }
    done
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
        for file in "$@"; do
            cp "$file" "$stable/$(stable_name "$(basename "$file")")"
        done
        exec gh release upload "$tag" "$@" "$stable"/* --clobber
    fi
    sleep 10
done
echo "::error::No draft release for $tag after 15 minutes; check the Release draft workflow." >&2
exit 1

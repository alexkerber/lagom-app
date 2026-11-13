#!/usr/bin/env bash
# Usage: scripts/upload-release.sh <tag> <file>...
# Uploads to the tag's draft release. Only .github/workflows/release-draft.yml creates the draft,
# so parallel platform builds cannot make two; this waits up to 15 minutes for it.
set -euo pipefail
tag=$1
shift
for _ in $(seq 90); do
    if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>/dev/null); then
        # A rerun must not swap files under a release people already download.
        [[ $draft == true ]] || { echo "::error::$tag is published; upload by hand if you mean it." >&2; exit 1; }
        exec gh release upload "$tag" "$@" --clobber
    fi
    sleep 10
done
echo "::error::No draft release for $tag after 15 minutes; check the Release draft workflow." >&2
exit 1

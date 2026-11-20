#!/usr/bin/env bash
# Usage: scripts/appcast.sh > site/appcast.xml   (needs gh with read access to the repository)
# The Sparkle feed for the dmg: the <item> files (lagom-<version>.appcast.xml, from
# scripts/sparkle-item.sh) of the 20 newest published releases. Drafts stay out: their download
# links do not work. Betas are in. shortcut: one channel for all, add sparkle:channel "beta" when stable releases start.
set -euo pipefail
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cat <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
    <channel>
        <title>lagom</title>
        <link>https://lagommail.app/appcast.xml</link>
XML
for tag in $(gh release list --exclude-drafts --limit 20 --json tagName --jq '.[].tagName'); do
    # A release without an item (built before Sparkle, or no key) is skipped; other errors stop the run.
    gh release view "$tag" --json assets --jq '.assets[].name' | grep -q '\.appcast\.xml$' || continue
    gh release download "$tag" --pattern '*.appcast.xml' --dir "$dir/$tag"
    cat "$dir/$tag"/*.appcast.xml
done
cat <<'XML'
    </channel>
</rss>
XML

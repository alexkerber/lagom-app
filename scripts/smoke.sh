#!/bin/sh
# Run from any directory; all generated files stay under target/smoke.
set -u
cd "$(dirname "$0")/.." || exit 1
quick=0
visual=0
for arg do
    case "$arg" in
        --quick) quick=1 ;;
        --visual) visual=1 ;;
        --no-visual) visual=0 ;;
        *) printf 'Usage: %s [--quick] [--visual]\n' "$0" >&2; exit 2 ;;
    esac
done
export TMPDIR="$PWD/target/smoke/tmp"
mkdir -p "$TMPDIR" || exit 1
out=target/smoke
failed=0
pid=
own_child() {
    kill -0 "$pid" 2>/dev/null &&
        [ "$(ps -o ppid= -p "$pid" | tr -d ' ')" = "$$" ]
}
cleanup() {
    if [ -n "$pid" ]; then
        if own_child; then
            kill "$pid" 2>/dev/null || :
            sleep 1
            if own_child; then
                case "$(ps -o comm= -p "$pid")" in
                    lagom|*/lagom) kill -KILL "$pid" 2>/dev/null || : ;;
                esac
            fi
        fi
        wait "$pid" 2>/dev/null || :
        pid=
    fi
}
trap cleanup 0
interrupted=0
launching=0
trap 'interrupted=129; [ "$launching" = 1 ] || exit "$interrupted"' HUP
trap 'interrupted=130; [ "$launching" = 1 ] || exit "$interrupted"' INT
trap 'interrupted=143; [ "$launching" = 1 ] || exit "$interrupted"' TERM
result() {
    printf '%s %s (%ss)\n' "$1" "$2" "$(($(date +%s) - start))"
    if [ "$1" = FAIL ]; then failed=1; fi
}
start=$(date +%s)
built=0
if "${CARGO:-cargo}" build --release -p lagom >"$out/build.log" 2>&1; then
    built=1
    result PASS 'release build'
else
    result FAIL "release build; see $out/build.log"
fi
start=$(date +%s)
if [ "$quick" = 1 ]; then
    result WARN 'workspace tests skipped (--quick)'
elif "${CARGO:-cargo}" test -q --workspace >"$out/test.log" 2>&1; then
    result PASS 'workspace tests'
else
    result FAIL "workspace tests; see $out/test.log"
fi
if [ "$visual" = 0 ]; then
    start=$(date +%s)
    result WARN 'visual smoke skipped (pass --visual; it opens app windows)'
    exit "$failed"
fi
start=$(date +%s)
if [ "$(uname -s)" != Darwin ] || [ "$built" != 1 ]; then
    result FAIL 'visual smoke requires macOS and a successful release build'
    exit 1
fi
# Poll in one Swift process, matching only this launch's layer-0 window.
if ! swiftc -o "$out/window-id" - >"$out/swift.log" 2>&1 <<'SWIFT'
import Foundation
import CoreGraphics
import Darwin
let pid = pid_t(CommandLine.arguments[1])!
let deadline = Date().addingTimeInterval(30)
while Date() < deadline {
    guard kill(pid, 0) == 0 else { exit(1) }
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    if let window = windows.first(where: {
        ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }), let id = window[kCGWindowNumber as String] as? Int {
        print(id)
        exit(0)
    }
    Thread.sleep(forTimeInterval: 0.1)
}
exit(1)
SWIFT
then
    result FAIL "window probe compilation; see $out/swift.log"
    exit 1
fi
result PASS 'window probe compilation'
# Keep in sync with LAGOM_SCREEN in apps/desktop/src (including live account views).
screens='inbox empty palette later folders settings html html-simple move compose compose-text compose-cc compose-md compose-md-bar glide swipe-neutral swipe swipe-later swipe-delete deleted add-account add-account-form'
for screen in $screens; do
    for theme in light dark; do
        start=$(date +%s)
        png="$out/$screen-$theme.png"
        rm -f "$png"
        launch_screen=$screen
        if [ "$screen" = inbox ]; then launch_screen=; fi
        launching=1
        LAGOM_DEMO=1 LAGOM_SCREEN="$launch_screen" LAGOM_THEME="$theme" \
            ./target/release/lagom >"$out/$screen-$theme.log" 2>&1 &
        pid=$!
        launching=0
        if [ "$interrupted" != 0 ]; then exit "$interrupted"; fi
        ok=0
        if window=$("$out/window-id" "$pid"); then
            sleep 2
            if kill -0 "$pid" 2>/dev/null &&
                screencapture -x -o -l "$window" "$png" >>"$out/$screen-$theme.log" 2>&1 &&
                [ -s "$png" ] && kill -0 "$pid" 2>/dev/null; then
                ok=1
            fi
        fi
        cleanup
        if [ "$ok" = 1 ]; then
            result PASS "visual $screen $theme"
        else
            rm -f "$png"
            result FAIL "visual $screen $theme; see $out/$screen-$theme.log"
        fi
    done
done
start=$(date +%s)
rows=0
for screen in $screens; do rows=$((rows + 1)); done
# Keep this outside an if condition so set -e catches every failed write.
(
set -e
{
    printf '<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="%s">\n' "$((rows * 360 + 40))"
    printf '<rect width="100%%" height="100%%" fill="#eee"/><g font-family="sans-serif" font-size="16" fill="#222">\n'
    printf '<text x="20" y="25">Light</text><text x="510" y="25">Dark</text>\n'
    y=40
    for screen in $screens; do
        printf '<text x="20" y="%s">%s</text>\n' "$((y + 20))" "$screen"
        x=20
        for theme in light dark; do
            if [ -s "$out/$screen-$theme.png" ]; then
                printf '<image href="%s-%s.png" x="%s" y="%s" width="480" height="320"/>\n' "$screen" "$theme" "$x" "$((y + 30))"
            fi
            x=510
        done
        y=$((y + 360))
    done
    printf '</g></svg>\n'
} >"$out/sheet.svg.tmp"
{
    printf '<!doctype html><html lang="en"><meta charset="utf-8"><title>Smoke test</title>\n'
    printf '<style>body{font:16px sans-serif;background:#eee;color:#222}table{border-spacing:12px}img{width:480px;max-width:45vw}th{text-align:left}</style>\n'
    printf '<table><thead><tr><th>Screen</th><th>Light</th><th>Dark</th></tr></thead><tbody>\n'
    for screen in $screens; do
        printf '<tr><th>%s</th>' "$screen"
        for theme in light dark; do
            if [ -s "$out/$screen-$theme.png" ]; then
                printf '<td><a href="%s-%s.png"><img src="%s-%s.png" alt="%s %s"></a></td>' "$screen" "$theme" "$screen" "$theme" "$screen" "$theme"
            else
                printf '<td>Capture failed</td>'
            fi
        done
        printf '</tr>\n'
    done
    printf '</tbody></table></html>\n'
} >"$out/index.html.tmp"
)
generated=$?
# Assert that both grids contain every screen before reporting success.
if [ "$generated" -eq 0 ] &&
    mv "$out/sheet.svg.tmp" "$out/sheet.svg" &&
    mv "$out/index.html.tmp" "$out/index.html" &&
    [ "$(grep -c '^<tr><th>' "$out/index.html")" -eq "$rows" ] &&
    [ "$(grep -c '<text x="20"' "$out/sheet.svg")" -eq "$((rows + 1))" ]; then
    result PASS "browser grid: $out/index.html"
else
    result FAIL 'contact sheet grid generation'
fi
start=$(date +%s)
rm -f "$out/sheet.png"
if ! command -v rsvg-convert >/dev/null 2>&1; then
    result WARN 'PNG sheet skipped (rsvg-convert missing, not a failure); install rsvg-convert with brew install librsvg'
elif rsvg-convert "$out/sheet.svg" -o "$out/sheet.png.tmp" >"$out/sheet.log" 2>&1 && [ -s "$out/sheet.png.tmp" ] &&
    mv "$out/sheet.png.tmp" "$out/sheet.png"; then
    result PASS "contact sheet: $out/sheet.png"
else
    result FAIL "contact sheet; see $out/sheet.log"
fi
exit "$failed"

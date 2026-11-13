# lagom

> **lagom** /ˈlɑːɡɔm/ ("LAH-gom") _adv., adj._ Swedish.
> Not too much. Not too little. Just right.

A fast, minimal, keyboard-first email client. Made in Sweden. Open source.

Status: early. Nothing to install yet.

[Privacy](site/privacy.html) · [Support](site/support.html)

## Layout

| Path                | What                                                             |
| ------------------- | ---------------------------------------------------------------- |
| `crates/lagom-core` | Shared Rust core: accounts, sync, local store and search. No UI. |
| `apps/desktop`      | macOS, Linux and Windows. First target.                          |
| `apps/mobile`       | Native iPhone, iPad, Android phone and tablet. Second target.    |
| `site`              | The website. Static HTML.                                        |

One repository holds all apps, so the core and every app change in the same commit.

## Smoke test

Run `./scripts/smoke.sh` to build the release app and run workspace tests.
Add `--visual` to also capture the main-window demo screens in light and dark themes. This
opens app windows one after another, so do not type while it runs. Visual checks
require macOS, a graphical session, Swift, and Screen Recording permission for
the terminal. Only the app processes started by the script are stopped.

Results, logs, and screenshots land in `target/smoke/`, with a browser grid at
`target/smoke/index.html` and a contact sheet at `target/smoke/sheet.png`.
Install `rsvg-convert` with `brew install librsvg` to generate the PNG sheet;
otherwise that sheet is skipped with a warning.
Use `--quick` to skip tests.

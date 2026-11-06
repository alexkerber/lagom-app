# lagom

> **lagom** /ˈlɑːɡɔm/ ("LAH-gom") _adv., adj._ Swedish.
> Not too much. Not too little. Just right.

A fast, minimal, keyboard-first email client. Made in Sweden. Open source.

Status: early. Nothing to install yet.

## Layout

| Path                | What                                                             |
| ------------------- | ---------------------------------------------------------------- |
| `crates/lagom-core` | Shared Rust core: accounts, sync, local store and search. No UI. |
| `apps/desktop`      | macOS, Linux and Windows. First target.                          |
| `apps/mobile`       | iOS, iPadOS and Android. Later.                                  |

One repository holds all apps, so the core and every app change in the same commit.

# Lagom mobile

Native apps on top of `lagom-core`. Second target, after desktop.

| Path                  | Platforms                | UI              |
| --------------------- | ------------------------ | --------------- |
| `apps/mobile/apple`   | iPhone and iPad          | SwiftUI         |
| `apps/mobile/android` | Android phone and tablet | Jetpack Compose |

Both call `lagom-core` through generated bindings (UniFFI). No business logic in the mobile apps.

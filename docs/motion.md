# Motion

Lagom must feel fast and modern. Motion confirms an action. It never delays one.

Reference: the original Spark (2015–2016). Simple, quick and satisfying.

## Rules

1. The action happens at once. The animation plays on top. The user can press the next key before it ends.
2. Short. 120–200 ms for most motion. Never more than 300 ms.
3. One easing family: fast out, gentle stop (ease-out). Springs only where an object moves in space.
4. Motion carries meaning. Every animation shows where something went.
5. Respect the system "reduce motion" setting. Fall back to a short fade.
6. 60 fps minimum on all three platforms. Drop an animation before dropping frames.

## Key moments

| Action               | Motion                                                                                                                 |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| Done (E)             | The row slides out to the side with an accent tint, and the list closes the gap. The next message is selected at once. |
| Archive or delete    | Same as Done, with its own direction and colour.                                                                       |
| Later (L)            | The row lifts and shrinks towards "Later" in the sidebar. The count there ticks up.                                    |
| Send                 | The compose window slides up and out. A short "Sent" toast confirms it, with Undo.                                     |
| Undo                 | The reverse of the original motion.                                                                                    |
| Inbox reaches zero   | A small, calm moment: the "Nothing left." state fades in with one illustration. No confetti.                           |
| Command palette (⌘K) | Scale 0.98 → 1 and fade in under 120 ms.                                                                               |
| Counts               | Numbers roll, they do not jump.                                                                                        |

## Implementation

GPUI has animation built in (`with_animation`, `Animation`, easing functions). Use it before adding a dependency.

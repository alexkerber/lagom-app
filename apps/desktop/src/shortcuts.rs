use gpui::{Keystroke, Modifiers};

pub(crate) struct SubmitShortcut {
    #[cfg(test)]
    pub(crate) keystroke: &'static str,
    pub(crate) hint: &'static str,
    modifiers: Modifiers,
}

pub(crate) fn submit() -> SubmitShortcut {
    for_platform(cfg!(target_os = "macos"))
}

fn for_platform(macos: bool) -> SubmitShortcut {
    SubmitShortcut {
        #[cfg(test)]
        keystroke: if macos { "cmd-enter" } else { "ctrl-enter" },
        hint: if macos { "⌘↵" } else { "Ctrl+Enter" },
        modifiers: Modifiers { platform: macos, control: !macos, ..Modifiers::none() },
    }
}

impl SubmitShortcut {
    pub(crate) fn matches(&self, key: &Keystroke) -> bool {
        key.key == "enter" && key.modifiers == self.modifiers
    }
}

/// A group heading and its rows: what the keys do, and the chords that do it.
pub(crate) type Group = (&'static str, &'static [(&'static str, &'static [&'static str])]);

/// The keyboard shortcut list in Settings, in keymap notation ("secondary" is ⌘ or Ctrl).
/// shortcut: hand-kept beside `main_window::bind_keys`, derive it from the keymap if the two drift.
pub(crate) const LIST: &[Group] = &[
    ("Mail", &[
        ("Next and previous message", &["j", "k"]),
        ("Done", &["e"]),
        ("Later", &["l"]),
        ("Reply", &["r"]),
        ("Reply all", &["shift-r"]),
        ("Forward", &["f"]),
        ("New message", &["c"]),
        ("Move to a folder", &["m"]),
        ("Mark read or unread", &["shift-u"]),
        ("Delete", &["backspace"]),
        ("Open in a new window", &["secondary-o"]),
        ("Undo", &["secondary-z"]),
        ("Send", &["secondary-enter"]),
    ]),
    ("Tasks and calendar", &[
        ("New task", &["t"]),
        ("Complete task", &["space"]),
        ("New event", &["n"]),
        ("Previous and next week", &["left", "right"]),
    ]),
    ("Everywhere", &[
        ("Commands and search", &["secondary-k"]),
        ("Today, Mail, Calendar, Todo", &["secondary-1", "secondary-2", "secondary-3", "secondary-4"]),
        ("Show or hide the sidebar", &["alt-secondary-s"]),
        ("Settings", &["secondary-,"]),
        ("Keyboard shortcuts", &["secondary-/"]),
    ]),
];

/// "alt-secondary-s" as "⌥⌘S" on macOS and "Ctrl+Alt+S" elsewhere.
pub(crate) fn label(chord: &str) -> String {
    label_for(chord, cfg!(target_os = "macos"))
}

fn label_for(chord: &str, macos: bool) -> String {
    let (mods, key) = chord.rsplit_once('-').unwrap_or(("", chord));
    let has = |m: &str| mods.split('-').any(|x| x == m);
    let key = match key {
        "backspace" if macos => "⌫".into(), "backspace" => "Backspace".into(),
        "enter" if macos => "↵".into(), "enter" => "Enter".into(),
        "space" => "Space".into(), "left" => "←".into(), "right" => "→".into(), "up" => "↑".into(), "down" => "↓".into(),
        k => k.to_uppercase(),
    };
    format!("{}{key}", prefix(macos, has("ctrl"), has("alt"), has("shift"), has("secondary")))
}

/// Modifier glyphs in Apple's order (⌃⌥⇧⌘); words joined by "+" elsewhere, Ctrl first.
fn prefix(macos: bool, ctrl: bool, alt: bool, shift: bool, secondary: bool) -> String {
    if macos {
        [(ctrl, "⌃"), (alt, "⌥"), (shift, "⇧"), (secondary, "⌘")].iter().filter(|m| m.0).map(|m| m.1).collect()
    } else {
        [(ctrl || secondary, "Ctrl+"), (alt, "Alt+"), (shift, "Shift+")].iter().filter(|m| m.0).map(|m| m.1).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_labels_follow_the_platform() {
        for (chord, mac, other) in [("alt-secondary-s", "⌥⌘S", "Ctrl+Alt+S"), ("shift-r", "⇧R", "Shift+R"),
            ("secondary-,", "⌘,", "Ctrl+,"), ("backspace", "⌫", "Backspace"), ("secondary-enter", "⌘↵", "Ctrl+Enter"),
            ("j", "J", "J"), ("left", "←", "←")] {
            assert_eq!((label_for(chord, true).as_str(), label_for(chord, false).as_str()), (mac, other), "{chord}");
        }
        // Every listed chord parses as a keystroke, so a typo cannot show a key that does nothing.
        for (_, rows) in LIST { for (_, chords) in *rows { for c in *chords {
            Keystroke::parse(c).unwrap_or_else(|e| panic!("{c}: {e}"));
        } } }
    }

    #[test]
    fn submit_chord_and_hint_follow_the_platform() {
        for (macos, chord, hint, other) in [(true, "cmd-enter", "⌘↵", "ctrl-enter"),
            (false, "ctrl-enter", "Ctrl+Enter", "cmd-enter")] {
            let shortcut = for_platform(macos);
            assert_eq!(shortcut.keystroke, chord);
            assert_eq!(shortcut.hint, hint);
            assert!(shortcut.matches(&Keystroke::parse(chord).unwrap()));
            for key in [other, "enter", "shift-enter", "ctrl-backspace", "cmd-backspace", "alt-enter"] {
                assert!(!shortcut.matches(&Keystroke::parse(key).unwrap()), "{key}, macos={macos}");
            }
        }
    }
}

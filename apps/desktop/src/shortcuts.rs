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

#[cfg(test)]
mod tests {
    use super::*;

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

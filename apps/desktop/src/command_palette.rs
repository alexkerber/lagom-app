//! Command palette (⌘K), concept B structure in Concept D tokens.
//! Source: CommandPalette on the Claude design canvas. Commands do not run yet: Enter only closes.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, prelude::*, px, Animation, AnimationExt, Context, Div, DismissEvent,
    EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyDownEvent, MouseButton,
    Rgba, SharedString, StyledText, Window,
};

use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

enum Hint {
    Key(&'static str),
    Note(&'static str),
}

struct Command {
    section: &'static str,
    label: &'static str,
    hint: Hint,
}

// The five commands in the design.
const COMMANDS: &[Command] = &[
    Command { section: "Actions", label: "Done", hint: Hint::Key("E") },
    Command { section: "Actions", label: "Done: everything from Ingrid Holm", hint: Hint::Key("⇧E") },
    Command { section: "Actions", label: "Done: everything older than a week", hint: Hint::Note("4 messages") },
    Command { section: "Actions", label: "Undo done", hint: Hint::Key("Z") },
    Command { section: "Go to", label: "Done mailbox", hint: Hint::Key("G D") },
];

/// Byte range of `query` in `label`, ignoring ASCII case. An empty query matches with no range.
fn find(label: &str, query: &str) -> Option<Option<std::ops::Range<usize>>> {
    if query.is_empty() {
        return Some(None);
    }
    let start = label.to_ascii_lowercase().find(&query.to_ascii_lowercase())?;
    Some(Some(start..start + query.len()))
}

pub struct CommandPalette {
    query: String,
    selected: usize,
    focus: FocusHandle,
}

impl EventEmitter<DismissEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CommandPalette {
    pub fn new(query: &str, cx: &mut Context<Self>) -> Self {
        Self { query: query.into(), selected: 0, focus: cx.focus_handle() }
    }

    fn matches(&self) -> Vec<(&'static Command, Option<std::ops::Range<usize>>)> {
        COMMANDS.iter().filter_map(|c| find(c.label, &self.query).map(|r| (c, r))).collect()
    }

    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let count = self.matches().len();
        match ks.key.as_str() {
            "escape" | "enter" => cx.emit(DismissEvent),
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            "backspace" => {
                self.query.pop();
                self.selected = 0;
            }
            _ => match &ks.key_char {
                Some(text) if !ks.modifiers.platform && !ks.modifiers.control => {
                    self.query.extend(text.chars().filter(|c| !c.is_control()));
                    self.selected = 0;
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

fn mono(text: impl Into<SharedString>, color: Rgba) -> Div {
    div().font_family(font::MONO).text_size(px(size::MONO)).text_color(color).child(text.into())
}

fn key_cap(t: &Theme, key: &'static str, selected: bool) -> Div {
    mono(key, if selected { t.ink } else { t.ink2 })
        .px(px(6.))
        .py(px(1.))
        .border_1()
        .border_color(if selected { t.ink2 } else { t.line })
        .rounded(px(radius::KEY))
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
        let matches = self.matches();
        let bold = HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };

        let mut list = div().flex().flex_col().p(px(8.));
        let mut section = "";
        for (i, (command, range)) in matches.iter().enumerate() {
            if command.section != section {
                section = command.section;
                list = list.child(
                    div()
                        .px(px(12.))
                        .pt(px(if i == 0 { 10. } else { 14. }))
                        .pb(px(6.))
                        .text_size(px(size::SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.ink2)
                        .child(section),
                );
            }
            let selected = i == self.selected;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(44.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .text_size(px(size::BODY))
                    .when(selected, |d| d.bg(t.sel))
                    .child(div().size(px(6.)).rounded_full().when(selected, |d| d.bg(t.accent)))
                    .child(
                        div()
                            .flex_1()
                            .child(StyledText::new(command.label).with_highlights(range.clone().map(|r| (r, bold)))),
                    )
                    .child(match command.hint {
                        Hint::Key(key) => key_cap(&t, key, selected),
                        Hint::Note(note) => mono(note, t.ink2),
                    }),
            );
        }

        let input = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .h(px(64.))
            .px(px(20.))
            .border_b_1()
            .border_color(t.line)
            .child(mono("›", t.accent).text_size(px(size::BODY)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .text_size(px(size::TITLE))
                    .font_weight(FontWeight::MEDIUM)
                    .when(!self.query.is_empty(), |d| d.child(self.query.clone()))
                    // Static caret: idle means idle, so no blink timer.
                    .child(div().w(px(2.)).h(px(22.)).ml(px(1.)).bg(t.accent))
                    .when(self.query.is_empty(), |d| d.child(div().text_color(t.ink2).child("Type a command"))),
            )
            .child(key_cap(&t, "esc", false));

        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(40.))
            .px(px(20.))
            .border_t_1()
            .border_color(t.line)
            .bg(t.subtle)
            .child(mono("↑ ↓ move · ↵ run · esc close", t.ink2))
            .child(mono(
                match matches.len() {
                    1 => "1 result".to_string(),
                    n => format!("{n} results"),
                },
                t.ink2,
            ));

        let dialog = div()
            .occlude()
            .w(px(640.))
            .flex()
            .flex_col()
            .bg(t.raised)
            .border_1()
            .border_color(t.line)
            .rounded(px(radius::PANEL))
            .shadow_xl()
            .overflow_hidden()
            .text_color(t.ink)
            .font_family(font::UI)
            .child(input)
            .child(list)
            .child(footer);

        div()
            .id("command-palette")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(112.))
            .bg(gpui::black().opacity(if t.dark { 0.5 } else { 0.18 }))
            .child(dialog)
            // docs/motion.md asks for scale 0.98 → 1 as well, but GPUI divs have no scale transform.
            .with_animation(
                "command-palette-in",
                Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()),
                |el, delta| el.opacity(delta),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ignores_case_and_returns_byte_range() {
        assert_eq!(find("Undo done", "DONE"), Some(Some(5..9)));
        assert_eq!(find("Done mailbox", ""), Some(None));
        assert_eq!(find("Done mailbox", "later"), None);
    }

    #[test]
    fn design_query_shows_all_five() {
        assert_eq!(COMMANDS.iter().filter(|c| find(c.label, "done").is_some()).count(), 5);
    }
}

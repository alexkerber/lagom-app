//! Empty inbox, "Nothing left." (EmptyInbox on the Claude design canvas, concept B layout
//! in Concept D tokens). Replaces the message list and reading pane when the inbox is at zero.

use std::time::Duration;

use gpui::{div, ease_out_quint, prelude::*, px, relative, svg, Animation, AnimationExt, Div, FontWeight};

use crate::main_window::mono;
use crate::theme::{font, size, Theme};

/// The headline is the one hero size in the app; the design sets it at 56 px.
const HEADLINE: f32 = 56.0;

pub fn view(t: &Theme) -> Div {
    let content = div()
        .w_full()
        .max_w(px(560.))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(28.))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child({
                    // 400 x 250 at most, shrinks with the window like the design's max-width: 100%.
                    let mut tray = svg().path("illustrations/empty-tray.svg").w_full().max_w(px(400.)).text_color(t.ink);
                    tray.style().aspect_ratio = Some(320. / 200.);
                    tray
                })
                .child(mono("Fig. 1 — Inbox. Contents: none.", t.ink2)),
        )
        .child(
            div()
                .font_family(font::DISPLAY)
                .text_size(px(HEADLINE))
                .font_weight(FontWeight::BOLD)
                .line_height(relative(1.))
                .child("Lagom. Nothing left."),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.))
                .pt(px(20.))
                .border_t_1()
                .border_color(t.line)
                .text_size(px(size::TITLE))
                .line_height(relative(1.4))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(px(5.))
                        .child(div().font_weight(FontWeight::BOLD).child("lagom"))
                        .child(div().font_family(font::MONO).text_size(px(size::BASE)).child("/ˈlɑːɡɔm/"))
                        .child(div().text_color(t.ink2).child("(\"LAH-gom\")"))
                        .child(div().italic().child("adv., adj."))
                        .child("Swedish."),
                )
                .child(div().text_color(t.ink2).child("Not too much. Not too little. Just right.")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(20.))
                .gap_y(px(8.))
                .child(mono("C compose", t.ink2))
                .child(mono("G D what you finished", t.ink2))
                .child(mono("⌘K commands", t.ink2)),
        )
        // Calm fade-in, no confetti (docs/motion.md). Plays once per element id.
        // shortcut: ignores the system reduce-motion setting, GPUI 0.2.2 does not expose it.
        .with_animation(
            "empty-inbox",
            Animation::new(Duration::from_millis(300)).with_easing(ease_out_quint()),
            |el, delta| el.opacity(delta),
        );

    div()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(24.))
                .py(px(48.))
                .child(content),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .px(px(16.))
                .py(px(12.))
                .border_t_1()
                .border_color(t.line)
                .child(mono("Home · synced 10:12", t.ink2))
                .child(mono("0 left", t.ink2)),
        )
}

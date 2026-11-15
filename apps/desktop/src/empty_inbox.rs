//! Empty inbox, "Nothing left." Replaces the message list and reading pane when the inbox is at
//! zero: one small illustration, one line, one quiet line under it, and the sync status.

use std::time::Duration;

use gpui::{div, ease_out_quint, prelude::*, px, svg, Animation, AnimationExt, Div, FontWeight, SharedString};

use crate::main_window::{icon, mono};
use crate::theme::{size, Theme};

/// `status`: the account and its sync state, quiet at the foot of the pane.
pub fn view(t: &Theme, status_icon: &'static str, status: SharedString) -> Div {
    let content = div()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .child({
            let mut tray = svg().path("illustrations/empty-tray.svg").w(px(120.)).text_color(t.ink2);
            tray.style().aspect_ratio = Some(320. / 200.);
            tray.mb(px(28.))
        })
        .child(div().text_size(px(size::SUBJECT)).font_weight(FontWeight::SEMIBOLD).child("Nothing left."))
        .child(div().mt(px(6.)).text_color(t.ink2).child("Not too much. Not too little. Just right."))
        // Calm fade-in, no confetti (docs/motion.md). Plays once per element id. A fade only, so
        // reduced motion keeps it.
        .with_animation(
            "empty-inbox",
            Animation::new(Duration::from_millis(300)).with_easing(ease_out_quint()),
            |el, delta| el.opacity(delta),
        );

    div()
        .debug_selector(|| "empty-inbox".into())
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        // Bottom padding lifts the content a little above the true centre, where the eye expects it.
        .child(div().flex_1().flex().items_center().justify_center().px(px(24.)).pb(px(64.)).child(content))
        .child(div().flex().items_center().gap(px(6.)).px(px(16.)).pb(px(20.)).child(icon(status_icon, t.ink2, 16.)).child(mono(status, t.ink2)))
}

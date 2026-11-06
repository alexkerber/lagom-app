//! Compose, Concept B structure (Compose on the Claude design canvas) in Concept D tokens.
//! A reply sheet over the main window. Fields are static sample text until a real editor exists.
//! shortcut: static fields, no text input; add an editor element when compose sends real mail.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, prelude::*, px, relative, Animation, AnimationExt, Context, Div,
    FocusHandle, FontWeight, KeyDownEvent, Rgba, Task, Window,
};

use crate::main_window::{key_button, mono};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

const SUBJECT: &str = "Re: Board meeting moved to Thursday";
const BODY: &str = "Thursday works. I will read the minutes before the meeting. Probably on the train.\n\nAlex";
/// How long the "Sent" toast offers Undo.
const UNDO_WINDOW: Duration = Duration::from_secs(5);
/// docs/motion.md: 120–200 ms, ease-out.
const SLIDE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    /// Sheet slides up and out, toast offers Undo.
    Sent,
    /// Undo: sheet slides back in.
    Restored,
    Gone,
}

pub struct Compose {
    theme: Theme,
    phase: Phase,
    focus: FocusHandle,
    _dismiss: Option<Task<()>>,
    _appearance: gpui::Subscription,
}

impl Compose {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            theme: Theme::for_appearance(window.appearance(), DEFAULT_ACCENT),
            phase: Phase::Open,
            focus,
            _dismiss: None,
            _appearance: appearance,
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent || self.phase == Phase::Gone {
            return;
        }
        self.phase = Phase::Sent;
        // Replacing the task cancels an older timer.
        self._dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO_WINDOW).await;
            this.update(cx, |this, cx| this.set(Phase::Gone, cx)).ok();
        }));
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent {
            self._dismiss = None;
            self.set(Phase::Restored, cx);
        }
    }

    /// The faded sheet keeps its hitboxes while Sent, so Discard must not act then.
    fn discard(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Sent {
            self.set(Phase::Gone, cx);
        }
    }

    fn set(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        if phase == Phase::Gone {
            cx.emit(gpui::DismissEvent);
        }
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if !k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "enter" => self.send(cx),
            "z" => self.undo(cx),
            "backspace" => self.discard(cx),
            _ => {}
        }
    }

    fn sheet(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let field = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .min_h(px(48.))
                .border_b_1()
                .border_color(t.line)
                .child(mono(label, t.ink2).w(px(64.)).flex_shrink_0())
        };
        let words = BODY.split_whitespace().count();

        div()
            .w(px(800.))
            .h(px(560.))
            .flex()
            .flex_col()
            .bg(t.raised)
            .border_1()
            .border_color(t.line)
            .rounded(px(radius::CARD))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(16.))
                    .pr(px(8.))
                    .bg(t.subtle)
                    .border_b_1()
                    .border_color(t.line)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Reply"))
                    .child(
                        div()
                            .id("discard")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(radius::CONTROL))
                            .text_color(t.ink2)
                            .cursor_pointer()
                            .hover(|d| d.bg(t.sel))
                            .on_click(cx.listener(|this, _, _, cx| this.discard(cx)))
                            .child("Discard")
                            .child(mono("⌘⌫", t.ink2)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(24.))
                    .child(field("From").child("Home · you@home.example"))
                    .child(
                        field("To").child(
                            div()
                                .flex()
                                .items_center()
                                .h(px(28.))
                                .px(px(10.))
                                .border_1()
                                .border_color(t.line)
                                .rounded(px(radius::CONTROL))
                                .child("Ingrid Holm"),
                        ),
                    )
                    .child(field("Subject").child(div().font_weight(FontWeight::SEMIBOLD).child(SUBJECT))),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .px(px(24.))
                    .py(px(20.))
                    .text_size(px(size::BODY))
                    .line_height(relative(1.6))
                    .children(BODY.split("\n\n").map(|p| div().child(p))),
            )
            .child(
                div()
                    .h(px(64.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        key_button(t, "Send", "⌘↵", true)
                            .id("send")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    )
                    .child(key_button(t, "Attach", "⌘⇧A", false))
                    .child(key_button(t, "Send later", "⌘L", false))
                    .child(div().flex_1())
                    .child(mono(format!("{words} words · saved 09:51"), t.ink2)),
            )
    }

    fn toast(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        div()
            .absolute()
            .bottom(px(24.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .h(px(48.))
                    .pl(px(16.))
                    .pr(px(7.))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(radius::PANEL))
                    .shadow_lg()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Sent")
                    .child(
                        key_button(t, "Undo", "⌘Z", false)
                            .id("undo")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                    ),
            )
            .with_animation("toast-in", Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()), |d, v| {
                d.opacity(v).bottom(px(16. + 8. * v))
            })
    }
}

impl gpui::EventEmitter<gpui::DismissEvent> for Compose {}

impl Render for Compose {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.phase == Phase::Gone {
            return div();
        }
        let t = self.theme;
        let scrim = Rgba { a: 0.7, ..t.subtle };
        let layer = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(scrim)
            .child(self.sheet(cx));
        let slide = Animation::new(SLIDE).with_easing(ease_out_quint());
        // Bottom padding shifts the centred sheet up by half its value.
        let out = |d: Div, v: f32| d.opacity(1. - v).pb(px(160. * v));
        let layer = match self.phase {
            Phase::Sent => layer.with_animation("compose-out", slide, out).into_any_element(),
            Phase::Restored => layer.with_animation("compose-in", slide, move |d, v| out(d, 1. - v)).into_any_element(),
            _ => layer.into_any_element(),
        };

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .text_color(t.ink)
            .font_family(font::UI)
            .text_size(px(size::BASE))
            .child(layer)
            .when(self.phase == Phase::Sent, |d| d.child(self.toast(cx)))
    }
}

//! Compose, Concept B structure (Compose on the Claude design canvas) in Concept D tokens.
//! A reply sheet over the main window. To, Cc (on demand), Subject and Body are `TextInput`s.
//! Esc closes and keeps the draft in memory; Send and Discard (⌘⌫) end it.
//! shortcut: the draft lives in memory only and is lost on quit; save it to Drafts with lagom-core.

use std::time::Duration;

use gpui::{
    div, ease_out_quint, prelude::*, px, relative, Animation, AnimationExt, Context, Div, Entity,
    FocusHandle, Focusable, FontWeight, KeyDownEvent, Rgba, Subscription, Task, Window,
};

use crate::main_window::{key_button, mono, pressable};
use crate::motion::spring;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

/// Sample reply for `LAGOM_SCREEN=compose-text`.
const BODY: &str = "Thursday works. I will read the minutes before the meeting. Probably on the train.\n\nAlex";
/// How long the "Sent" and "Deleted" toasts offer Undo.
pub(crate) const UNDO_WINDOW: Duration = Duration::from_secs(5);
/// docs/motion.md: 120–200 ms, ease-out.
const SLIDE: Duration = Duration::from_millis(200);
/// The word count waits for a pause in typing, so a keystroke never walks the whole body.
const COUNT_DELAY: Duration = Duration::from_millis(300);

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
    to: Entity<TextInput>,
    cc: Entity<TextInput>,
    subject: Entity<TextInput>,
    body: Entity<TextInput>,
    show_cc: bool,
    words: usize,
    /// Esc closed the sheet: the main window keeps this draft and reopens it.
    keep: bool,
    /// The field that had focus when Esc closed the sheet.
    refocus: Option<FocusHandle>,
    _count: Option<Task<()>>,
    _dismiss: Option<Task<()>>,
    _appearance: Subscription,
    _fields: Vec<Subscription>,
}

impl Compose {
    pub fn new(to: &str, subject: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        let to = cx.new(|cx| TextInput::new(window, cx).bordered().placeholder("Recipients").with_text(to));
        let cc = cx.new(|cx| TextInput::new(window, cx).bordered().placeholder("Recipients"));
        let subject = cx.new(|cx| TextInput::new(window, cx).placeholder("Subject").with_text(subject));
        let body = cx.new(|cx| TextInput::new(window, cx).multiline().placeholder("Write your reply"));
        let fields = [&to, &cc, &subject, &body].map(|f| cx.subscribe_in(f, window, Self::on_field)).into();
        // A reply starts in the body.
        window.focus(&body.focus_handle(cx));
        Self {
            theme: Theme::for_appearance(window.appearance(), DEFAULT_ACCENT),
            phase: Phase::Open,
            focus: cx.focus_handle(),
            to,
            cc,
            subject,
            body,
            show_cc: false,
            words: 0,
            keep: false,
            refocus: None,
            _count: None,
            _dismiss: None,
            _appearance: appearance,
            _fields: fields,
        }
    }

    /// `LAGOM_SCREEN=compose-text`: a written reply with a word selected.
    pub fn sample(&mut self, cx: &mut Context<Self>) {
        self.body.update(cx, |body, cx| {
            body.set_text(BODY, cx);
            let at = BODY.find("minutes").unwrap_or(0);
            body.select(at..at + "minutes".len(), cx);
        });
        self.count(cx);
    }

    /// `LAGOM_SCREEN=compose-cc`: Cc shown and focused, for its focus ring.
    pub fn add_cc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_cc = true;
        window.focus(&self.cc.focus_handle(cx));
        cx.notify();
    }

    /// Esc kept the draft: the sheet comes back as it was, focus where it was.
    pub fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep = false;
        self.set(Phase::Open, cx);
        let focus = self.refocus.take().unwrap_or_else(|| self.body.focus_handle(cx));
        window.focus(&focus);
    }

    pub fn keeps_draft(&self) -> bool {
        self.keep
    }

    fn on_field(&mut self, field: &Entity<TextInput>, e: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            TextInputEvent::Escape => self.close(window, cx),
            // Enter in To, Cc or Subject moves on, as Tab does.
            TextInputEvent::Enter => window.focus_next(),
            TextInputEvent::Changed if *field == self.body => {
                self._count = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(COUNT_DELAY).await;
                    this.update(cx, |this, cx| this.count(cx)).ok();
                }));
            }
            TextInputEvent::Changed => {}
        }
    }

    fn count(&mut self, cx: &mut Context<Self>) {
        self.words = self.body.read(cx).text().split_whitespace().count();
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent || self.phase == Phase::Gone {
            return;
        }
        self.phase = Phase::Sent;
        self.keep = false;
        // Off the fading fields: ⌘Z now undoes the send, and typing goes nowhere.
        window.focus(&self.focus);
        // Replacing the task cancels an older timer.
        self._dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO_WINDOW).await;
            this.update(cx, |this, cx| this.set(Phase::Gone, cx)).ok();
        }));
        cx.notify();
    }

    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent {
            self._dismiss = None;
            self.set(Phase::Restored, cx);
            window.focus(&self.body.focus_handle(cx));
        }
    }

    /// The faded sheet keeps its hitboxes while Sent, so Discard must not act then.
    fn discard(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Sent {
            self.keep = false;
            self.set(Phase::Gone, cx);
        }
    }

    /// Esc: the sheet goes, the draft stays.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent {
            return;
        }
        self.refocus = self.focused_field(window, cx).map(|i| self.fields()[i].focus_handle(cx));
        self.keep = true;
        self.set(Phase::Gone, cx);
    }

    fn fields(&self) -> [&Entity<TextInput>; 4] {
        [&self.to, &self.cc, &self.subject, &self.body]
    }

    /// 0 To, 1 Cc, 2 Subject, 3 Body.
    pub(crate) fn focused_field(&self, window: &Window, cx: &gpui::App) -> Option<usize> {
        self.fields().iter().position(|f| f.focus_handle(cx).is_focused(window))
    }

    #[cfg(test)]
    pub(crate) fn body_text(&self, cx: &gpui::App) -> String {
        self.body.read(cx).text().to_string()
    }

    fn set(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        if phase == Phase::Gone {
            cx.emit(gpui::DismissEvent);
        }
        cx.notify();
    }

    /// Keys a field leaves alone bubble up here: ⌘↵, ⌘⌫, ⌘⇧C, and Esc while no field has focus.
    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let (cmd, shift) = (k.modifiers.platform, k.modifiers.shift);
        match k.key.as_str() {
            "escape" => self.close(window, cx),
            "enter" if cmd => self.send(window, cx),
            "backspace" if cmd => self.discard(cx),
            "c" if cmd && shift => self.add_cc(window, cx),
            _ => return,
        }
        cx.stop_propagation();
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
        let grow = |input: &Entity<TextInput>| div().flex_1().min_w(px(0.)).child(input.clone());

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
                        pressable(div().id("discard"), t, false)
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(radius::CONTROL))
                            .text_color(t.ink2)
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
                    .child(field("To").child(grow(&self.to)).when(!self.show_cc, |d| {
                        d.child(
                            pressable(div().id("add-cc"), t, false)
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .h(px(30.))
                                .px(px(10.))
                                .rounded(px(radius::CONTROL))
                                .text_color(t.ink2)
                                .on_click(cx.listener(|this, _, window, cx| this.add_cc(window, cx)))
                                .child("Cc")
                                .child(mono("⌘⇧C", t.ink2)),
                        )
                    }))
                    .when(self.show_cc, |d| d.child(field("Cc").child(grow(&self.cc))))
                    .child(field("Subject").child(grow(&self.subject).font_weight(FontWeight::SEMIBOLD))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(24.))
                    .py(px(20.))
                    .text_size(px(size::BODY))
                    .line_height(relative(1.6))
                    .child(self.body.clone()),
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
                        key_button(t, "Send", "⌘↵", Some(t.accent))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    )
                    .child(key_button(t, "Attach", "⌘⇧A", None))
                    .child(key_button(t, "Send later", "⌘L", None))
                    .child(div().flex_1())
                    .child(mono(
                        match self.words {
                            1 => "1 word · saved".to_string(),
                            n => format!("{n} words · saved"),
                        },
                        t.ink2,
                    )),
            )
    }

    fn toast(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        toast(
            t,
            "Sent",
            key_button(t, "Undo", "⌘Z", None)
                .on_click(cx.listener(|this, _, window, cx| this.undo(window, cx))),
        )
    }
}

/// A toast at the bottom of the window: a label and an Undo button.
pub(crate) fn toast(t: &Theme, label: &'static str, undo: impl IntoElement) -> impl IntoElement {
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
                .child(label)
                .child(undo),
        )
        .with_animation("toast-in", Animation::new(Duration::from_millis(160)), |d, v| {
            d.opacity(ease_out_quint()(v)).bottom(px(16. + 8. * spring(v)))
        })
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
        // Bottom padding shifts the centred sheet up by half its value.
        let out = |d: Div, v: f32| d.opacity(1. - v).pb(px(160. * v));
        let layer = match self.phase {
            // Leaving: ease-out, as a spring would bounce a sheet that is on its way off screen.
            Phase::Sent => layer.with_animation("compose-out", Animation::new(SLIDE).with_easing(ease_out_quint()), out).into_any_element(),
            // Arriving (open, or back after Undo): rises 12 px into place on a spring and fades in.
            Phase::Open | Phase::Restored => layer
                .with_animation(if self.phase == Phase::Open { "compose-open" } else { "compose-in" }, Animation::new(SLIDE), |d, v| {
                    d.opacity(ease_out_quint()(v)).pt(px(24. * (1. - spring(v))))
                })
                .into_any_element(),
            Phase::Gone => layer.into_any_element(),
        };

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            // A focused field takes ⌘Z first; after Send the sheet holds focus, so Undo lands here.
            .on_action(cx.listener(|this, _: &crate::main_window::Undo, window, cx| this.undo(window, cx)))
            // Block clicks to the inbox while the sheet is up; after Send the inbox is usable again.
            .when(self.phase != Phase::Sent, |d| d.occlude())
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .child(layer)
            .when(self.phase == Phase::Sent, |d| d.child(self.toast(cx)))
    }
}

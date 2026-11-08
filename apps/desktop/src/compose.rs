//! Compose, Concept B structure (Compose on the Claude design canvas) in Concept D tokens.
//! A reply sheet over the main window. To, Cc (on demand), Subject and Body are `TextInput`s.
//! Esc closes and keeps the draft in memory; Send and Discard (⌘⌫) end it.
//! shortcut: the draft lives in memory only and is lost on quit; save it to Drafts with lagom-core.

use std::{io::Read, path::{Path, PathBuf}, time::Duration};

use gpui::{
    div, ease_out_quint, prelude::*, px, relative, Animation, AnimationExt, Context, Div, Entity,
    ExternalPaths, FocusHandle, Focusable, FontWeight, KeyDownEvent, PathPromptOptions, Rgba, Subscription, Task, Window,
};

use crate::main_window::{button, mono, pressable};
use crate::motion::spring;
use crate::mail::recipients;
use crate::text_input::{TextInput, TextInputEvent};
use lagom_core::smtp::Outgoing;
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT, READING_LEADING};

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
    /// "Account · address" in the From row.
    from: String,
    /// One per sheet, kept across resends, so clients can spot a duplicate. It does not stop one.
    message_id: String,
    /// In-Reply-To and References, for a reply.
    reply: Option<(Option<String>, Vec<String>)>,
    /// Why Send did nothing, shown in the footer.
    problem: Option<&'static str>,
    /// After Send, the sheet has finished leaving and no longer renders.
    slid_out: bool,
    import_error: Option<&'static str>,
    _error: Option<Task<()>>,
    _count: Option<Task<()>>,
    _dismiss: Option<Task<()>>,
    _appearance: Subscription,
    _fields: Vec<Subscription>,
}

/// What the main window hears from the sheet besides `DismissEvent`.
pub enum ComposeEvent {
    /// Send was pressed: the sheet is no longer a modal, the Undo window runs.
    Sending,
    /// Undo in the window: the sheet is a modal again.
    Restored,
    /// The Undo window ended: send this now.
    Send(Box<Outgoing>),
}

impl gpui::EventEmitter<ComposeEvent> for Compose {}

impl Compose {
    /// `reply`: In-Reply-To and References. `None` is a new message.
    pub fn new(from: &str, to: &str, subject: &str, reply: Option<(Option<String>, Vec<String>)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        let to = cx.new(|cx| TextInput::new(window, cx).bordered().placeholder("Recipients").with_text(to));
        let cc = cx.new(|cx| TextInput::new(window, cx).bordered().placeholder("Recipients"));
        let subject = cx.new(|cx| TextInput::new(window, cx).placeholder("Subject").with_text(subject));
        let body = cx.new(|cx| TextInput::new(window, cx).markdown().placeholder("Write your message"));
        let fields = [&to, &cc, &subject, &body].map(|f| cx.subscribe_in(f, window, Self::on_field)).into();
        // A reply starts in the body, a new message in To.
        let first = if reply.is_some() { &body } else { &to };
        window.focus(&first.focus_handle(cx));
        Self {
            from: from.into(),
            message_id: lagom_core::smtp::message_id(from),
            reply,
            problem: None,
            slid_out: false,
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
            import_error: None,
            _error: None,
            _count: None,
            _dismiss: None,
            _appearance: appearance,
            _fields: fields,
        }
    }

    pub(crate) fn focus_body(&self, window: &mut Window, cx: &gpui::App) {
        window.focus(&self.body.focus_handle(cx));
    }

    pub(crate) fn insert_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_modal() { return; }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true, directories: false, multiple: false, prompt: Some("Insert file".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            match paths.await {
                Ok(Ok(Some(paths))) => { this.update_in(cx, |this, window, cx| this.import_paths(&paths, window, cx)).ok(); }
                Ok(Ok(None)) => {}
                _ => { this.update(cx, |this, cx| this.file_error("Could not open the file picker", cx)).ok(); }
            }
        }).detach();
    }

    fn file_error(&mut self, message: &'static str, cx: &mut Context<Self>) {
        self.import_error = Some(message);
        self._error = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO_WINDOW).await;
            this.update(cx, |this, cx| { this.import_error = None; cx.notify(); }).ok();
        }));
        cx.notify();
    }

    fn import_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_modal() { return; }
        let [path] = paths else { return self.file_error("Insert one Markdown or text file at a time", cx); };
        let path = path.clone();
        let read_path = path.clone();
        let read = cx.background_executor().spawn(async move { read_file(&read_path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| {
                if !this.is_modal() { return; }
                match result {
                    Ok(text) => {
                        this.insert_file_text(&path, &text, cx);
                        this.focus_body(window, cx);
                    }
                    Err(error) => this.file_error(error, cx),
                }
            }).ok();
        }).detach();
    }

    fn insert_file_text(&mut self, path: &Path, text: &str, cx: &mut Context<Self>) {
        let mut body = text;
        if markdown_path(path) && self.body.read(cx).text().is_empty() && self.subject.read(cx).text().is_empty() {
            let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
            if let Some(heading) = first.trim_end_matches('\r').strip_prefix("# ").filter(|h| !h.trim().is_empty()) {
                self.subject.update(cx, |subject, cx| subject.insert(heading.trim(), false, cx));
                body = rest;
            }
        }
        self.body.update(cx, |input, cx| input.insert(body, false, cx));
        self.import_error = None;
        cx.notify();
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
        // A sent sheet is off screen: focus there would take ⌘Z away from Undo send.
        if matches!(self.phase, Phase::Sent | Phase::Gone) {
            return;
        }
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

    pub(crate) fn outgoing(&self, cx: &gpui::App) -> Outgoing {
        let text = |f: &Entity<TextInput>| f.read(cx).text().to_string();
        let (in_reply_to, references) = self.reply.clone().unwrap_or_default();
        Outgoing {
            to: recipients(&text(&self.to)),
            cc: recipients(&text(&self.cc)),
            subject: text(&self.subject),
            text: text(&self.body),
            in_reply_to,
            references,
            message_id: Some(self.message_id.clone()),
            ..Default::default()
        }
    }

    /// Nothing leaves before the Undo window ends: the timer sends, Undo drops the timer.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Sent || self.phase == Phase::Gone {
            return;
        }
        let mail = self.outgoing(cx);
        if mail.to.is_empty() && mail.cc.is_empty() {
            self.problem = Some("Add a recipient");
            window.focus(&self.to.focus_handle(cx));
            cx.notify();
            return;
        }
        if mail.to.iter().chain(&mail.cc).any(|a| !a.contains('@')) {
            self.problem = Some("A recipient is not an email address");
            window.focus(&self.to.focus_handle(cx));
            cx.notify();
            return;
        }
        self.problem = None;
        self.phase = Phase::Sent;
        self.keep = false;
        // Off the fading fields. The main window takes focus on `Sending`, so inbox keys work
        // during the Undo window and its ⌘Z undoes this send first.
        window.focus(&self.focus);
        // Replacing the task cancels an older timer.
        self.slid_out = false;
        self._dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLIDE).await;
            this.update(cx, |this, cx| {
                this.slid_out = true;
                cx.notify();
            })
            .ok();
            cx.background_executor().timer(UNDO_WINDOW - SLIDE).await;
            this.update(cx, |this, cx| {
                cx.emit(ComposeEvent::Send(Box::new(this.outgoing(cx))));
                this.set(Phase::Gone, cx);
            })
            .ok();
        }));
        cx.emit(ComposeEvent::Sending);
        cx.notify();
    }

    /// The sender address the sheet shows.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// Quit or close during the Undo window: the mail goes now instead of with the dropped timer.
    pub fn send_now(&mut self, cx: &mut Context<Self>) -> Option<Outgoing> {
        if self.phase != Phase::Sent {
            return None;
        }
        self._dismiss = None;
        let mail = self.outgoing(cx);
        self.set(Phase::Gone, cx);
        Some(mail)
    }

    pub fn undoable(&self) -> bool {
        self.phase == Phase::Sent
    }

    /// Undo send. False once the window has passed.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.phase != Phase::Sent {
            return false;
        }
        self._dismiss = None;
        self.set(Phase::Restored, cx);
        cx.emit(ComposeEvent::Restored);
        window.focus(&self.body.focus_handle(cx));
        true
    }

    /// The sheet blocks the inbox. After Send it only shows the Undo toast.
    pub fn is_modal(&self) -> bool {
        matches!(self.phase, Phase::Open | Phase::Restored)
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

    fn sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .id("compose-sheet")
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.import_paths(paths.paths(), window, cx)))
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
            // The fading sheet keeps its hitboxes: a click must not refocus a field and take ⌘Z.
            .when(self.phase == Phase::Sent, |d| d.capture_any_mouse_down(|_, _, cx| cx.stop_propagation()))
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(if self.reply.is_some() { "Reply" } else { "New message" }))
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
                            .child("Discard"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(24.))
                    .child(field("From").child(self.from.clone()))
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
                                .child("Cc"),
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
                    .line_height(relative(READING_LEADING))
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
                        button(t, "Send", Some(t.accent))
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    )
                    // shortcut: Attach and Send later are left out until the core supports them.
                    .child(div().flex_1())
                    .child(match self.problem {
                        Some(p) => div().text_size(px(size::SNIPPET)).text_color(t.danger).child(p),
                        None => mono(
                            match self.words {
                                1 => "1 word".to_string(),
                                n => format!("{n} words"),
                            },
                            t.ink2,
                        ),
                    }),
            )
    }

}

/// Where toasts stack: bottom-centre of the element it is placed in, `bottom` px up.
pub(crate) fn toast_layer(bottom: f32) -> Div {
    div().absolute().bottom(px(bottom)).left_0().right_0().flex().flex_col().items_center().gap(px(8.))
}

/// A toast: a label and, optionally, an Undo button. Put it in a `toast_layer`.
pub(crate) fn toast(t: &Theme, label: impl Into<gpui::SharedString>, undo: Option<gpui::AnyElement>) -> impl IntoElement {
    let label = label.into();
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(40.))
        .pl(px(14.))
        .pr(px(if undo.is_some() { 6. } else { 14. }))
        .bg(t.raised)
        .border_1()
        .border_color(t.line)
        .rounded(px(radius::PANEL))
        .shadow_lg()
        .font_weight(FontWeight::MEDIUM)
        .child(label)
        .children(undo)
        .with_animation("toast-in", Animation::new(Duration::from_millis(160)), |d, v| {
            d.opacity(ease_out_quint()(v)).relative().top(px(8. * (1. - spring(v))))
        })
}

impl gpui::EventEmitter<gpui::DismissEvent> for Compose {}

impl Render for Compose {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.phase == Phase::Gone {
            return div();
        }
        let t = self.theme;
        // A light veil, as AppKit sheets barely dim the window behind them.
        let scrim = Rgba { a: 0.12, ..gpui::rgb(0x000000) };
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
        // Once the sheet has slid out, its invisible hitboxes must not catch inbox clicks.
        let layer = (!(self.phase == Phase::Sent && self.slid_out)).then_some(layer);

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            // A focused field takes ⌘Z first. After Send the inbox has focus and routes ⌘Z here.
            .on_action(cx.listener(|this, _: &crate::main_window::Undo, window, cx| {
                this.undo(window, cx);
            }))
            // Block clicks to the inbox while the sheet is up; after Send the inbox is usable again.
            .when(self.phase != Phase::Sent, |d| d.occlude())
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .children(layer)
            // The "Sending" toast after Send is the main window's, placed over the reading pane.
            .when_some(self.import_error.filter(|_| self.is_modal()), |d, message| d.child(toast_layer(24.).child(toast(&t, message, None))))
    }
}

fn markdown_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

fn read_file(path: &Path) -> Result<String, &'static str> {
    if !markdown_path(path) && !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("txt")) {
        return Err("Choose a .md, .markdown or .txt file");
    }
    const LIMIT: u64 = 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|_| "Could not open the file")?;
    if !file.metadata().is_ok_and(|m| m.is_file()) { return Err("Choose a regular text file"); }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|_| "Could not read the file")?;
    if bytes.len() as u64 > LIMIT { return Err("The file is larger than 1 MB"); }
    String::from_utf8(bytes).map_err(|_| "The file must be UTF-8 text")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext) -> (Entity<Compose>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::text_input::bind_keys(cx);
            cx.bind_keys([gpui::KeyBinding::new("secondary-z", crate::main_window::Undo, Some("TextInput"))]);
        });
        cx.add_window_view(|window, cx| Compose::new("a@example.com", "b@example.com", "", Some((None, vec![])), window, cx))
    }

    #[gpui::test]
    fn bold_and_italic_wrap_toggle_and_undo(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("Hej Åsa");
        cx.simulate_keystrokes("secondary-a secondary-b");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "**Hej Åsa**");
        cx.simulate_keystrokes("secondary-b");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "Hej Åsa");
        cx.simulate_keystrokes("secondary-i");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "_Hej Åsa_");
        cx.simulate_keystrokes("secondary-i right secondary-b");
        cx.simulate_input("fet");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "Hej Åsa**fet**");
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "Hej Åsa****");
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "Hej Åsa");
    }

    #[gpui::test]
    fn enter_continues_and_ends_lists(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        for (source, after) in [("- ett", "- ett\n- "), ("* ett", "* ett\n* "), ("9. nio", "9. nio\n10. "), ("  - nästlad", "  - nästlad\n  - ")] {
            view.update(cx, |v, cx| v.body.update(cx, |body, cx| body.set_text(source, cx)));
            cx.simulate_keystrokes("enter");
            assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), after);
            cx.simulate_keystrokes("enter");
            assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), format!("{source}\n"));
        }
    }

    #[gpui::test]
    fn list_shortcuts_toggle_selected_lines(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("ett\ntvå");
        cx.simulate_keystrokes("secondary-a secondary-shift-8");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "- ett\n- två");
        cx.simulate_keystrokes("secondary-shift-7");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "1. ett\n2. två");
        cx.simulate_keystrokes("secondary-shift-7");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "ett\ntvå");
    }

    #[gpui::test]
    fn markdown_heading_fills_only_an_empty_subject_and_body(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, cx| v.insert_file_text(Path::new("brev.md"), "# Hälsningar\r\n\r\n**Hej**", cx));
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).subject), "Hälsningar");
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "\n**Hej**");
        for (subject, body, path) in [("Befintligt", "", "brev.md"), ("", "text", "brev.md"), ("", "", "brev.txt")] {
            view.update(cx, |v, cx| {
                v.subject.update(cx, |input, cx| input.set_text(subject, cx));
                v.body.update(cx, |input, cx| input.set_text(body, cx));
                v.insert_file_text(Path::new(path), "# Rubrik\ntext", cx);
            });
            assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).subject), subject);
            assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), format!("{body}# Rubrik\ntext"));
        }
        view.update(cx, |v, cx| {
            v.body.update(cx, |input, cx| { input.set_text("före efter", cx); input.select(6..6, cx); });
            v.insert_file_text(Path::new("brev.markdown"), "mitten ", cx);
        });
        assert_eq!(view.read_with(cx, |v, cx| v.body_text(cx)), "före mitten efter");
    }

    #[test]
    fn file_import_checks_extension_size_and_utf8() {
        let dir = std::env::temp_dir().join(format!("lagom-markdown-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.MD");
        std::fs::write(&path, "# Hälsningar").unwrap();
        assert_eq!(read_file(&path).unwrap(), "# Hälsningar");
        assert!(read_file(&dir.join("missing.txt")).is_err());
        assert!(read_file(&dir.join("image.png")).is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_file(&path).unwrap_err().contains("UTF-8"));
        std::fs::write(&path, vec![b'a'; 1024 * 1024]).unwrap();
        assert_eq!(read_file(&path).unwrap().len(), 1024 * 1024);
        std::fs::write(&path, vec![b'a'; 1024 * 1024 + 1]).unwrap();
        assert!(read_file(&path).unwrap_err().contains("1 MB"));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

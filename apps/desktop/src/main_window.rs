//! Main window, Concept D (MainWindowD on the Claude design canvas).
//! Sidebar, message list and reading pane. Sample data until lagom-core syncs real mail.

use gpui::{
    div, prelude::*, px, relative, svg, Context, Div, FontWeight, Rgba, SharedString, Window,
};

use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

struct Message {
    from: &'static str,
    time: &'static str,
    subject: &'static str,
    snippet: &'static str,
    unread: bool,
}

// Sample inbox from MainWindowD.
const INBOX: &[Message] = &[
    Message { from: "Ingrid Holm", time: "09:42", subject: "Board meeting moved to Thursday", snippet: "Thursday at 16:00 works for everyone except the projector.", unread: true },
    Message { from: "Johan Ek", time: "08:57", subject: "Invoice 2026-114", snippet: "Attached. Same as last month, minus the coffee machine.", unread: true },
    Message { from: "Build bot", time: "08:15", subject: "Nightly build passed", snippet: "All tests green. Nothing to see here.", unread: false },
    Message { from: "Maja Lind", time: "Yesterday", subject: "Lunch Thursday?", snippet: "Somewhere with a lunch special. You choose, I will complain.", unread: true },
    Message { from: "Housing co-op", time: "Yesterday", subject: "Laundry room schedule, December", snippet: "The new schedule is on the door. It is also attached.", unread: false },
    Message { from: "Tickets", time: "Tue", subject: "Stockholm C → Göteborg C, 14 Nov", snippet: "Coach 3, seat 41. Quiet carriage.", unread: false },
    Message { from: "Lagom releases", time: "Mon", subject: "Version 0.4 is out", snippet: "Faster threads, fewer settings. Changelog inside.", unread: false },
];

pub struct MainWindow {
    theme: Theme,
    selected: usize,
    _appearance: gpui::Subscription,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        Self {
            theme: Theme::for_appearance(window.appearance(), DEFAULT_ACCENT),
            selected: 0,
            _appearance: appearance,
        }
    }
}

fn mono(text: impl Into<SharedString>, color: Rgba) -> Div {
    div().font_family(font::MONO).text_size(px(size::MONO)).text_color(color).child(text.into())
}

fn icon(path: &'static str, color: Rgba, s: f32) -> impl IntoElement {
    svg().path(path).size(px(s)).flex_shrink_0().text_color(color)
}

fn app_mark(t: &Theme) -> Div {
    // 22 px app icon: accent tile, white bar, accent dot (1024 master scaled).
    div()
        .size(px(22.))
        .flex_shrink_0()
        .rounded(px(4.9))
        .bg(t.accent)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(13.75))
                .h(px(3.8))
                .rounded_full()
                .bg(gpui::white())
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(2.)).rounded_full().bg(t.accent)),
        )
}

fn key_button(t: &Theme, label: &'static str, key: &'static str, primary: bool) -> Div {
    let base = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(34.))
        .px(px(14.))
        .rounded(px(radius::CONTROL))
        .text_size(px(13.))
        .font_weight(if primary { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .child(label);
    if primary {
        base.bg(t.accent).text_color(t.accent_ink).child(mono(key, t.accent_ink))
    } else {
        base.border_1().border_color(t.line).text_color(t.ink).child(mono(key, t.ink2))
    }
}

impl MainWindow {
    fn sidebar(&self) -> Div {
        let t = &self.theme;
        let nav = |path: &'static str, label: &'static str, count: Option<&'static str>, active: bool| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(radius::CONTROL))
                .when(active, |d| d.bg(t.sel).text_color(t.ink).font_weight(FontWeight::SEMIBOLD))
                .when(!active, |d| d.text_color(t.ink2))
                .child(icon(path, if active { t.ink } else { t.ink2 }, 16.))
                .child(div().flex_1().child(label))
                .when_some(count, |d, c| {
                    if active {
                        d.child(
                            div()
                                .min_w(px(20.))
                                .h(px(20.))
                                .px(px(6.))
                                .rounded_full()
                                .bg(t.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(mono(c, t.accent_ink).font_weight(FontWeight::MEDIUM)),
                        )
                    } else {
                        d.child(mono(c, t.ink2))
                    }
                })
        };
        let account = |name: &'static str, address: &'static str, active: bool| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(40.))
                .px(px(10.))
                .rounded(px(radius::CONTROL))
                .child(
                    div()
                        .size(px(8.))
                        .rounded_full()
                        .when(active, |d| d.bg(t.accent))
                        .when(!active, |d| d.border_1().border_color(t.ink2)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_color(t.ink).font_weight(FontWeight::MEDIUM).child(name))
                        .child(div().text_size(px(size::SMALL)).text_color(t.ink2).child(address)),
                )
        };

        div()
            .w(px(248.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(20.))
            .px(px(12.))
            .py(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(40.))
                    .px(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(app_mark(t))
                    .child(div().flex_1().child("Home"))
                    .child(icon("icons/chevron.svg", t.ink2, 14.)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(36.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.raised)
                    .text_color(t.ink2)
                    .child(icon("icons/search.svg", t.ink2, 15.))
                    .child(div().flex_1().child("Search or run"))
                    .child(
                        mono("⌘K", t.ink2)
                            .px(px(6.))
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(radius::KEY)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(nav("icons/inbox.svg", "Inbox", Some("7"), true))
                    .child(nav("icons/later.svg", "Later", Some("2"), false))
                    .child(nav("icons/sent.svg", "Sent", None, false))
                    .child(nav("icons/done.svg", "Done", Some("G D"), false)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .px(px(10.))
                            .pb(px(6.))
                            .text_size(px(size::SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.ink2)
                            .child("Accounts"),
                    )
                    .child(account("Home", "you@home.example", true))
                    .child(account("Work", "you@work.example", false)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(34.))
                    .px(px(10.))
                    .text_color(t.ink2)
                    .child("Settings")
                    .child(mono("⌘,", t.ink2)),
            )
    }

    fn message_list(&self) -> Div {
        let t = &self.theme;
        let rows = INBOX.iter().enumerate().map(|(i, m)| {
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(radius::CONTROL))
                .when(i == self.selected, |d| d.bg(t.sel))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().size(px(7.)).rounded_full().when(m.unread, |d| d.bg(t.accent)))
                        .child(
                            div()
                                .flex_1()
                                .when(m.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(m.from),
                        )
                        .child(mono(m.time, t.ink2)),
                )
                .child(
                    div()
                        .pl(px(15.))
                        .when(m.unread, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(m.subject),
                )
                .child(
                    div()
                        .pl(px(15.))
                        .text_size(px(size::SNIPPET))
                        .text_color(t.ink2)
                        .truncate()
                        .child(m.snippet),
                )
        });

        let filter = div()
            .flex()
            .p(px(2.))
            .rounded(px(radius::CONTROL))
            .bg(t.subtle)
            .border_1()
            .border_color(t.line)
            .text_size(px(size::SMALL))
            .child(
                div()
                    .h(px(26.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(t.raised)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("All"),
            )
            .child(div().h(px(26.)).px(px(12.)).flex().items_center().text_color(t.ink2).child("Unread"));

        div()
            .w(px(420.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(56.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .font_family(font::DISPLAY)
                            .text_size(px(size::TITLE))
                            .font_weight(FontWeight::BOLD)
                            .child("Inbox"),
                    )
                    .child(filter),
            )
            .child(div().flex_1().overflow_hidden().flex().flex_col().p(px(6.)).children(rows))
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(mono("7 left", t.ink2))
                    .child(mono("J K move", t.ink2))
                    .child(mono("E done", t.ink2))
                    .child(mono("⌘K run", t.ink2)),
            )
    }

    fn reading_pane(&self) -> Div {
        let t = &self.theme;
        let m = &INBOX[self.selected];
        let para = |text: &'static str| div().child(text);

        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(56.))
                    .px(px(24.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(key_button(t, "Done", "E", true))
                    .child(key_button(t, "Reply", "R", false))
                    .child(key_button(t, "Later", "L", false))
                    .child(key_button(t, "Forward", "F", false))
                    .child(div().flex_1())
                    .child(mono(format!("{} / {}", self.selected + 1, INBOX.len()), t.ink2)),
            )
            .child(
                div()
                    .flex_1()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .pt(px(36.))
                    .px(px(40.))
                    .pb(px(28.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .child(
                                div()
                                    .font_family(font::DISPLAY)
                                    .text_size(px(size::SUBJECT))
                                    .font_weight(FontWeight::BOLD)
                                    .line_height(relative(1.15))
                                    .child(m.subject),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .size(px(32.))
                                            .rounded_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(t.subtle)
                                            .border_1()
                                            .border_color(t.line)
                                            .text_size(px(size::SMALL))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("IH"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap(px(1.))
                                            .child(div().font_weight(FontWeight::SEMIBOLD).child(m.from))
                                            .child(
                                                div()
                                                    .text_size(px(size::SMALL))
                                                    .text_color(t.ink2)
                                                    .child("ingrid@holm.example → you"),
                                            ),
                                    )
                                    .child(mono("Fri 6 Nov · 09:42", t.ink2)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .text_size(px(size::BODY))
                            .line_height(relative(1.6))
                            .child(para("Hi,"))
                            .child(para("Thursday at 16:00 works for everyone except the projector, which has asked to be replaced. I will bring a laptop instead."))
                            .child(para("The minutes from last time are attached. Please read them before the meeting, not during it."))
                            .child(para("Ingrid"))
                            .child(
                                div()
                                    .flex()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(12.))
                                            .px(px(14.))
                                            .py(px(10.))
                                            .rounded(px(radius::ATTACHMENT))
                                            .border_1()
                                            .border_color(t.line)
                                            .bg(t.subtle)
                                            .text_size(px(size::SNIPPET))
                                            .line_height(relative(1.3))
                                            .child(div().font_weight(FontWeight::SEMIBOLD).child("minutes-oct.pdf"))
                                            .child(mono("212 KB", t.ink2)),
                                    ),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(48.))
                            .px(px(18.))
                            .rounded(px(radius::ATTACHMENT))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.subtle)
                            .text_color(t.ink2)
                            .child("Reply to Ingrid")
                            .child(mono("R", t.ink2)),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .size_full()
            .flex()
            .bg(t.subtle)
            .text_color(t.ink)
            .font_family(font::UI)
            .text_size(px(size::BASE))
            .child(self.sidebar())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .my(px(8.))
                    .mr(px(8.))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(radius::PANEL))
                    .overflow_hidden()
                    .child(self.message_list())
                    .child(self.reading_pane()),
            )
    }
}

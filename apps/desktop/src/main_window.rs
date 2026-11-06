//! Main window, Concept D (MainWindowD on the Claude design canvas).
//! Sidebar, message list and reading pane. Sample data until lagom-core syncs real mail.

use std::time::{Duration, Instant};

use gpui::{
    actions, div, ease_out_quint, prelude::*, px, relative, svg, Animation, AnimationExt, AnyElement,
    App, Context, Div, ElementId, FocusHandle, FontWeight, KeyBinding, Rgba, ScrollDelta,
    ScrollWheelEvent, SharedString, TouchPhase, Window,
};

use crate::command_palette::CommandPalette;
use crate::compose::Compose;
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

// Messages already snoozed in the sample data.
const LATER_SAMPLE: usize = 2;

actions!(main_window, [SelectNext, SelectPrev, Done, Later, Undo, OpenPalette, Reply, Cancel]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("MainWindow");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("k", SelectPrev, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("e", Done, ctx),
        KeyBinding::new("l", Later, ctx),
        KeyBinding::new("secondary-z", Undo, ctx),
        KeyBinding::new("secondary-k", OpenPalette, ctx),
        KeyBinding::new("r", Reply, ctx),
        KeyBinding::new("escape", Cancel, ctx),
    ]);
}

// Motion per docs/motion.md: the state changes at once, the animation only paints on top.
// shortcut: GPUI 0.2.2 exposes no "reduce motion" setting, so motion always plays. Read
// NSWorkspace.accessibilityDisplayShouldReduceMotion (and the Windows/Linux equivalents) and fall
// back to a short fade once GPUI or lagom-core can provide it.
const MOTION: Duration = Duration::from_millis(180);
// GPUI line height is phi x font size. A row is two BASE lines, one SNIPPET line, gaps and padding.
const ROW_H: f32 = 2. * 10. + 2. * 3. + (2. * size::BASE + size::SNIPPET) * 1.618_034;

fn motion() -> Animation {
    Animation::new(MOTION).with_easing(ease_out_quint())
}

/// A removed row that still paints its exit motion. It is not in the inbox any more.
struct Ghost {
    id: usize,
    ix: usize,
    what: Move,
    seq: usize,
    at: Instant,
    // Where a swipe let go of the row, so the exit motion starts there. 0 for E and L.
    dx: f32,
}

pub struct MainWindow {
    theme: Theme,
    inbox: Inbox,
    focus: FocusHandle,
    ghosts: Vec<Ghost>,
    // Rows that undo put back, still playing the reverse motion.
    entering: Vec<Ghost>,
    seq: usize,
    swipe: Swipe,
    // The row under the pointer for the current scroll event. Rows set it before the list reads it.
    under: Option<usize>,
    // (message id, offset it springs back from, seq) for a swipe that did not reach the threshold.
    spring: Option<(usize, f32, usize)>,
    palette: Option<gpui::Entity<CommandPalette>>,
    compose: Option<gpui::Entity<Compose>>,
    _appearance: gpui::Subscription,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        // `LAGOM_SCREEN` opens one screen at launch, for screenshots.
        let screen = std::env::var("LAGOM_SCREEN").unwrap_or_default();
        let mut this = Self {
            theme: Theme::for_appearance(window.appearance(), DEFAULT_ACCENT),
            inbox: Inbox::new(if screen == "empty" { 0 } else { INBOX.len() }, LATER_SAMPLE),
            focus,
            ghosts: Vec::new(),
            entering: Vec::new(),
            seq: 0,
            swipe: Swipe::Idle,
            under: None,
            spring: None,
            palette: None,
            compose: None,
            _appearance: appearance,
        };
        match screen.as_str() {
            "palette" => this.open_palette("done", window, cx),
            "compose" => this.open_compose(window, cx),
            // A row held mid-swipe, past the Done threshold, for screenshots.
            "swipe" => this.swipe = Swipe::Row { id: 1, dx: 150. },
            "swipe-later" => this.swipe = Swipe::Row { id: 1, dx: -90. },
            _ => {}
        }
        this
    }

    // Overlays sit inside this key context, so its bindings fire first: hand the key on to them.
    fn overlay_open(&self, cx: &mut Context<Self>) -> bool {
        let open = self.palette.is_some() || self.compose.is_some();
        if open {
            cx.propagate();
        }
        open
    }

    fn open_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The overlay swallows the gesture's Ended, so end a swipe now.
        self.release(false, cx);
        let palette = cx.new(|cx| CommandPalette::new(query, cx));
        cx.subscribe_in(&palette, window, |this, _, _: &gpui::DismissEvent, window, cx| {
            this.palette = None;
            window.focus(&this.focus);
            cx.notify();
        })
        .detach();
        window.focus(&gpui::Focusable::focus_handle(&palette, cx));
        self.palette = Some(palette);
        cx.notify();
    }

    fn open_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        let compose = cx.new(|cx| Compose::new(window, cx));
        cx.subscribe_in(&compose, window, |this, _, _: &gpui::DismissEvent, window, cx| {
            this.compose = None;
            window.focus(&this.focus);
            cx.notify();
        })
        .detach();
        self.compose = Some(compose);
        cx.notify();
    }

    fn act(&mut self, what: Move, cx: &mut Context<Self>) {
        self.act_at(self.inbox.selected, what, 0., cx);
    }

    fn act_at(&mut self, ix: usize, what: Move, dx: f32, cx: &mut Context<Self>) {
        let Some((id, ix)) = self.inbox.act_at(ix, what) else { return };
        self.seq += 1;
        for g in &mut self.ghosts {
            if g.ix > ix {
                g.ix -= 1;
            }
        }
        self.entering.retain(|g| g.id != id);
        self.spring = self.spring.filter(|s| s.0 != id);
        self.ghosts.push(Ghost { id, ix, what, seq: self.seq, at: Instant::now(), dx });
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some((what, id, ix)) = self.inbox.undo() else { return };
        self.seq += 1;
        self.ghosts.retain(|g| g.id != id);
        // Otherwise a finished spring on this row would hide the undo motion.
        self.spring = self.spring.filter(|s| s.0 != id);
        for g in &mut self.ghosts {
            if g.ix > ix {
                g.ix += 1;
            }
        }
        self.entering.push(Ghost { id, ix, what, seq: self.seq, at: Instant::now(), dx: 0. });
        cx.notify();
    }

    /// Two-finger swipe on the list. Rows bubble first (GPUI paints child listeners after the
    /// parent's), so `under` already names the row when this runs.
    fn on_scroll(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let under = self.under.take();
        // Line deltas come from a classic wheel: no phases and no horizontal gesture to follow.
        let ScrollDelta::Pixels(d) = e.delta else { return };
        if self.overlay_open(cx) {
            return;
        }
        let before = self.swipe;
        match e.touch_phase {
            TouchPhase::Started => {
                self.swipe = under.map_or(Swipe::Idle, Swipe::start);
                self.swipe.moved(d.x.into(), d.y.into());
            }
            // Momentum events after Ended also arrive as Moved; Idle ignores them, so none re-acts.
            TouchPhase::Moved => self.swipe.moved(d.x.into(), d.y.into()),
            TouchPhase::Ended => self.release(true, cx),
        }
        if self.swipe != before {
            cx.notify();
        }
    }

    /// Fingers lift (`act`) or Esc (not `act`): past the threshold the row goes, else it springs back.
    fn release(&mut self, act: bool, cx: &mut Context<Self>) {
        let Some((id, dx, what)) = self.swipe.end() else { return };
        let ix = self.inbox.items.iter().position(|&i| i == id);
        match (what.filter(|_| act), ix) {
            (Some(what), Some(ix)) => self.act_at(ix, what, dx, cx),
            _ => {
                self.seq += 1;
                self.spring = Some((id, dx, self.seq));
            }
        }
        cx.notify();
    }

    fn select(&mut self, f: impl FnOnce(&mut Inbox), cx: &mut Context<Self>) {
        f(&mut self.inbox);
        cx.notify();
    }
}

/// p = 0 is a resting row, p = 1 is fully gone. Undo plays the same curve backwards.
fn away<E: Styled>(row: E, t: Theme, what: Move, p: f32, dx: f32) -> E {
    if p <= 0. && dx == 0. {
        return row; // At rest: keep the selection background.
    }
    // Slide first, then close the gap. Padding shrinks too, or the row keeps 20 px.
    let collapse = ((p - 0.3) / 0.7).clamp(0., 1.);
    let row = row
        .relative()
        .h(px(ROW_H * (1. - collapse)))
        .py(px(10. * (1. - collapse)))
        .overflow_hidden()
        .opacity(1. - p);
    match what {
        // A swiped row already shows the full tint behind it, so it keeps it.
        Move::Done => row.left(px(dx + p * 200.)).bg(Rgba { a: 0.2 * if dx > 0. { 1. } else { (p * 3.).min(1.) }, ..t.accent }),
        // Towards Later in the sidebar: lift (shadow), shrink and move left.
        Move::Later => row.left(px(dx - p * 80.)).w(relative(1. - 0.3 * p)).bg(t.raised).shadow_md(),
    }
}

/// The action behind a swiped row: Done to the right (dx > 0), Later to the left.
/// The tint fills as the row nears the threshold; the icon takes colour once it is past.
fn behind(t: Theme, dx: f32) -> Div {
    let done = dx >= 0.;
    let ready = (dx.abs() / THRESHOLD).min(1.);
    let (path, color) = if done { ("icons/done.svg", t.accent) } else { ("icons/later.svg", t.ink) };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .when(!done, |d| d.justify_end())
        .px(px(20.))
        .rounded(px(radius::CONTROL))
        .bg(Rgba { a: 0.2 * ready, ..color })
        .child(icon(path, if ready >= 1. { color } else { t.ink2 }, 18.))
}

/// Counts roll in from below when they change. The id holds the value, so a new value restarts it.
fn roll(el: Div, id: String) -> AnyElement {
    el.with_animation(ElementId::Name(id.into()), motion(), |el, d| {
        el.relative().top(px((1. - d) * 6.)).opacity(d)
    })
    .into_any_element()
}

pub(crate) fn mono(text: impl Into<SharedString>, color: Rgba) -> Div {
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

pub(crate) fn key_button(t: &Theme, label: &'static str, key: &'static str, primary: bool) -> Div {
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
        let nav = |path: &'static str, label: &'static str, count: Option<SharedString>, active: bool| {
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
                    let numeric = c.chars().all(|ch| ch.is_ascii_digit());
                    let id = format!("{label}-{c}");
                    let text = |color| {
                        let el = mono(c.clone(), color);
                        if numeric { roll(el, id.clone()) } else { el.into_any_element() }
                    };
                    // A zero count is calm: no accent badge.
                    if active && c != "0" {
                        d.child(
                            div()
                                .min_w(px(20.))
                                .h(px(20.))
                                .px(px(6.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(t.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_weight(FontWeight::MEDIUM)
                                .child(text(t.accent_ink)),
                        )
                    } else {
                        d.child(text(t.ink2))
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
            .pb(px(14.))
            // Room for the macOS traffic lights.
            .when(cfg!(target_os = "macos"), |d| d.child(div().h(px(16.)).flex_shrink_0()))
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
                    .child(nav("icons/inbox.svg", "Inbox", Some(self.inbox.items.len().to_string().into()), true))
                    .child(nav("icons/later.svg", "Later", Some(self.inbox.later.to_string().into()), false))
                    .child(nav("icons/sent.svg", "Sent", None, false))
                    .child(nav("icons/done.svg", "Done", Some("G D".into()), false)),
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

    fn message_list(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let row = |m: &Message, selected: bool| {
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(radius::CONTROL))
                .when(selected, |d| d.bg(t.sel))
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
        };
        let theme = self.theme;
        let ghosts_at = |ix: usize| {
            self.ghosts.iter().filter(move |g| g.ix == ix).map(move |g| {
                let (what, dx) = (g.what, g.dx);
                row(&INBOX[g.id], false)
                    .with_animation(("ghost", g.seq), motion(), move |el, d| away(el, theme, what, d, dx))
                    .into_any_element()
            })
        };
        // shortcut: renders every row, fine for the sample inbox. Switch to uniform_list (visible
        // rows only, docs/performance.md rule 3) when real mail arrives.
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, &id) in self.inbox.items.iter().enumerate() {
            rows.extend(ghosts_at(i));
            let selected = i == self.inbox.selected;
            let el = row(&INBOX[id], selected)
                .id(("row", id))
                .on_click(cx.listener(move |this, _, _, cx| this.select(|inbox| inbox.select(i), cx)))
                .on_scroll_wheel(cx.listener(move |this, _, _, _| this.under = Some(id)));
            // Under the fingers the row follows dx; after a short swipe it springs back from it.
            let spring = self.spring.filter(|s| s.0 == id).map(|(_, dx, seq)| (dx, Some(seq)));
            if let Some((dx, seq)) = self.swipe.offset(id).map(|dx| (dx, None)).or(spring) {
                // Opaque, so the action behind shows only where the row has moved away.
                let el = el.relative().when(!selected, |d| d.bg(t.raised));
                let el = match seq {
                    None => el.left(px(dx)).into_any_element(),
                    Some(seq) => el
                        .with_animation(("spring", seq), motion(), move |el, d| el.left(px(dx * (1. - d))))
                        .into_any_element(),
                };
                rows.push(
                    div()
                        .relative()
                        .overflow_hidden()
                        .rounded(px(radius::CONTROL))
                        .child(behind(theme, dx))
                        .child(el)
                        .into_any_element(),
                );
                continue;
            }
            rows.push(match self.entering.iter().find(|g| g.id == id) {
                Some(g) => {
                    let what = g.what;
                    el.with_animation(("enter", g.seq), motion(), move |el, d| away(el, theme, what, 1. - d, 0.))
                        .into_any_element()
                }
                None => el.into_any_element(),
            });
        }
        rows.extend(ghosts_at(self.inbox.items.len()));

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
            .child(
                div()
                    .id("rows")
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .p(px(6.))
                    .on_scroll_wheel(cx.listener(|this, e, _, cx| this.on_scroll(e, cx)))
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(roll(
                        mono(format!("{} left", self.inbox.items.len()), t.ink2),
                        format!("left-{}", self.inbox.items.len()),
                    ))
                    .child(mono("J K move", t.ink2))
                    .child(mono("E done", t.ink2))
                    .child(mono("⌘K run", t.ink2)),
            )
    }

    fn reading_pane(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let Some(id) = self.inbox.current() else { return div().flex_1() };
        let m = &INBOX[id];
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
                    .child(
                        key_button(t, "Done", "E", true)
                            .id("done")
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Done, cx))),
                    )
                    .child(key_button(t, "Reply", "R", false))
                    .child(
                        key_button(t, "Later", "L", false)
                            .id("later")
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx))),
                    )
                    .child(key_button(t, "Forward", "F", false))
                    .child(div().flex_1())
                    .child(mono(format!("{} / {}", self.inbox.selected + 1, self.inbox.items.len()), t.ink2)),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        // Finished ghosts paint nothing; drop them on the next render.
        self.ghosts.retain(|g| g.at.elapsed() < MOTION * 2);
        self.entering.retain(|g| g.at.elapsed() < MOTION * 2);
        // The sidebar shows the blurred window background through a translucent tint.
        // 0.9 keeps ink2 above 4.5:1 even over a black or white desktop.
        let tint = Rgba { a: 0.9, ..t.subtle };
        div()
            .key_context("MainWindow")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| if !this.overlay_open(cx) { this.select(Inbox::next, cx) }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| if !this.overlay_open(cx) { this.select(Inbox::prev, cx) }))
            .on_action(cx.listener(|this, _: &Done, _, cx| if !this.overlay_open(cx) { this.act(Move::Done, cx) }))
            .on_action(cx.listener(|this, _: &Later, _, cx| if !this.overlay_open(cx) { this.act(Move::Later, cx) }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| if !this.overlay_open(cx) { this.undo(cx) }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| if !this.overlay_open(cx) { this.open_palette("", window, cx) }))
            .on_action(cx.listener(|this, _: &Reply, window, cx| if !this.overlay_open(cx) { this.open_compose(window, cx) }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| if !this.overlay_open(cx) { this.release(false, cx) }))
            .size_full()
            .flex()
            .bg(tint)
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
                    // Undo from the empty screen brings the list back.
                    .when(self.inbox.items.is_empty(), |d| d.child(crate::empty_inbox::view(&t)))
                    .when(!self.inbox.items.is_empty(), |d| d.child(self.message_list(cx)).child(self.reading_pane(cx))),
            )
            .children(self.palette.clone())
            .children(self.compose.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Move {
    Done,
    Later,
}

/// Finger travel before the swipe locks to one direction.
const SLOP: f32 = 8.;
/// docs/motion.md: about 35 % of the 408 px row, or 120 px, whichever is less.
const THRESHOLD: f32 = 120.;

/// A two-finger swipe on one row, as ids into `INBOX`. Plain Rust, so tests run without a window.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Swipe {
    Idle,
    /// Fingers down on a row, direction not locked yet.
    Undecided { id: usize, dx: f32, dy: f32 },
    /// Locked horizontal: the row follows dx.
    Row { id: usize, dx: f32 },
    /// Locked vertical: a list scroll, not a swipe.
    Scroll,
}

impl Swipe {
    fn start(id: usize) -> Self {
        Swipe::Undecided { id, dx: 0., dy: 0. }
    }

    fn moved(&mut self, x: f32, y: f32) {
        *self = match *self {
            Swipe::Undecided { id, dx, dy } => {
                let (dx, dy) = (dx + x, dy + y);
                if dx.abs().max(dy.abs()) < SLOP {
                    Swipe::Undecided { id, dx, dy }
                } else if dx.abs() > dy.abs() {
                    Swipe::Row { id, dx }
                } else {
                    Swipe::Scroll
                }
            }
            Swipe::Row { id, dx } => Swipe::Row { id, dx: dx + x },
            other => other,
        };
    }

    fn offset(&self, row: usize) -> Option<f32> {
        match *self {
            Swipe::Row { id, dx } if id == row => Some(dx),
            _ => None,
        }
    }

    /// Ends the swipe. Returns the row, its offset and the move it reached, if any.
    fn end(&mut self) -> Option<(usize, f32, Option<Move>)> {
        let Swipe::Row { id, dx } = std::mem::replace(self, Swipe::Idle) else { return None };
        let what = match dx {
            dx if dx >= THRESHOLD => Some(Move::Done),
            dx if dx <= -THRESHOLD => Some(Move::Later),
            _ => None,
        };
        Some((id, dx, what))
    }
}

/// Inbox state as ids into `INBOX`. Plain Rust, so tests run without a window.
struct Inbox {
    items: Vec<usize>,
    selected: usize,
    later: usize,
    // (what, message id, index it left from). Act and undo are LIFO, so the index always fits.
    history: Vec<(Move, usize, usize)>,
}

impl Inbox {
    fn new(len: usize, later: usize) -> Self {
        Self { items: (0..len).collect(), selected: 0, later, history: Vec::new() }
    }

    fn current(&self) -> Option<usize> {
        self.items.get(self.selected).copied()
    }

    fn select(&mut self, ix: usize) {
        if ix < self.items.len() {
            self.selected = ix;
        }
    }

    fn next(&mut self) {
        self.select(self.selected + 1);
    }

    fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Removes the message at `ix`. If it was selected, the one below it (above it at the end)
    /// becomes selected. A swipe can hit any row, so otherwise the selection stays on its message.
    fn act_at(&mut self, ix: usize, what: Move) -> Option<(usize, usize)> {
        let id = *self.items.get(ix)?;
        self.items.remove(ix);
        if ix < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        if what == Move::Later {
            self.later += 1;
        }
        self.history.push((what, id, ix));
        Some((id, ix))
    }

    /// Puts the last Done or Later message back where it was and selects it.
    fn undo(&mut self) -> Option<(Move, usize, usize)> {
        let (what, id, ix) = self.history.pop()?;
        self.items.insert(ix, id);
        self.selected = ix;
        if what == Move::Later {
            self.later -= 1;
        }
        Some((what, id, ix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_stays_in_bounds() {
        let mut inbox = Inbox::new(3, 0);
        inbox.prev();
        assert_eq!(inbox.selected, 0);
        inbox.next();
        inbox.next();
        inbox.next();
        assert_eq!(inbox.selected, 2);
        inbox.prev();
        assert_eq!(inbox.current(), Some(1));
    }

    #[test]
    fn done_selects_the_next_message() {
        let mut inbox = Inbox::new(3, 0);
        inbox.next();
        assert_eq!(inbox.act_at(inbox.selected, Move::Done), Some((1, 1)));
        assert_eq!(inbox.items, [0, 2]);
        assert_eq!(inbox.current(), Some(2));
        // At the end, the one above becomes selected.
        assert_eq!(inbox.act_at(inbox.selected, Move::Done), Some((2, 1)));
        assert_eq!(inbox.current(), Some(0));
        assert_eq!(inbox.later, 0);
    }

    #[test]
    fn later_counts_up() {
        let mut inbox = Inbox::new(2, LATER_SAMPLE);
        inbox.act_at(inbox.selected, Move::Later);
        assert_eq!(inbox.later, LATER_SAMPLE + 1);
        assert_eq!(inbox.items, [1]);
    }

    #[test]
    fn undo_reverses_in_order() {
        let mut inbox = Inbox::new(4, 2);
        inbox.next();
        inbox.act_at(inbox.selected, Move::Done); // removes 1
        inbox.act_at(inbox.selected, Move::Later); // removes 2
        inbox.next();
        assert_eq!(inbox.undo(), Some((Move::Later, 2, 1)));
        assert_eq!((inbox.later, inbox.current()), (2, Some(2)));
        assert_eq!(inbox.undo(), Some((Move::Done, 1, 1)));
        assert_eq!(inbox.items, [0, 1, 2, 3]);
        assert_eq!(inbox.current(), Some(1));
        assert_eq!(inbox.undo(), None);
    }

    #[test]
    fn act_at_keeps_the_selection_on_its_message() {
        let mut inbox = Inbox::new(4, 0);
        inbox.select(2);
        assert_eq!(inbox.act_at(0, Move::Done), Some((0, 0)));
        assert_eq!(inbox.current(), Some(2));
        assert_eq!(inbox.act_at(2, Move::Later), Some((3, 2)));
        assert_eq!(inbox.current(), Some(2));
        assert_eq!(inbox.act_at(5, Move::Done), None);
        assert_eq!(inbox.undo(), Some((Move::Later, 3, 2)));
        assert_eq!(inbox.items, [1, 2, 3]);
    }

    fn swipe(moves: &[(f32, f32)]) -> Swipe {
        let mut s = Swipe::start(7);
        for &(x, y) in moves {
            s.moved(x, y);
        }
        s
    }

    #[test]
    fn swipe_locks_direction_after_slop() {
        assert_eq!(swipe(&[(3., 1.)]), Swipe::Undecided { id: 7, dx: 3., dy: 1. });
        // Mostly horizontal: the row follows every pixel, vertical motion is ignored once locked.
        assert_eq!(swipe(&[(10., 2.), (6., 40.)]), Swipe::Row { id: 7, dx: 16. });
        // Mostly vertical: a list scroll, and later horizontal motion never turns it into a swipe.
        assert_eq!(swipe(&[(2., 9.), (200., 0.)]), Swipe::Scroll);
        assert_eq!(swipe(&[(-20., 0.)]).offset(7), Some(-20.));
        assert_eq!(swipe(&[(-20., 0.)]).offset(3), None);
    }

    #[test]
    fn swipe_threshold_decides_the_move() {
        assert_eq!(swipe(&[(THRESHOLD, 0.)]).end(), Some((7, THRESHOLD, Some(Move::Done))));
        assert_eq!(swipe(&[(-THRESHOLD - 1., 0.)]).end(), Some((7, -THRESHOLD - 1., Some(Move::Later))));
        // Short of it, or back towards the start: spring back.
        assert_eq!(swipe(&[(119., 0.)]).end(), Some((7, 119., None)));
        assert_eq!(swipe(&[(150., 0.), (-140., 0.)]).end(), Some((7, 10., None)));
        assert_eq!(swipe(&[(0., 30.)]).end(), None);
    }

    #[test]
    fn swipe_ignores_motion_after_it_ends() {
        let mut s = swipe(&[(150., 0.)]);
        assert!(s.end().is_some());
        // Momentum events after Ended arrive as Moved; Idle drops them, so nothing acts twice.
        s.moved(300., 0.);
        assert_eq!(s, Swipe::Idle);
        assert_eq!(s.end(), None);
    }

    #[test]
    fn reaching_zero() {
        let mut inbox = Inbox::new(2, 0);
        inbox.act_at(inbox.selected, Move::Done);
        inbox.act_at(inbox.selected, Move::Later);
        assert!(inbox.items.is_empty());
        assert_eq!(inbox.current(), None);
        assert_eq!(inbox.act_at(inbox.selected, Move::Done), None);
        inbox.next();
        inbox.prev();
        assert_eq!(inbox.undo(), Some((Move::Later, 1, 0)));
        assert_eq!(inbox.current(), Some(1));
    }
}

//! Main window, Concept D (MainWindowD on the Claude design canvas).
//! Sidebar, message list and reading pane. Sample data until lagom-core syncs real mail.

use std::time::{Duration, Instant};

use gpui::{
    actions, div, ease_out_quint, prelude::*, px, relative, svg, Animation, AnimationExt, AnyElement,
    App, Context, CursorStyle, Div, ElementId, FocusHandle, FontWeight, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Rgba, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, Task, TouchPhase, Window,
};

use crate::command_palette::{CommandPalette, MoveTo};
use crate::compose::Compose;
use crate::folders::{self, Folder};
use crate::motion::spring;
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

struct Message {
    from: &'static str,
    address: &'static str,
    time: &'static str,
    subject: &'static str,
    snippet: &'static str,
    body: &'static [&'static str],
    // (file name, size) of one attachment.
    attachment: Option<(&'static str, &'static str)>,
    unread: bool,
}

macro_rules! m {
    ($from:literal, $address:literal, $time:literal, $subject:literal, $snippet:literal, [$($p:literal),*], $attachment:expr, $unread:literal) => {
        Message { from: $from, address: $address, time: $time, subject: $subject, snippet: $snippet, body: &[$($p),*], attachment: $attachment, unread: $unread }
    };
}

// Sample mail: the inbox from MainWindowD (ids 0..INBOX_LEN), then the folder contents in `folders::sample`.
const INBOX_LEN: usize = 7;
const MESSAGES: &[Message] = &[
    m!("Ingrid Holm", "ingrid@holm.example", "09:42", "Board meeting moved to Thursday", "Thursday at 16:00 works for everyone except the projector.",
        ["Hi,", "Thursday at 16:00 works for everyone except the projector, which has asked to be replaced. I will bring a laptop instead.", "The minutes from last time are attached. Please read them before the meeting, not during it.", "Ingrid"],
        Some(("minutes-oct.pdf", "212 KB")), true),
    m!("Johan Ek", "johan@ek.example", "08:57", "Invoice 2026-114", "Attached. Same as last month, minus the coffee machine.",
        ["Hi,", "Invoice 2026-114 is attached. Same as last month, minus the coffee machine, which we now own.", "Thirty days, as usual.", "Johan"],
        Some(("invoice-2026-114.pdf", "48 KB")), true),
    m!("Build bot", "ci@lagom.example", "08:15", "Nightly build passed", "All tests green. Nothing to see here.",
        ["Nightly build 1147 passed on macOS, Linux and Windows.", "All tests green. Nothing to see here."], None, false),
    m!("Maja Lind", "maja@lind.example", "Yesterday", "Lunch Thursday?", "Somewhere with a lunch special. You choose, I will complain.",
        ["Lunch on Thursday? Somewhere with a lunch special.", "You choose, I will complain. It is tradition.", "Maja"], None, true),
    m!("Housing co-op", "board@coop.example", "Yesterday", "Laundry room schedule, December", "The new schedule is on the door. It is also attached.",
        ["Hello neighbours,", "The December laundry schedule is on the door. It is also attached, for those who never pass the door.", "The board"],
        Some(("laundry-december.pdf", "96 KB")), false),
    m!("Tickets", "no-reply@tickets.example", "Tue", "Stockholm C → Göteborg C, 14 Nov", "Coach 3, seat 41. Quiet carriage.",
        ["Your journey on 14 November: Stockholm C 07:10, Göteborg C 10:05.", "Coach 3, seat 41. Quiet carriage."], None, false),
    m!("Lagom releases", "releases@lagom.example", "Mon", "Version 0.4 is out", "Faster threads, fewer settings. Changelog inside.",
        ["Version 0.4 is out.", "Threads open faster and there are fewer settings. The full changelog is on the site."], None, false),
    m!("Coffee roastery", "orders@roastery.example", "Mon", "Order 4411 has shipped", "Two bags of the Ethiopian, ground for filter.",
        ["Order 4411 has shipped: two bags of the Ethiopian, ground for filter.", "It should arrive on Thursday."], None, true),
    m!("Bike shop", "service@bikeshop.example", "3 Nov", "Service receipt", "New chain, new brake pads, same squeak.",
        ["Thanks for your visit.", "New chain and new brake pads. We could not find the squeak."], Some(("receipt-881.pdf", "31 KB")), true),
    m!("Hardware store", "receipts@hardware.example", "1 Nov", "Your receipt", "One ladder, one ladder hook.",
        ["Your receipt: one ladder, one ladder hook.", "Keep it for the three-year warranty."], None, true),
    m!("Maja Lind", "maja@lind.example", "Oct", "Photos from Lisbon", "Mostly tiles. A few of us, in front of tiles.",
        ["The Lisbon photos are in the shared album.", "Mostly tiles. A few of us, in front of tiles.", "Maja"], None, false),
    m!("Ryokan Hanaya", "stay@hanaya.example", "Oct", "Booking confirmed, 3–6 April", "Dinner is served at 18:30. Slippers are provided.",
        ["Your stay from 3 to 6 April is confirmed. Two guests, garden room.", "Dinner is served at 18:30. Slippers are provided."], None, false),
    m!("Tickets", "no-reply@tickets.example", "Sun", "Göteborg C → Stockholm C, 16 Nov", "Coach 5, seat 12. Quiet carriage again.",
        ["Your journey on 16 November: Göteborg C 17:05, Stockholm C 20:00.", "Coach 5, seat 12. Quiet carriage again."], None, true),
    m!("Johan Ek", "johan@ek.example", "Sep", "Folder icons", "Stroke 1.5, like the rest. Sketches attached.",
        ["Folder icons, stroke 1.5 like the rest of the set.", "Sketches attached. The open folder is the hard one.", "Johan"],
        Some(("folder-icons.png", "640 KB")), false),
    m!("Seed shop", "hello@seeds.example", "Aug", "Spring catalogue", "Forty kinds of tomato. You need two.",
        ["The spring catalogue is here.", "Forty kinds of tomato. You need two."], None, false),
    m!("Housing co-op", "board@coop.example", "Oct", "Annual meeting minutes", "The bike room stays. The sauna gets a new door.",
        ["Hello neighbours,", "The minutes from the annual meeting are attached. The bike room stays. The sauna gets a new door.", "The board"],
        Some(("annual-meeting.pdf", "180 KB")), true),
    m!("This Week in Rust", "twir@rust.example", "Thu", "Issue 571", "Crate of the week, a few RFCs and a lot of PRs.",
        ["Issue 571: crate of the week, a few RFCs and a lot of merged PRs.", "Read it on the web, or not at all."], None, true),
    m!("Kitchen notes", "notes@kitchen.example", "Wed", "Soup season", "Six soups that freeze well, and one that does not.",
        ["Soup season. Six soups that freeze well, and one that does not.", "Start with the lentil one."], None, true),
];

// Messages already snoozed in the sample data.
const LATER_SAMPLE: usize = 2;

actions!(main_window, [SelectNext, SelectPrev, Done, Later, DeleteMessage, Undo, OpenPalette, Reply, Cancel]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("MainWindow");
    // Plain keys type into a focused text field instead. The cmd keys stay: the overlay guard
    // hands them on, and a field handles ⌘Z itself because it sits deeper in the dispatch path.
    let keys = Some("MainWindow && !TextInput");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, keys),
        KeyBinding::new("down", SelectNext, keys),
        KeyBinding::new("k", SelectPrev, keys),
        KeyBinding::new("up", SelectPrev, keys),
        KeyBinding::new("e", Done, keys),
        // Arrows mirror the swipe directions: right is Done, left is Later.
        KeyBinding::new("right", Done, keys),
        KeyBinding::new("l", Later, keys),
        KeyBinding::new("left", Later, keys),
        // The macOS menu shows an action's first binding, so the cmd variants come first.
        // ⌘⌫ in a field reaches Compose, which discards with it.
        KeyBinding::new("secondary-backspace", DeleteMessage, keys),
        KeyBinding::new("backspace", DeleteMessage, keys),
        KeyBinding::new("delete", DeleteMessage, keys),
        KeyBinding::new("secondary-z", Undo, ctx),
        KeyBinding::new("secondary-k", OpenPalette, ctx),
        KeyBinding::new("secondary-r", Reply, ctx),
        KeyBinding::new("r", Reply, keys),
        KeyBinding::new("escape", Cancel, keys),
    ]);
}

// Motion per docs/motion.md: the state changes at once, the animation only paints on top.
// shortcut: GPUI 0.2.2 exposes no "reduce motion" setting, so motion always plays. Read
// NSWorkspace.accessibilityDisplayShouldReduceMotion (and the Windows/Linux equivalents) and fall
// back to a short fade once GPUI or lagom-core can provide it.
const MOTION: Duration = Duration::from_millis(180);
/// Colour change behind a swiped row when it crosses a threshold.
const TINT_FADE: Duration = Duration::from_millis(100);
// GPUI line height is phi x font size. A row is two BASE lines, one SNIPPET line, gaps and padding.
const ROW_H: f32 = 2. * 10. + 2. * 3. + (2. * size::BASE + size::SNIPPET) * 1.618_034;

/// Selection highlight moving to the row J, K or a click selected.
const GLIDE: Duration = Duration::from_millis(140);
/// Padding around the rows in the list.
const LIST_PAD: f32 = 6.;

fn motion() -> Animation {
    Animation::new(MOTION).with_easing(ease_out_quint())
}

/// Linear progress for animators that apply `spring` themselves (GPUI's easing must stay in 0..=1).
fn springy() -> Animation {
    Animation::new(MOTION)
}

/// The selection highlight on its way between rows: y positions inside the list.
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    at: Instant,
    seq: usize,
}

impl Glide {
    /// Where the highlight is now, so a new key press starts from there and never jumps.
    fn y(&self) -> f32 {
        let d = (self.at.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.);
        self.from + (self.to - self.from) * ease_out_quint()(d)
    }
}

fn row_y(ix: usize) -> f32 {
    LIST_PAD + ix as f32 * ROW_H
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
    // Last pointer position while the left button is down on a row: a click, or a mouse swipe.
    drag: Option<Point<Pixels>>,
    // (message id, offset it springs back from, seq) for a swipe that did not reach the threshold.
    spring: Option<(usize, f32, usize)>,
    // (move it was, move it is, seq) for the colour behind a swiped row. See `retint`.
    tint: (Option<Move>, Option<Move>, usize),
    // The "Deleted" toast with Undo. Dropping the task cancels its timer.
    toast: Option<Task<()>>,
    palette: Option<gpui::Entity<CommandPalette>>,
    compose: Option<gpui::Entity<Compose>>,
    // A reply that Esc closed. Send and Discard end it.
    draft: Option<gpui::Entity<Compose>>,
    // "N more" was clicked, so every folder row shows.
    all_folders: bool,
    scroll: ScrollHandle,
    // (list offset when the trackpad gesture started, hold it): a sideways swipe and its momentum
    // keep the list where it was, including the drift before the direction locked.
    scroll_hold: (Pixels, bool),
    glide: Option<Glide>,
    // `LAGOM_SCREEN=hover`: paints this row as hovered, as a screenshot cannot move the pointer.
    shown_hover: Option<usize>,
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
            inbox: Inbox { folders: folders::sample(), ..Inbox::new(if screen == "empty" { 0 } else { INBOX_LEN }, LATER_SAMPLE) },
            focus,
            ghosts: Vec::new(),
            entering: Vec::new(),
            seq: 0,
            swipe: Swipe::Idle,
            under: None,
            drag: None,
            spring: None,
            tint: (None, None, 0),
            toast: None,
            palette: None,
            compose: None,
            draft: None,
            all_folders: false,
            scroll: ScrollHandle::new(),
            scroll_hold: (Pixels::ZERO, false),
            glide: None,
            shown_hover: None,
            _appearance: appearance,
        };
        match screen.as_str() {
            "palette" => this.open_palette("done", window, cx),
            "folders" => this.inbox.open(Some(0)),
            "move" => this.open_palette("move tr", window, cx),
            "compose" => this.open_compose(window, cx),
            // A written reply with a word selected, and Cc open with its focus ring.
            "compose-text" | "compose-cc" => {
                this.open_compose(window, cx);
                if let Some(c) = this.compose.clone() {
                    c.update(cx, |c, cx| if screen == "compose-text" { c.sample(cx) } else { c.add_cc(window, cx) });
                }
            }
            "hover" => this.shown_hover = Some(2),
            // Selects the last inbox row once the list has a layout, as J would; the list scrolls to it
            // in a short window.
            "glide" => cx
                .spawn(async |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    this.update(cx, |this, cx| this.select(|inbox| inbox.select(INBOX_LEN - 1), cx)).ok();
                })
                .detach(),
            // A row held mid-swipe in each state, for screenshots.
            "swipe-neutral" => this.swipe = Swipe::Row { id: 1, dx: 70. },
            "swipe" => this.swipe = Swipe::Row { id: 1, dx: 150. },
            "swipe-later" => this.swipe = Swipe::Row { id: 1, dx: -160. },
            "swipe-delete" => this.swipe = Swipe::Row { id: 1, dx: -280. },
            // The first message just deleted: the "Deleted" toast.
            "deleted" => this.act(Move::Delete, cx),
            _ => {}
        }
        this.retint();
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
        // The overlay takes ⌘Z, so the toast's Undo would not work while it is open.
        self.toast = None;
        // The open folder stays listed, so its name never offers "New folder". Moving there is a no-op.
        let targets = folders::tree(&self.inbox.folders, true)
            .into_iter()
            .map(|(f, _)| (f, folders::path(&self.inbox.folders, f)))
            .collect();
        let subject = self.inbox.current().map(|id| MESSAGES[id].subject.to_string());
        let palette = cx.new(|cx| CommandPalette::new(query, targets, subject, window, cx));
        cx.subscribe_in(&palette, window, |this, _, _: &gpui::DismissEvent, window, cx| {
            this.palette = None;
            window.focus(&this.focus);
            cx.notify();
        })
        .detach();
        cx.subscribe(&palette, |this, _, target: &MoveTo, cx| {
            let f = match target {
                MoveTo::Folder(f) => *f,
                MoveTo::New(name) => this.inbox.create_folder(name),
            };
            this.act(Move::Folder(f), cx);
        })
        .detach();
        window.focus(&gpui::Focusable::focus_handle(&palette, cx));
        self.palette = Some(palette);
        cx.notify();
    }

    fn open_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.toast = None;
        // One draft at a time: R brings back a draft that Esc closed, whichever message is selected.
        let compose = match self.draft.take() {
            Some(compose) => {
                compose.update(cx, |c, cx| c.reopen(window, cx));
                compose
            }
            None => {
                let m = self.inbox.current().map(|id| &MESSAGES[id]);
                let to = m.map_or("", |m| m.from);
                let subject = m.map_or(String::new(), |m| format!("Re: {}", m.subject));
                let compose = cx.new(|cx| Compose::new(to, &subject, window, cx));
                cx.subscribe_in(&compose, window, |this, compose, _: &gpui::DismissEvent, window, cx| {
                    this.compose = None;
                    this.draft = compose.read(cx).keeps_draft().then(|| compose.clone());
                    window.focus(&this.focus);
                    cx.notify();
                })
                .detach();
                compose
            }
        };
        self.compose = Some(compose);
        cx.notify();
    }

    fn act(&mut self, what: Move, cx: &mut Context<Self>) {
        self.act_at(self.inbox.selected, what, 0., cx);
    }

    fn act_at(&mut self, ix: usize, what: Move, dx: f32, cx: &mut Context<Self>) {
        let Some((id, ix)) = self.inbox.act_at(ix, what) else { return };
        // The list changes height under the highlight; rows paint the selection until it settles.
        self.glide = None;
        self.seq += 1;
        for g in &mut self.ghosts {
            if g.ix > ix {
                g.ix -= 1;
            }
        }
        self.entering.retain(|g| g.id != id);
        self.spring = self.spring.filter(|s| s.0 != id);
        self.ghosts.push(Ghost { id, ix, what, seq: self.seq, at: Instant::now(), dx });
        // The toast's Undo is ⌘Z, so it only shows while ⌘Z would undo this delete.
        self.toast = (what == Move::Delete).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(crate::compose::UNDO_WINDOW).await;
                this.update(cx, |this, cx| {
                    this.toast = None;
                    cx.notify();
                })
                .ok();
            })
        });
        cx.notify();
    }

    /// Shows the inbox (`None`) or a folder in the list.
    fn open_box(&mut self, f: Option<usize>, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.clear_motion();
        self.inbox.open(f);
        cx.notify();
    }

    // Ghosts and springs sit at indices in the list on screen, so they go when the list changes.
    fn clear_motion(&mut self) {
        self.glide = None;
        self.ghosts.clear();
        self.entering.clear();
        self.spring = None;
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let open = self.inbox.open;
        let Some((what, id, ix)) = self.inbox.undo() else { return };
        self.toast = None;
        self.glide = None;
        // Undo goes back to the list the message left, as the reverse motion needs it on screen.
        if self.inbox.open != open {
            self.swipe = Swipe::Idle;
            self.clear_motion();
        }
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
        // A mouse press owns the swipe; trackpad momentum must not move or release it.
        if self.drag.is_some() || self.overlay_open(cx) {
            return;
        }
        let before = self.swipe;
        match e.touch_phase {
            TouchPhase::Started => {
                // The list's own scroll listener runs first and already applied d.y.
                self.scroll_hold = (self.scroll.offset().y - d.y, false);
                // A new swipe starts neutral, not with the fade the last acted swipe never showed.
                self.tint = (None, None, self.tint.2);
                self.swipe = under.map_or(Swipe::Idle, Swipe::start);
                self.swipe.moved(d.x.into(), d.y.into());
            }
            // Momentum events after Ended also arrive as Moved; Idle ignores them, so none re-acts.
            TouchPhase::Moved => self.swipe.moved(d.x.into(), d.y.into()),
            TouchPhase::Ended => self.release(true, cx),
        }
        // A row under the fingers keeps the list still, as in native lists, until the next gesture.
        if matches!(self.swipe, Swipe::Row { .. }) {
            self.scroll_hold.1 = true;
        }
        if self.scroll_hold.1 {
            self.scroll.set_offset(gpui::point(self.scroll.offset().x, self.scroll_hold.0));
        }
        self.retint();
        if self.swipe != before {
            cx.notify();
        }
    }

    /// Mouse down on a row: maybe a click, maybe the start of a mouse swipe.
    fn press(&mut self, id: usize, at: Point<Pixels>) {
        self.tint = (None, None, self.tint.2);
        self.swipe = Swipe::start(id);
        self.drag = Some(at);
    }

    /// Pointer moves with the button down: the same state machine as a trackpad swipe.
    fn drag_to(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(last) = self.drag else { return };
        // The button came up where we did not see it: end the drag as a release.
        if e.pressed_button != Some(MouseButton::Left) {
            return self.drop_drag(cx);
        }
        self.drag = Some(e.position);
        let before = self.swipe;
        self.swipe.moved_with((e.position.x - last.x).into(), (e.position.y - last.y).into(), MOUSE_SLOP);
        self.retint();
        if self.swipe != before {
            cx.notify();
        }
    }

    fn mouse_swiping(&self) -> bool {
        self.drag.is_some() && matches!(self.swipe, Swipe::Row { .. })
    }

    /// Mouse up, inside the window or out: the same decision as lifting the fingers.
    fn drop_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            self.release(true, cx);
        }
    }

    /// Starts a short cross-fade when the move a release would make changes.
    // shortcut: no trackpad haptic at the thresholds; GPUI 0.2.2 has no API for
    // NSHapticFeedbackManager. Add one with a small objc call or a GPUI patch.
    fn retint(&mut self) {
        let to = match self.swipe {
            Swipe::Row { dx, .. } => reach(dx),
            _ => None,
        };
        if to != self.tint.1 {
            self.seq += 1;
            self.tint = (self.tint.1, to, self.seq);
        }
    }

    /// Fingers lift (`act`) or Esc (not `act`): past the threshold the row goes, else it springs back.
    fn release(&mut self, act: bool, cx: &mut Context<Self>) {
        // Esc mid-drag ends the drag too; the later mouse up then does nothing.
        self.drag = None;
        let Some((id, dx, what)) = self.swipe.end() else { return };
        self.retint();
        let ix = self.inbox.list().iter().position(|&i| i == id);
        match (what.filter(|_| act), ix) {
            (Some(what), Some(ix)) => self.act_at(ix, what, dx, cx),
            _ => {
                self.seq += 1;
                self.spring = Some((id, dx, self.seq));
            }
        }
        cx.notify();
    }

    /// Finished ghosts paint nothing; drop them, so the next render and `select` agree on the rows.
    fn prune(&mut self) {
        self.ghosts.retain(|g| g.at.elapsed() < MOTION * 2);
        self.entering.retain(|g| g.at.elapsed() < MOTION * 2);
    }

    fn select(&mut self, f: impl FnOnce(&mut Inbox), cx: &mut Context<Self>) {
        let before = self.inbox.selected;
        f(&mut self.inbox);
        let ix = self.inbox.selected;
        if ix == before {
            return;
        }
        self.prune();
        // While rows leave or come back, the rows paint the selection; a glide would start late.
        self.glide = (self.ghosts.is_empty() && self.entering.is_empty()).then(|| {
            let from = self.glide.map_or(row_y(before), |g| g.y());
            self.seq += 1;
            Glide { from, to: row_y(ix), at: Instant::now(), seq: self.seq }
        });
        // Child 0 is the highlight; ghosts at or before `ix` sit in front of the row.
        let ghosts = self.ghosts.iter().filter(|g| g.ix <= ix).count();
        self.scroll.scroll_to_item(1 + ix + ghosts);
        cx.notify();
    }
}

/// p = 0 is a resting row, p = 1 is fully gone. Undo plays the same curve backwards.
fn away<E: Styled>(row: E, t: Theme, what: Move, p: f32, dx: f32) -> E {
    if p == 0. && dx == 0. {
        return row; // At rest: keep the selection background.
    }
    // Slide first, then close the gap. Padding shrinks too, or the row keeps 20 px.
    let collapse = ((p - 0.3) / 0.7).clamp(0., 1.);
    let row = row
        .relative()
        .h(px(ROW_H * (1. - collapse)))
        .py(px(10. * (1. - collapse)))
        .overflow_hidden()
        // The spring overshoots a little past 0 and 1; position follows it, opacity stops at the ends.
        .opacity((1. - p).clamp(0., 1.));
    // Opaque, so the colour of the move shows only where the row has left (see `with_behind`).
    let row = row.bg(t.raised);
    match what {
        // A move to a folder leaves like Done.
        Move::Done | Move::Folder(_) => row.left(px(dx + p * 200.)),
        // Delete leaves the other way, at the same speed.
        Move::Delete => row.left(px(dx - p * 200.)),
        // Towards Later in the sidebar: lift (shadow), shrink and move left.
        Move::Later => row.left(px(dx - p * 80.)).w(relative(1. - 0.3 * p)).shadow_md(),
    }
}

/// The colour behind a row for the move it commits to, and the ink on it. `None`: short of a threshold.
fn swipe_colors(t: &Theme, what: Option<Move>) -> (Rgba, Rgba) {
    match what {
        None => (t.sel, t.ink2),
        Some(Move::Done | Move::Folder(_)) => (t.swipe_done, t.accent_ink),
        Some(Move::Later) => (t.swipe_later, t.accent_ink),
        Some(Move::Delete) => (t.danger, t.accent_ink),
    }
}

/// What shows behind a moving row: the colour of `what`, and its icon on the side the row uncovers.
fn behind(t: Theme, what: Option<Move>, right: bool) -> Div {
    let (bg, ink) = swipe_colors(&t, what);
    let path = match what {
        Some(Move::Delete) => "icons/trash.svg",
        Some(Move::Later) => "icons/later.svg",
        Some(_) => "icons/done.svg",
        None if right => "icons/done.svg",
        None => "icons/later.svg",
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .when(!right, |d| d.justify_end())
        .px(px(20.))
        .rounded(px(radius::CONTROL))
        .bg(bg)
        .child(icon(path, ink, 18.))
}

/// A row that leaves (or comes back) over the colour of its move, so E, L and ⌫ look like a swipe.
/// The colour fades with the row; at rest (p = 0) it is gone, as resting rows are not opaque.
fn with_behind(row: AnyElement, t: Theme, what: Move, seq: usize, p: impl Fn(f32) -> f32 + 'static) -> AnyElement {
    let right = !matches!(what, Move::Later | Move::Delete);
    div()
        .relative()
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(radius::CONTROL))
        .child(behind(t, Some(what), right).with_animation(("behind", seq), springy(), move |el, d| {
            let p = p(d);
            el.opacity(if p > 0. { (1. - p).max(0.) } else { 0. })
        }))
        .child(row)
        .into_any_element()
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

/// Hover and press feedback, and the pointer, for anything clickable. Instant, as in native
/// macOS controls: GPUI has no style transitions, and a fade would make the click feel late.
/// `selected` rows keep their `sel` background under the pointer; hover would replace it.
pub(crate) fn pressable(el: Stateful<Div>, t: &Theme, selected: bool) -> Stateful<Div> {
    let (hover, press) = (t.hover, t.press);
    el.cursor_pointer().when(!selected, |el| el.hover(move |s| s.bg(hover))).active(move |s| s.bg(press))
}

/// A button with its key. `fill`: the colour of a primary button; `None` is a bordered one.
pub(crate) fn key_button(t: &Theme, label: &'static str, key: &'static str, fill: Option<Rgba>) -> Stateful<Div> {
    let base = div()
        .id(label)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(34.))
        .px(px(14.))
        .rounded(px(radius::CONTROL))
        .text_size(px(13.))
        .font_weight(if fill.is_some() { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .child(label);
    match fill {
        // A translucent tint would replace the fill, so a filled button fades a little instead.
        Some(bg) => base
            .bg(bg)
            .text_color(t.accent_ink)
            .child(mono(key, t.accent_ink))
            .cursor_pointer()
            .hover(|s| s.opacity(0.92))
            .active(|s| s.opacity(0.8)),
        None => pressable(base.border_1().border_color(t.line).text_color(t.ink).child(mono(key, t.ink2)), t, false),
    }
}

impl MainWindow {
    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let heading = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .px(px(10.))
                .pb(px(6.))
                .text_size(px(size::SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.ink2)
                .child(div().flex_1().child(label))
        };
        let nav = |path: &'static str, label: &'static str, count: Option<SharedString>, active: bool| {
            pressable(div().id(SharedString::from(format!("nav-{label}"))), t, active)
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
            pressable(div().id(SharedString::from(format!("account-{name}"))), t, false)
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
                pressable(div().id("account-switcher"), t, false)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(40.))
                    .px(px(8.))
                    .rounded(px(radius::CONTROL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(app_mark(t))
                    .child(div().flex_1().child("Home"))
                    .child(icon("icons/chevron.svg", t.ink2, 14.)),
            )
            .child(
                div()
                    .id("search")
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
                    // An opaque field: the pointer deepens its border instead of tinting it.
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.ink2))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette("", window, cx)))
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
                    .child(
                        nav("icons/inbox.svg", "Inbox", Some(self.inbox.items.len().to_string().into()), self.inbox.open.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.open_box(None, cx))),
                    )
                    .child(nav("icons/later.svg", "Later", Some(self.inbox.later.to_string().into()), false))
                    .child(nav("icons/sent.svg", "Sent", None, false))
                    .child(nav("icons/done.svg", "Done", Some("G D".into()), false)),
            )
            .child(self.folder_group(heading("Folders"), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(heading("Accounts"))
                    .child(account("Home", "you@home.example", true))
                    .child(account("Work", "you@work.example", false)),
            )
            .child(div().flex_1())
            .child(
                pressable(div().id("settings"), t, false)
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(34.))
                    .px(px(10.))
                    .rounded(px(radius::CONTROL))
                    .text_color(t.ink2)
                    .child("Settings")
                    .child(mono("⌘,", t.ink2)),
            )
    }

    /// The "Folders" group: the nav row style, subfolders indented, "N more" past six rows.
    fn folder_group(&self, heading: Div, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let all = &self.inbox.folders;
        let row = |id: ElementId, active: bool| {
            pressable(div().id(id), t, active).flex().items_center().gap(px(10.)).h(px(34.)).px(px(10.)).rounded(px(radius::CONTROL))
        };
        // shortcut: "+" and "New folder" do nothing yet; ⌘K "move <name>" creates a folder. Wire
        // them to an inline name field once the app has text inputs.
        let group = div().flex().flex_col().gap(px(2.)).child(heading.child(icon("icons/plus.svg", t.ink2, 14.)));
        if all.is_empty() {
            return group.child(row("new-folder".into(), false).text_color(t.ink2).child(icon("icons/plus.svg", t.ink2, 16.)).child("New folder"));
        }
        // Once any folder has subfolders, every row keeps a chevron slot, so the icons line up.
        let nested = all.iter().any(|f| f.parent.is_some());
        let rows = folders::tree(all, false);
        let shown = folders::shown(rows.len(), self.all_folders);
        let slot = || div().size(px(12.)).flex_shrink_0();
        let mut group = group;
        for &(f, depth) in &rows[..shown] {
            let unread = self.inbox.unread(f);
            let active = self.inbox.open == Some(f);
            let color = if active || unread > 0 { t.ink } else { t.ink2 };
            let chevron = folders::has_children(all, f).then(|| {
                let path = if all[f].open { "icons/chevron.svg" } else { "icons/chevron-right.svg" };
                pressable(slot().id(("chevron", f)), t, false).rounded(px(radius::KEY)).child(icon(path, t.ink2, 12.)).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.inbox.folders[f].open ^= true;
                    cx.notify();
                }))
            });
            group = group.child(
                row(("folder", f).into(), active)
                    .pl(px(10. + 22. * depth as f32))
                    .text_color(color)
                    .when(unread > 0, |d| d.font_weight(FontWeight::MEDIUM))
                    .when(active, |d| d.bg(t.sel).font_weight(FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_box(Some(f), cx)))
                    .when(nested, |d| match chevron {
                        Some(c) => d.child(c),
                        None => d.child(slot()),
                    })
                    .child(icon("icons/folder.svg", color, 16.))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(all[f].name.clone()))
                    // A zero count is calm: no number at all.
                    .when(unread > 0, |d| d.child(roll(mono(unread.to_string(), t.ink2), format!("folder-{f}-{unread}")))),
            );
        }
        if shown < rows.len() {
            group = group.child(
                row("folders-more".into(), false)
                    .text_color(t.ink2)
                    .when(nested, |d| d.child(slot()))
                    .child(icon("icons/chevron.svg", t.ink2, 16.))
                    .child(format!("{} more", rows.len() - shown))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_folders = true;
                        cx.notify();
                    })),
            );
        }
        group
    }

    fn message_list(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let row = |m: &Message, selected: bool| {
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .h(px(ROW_H))
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
                let el = row(&MESSAGES[g.id], false)
                    .with_animation(("ghost", g.seq), springy(), move |el, d| away(el, theme, what, spring(d), dx))
                    .into_any_element();
                with_behind(el, theme, what, g.seq, spring)
            })
        };
        // The selection highlight glides between rows while nothing else moves the list.
        let gliding = self.glide.filter(|_| self.ghosts.is_empty() && self.entering.is_empty());
        // shortcut: renders every row, fine for the sample inbox. Virtualise (uniform_list or
        // gpui::list, docs/performance.md rule 3) when real mail arrives; rows leaving and
        // coming back change height, which uniform_list cannot lay out.
        let mut rows: Vec<AnyElement> = vec![match gliding {
            // First child, so it paints under the rows. `child_ix` counts it.
            Some(g) => div()
                .absolute()
                .left(px(LIST_PAD))
                .right(px(LIST_PAD))
                .h(px(ROW_H))
                .rounded(px(radius::CONTROL))
                .bg(t.sel)
                .with_animation(("glide", g.seq), Animation::new(GLIDE).with_easing(ease_out_quint()), move |el, d| {
                    el.top(px(g.from + (g.to - g.from) * d))
                })
                .into_any_element(),
            None => div().into_any_element(),
        }];
        let dragging = self.mouse_swiping();
        for (i, &id) in self.inbox.list().iter().enumerate() {
            rows.extend(ghosts_at(i));
            let selected = i == self.inbox.selected;
            let el = row(&MESSAGES[id], selected && gliding.is_none())
                .id(("row", id))
                // Arrow at rest, as in native lists; the hover tint shows the row responds.
                // Every row, not only the dragged one: the topmost hovered row's cursor wins.
                .when(dragging, |d| d.cursor(CursorStyle::ClosedHand))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, _| this.press(id, e.position)))
                // Runs before the window's mouse up ends the drag: a dragged row is not a click.
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.swipe.offset(id).is_none() {
                        this.select(|inbox| inbox.select(i), cx)
                    }
                }))
                .on_scroll_wheel(cx.listener(move |this, _, _, _| this.under = Some(id)));
            // Under the fingers the row follows dx; after a short swipe it springs back from it.
            let spring_back = self.spring.filter(|s| s.0 == id).map(|(_, dx, seq)| (dx, Some(seq)));
            if let Some((dx, seq)) = self.swipe.offset(id).map(|dx| (dx, None)).or(spring_back) {
                // Opaque, so the action behind shows only where the row has moved away.
                let el = el.relative().bg(if selected { t.sel } else { t.raised });
                let el = match seq {
                    None => el.left(px(dx)).into_any_element(),
                    Some(seq) => el
                        .with_animation(("spring", seq), springy(), move |el, d| el.left(px(dx * (1. - spring(d)))))
                        .into_any_element(),
                };
                // The colour of the move the row would make, cross-faded from the last one so a
                // change of state never cuts hard. The row itself never waits for it.
                let (from, to, tint) = self.tint;
                rows.push(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(radius::CONTROL))
                        .child(behind(theme, from, dx >= 0.))
                        .child(behind(theme, to, dx >= 0.).with_animation(
                            ("tint", tint),
                            Animation::new(TINT_FADE).with_easing(ease_out_quint()),
                            |el, d| el.opacity(d),
                        ))
                        .child(el)
                        .into_any_element(),
                );
                continue;
            }
            rows.push(match self.entering.iter().find(|g| g.id == id) {
                Some(g) => {
                    let what = g.what;
                    let el = el
                        .with_animation(("enter", g.seq), springy(), move |el, d| away(el, theme, what, 1. - spring(d), 0.))
                        .into_any_element();
                    with_behind(el, theme, what, g.seq, |d| 1. - spring(d))
                }
                // At rest: hover between bg and sel, press past sel. The selected row keeps sel.
                None => {
                    let (hover, press) = (t.hover, t.press);
                    el.when(!selected, |el| el.hover(move |s| s.bg(hover)))
                        .when(self.shown_hover == Some(i), |el| el.bg(hover))
                        .active(move |s| s.bg(press))
                        .into_any_element()
                }
            });
        }
        rows.extend(ghosts_at(self.inbox.list().len()));

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
            .child(
                pressable(div().id("filter-unread"), t, false)
                    .h(px(26.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_color(t.ink2)
                    .child("Unread"),
            );

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
                            .child(self.inbox.open.map_or("Inbox".to_string(), |f| self.inbox.folders[f].name.clone())),
                    )
                    .child(filter),
            )
            .child(
                {
                    let mut list = div()
                        .id("rows")
                        .flex_1()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .flex()
                        .flex_col()
                        .p(px(LIST_PAD))
                        .on_scroll_wheel(cx.listener(|this, e, _, cx| this.on_scroll(e, cx)))
                        .children(rows);
                    // A sideways swipe must never scroll the list vertically by its x delta.
                    list.style().restrict_scroll_to_axis = Some(true);
                    list
                },
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
                        mono(format!("{} left", self.inbox.list().len()), t.ink2),
                        format!("left-{}", self.inbox.list().len()),
                    ))
                    .child(mono("J K move", t.ink2))
                    .child(mono("E done", t.ink2))
                    .child(mono("⌘K run", t.ink2)),
            )
    }

    fn reading_pane(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let Some(id) = self.inbox.current() else { return div().flex_1() };
        let m = &MESSAGES[id];
        let initials: String = m.from.split_whitespace().take(2).filter_map(|w| w.chars().next()).collect();

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
                        // Pine green, like the Done swipe.
                        key_button(t, "Done", "E", Some(t.swipe_done))
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Done, cx))),
                    )
                    .child(key_button(t, "Reply", "R", None).on_click(cx.listener(|this, _, window, cx| this.open_compose(window, cx))))
                    .child(
                        key_button(t, "Later", "L", None)
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx))),
                    )
                    .child(key_button(t, "Forward", "F", None))
                    .child(div().flex_1())
                    .child(mono(format!("{} / {}", self.inbox.selected + 1, self.inbox.list().len()), t.ink2)),
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
                                            .child(initials.to_uppercase()),
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
                                                    .child(format!("{} → you", m.address)),
                                            ),
                                    )
                                    .child(mono(m.time, t.ink2)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .text_size(px(size::BODY))
                            .line_height(relative(1.6))
                            .children(m.body.iter().map(|&p| div().child(p)))
                            .children(m.attachment.map(|(name, kb)| {
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
                                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                                            .child(mono(kb, t.ink2)),
                                    )
                            })),
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
                            .child(format!("Reply to {}", m.from))
                            .child(mono("R", t.ink2)),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.prune();
        // The sidebar shows the blurred window background through a translucent tint.
        // 0.9 keeps ink2 above 4.5:1 even over a black or white desktop.
        let tint = Rgba { a: 0.9, ..t.subtle };
        div()
            .key_context("MainWindow")
            // Mouse swipe: the root sees every move inside the window; `on_mouse_up_out` catches a
            // release outside it. shortcut: moves outside the window are not followed (the row waits
            // at the edge); a window-level listener via `window.on_mouse_event` would follow them.
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag_to(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .when(self.mouse_swiping(), |d| d.cursor(CursorStyle::ClosedHand))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| if !this.overlay_open(cx) { this.select(Inbox::next, cx) }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| if !this.overlay_open(cx) { this.select(Inbox::prev, cx) }))
            .on_action(cx.listener(|this, _: &Done, _, cx| if !this.overlay_open(cx) { this.act(Move::Done, cx) }))
            .on_action(cx.listener(|this, _: &Later, _, cx| if !this.overlay_open(cx) { this.act(Move::Later, cx) }))
            // The guard hands ⌫ to the palette, where it edits the query.
            .on_action(cx.listener(|this, _: &DeleteMessage, _, cx| if !this.overlay_open(cx) { this.act(Move::Delete, cx) }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| if !this.overlay_open(cx) { this.undo(cx) }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| if !this.overlay_open(cx) { this.open_palette("", window, cx) }))
            .on_action(cx.listener(|this, _: &Reply, window, cx| if !this.overlay_open(cx) { this.open_compose(window, cx) }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| if !this.overlay_open(cx) { this.release(false, cx) }))
            .size_full()
            .flex()
            .bg(tint)
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .child(self.sidebar(cx))
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
                    // Undo from the empty screen brings the list back. An empty folder keeps its header.
                    .when(self.inbox.list().is_empty() && self.inbox.open.is_none(), |d| d.child(crate::empty_inbox::view(&t)))
                    .when(!self.inbox.list().is_empty() || self.inbox.open.is_some(), |d| d.child(self.message_list(cx)).child(self.reading_pane(cx))),
            )
            .children(self.palette.clone())
            .when(self.toast.is_some(), |d| {
                d.child(crate::compose::toast(
                    &t,
                    "Deleted",
                    key_button(&t, "Undo", "⌘Z", None)
                        .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                ))
            })
            .children(self.compose.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Move {
    Done,
    Later,
    /// Into the folder at this index in `Inbox::folders`.
    Folder(usize),
    /// To Trash.
    Delete,
}

/// Finger travel before the swipe locks to one direction.
const SLOP: f32 = 8.;
/// Pointer travel before a mouse press on a row becomes a drag. Below it, the press is a click.
const MOUSE_SLOP: f32 = 6.;
/// docs/motion.md: about 35 % of the 408 px row, or 120 px, whichever is less.
const THRESHOLD: f32 = 120.;
/// A long left swipe deletes: about 60 % of the 408 px row (the list is a fixed 420 px wide).
const DELETE_THRESHOLD: f32 = 0.6 * 408.;

/// The move a swipe released at `dx` makes. Right has one stage (Done), left has two (Later, Delete).
fn reach(dx: f32) -> Option<Move> {
    match dx {
        dx if dx >= THRESHOLD => Some(Move::Done),
        dx if dx <= -DELETE_THRESHOLD => Some(Move::Delete),
        dx if dx <= -THRESHOLD => Some(Move::Later),
        _ => None,
    }
}

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
        self.moved_with(x, y, SLOP);
    }

    /// `slop`: travel before the direction locks. A mouse drag uses `MOUSE_SLOP`.
    fn moved_with(&mut self, x: f32, y: f32, slop: f32) {
        *self = match *self {
            Swipe::Undecided { id, dx, dy } => {
                let (dx, dy) = (dx + x, dy + y);
                if dx.abs().max(dy.abs()) < slop {
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
        Some((id, dx, reach(dx)))
    }
}

/// Inbox and folder state as ids into `MESSAGES`. Plain Rust, so tests run without a window.
struct Inbox {
    /// The inbox.
    items: Vec<usize>,
    folders: Vec<Folder>,
    /// The list on screen: the inbox (`None`) or a folder.
    open: Option<usize>,
    selected: usize,
    later: usize,
    /// Deleted message ids, newest first.
    trash: Vec<usize>,
    // (what, message id, list it left, index it left from). Act and undo are LIFO, so the index always fits.
    history: Vec<(Move, usize, Option<usize>, usize)>,
}

impl Inbox {
    fn new(len: usize, later: usize) -> Self {
        Self { items: (0..len).collect(), folders: Vec::new(), open: None, selected: 0, later, trash: Vec::new(), history: Vec::new() }
    }

    fn list(&self) -> &Vec<usize> {
        self.open.map_or(&self.items, |f| &self.folders[f].items)
    }

    fn list_mut(&mut self) -> &mut Vec<usize> {
        match self.open {
            Some(f) => &mut self.folders[f].items,
            None => &mut self.items,
        }
    }

    fn open(&mut self, f: Option<usize>) {
        self.open = f;
        self.selected = 0;
    }

    fn create_folder(&mut self, name: &str) -> usize {
        self.folders.push(Folder::new(name, None, &[]));
        self.folders.len() - 1
    }

    fn unread(&self, f: usize) -> usize {
        self.folders[f].items.iter().filter(|&&id| MESSAGES[id].unread).count()
    }

    fn current(&self) -> Option<usize> {
        self.list().get(self.selected).copied()
    }

    fn select(&mut self, ix: usize) {
        if ix < self.list().len() {
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
        if let Move::Folder(f) = what {
            // A move into the open folder, or a folder that does not exist, changes nothing.
            if self.open == Some(f) || f >= self.folders.len() {
                return None;
            }
        }
        let id = *self.list().get(ix)?;
        self.list_mut().remove(ix);
        if ix < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.list().len().saturating_sub(1));
        match what {
            Move::Later => self.later += 1,
            Move::Folder(f) => self.folders[f].items.insert(0, id),
            Move::Delete => self.trash.insert(0, id),
            Move::Done => {}
        }
        self.history.push((what, id, self.open, ix));
        Some((id, ix))
    }

    /// Puts the last moved message back where it was, opens that list and selects the message.
    fn undo(&mut self) -> Option<(Move, usize, usize)> {
        let (what, id, from, ix) = self.history.pop()?;
        match what {
            Move::Later => self.later -= 1,
            Move::Folder(f) => self.folders[f].items.retain(|&i| i != id),
            Move::Delete => self.trash.retain(|&i| i != id),
            Move::Done => {}
        }
        self.open = from;
        self.list_mut().insert(ix, id);
        self.selected = ix;
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
    fn long_left_swipe_deletes() {
        // Left has two stages: Later past 120 px, Delete past 60 % of the row.
        assert_eq!(swipe(&[(-DELETE_THRESHOLD + 1., 0.)]).end(), Some((7, -DELETE_THRESHOLD + 1., Some(Move::Later))));
        assert_eq!(swipe(&[(-DELETE_THRESHOLD, 0.)]).end(), Some((7, -DELETE_THRESHOLD, Some(Move::Delete))));
        // Back below the delete threshold is Later again; back near the start springs back.
        assert_eq!(swipe(&[(-300., 0.), (100., 0.)]).end(), Some((7, -200., Some(Move::Later))));
        assert_eq!(swipe(&[(-300., 0.), (250., 0.)]).end(), Some((7, -50., None)));
        // Right has no long stage.
        assert_eq!(reach(400.), Some(Move::Done));
        assert_eq!(reach(-0.), None);
    }

    #[test]
    fn delete_goes_to_trash_and_undo_restores() {
        let mut inbox = Inbox::new(3, 0);
        inbox.next();
        assert_eq!(inbox.act_at(inbox.selected, Move::Delete), Some((1, 1)));
        assert_eq!((inbox.items.as_slice(), inbox.trash.as_slice()), ([0, 2].as_slice(), [1].as_slice()));
        assert_eq!(inbox.current(), Some(2));
        assert_eq!(inbox.undo(), Some((Move::Delete, 1, 1)));
        assert_eq!(inbox.items, [0, 1, 2]);
        assert!(inbox.trash.is_empty());
        assert_eq!((inbox.current(), inbox.later), (Some(1), 0));
    }

    #[test]
    fn mouse_click_versus_drag() {
        let drag = |moves: &[(f32, f32)]| {
            let mut s = Swipe::start(7);
            for &(x, y) in moves {
                s.moved_with(x, y, MOUSE_SLOP);
            }
            s
        };
        // A press that moves less than 6 px is a click: nothing to end, the row stays put.
        let mut click = drag(&[(2., 1.), (3., -2.)]);
        assert_eq!(click.offset(7), None);
        assert_eq!(click.end(), None);
        // 6 px, mostly horizontal: a swipe that follows the pointer, decided like the trackpad.
        assert_eq!(drag(&[(6., 2.)]), Swipe::Row { id: 7, dx: 6. });
        assert_eq!(drag(&[(-6., 0.), (-300., 0.)]).end(), Some((7, -306., Some(Move::Delete))));
        // Mostly vertical: nothing happens, even when it later goes sideways.
        assert_eq!(drag(&[(1., 6.), (200., 0.)]).end(), None);
        // The same 6 px is still inside the trackpad slop.
        assert!(matches!(swipe(&[(6., 0.)]), Swipe::Undecided { .. }));
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

    fn with_folders() -> Inbox {
        Inbox { folders: folders::sample(), ..Inbox::new(INBOX_LEN, 0) }
    }

    #[test]
    fn move_to_folder_and_undo() {
        let mut inbox = with_folders();
        // Tickets (5) into Trains (2): it leaves the inbox, the next message is selected.
        inbox.select(5);
        assert_eq!(inbox.act_at(inbox.selected, Move::Folder(2)), Some((5, 5)));
        assert_eq!(inbox.current(), Some(6));
        assert_eq!(inbox.folders[2].items, [5, 12]);
        // Moving into the folder already open does nothing.
        inbox.open(Some(2));
        assert_eq!(inbox.act_at(0, Move::Folder(2)), None);
        // Undo from another list goes back to the inbox, at the same place.
        assert_eq!(inbox.undo(), Some((Move::Folder(2), 5, 5)));
        assert_eq!((inbox.open, inbox.current()), (None, Some(5)));
        assert_eq!(inbox.folders[2].items, [12]);
        assert_eq!(inbox.items, (0..INBOX_LEN).collect::<Vec<_>>());
    }

    #[test]
    fn act_inside_a_folder() {
        let mut inbox = with_folders();
        inbox.open(Some(0));
        assert_eq!(inbox.list(), &[7, 8, 9]);
        inbox.act_at(0, Move::Done);
        assert_eq!((inbox.list().len(), inbox.current()), (2, Some(8)));
        inbox.open(None);
        assert_eq!(inbox.undo(), Some((Move::Done, 7, 0)));
        assert_eq!((inbox.open, inbox.list().len()), (Some(0), 3));
    }

    #[test]
    fn folder_counts_are_unread_only() {
        let mut inbox = with_folders();
        let counts: Vec<_> = (0..inbox.folders.len()).map(|f| inbox.unread(f)).collect();
        assert_eq!(counts, [3, 0, 1, 0, 0, 0, 0, 1, 2]);
        // A read message (Build bot) into Travel keeps Travel at zero; an unread one counts.
        inbox.act_at(2, Move::Folder(1));
        assert_eq!(inbox.unread(1), 0);
        inbox.act_at(0, Move::Folder(1));
        assert_eq!(inbox.unread(1), 1);
    }

    #[test]
    fn create_folder_from_the_palette() {
        let mut inbox = with_folders();
        let f = inbox.create_folder("Taxes");
        assert_eq!(folders::path(&inbox.folders, f), "Taxes");
        inbox.act_at(1, Move::Folder(f));
        assert_eq!(inbox.folders[f].items, [1]);
        assert_eq!(inbox.act_at(0, Move::Folder(99)), None);
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

/// Key dispatch through GPUI's test window: real bindings, contexts and focus, no key presses.
#[cfg(test)]
mod dispatch {
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::*;

    fn open(cx: &mut TestAppContext) -> (Entity<MainWindow>, &mut VisualTestContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::text_input::bind_keys(cx);
        });
        cx.add_window_view(MainWindow::new)
    }

    fn inbox_len(view: &Entity<MainWindow>, cx: &mut VisualTestContext) -> usize {
        view.read_with(cx, |v, _| v.inbox.list().len())
    }

    #[gpui::test]
    fn palette_takes_letters_backspace_and_undo(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = inbox_len(&view, cx);
        cx.simulate_keystrokes("cmd-k");
        cx.simulate_input("elrj");
        cx.simulate_keystrokes("backspace left right");
        let palette = view.read_with(cx, |v, _| v.palette.clone()).expect("palette open");
        assert_eq!(palette.read_with(cx, |p, cx| p.query(cx)), "elr");
        // None of e, l, r, j, ⌫, ←, → reached the inbox.
        assert_eq!(inbox_len(&view, cx), n);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(palette.read_with(cx, |p, cx| p.query(cx)), "");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.palette.is_none()));
        // Without a field, the same keys act on the inbox again.
        cx.simulate_keystrokes("e");
        assert_eq!(inbox_len(&view, cx), n - 1);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(inbox_len(&view, cx), n);
    }

    #[gpui::test]
    fn compose_keeps_the_draft_on_escape(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = inbox_len(&view, cx);
        let body = |view: &Entity<MainWindow>, cx: &mut VisualTestContext| {
            let c = view.read_with(cx, |v, _| v.compose.clone()).expect("compose open");
            c.read_with(cx, |c, cx| c.body_text(cx))
        };
        cx.simulate_keystrokes("r");
        cx.simulate_input("Hi there");
        assert_eq!(body(&view, cx), "Hi there");
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(body(&view, cx), "");
        cx.simulate_input("Hello");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("Alex");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.compose.is_none() && v.draft.is_some()));
        cx.simulate_keystrokes("r");
        assert_eq!(body(&view, cx), "Hello\nAlex");
        // ⌘⌫ in the body discards the draft, not a message.
        cx.simulate_keystrokes("cmd-backspace");
        assert!(view.read_with(cx, |v, _| v.compose.is_none() && v.draft.is_none()));
        assert_eq!(inbox_len(&view, cx), n);
        cx.simulate_keystrokes("r");
        assert_eq!(body(&view, cx), "");
    }

    #[gpui::test]
    fn tab_cycles_compose_fields(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("r");
        let c = view.read_with(cx, |v, _| v.compose.clone()).unwrap();
        // Body is last, so Tab wraps to To; Shift-Tab goes back.
        cx.simulate_keystrokes("tab");
        assert_eq!(cx.update(|window, cx| c.read(cx).focused_field(window, cx)), Some(0));
        cx.simulate_keystrokes("tab");
        assert_eq!(cx.update(|window, cx| c.read(cx).focused_field(window, cx)), Some(2));
        cx.simulate_keystrokes("shift-tab shift-tab");
        assert_eq!(cx.update(|window, cx| c.read(cx).focused_field(window, cx)), Some(3));
        cx.simulate_keystrokes("cmd-shift-c");
        assert_eq!(cx.update(|window, cx| c.read(cx).focused_field(window, cx)), Some(1));
    }
}

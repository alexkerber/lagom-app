//! Main window, Concept D (MainWindowD on the Claude design canvas).
//! Sidebar, message list and reading pane. Real mail from the local store (`Live`), or the
//! sample mail in demo mode (`LAGOM_DEMO=1`, screenshots and tests).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    actions, div, ease_out_quint, prelude::*, px, relative, svg, Animation, AnimationExt, AnyElement,
    App, Context, CursorStyle, Div, ElementId, Entity, FocusHandle, FontFeatures, FontWeight, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, Task, TouchPhase, Window,
};
use lagom_core::{account::Account, store::Store};

use crate::accounts::{AddAccount, Connect};
use crate::command_palette::{CommandPalette, MoveTo, Run};
use crate::compose::{Compose, ComposeEvent};
use crate::folders::{self, Folder, Kind};
use crate::mail::{self, LaterWhen, Mail, INBOX_LEN, LATER_SAMPLE};
use crate::motion::spring;
use crate::sync::{Act, Bridge, Cmd, Delivery, Event, SyncState};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT, LEADING, READING_LEADING};

actions!(main_window, [SelectNext, SelectPrev, Done, Later, DeleteMessage, Undo, OpenPalette, Reply, Cancel, NewMessage, OpenSettings, ToggleSidebar]);

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
        KeyBinding::new("secondary-n", NewMessage, ctx),
        KeyBinding::new("c", NewMessage, keys),
        KeyBinding::new("secondary-,", OpenSettings, ctx),
        KeyBinding::new("alt-secondary-s", ToggleSidebar, ctx),
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
/// Vertical padding and line gap inside a message row.
const ROW_PAD: f32 = 8.;
const ROW_GAP: f32 = 2.;
/// Preview lines under the subject, as in Mail.
const SNIPPET_LINES: usize = 2;
// The root sets the line height to LEADING x font size. A row is two BASE lines, two SNIPPET
// lines, gaps and padding. Every row is exactly this tall, so off-screen rows can stand in as boxes.
const ROW_H: f32 = 2. * ROW_PAD + 2. * ROW_GAP + (2. * size::BASE + SNIPPET_LINES as f32 * size::SNIPPET) * LEADING;

/// Selection highlight moving to the row J, K or a click selected.
const GLIDE: Duration = Duration::from_millis(140);
/// Padding around the rows in the list.
const LIST_PAD: f32 = 6.;
/// Sidebar rows, the macOS medium sidebar size.
const SIDEBAR_ROW: f32 = 28.;
/// The list and reading pane headers, a macOS toolbar's height.
const HEADER_H: f32 = 52.;
/// 600 pt tall keeps the sidebar's groups on screen.
pub const MIN_WINDOW: (f32, f32) = (970., 600.);

mod panes;
use panes::{default_list, Panes, SIDEBAR_DEFAULT};

#[derive(Clone, Copy)]
enum Divider { Sidebar, List }

#[derive(Clone, Copy)]
struct Resize {
    divider: Divider,
    start_x: Pixels,
    start_width: f32,
    start_panes: Panes,
    moved: bool,
}

fn motion() -> Animation {
    Animation::new(MOTION).with_easing(ease_out_quint())
}

/// Linear progress for animators that apply `spring` themselves (GPUI's easing must stay in 0..=1).
/// Rows that leave, come back or spring back: long enough to read the motion (see `motion::spring`).
const SPRING: Duration = Duration::from_millis(280);

fn springy() -> Animation {
    Animation::new(SPRING)
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

mod html;
mod live;
pub use live::Live;

/// What fills the main panel.
enum Page {
    Mail,
    AddAccount(Entity<AddAccount>),
    /// Settings, with the account whose removal waits for a confirm.
    Settings(Option<String>),
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
    // The toast: its text, whether it offers Undo, and its timer. Dropping the task cancels it.
    toast: Option<(SharedString, bool, Task<()>)>,
    palette: Option<gpui::Entity<CommandPalette>>,
    compose: Option<gpui::Entity<Compose>>,
    // Sheets Esc closed or a failed send returned, newest last. R and C reopen the newest.
    drafts: Vec<gpui::Entity<Compose>>,
    // Sheets after Send, in their Undo window: they show a toast and are no longer modal.
    sending: Vec<Entity<Compose>>,
    // Quit or close waits for these sends to reach the worker's answer.
    exit: Option<Exit>,
    // Gives up the wait after `EXIT_WAIT`; dropping it cancels.
    exit_timer: Option<Task<()>>,
    // The store and the mail worker. `None` in demo mode.
    live: Option<live::Live>,
    page: Page,
    // "N more" was clicked, so every folder row shows.
    all_folders: bool,
    scroll: ScrollHandle,
    // (list offset when the trackpad gesture started, hold it): a sideways swipe and its momentum
    // keep the list where it was, including the drift before the direction locked.
    scroll_hold: (Pixels, bool),
    glide: Option<Glide>,
    // The list has the keys and the window is active: selection in the accent. Set each render.
    list_focused: bool,
    window_active: bool,
    // The list column's width for this render's window size.
    list_w: f32,
    sidebar_w: f32,
    panes: Panes,
    preferences_path: Option<std::path::PathBuf>,
    resize: Option<Resize>,
    // The window title last set, so render sets it only when the list changes.
    title: SharedString,
    // The web view for HTML mail.
    web: html::Web,
    _appearance: gpui::Subscription,
    _activation: gpui::Subscription,
}

impl MainWindow {
    /// `demo`: the sample mail, no store and no network.
    pub fn new(demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        // The close button: a send in its Undo window goes first instead of being dropped.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| this.update(cx, |this, cx| !this.hold_exit(Exit::Close, cx)).unwrap_or(true));
        // `LAGOM_SCREEN` opens one screen at launch, for screenshots.
        let screen = std::env::var("LAGOM_SCREEN").unwrap_or_default();
        let (live, problem) = match demo {
            true => (None, None),
            false => match Live::start(window, cx) {
                Ok(live) => (Some(live), None),
                Err(e) => (None, Some(format!("Could not open the mail store: {e}"))),
            },
        };
        let inbox = match &live {
            Some(_) => Inbox::live(),
            None => Inbox::demo(if screen == "empty" { 0 } else { INBOX_LEN }),
        };
        let viewport = f32::from(window.viewport_size().width);
        // Demo mode never resolves a store path or reads/writes preferences.
        let preferences = if demo { Ok(None) } else {
            Store::default_path().map(|p| Some(p.with_file_name("window.json")))
        };
        let mut preference_problem = None;
        let preferences_path = match preferences {
            Ok(path) => path,
            Err(e) => {
                preference_problem = Some(format!("Could not locate window preferences: {e}"));
                None
            }
        };
        let panes = match preferences_path.as_ref().map(|p| Panes::load(p, viewport)).transpose() {
            Ok(panes) => panes.unwrap_or_else(|| Panes::new(viewport)),
            Err(e) => {
                preference_problem = Some(format!("Could not read window preferences: {e}"));
                Panes::new(viewport)
            }
        };
        let (sidebar_w, list_w) = panes.fit(viewport);
        let mut this = Self {
            theme: Theme::for_appearance(window.appearance(), DEFAULT_ACCENT),
            inbox,
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
            drafts: Vec::new(),
            sending: Vec::new(),
            exit: None,
            exit_timer: None,
            live,
            page: Page::Mail,
            all_folders: false,
            scroll: ScrollHandle::new(),
            scroll_hold: (Pixels::ZERO, false),
            glide: None,
            list_focused: false,
            window_active: false,
            list_w,
            sidebar_w,
            panes,
            preferences_path,
            resize: None,
            title: SharedString::default(),
            web: html::Web::default(),
            _appearance: appearance,
            _activation: cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        };
        if let Some(problem) = problem.or(preference_problem) {
            this.notify(problem, cx);
        }
        if this.live.is_some() {
            this.reload(cx);
            if this.live.as_ref().is_some_and(|l| l.accounts.is_empty()) || screen.starts_with("add-account") {
                this.add_account(window, cx);
            }
        }
        match screen.as_str() {
            "palette" => this.open_palette("done", window, cx),
            "later" => this.open_palette("later", window, cx),
            "folders" => {
                let first = folders::tree(&this.inbox.folders, false).first().map(|r| r.0);
                this.open_box(first, cx);
            }
            "settings" => this.page = Page::Settings(None),
            // The HTML newsletter and a simple HTML message.
            "html" => this.inbox.select(INBOX_LEN - 1),
            "html-simple" => this.inbox.select(3),
            "move" => this.open_palette("move tr", window, cx),
            "compose" => this.open_compose(true, window, cx),
            // A written reply with a word selected, Cc open with its focus ring, and live Markdown.
            "compose-text" | "compose-cc" | "compose-md" | "compose-md-bar" => {
                this.open_compose(true, window, cx);
                if let Some(c) = this.compose.clone() {
                    c.update(cx, |c, cx| match screen.as_str() {
                        "compose-text" => c.sample(cx),
                        "compose-cc" => c.add_cc(window, cx),
                        s => c.sample_markdown(s == "compose-md-bar", cx),
                    });
                }
            }
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
    // A sheet in its Undo window after Send is not an overlay: the inbox keys work, and ⌘Z still
    // reaches the sheet, which holds focus. Another page (add account, settings) takes no inbox keys.
    fn overlay_open(&self, cx: &mut Context<Self>) -> bool {
        let open = self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) || !matches!(self.page, Page::Mail);
        if open {
            cx.propagate();
        }
        open
    }

    /// A toast without Undo, for 5 s.
    fn notify(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::compose::UNDO_WINDOW).await;
            this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            })
            .ok();
        });
        self.toast = Some((text.into(), false, task));
        cx.notify();
    }

    fn open_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The overlay swallows the gesture's Ended, so end a swipe now.
        self.release(false, cx);
        // The overlay takes ⌘Z, so the toast's Undo would not work while it is open.
        self.toast = None;
        // The open folder stays listed, so its name never offers "New folder". Moving there is a no-op.
        let targets = folders::tree(&self.inbox.folders, true)
            .into_iter()
            .filter(|(f, _)| self.inbox.folders[*f].kind == Kind::Custom)
            .map(|(f, _)| (f, folders::path(&self.inbox.folders, f)))
            .collect();
        let subject = self.inbox.current().map(|id| self.inbox.mail[id].subject.clone());
        let search = self.live.as_ref().and_then(|l| l.search());
        let composing = self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal());
        let palette = cx.new(|cx| CommandPalette::new(query, composing, targets, subject, search, window, cx));
        cx.subscribe_in(&palette, window, |this, _, _: &gpui::DismissEvent, window, cx| {
            this.palette = None;
            if let Some(compose) = this.compose.as_ref().filter(|c| c.read(cx).is_modal()) {
                compose.update(cx, |c, cx| c.focus_body(window, cx));
            } else {
                window.focus(&this.focus);
            }
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
        cx.subscribe_in(&palette, window, |_, _, run: &Run, window, cx| {
            // The palette closes after this; its dismiss would take focus back from a new sheet.
            let run = *run;
            cx.defer_in(window, move |this, window, cx| this.run(run, window, cx));
        })
        .detach();
        window.focus(&gpui::Focusable::focus_handle(&palette, cx));
        self.palette = Some(palette);
        cx.notify();
    }

    fn run(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            Run::Done => self.act(Move::Done, cx),
            Run::Later(when) => self.act_with(self.inbox.selected, Move::Later, 0., when, cx),
            Run::Delete => self.act(Move::Delete, cx),
            Run::Reply => self.open_compose(true, window, cx),
            Run::Compose => self.open_compose(false, window, cx),
            Run::InsertFile => {
                if self.compose.is_none() { self.open_compose(false, window, cx); }
                if let Some(compose) = &self.compose {
                    compose.update(cx, |c, cx| c.insert_file(window, cx));
                }
            }
            Run::Sync => self.sync_now(),
            Run::Settings => self.open_settings(cx),
            Run::AddAccount => self.add_account(window, cx),
            Run::Open(id) => self.open_found(id, cx),
        }
    }

    /// `reply`: to the selected message. One draft at a time: R and C bring back a draft that
    /// Esc closed, whichever message is selected.
    fn open_compose(&mut self, reply: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.toast = None;
        let compose = match self.drafts.pop() {
            Some(compose) => {
                compose.update(cx, |c, cx| c.reopen(window, cx));
                compose
            }
            None => {
                let m = self.inbox.current().filter(|_| reply).map(|id| &self.inbox.mail[id]);
                let to = m.map_or("", |m| m.address.as_str());
                let subject = m.map_or(String::new(), |m| mail::reply_subject(&m.subject));
                let headers = m.map(|m| mail::reply_headers(&m.message_id, &m.thread_key));
                let from = match self.live.as_ref().and_then(|l| l.account()) {
                    Some(a) => a.email.clone(),
                    None => "Home · you@home.example".into(),
                };
                let compose = cx.new(|cx| Compose::new(&from, to, &subject, headers, window, cx));
                self.watch_compose(&compose, window, cx);
                compose
            }
        };
        self.compose = Some(compose);
        cx.notify();
    }

    /// Before quit or close. One rule: nothing unsent (a send on its way, a draft, an open sheet) is lost
    /// unless the user quits again while the toast that asks for it is on screen. Sheets in
    /// their Undo window send now. True: do not exit yet.
    fn hold_exit(&mut self, exit: Exit, cx: &mut Context<Self>) -> bool {
        for compose in self.sending.clone() {
            if let Some(mail) = compose.update(cx, |c, cx| c.send_now(cx)) {
                self.send_mail(compose, mail, cx);
            }
        }
        let sending = self.live.as_ref().is_some_and(|l| l.sending());
        let asked = self.toast.as_ref().is_some_and(|t| t.0 == QUIT_AGAIN_SENDING || t.0 == QUIT_AGAIN_UNSENT);
        let drafts = !self.drafts.is_empty() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal());
        if !sending && !drafts || asked {
            self.cancel_exit();
            return false;
        }
        if !sending {
            self.cancel_exit();
            self.notify(QUIT_AGAIN_UNSENT, cx);
            return true;
        }
        if self.exit.is_none() {
            self.exit_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(EXIT_WAIT).await;
                this.update(cx, |this, cx| {
                    if this.exit.take().is_some() {
                        this.notify(QUIT_AGAIN_SENDING, cx);
                    }
                })
                .ok();
            }));
        }
        self.exit = Some(exit);
        self.notify("Sending first, then closing", cx);
        true
    }

    fn cancel_exit(&mut self) {
        self.exit = None;
        self.exit_timer = None;
    }

    fn watch_compose(&mut self, compose: &Entity<Compose>, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(compose, window, |this, compose, _: &gpui::DismissEvent, window, cx| {
            if this.compose.as_ref() == Some(compose) {
                this.compose = None;
                if compose.read(cx).keeps_draft() {
                    this.drafts.push(compose.clone());
                }
            }
            this.sending.retain(|c| c != compose);
            if this.compose.is_none() && this.palette.is_none() {
                window.focus(&this.focus);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(compose, window, |this, compose, e: &ComposeEvent, window, cx| {
            match e {
                // The sheet leaves the modal slot but stays alive for its Undo window. The inbox
                // takes the keys meanwhile.
                ComposeEvent::Sending => {
                    if this.compose.as_ref() == Some(compose) {
                        this.compose = None;
                    }
                    this.sending.push(compose.clone());
                    if this.compose.is_none() && this.palette.is_none() {
                        window.focus(&this.focus);
                    }
                }
                ComposeEvent::Restored => {
                    let compose = compose.clone();
                    this.sending.retain(|c| *c != compose);
                    // A sheet opened meanwhile becomes the kept draft.
                    if let Some(other) = this.compose.replace(compose) {
                        this.drafts.push(other);
                    }
                }
                ComposeEvent::Send(mail) => this.send_mail(compose.clone(), (**mail).clone(), cx),
            }
            cx.notify();
        })
        .detach();
    }

    fn act(&mut self, what: Move, cx: &mut Context<Self>) {
        self.act_at(self.inbox.selected, what, 0., cx);
    }

    fn act_at(&mut self, ix: usize, what: Move, dx: f32, cx: &mut Context<Self>) {
        self.act_with(ix, what, dx, LaterWhen::Tomorrow, cx);
    }

    /// Optimistic: the row leaves at once, then the worker applies the move on the server.
    /// `when`: the time a Later move brings the message back.
    fn act_with(&mut self, ix: usize, what: Move, dx: f32, when: LaterWhen, cx: &mut Context<Self>) {
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
        self.server(what, id, when, cx);
        // The toast's Undo is ⌘Z, so it only shows while ⌘Z would undo this delete.
        if what == Move::Delete {
            self.notify("Deleted", cx);
            if let Some(toast) = &mut self.toast {
                toast.1 = true;
            }
        } else if self.toast.as_ref().is_some_and(|t| t.1) {
            self.toast = None;
        }
        cx.notify();
    }

    /// Shows the inbox (`None`) or a folder in the list.
    fn open_box(&mut self, f: Option<usize>, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.clear_motion();
        self.page = Page::Mail;
        self.inbox.open(f);
        if let Some(live) = &mut self.live {
            live.pin = None;
        }
        // Selection is row 0 of the new list; the old list's offset would hide it.
        self.scroll.set_offset(Point::default());
        self.reload(cx);
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
        self.unserver(what, id, cx);
        self.toast = None;
        self.come_back(what, id, ix, self.inbox.open != open, cx);
    }

    /// The reverse motion of `what` for a row put back at `ix`, after undo or a failed move.
    fn come_back(&mut self, what: Move, id: usize, ix: usize, list_changed: bool, cx: &mut Context<Self>) {
        self.glide = None;
        // Undo goes back to the list the message left, as the reverse motion needs it on screen.
        if list_changed {
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
        // The row comes back where it was, which may be scrolled off, or in another list.
        if self.inbox.selected == ix {
            let ghosts = self.ghosts.iter().filter(|g| g.ix <= ix).count();
            self.scroll.scroll_to_item(1 + ix + ghosts);
        }
        cx.notify();
    }

    /// Two-finger swipe on the list. Rows bubble first (GPUI paints child listeners after the
    /// parent's), so `under` already names the row when this runs.
    fn on_scroll(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let under = self.under.take();
        // Line deltas come from a classic wheel: no phases and no horizontal gesture to follow.
        let ScrollDelta::Pixels(d) = e.delta else { return };
        // A mouse press owns the swipe; trackpad momentum must not move or release it.
        if self.drag.is_some() || self.resize.is_some() || self.overlay_open(cx) {
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
        self.ghosts.retain(|g| g.at.elapsed() < SPRING * 2);
        self.entering.retain(|g| g.at.elapsed() < SPRING * 2);
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
        self.near_end(cx);
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
        .py(px(ROW_PAD * (1. - collapse)))
        .overflow_hidden()
        // The spring overshoots a little past 0 and 1; position follows it, opacity stops at the ends.
        .opacity((1. - p).clamp(0., 1.));
    // Opaque, so the colour of the move shows only where the row has left (see `with_behind`).
    let row = row.bg(t.bg);
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

/// Times, counts and other secondary labels: the system font with tabular figures, so digits
/// line up and a changing count never shifts its neighbours.
pub(crate) fn mono(text: impl Into<SharedString>, color: Rgba) -> Div {
    let mut el = div().text_size(px(size::SMALL)).text_color(color).child(text.into());
    el.text_style().get_or_insert_with(Default::default).font_features = Some(FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])));
    el
}

pub(crate) fn icon(path: &'static str, color: Rgba, s: f32) -> impl IntoElement {
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

/// One line that ends in "…" when it does not fit. GPUI's `truncate()` measures nowrap text once,
/// before the width is known, so it clips mid-glyph in flex rows; wrapping text clamped to one
/// line is measured again at its final width and gets the ellipsis.
pub(crate) trait OneLine: Styled + Sized {
    fn one_line(self) -> Self {
        self.line_clamp(1).text_ellipsis()
    }
}

impl<E: Styled> OneLine for E {}

/// Press feedback for anything clickable. As in native macOS controls: the arrow pointer, no
/// hover state, and an instant press tint (a fade would make the click feel late).
/// `selected` rows keep their selection fill while pressed.
pub(crate) fn pressable(el: Stateful<Div>, t: &Theme, selected: bool) -> Stateful<Div> {
    let press = t.press;
    el.when(!selected, |el| el.active(move |s| s.bg(press)))
}

/// A push button. `fill`: the colour of a primary button; `None` is a bordered one. Shortcuts
/// live in the menus and the command palette, not on the button.
pub(crate) fn button(t: &Theme, label: &'static str, fill: Option<Rgba>) -> Stateful<Div> {
    let base = div()
        .id(label)
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(radius::CONTROL))
        .text_size(px(size::BASE))
        .font_weight(FontWeight::MEDIUM)
        .child(label);
    match fill {
        // A translucent tint would replace the fill, so a filled button dims a little instead.
        Some(bg) => base.bg(bg).text_color(t.accent_ink).active(|s| s.opacity(0.8)),
        None => pressable(base.border_1().border_color(t.line).bg(t.bg).text_color(t.ink), t, false),
    }
}

impl MainWindow {
    fn save_panes(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = &self.preferences_path {
            if let Err(e) = self.panes.save(path) {
                self.notify(format!("Could not save window preferences: {e}"), cx);
            }
        }
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if let Some(resize) = self.resize.take() {
            if resize.moved {
                self.save_panes(cx);
            }
            cx.notify();
        }
    }

    fn resize_to(&mut self, x: Pixels, window: &Window, cx: &mut Context<Self>) {
        let Some(resize) = self.resize.as_mut() else { return };
        if x == resize.start_x && !resize.moved { return; }
        resize.moved = true;
        self.panes = resize.start_panes;
        let width = resize.start_width + f32::from(x - resize.start_x);
        let viewport = f32::from(window.viewport_size().width);
        match resize.divider {
            Divider::Sidebar => self.panes.resize_sidebar(width, viewport),
            Divider::List => self.panes.resize_list(width, viewport),
        }
        cx.notify();
    }

    fn divider(&self, divider: Divider, left: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(match divider { Divider::Sidebar => "sidebar-divider", Divider::List => "list-divider" })
            .absolute()
            .left(px(left))
            .top(px(8.))
            .bottom(px(8.))
            .w(px(6.))
            .occlude()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.release(false, cx);
                let viewport = f32::from(window.viewport_size().width);
                if e.click_count == 2 {
                    match divider {
                        Divider::Sidebar => {
                            this.panes.sidebar = SIDEBAR_DEFAULT;
                            this.panes.sidebar_hidden = false;
                        }
                        Divider::List => this.panes.list = default_list(viewport, this.sidebar_w),
                    }
                    this.resize = None;
                    this.save_panes(cx);
                } else {
                    this.resize = Some(Resize {
                        divider,
                        start_x: e.position.x,
                        start_width: match divider { Divider::Sidebar => this.sidebar_w, Divider::List => this.list_w },
                        start_panes: this.panes,
                        moved: false,
                    });
                }
                cx.notify();
            }))
    }

    /// Window listeners retain the drag when the pointer leaves the divider or the window.
    fn resize_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let resizing = self.resize.is_some();
        let on_move = cx.listener(|this, e: &MouseMoveEvent, window, cx| {
            if this.resize.is_some() {
                if e.pressed_button == Some(MouseButton::Left) {
                    this.resize_to(e.position.x, window, cx);
                } else {
                    this.finish_resize(cx);
                }
            }
        });
        let on_up = cx.listener(|this, e: &MouseUpEvent, window, cx| {
            if e.button == MouseButton::Left && this.resize.is_some() {
                this.resize_to(e.position.x, window, cx);
                this.finish_resize(cx);
            }
        });
        gpui::canvas(|_, _, _| (), move |_, _, window, _| {
            if resizing { window.set_window_cursor_style(CursorStyle::ResizeLeftRight); }
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                if phase.bubble() { on_move(e, window, cx); }
            });
            window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                if phase.bubble() { on_up(e, window, cx); }
            });
        }).absolute().size(px(0.))
    }

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
        // A sidebar keeps a grey selection; the active row's icon takes the accent while the
        // window is active, as in Finder and Mail. Weight never changes, so labels never shift.
        let active_icon = if self.window_active { t.sel_focus } else { t.ink2 };
        let nav = |path: &'static str, label: &'static str, count: Option<usize>, active: bool| {
            pressable(div().id(SharedString::from(format!("nav-{label}"))), t, active)
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(SIDEBAR_ROW))
                .px(px(10.))
                .rounded(px(radius::CONTROL))
                .text_color(t.ink)
                .when(active, |d| d.bg(t.sel))
                .child(icon(path, if active { active_icon } else { t.ink2 }, 16.))
                .child(div().flex_1().child(label))
                // A zero count is calm: no number at all.
                .when_some(count.filter(|&c| c > 0), |d, c| d.child(roll(mono(c.to_string(), t.ink2), format!("{label}-{c}"))))
        };
        let account = |ix: usize, name: SharedString, address: SharedString, active: bool| {
            pressable(div().id(("account", ix)), t, false)
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(40.))
                .px(px(10.))
                .rounded(px(radius::CONTROL))
                .child(
                    div()
                        .size(px(8.))
                        .flex_shrink_0()
                        .rounded_full()
                        .when(active, |d| d.bg(t.accent))
                        .when(!active, |d| d.border_1().border_color(t.ink2)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .min_w(px(0.))
                        .child(div().text_color(t.ink).font_weight(FontWeight::MEDIUM).one_line().child(name))
                        .child(div().text_size(px(size::SMALL)).text_color(t.ink2).one_line().child(address)),
                )
        };
        let accounts: Vec<(SharedString, SharedString, bool)> = match &self.live {
            Some(live) => live.accounts.iter().enumerate().map(|(i, a)| (live::short_name(a).into(), a.email.clone().into(), i == live.current)).collect(),
            None => mail::demo_accounts().iter().enumerate().map(|(i, a)| (live::short_name(a).into(), a.email.clone().into(), i == 0)).collect(),
        };
        let current = accounts.iter().find(|a| a.2).map_or(SharedString::from("Lagom"), |a| a.0.clone());
        let on_mail = matches!(self.page, Page::Mail);
        let open_kind = |kind| on_mail && self.inbox.open.is_some() && self.inbox.open == self.inbox.find_kind(kind);
        let mut account_rows = div().flex().flex_col().gap(px(2.)).child(heading("Accounts"));
        for (i, (name, address, active)) in accounts.into_iter().enumerate() {
            account_rows = account_rows.child(account(i, name, address, active).on_click(cx.listener(move |this, _, _, cx| this.switch_account(i, cx))));
        }
        let in_settings = matches!(self.page, Page::Settings(_));

        div()
            .w(px(self.sidebar_w))
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
                    .child(div().flex_1().min_w(px(0.)).one_line().child(current))
                    .child(icon("icons/chevron.svg", t.ink2, 14.)),
            )
            .child(
                div()
                    .id("search")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(radius::CONTROL))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.raised)
                    .text_color(t.ink2)
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette("", window, cx)))
                    .child(icon("icons/search.svg", t.ink2, 14.))
                    .child(div().flex_1().child("Search")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        nav("icons/inbox.svg", "Inbox", Some(self.inbox.count()), on_mail && self.inbox.open.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.open_box(None, cx))),
                    )
                    .child(
                        nav("icons/later.svg", "Later", Some(self.inbox.later), open_kind(Kind::Later))
                            .on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Later, cx))),
                    )
                    .child(nav("icons/sent.svg", "Sent", None, open_kind(Kind::Sent)).on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Sent, cx))))
                    .child(nav("icons/done.svg", "Done", None, open_kind(Kind::Done)).on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Done, cx)))),
            )
            .child(self.folder_group(heading("Folders"), cx))
            .child(account_rows)
            .child(div().flex_1())
            .child(
                pressable(div().id("settings"), t, in_settings)
                    .flex()
                    .items_center()
                    .h(px(SIDEBAR_ROW))
                    .px(px(10.))
                    .rounded(px(radius::CONTROL))
                    .text_color(t.ink)
                    .when(in_settings, |d| d.bg(t.sel))
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)))
                    .child("Settings"),
            )
    }

    /// The "Folders" group: the nav row style, subfolders indented, "N more" past six rows.
    fn folder_group(&self, heading: Div, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let all = &self.inbox.folders;
        let row = |id: ElementId, active: bool| {
            pressable(div().id(id), t, active).flex().items_center().gap(px(8.)).h(px(SIDEBAR_ROW)).px(px(10.)).rounded(px(radius::CONTROL))
        };
        // shortcut: "+" and "New folder" do nothing yet; ⌘K "move <name>" creates a folder. Wire
        // them to an inline name field once the app has text inputs.
        let group = div().flex().flex_col().gap(px(2.)).child(heading.child(icon("icons/plus.svg", t.ink2, 14.)));
        if !all.iter().any(Folder::listed) {
            return group.child(row("new-folder".into(), false).text_color(t.ink2).child(icon("icons/plus.svg", t.ink2, 16.)).child("New folder"));
        }
        // Once any folder has subfolders, every row keeps a chevron slot, so the icons line up.
        let nested = all.iter().any(|f| f.parent.is_some() && f.listed());
        let rows = folders::tree(all, false);
        let shown = folders::shown(rows.len(), self.all_folders);
        let slot = || div().size(px(12.)).flex_shrink_0();
        let active_icon = if self.window_active { t.sel_focus } else { t.ink2 };
        let mut group = group;
        for &(f, depth) in &rows[..shown] {
            let unread = self.inbox.unread(f);
            let active = self.inbox.open == Some(f);
            let color = if active || unread > 0 { t.ink } else { t.ink2 };
            let folder_icon = if active { active_icon } else { color };
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
                    .when(active, |d| d.bg(t.sel))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_box(Some(f), cx)))
                    .when(nested, |d| match chevron {
                        Some(c) => d.child(c),
                        None => d.child(slot()),
                    })
                    .child(icon("icons/folder.svg", folder_icon, 16.))
                    .child(div().flex_1().min_w(px(0.)).one_line().child(all[f].name.clone()))
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
        // The focused list in the active window selects in the accent with white text; otherwise grey.
        let (sel_bg, sel_ink, sel_ink2, dot) = if self.list_focused {
            let white = gpui::rgb(0xFFFFFF);
            (t.sel_focus, white, Rgba { a: 0.85, ..white }, white)
        } else {
            (t.sel, t.ink, t.ink2, t.accent)
        };
        // `bg`: paints the selection fill; `on_sel`: text colours for a row on the selection fill.
        // Unread is a dot and a semibold sender; the sender truncates, so nothing beside it moves.
        let row = |m: &Mail, bg: bool, on_sel: bool| {
            let (ink, ink2) = if on_sel { (sel_ink, sel_ink2) } else { (t.ink, t.ink2) };
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .h(px(ROW_H))
                .gap(px(ROW_GAP))
                .px(px(10.))
                .py(px(ROW_PAD))
                .rounded(px(radius::CONTROL))
                .text_color(ink)
                .when(bg, |d| d.bg(sel_bg))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().size(px(7.)).flex_shrink_0().rounded_full().when(m.unread, |d| d.bg(if on_sel { dot } else { t.accent })))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .one_line()
                                .when(m.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(m.from.clone()),
                        )
                        .child(mono(m.time.clone(), ink2)),
                )
                .child(div().pl(px(13.)).one_line().child(m.subject.clone()))
                .child(
                    div()
                        .pl(px(13.))
                        .text_size(px(size::SNIPPET))
                        .text_color(ink2)
                        .line_clamp(SNIPPET_LINES)
                        .text_ellipsis()
                        .child(m.snippet.clone()),
                )
        };
        let theme = self.theme;
        let ghosts_at = |ix: usize| {
            self.ghosts.iter().filter(move |g| g.ix == ix).map(move |g| {
                let (what, dx) = (g.what, g.dx);
                let el = row(&self.inbox.mail[g.id], false, false)
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
                .bg(sel_bg)
                .with_animation(("glide", g.seq), Animation::new(GLIDE).with_easing(ease_out_quint()), move |el, d| {
                    el.top(px(g.from + (g.to - g.from) * d))
                })
                .into_any_element(),
            None => div().into_any_element(),
        }];
        let dragging = self.mouse_swiping();
        // Only rows near the viewport or the selection get text; the rest are empty boxes of the
        // same height. Shaping ~500 rows on every animation frame made E, L and the arrows stutter.
        // One element per row stays, so `scroll_to_item` indices still hold.
        let top = -f32::from(self.scroll.offset().y);
        let view = f32::from(self.scroll.bounds().size.height).max(1200.);
        let (first, last) = ((top / ROW_H) as isize - 8, ((top + view) / ROW_H) as isize + 8);
        let (sel, span) = (self.inbox.selected as isize, (view / ROW_H) as isize + 8);
        let near = |i: usize, id: usize| {
            let i = i as isize;
            (first..=last).contains(&i)
                || (sel - span..=sel + span).contains(&i)
                || self.entering.iter().any(|g| g.id == id)
                || self.swipe.offset(id).is_some()
                || self.spring.is_some_and(|s| s.0 == id)
        };
        for (i, &id) in self.inbox.list().iter().enumerate() {
            rows.extend(ghosts_at(i));
            if !near(i, id) {
                rows.push(div().flex_shrink_0().h(px(ROW_H)).into_any_element());
                continue;
            }
            let selected = i == self.inbox.selected;
            // White text waits for the gliding fill to arrive. The glide animates frame by frame,
            // so the frame it lands on renders this again.
            let landed = gliding.is_none_or(|g| g.at.elapsed() >= GLIDE);
            let el = row(&self.inbox.mail[id], selected && gliding.is_none(), selected && landed)
                .id(("row", id))
                // Arrow at rest, as in native lists.
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
                let el = el.relative().bg(if selected { sel_bg } else { t.bg });
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
                // At rest. No hover fill, as in native lists; a press tints, the selection keeps its fill.
                None => {
                    let press = t.press;
                    el.when(!selected, |el| el.active(move |s| s.bg(press))).into_any_element()
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
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(t.raised)
                    .child("All"),
            )
            .child(
                pressable(div().id("filter-unread"), t, false)
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_color(t.ink2)
                    .child("Unread"),
            );

        // Title and subtitle as in a macOS toolbar: the count, and a quiet sync status.
        let left = self.inbox.shown_count();
        let subtitle = match self.sync_state() {
            Some(sync) => format!("{left} left · {sync}"),
            None => format!("{left} left"),
        };
        div()
            .w(px(self.list_w))
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
                    .gap(px(8.))
                    .h(px(HEADER_H))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .child(div().font_weight(FontWeight::BOLD).one_line().child(self.list_title()))
                            .child(roll(mono(subtitle, t.ink2).one_line(), format!("left-{left}"))),
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
    }

    /// The list on screen, for its header and the window title.
    fn list_title(&self) -> SharedString {
        self.inbox.open.map_or("Inbox".into(), |f| self.inbox.folders[f].name.clone().into())
    }

    fn reading_pane(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let Some(id) = self.inbox.current() else { return div().flex_1() };
        let m = &self.inbox.mail[id];
        let initials: String = m.from.split_whitespace().take(2).filter_map(|w| w.chars().next()).collect();
        let to = if m.to.is_empty() { "you".to_string() } else { m.to.clone() };
        // HTML mail: the web view covers the body area and draws header and mail itself. GPUI
        // draws the same header under it, and the text only while an overlay hides the web view.
        let web = html::SUPPORTED && m.html.is_some();
        let text = !web || self.palette.is_some() || self.compose.is_some();
        // Toasts sit just above the reply field; the web view would cover them.
        let toasts = self.toast.is_some() as usize + self.sending.iter().filter(|c| c.read(cx).undoable()).count();
        let slot_bottom = 20. + 36. + 12. + 48. * toasts as f32;

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
                    .h(px(HEADER_H))
                    .flex_shrink_0()
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        // Pine green, like the Done swipe.
                        button(t, "Done", Some(t.swipe_done))
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Done, cx))),
                    )
                    .child(button(t, "Reply", None).on_click(cx.listener(|this, _, window, cx| this.open_compose(true, window, cx))))
                    .child(
                        button(t, "Later", None)
                            .on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx))),
                    )
                    .when(web && self.web.has_remote && !m.remote, |d| {
                        d.child(button(t, "Load remote images", None).on_click(cx.listener(move |this, _, _, cx| {
                            this.inbox.mail[id].remote = true;
                            cx.notify();
                        })))
                    })
                    // shortcut: no Forward button until forwarding exists.
                    .child(div().flex_1())
                    .child(mono(format!("{} / {}", self.inbox.selected + 1, self.inbox.shown_count()), t.ink2)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .flex()
                    .flex_col()
                    .when(web, |d| d.child(div().absolute().top_0().left_0().right_0().bottom(px(slot_bottom)).child(self.web_slot())))
                    .child(
                div()
                    .id(("reading", id))
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .pt(px(24.))
                    .px(px(32.))
                    .pb(px(20.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_size(px(size::SUBJECT))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(m.subject.clone()),
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
                                            .min_w(px(0.))
                                            .child(div().font_weight(FontWeight::SEMIBOLD).child(m.from.clone()))
                                            .child(
                                                div()
                                                    .text_size(px(size::SMALL))
                                                    .text_color(t.ink2)
                                                    .one_line()
                                                    .child(format!("{} → {to}", m.address)),
                                            ),
                                    )
                                    .child(mono(m.time.clone(), t.ink2)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .text_size(px(size::BODY))
                            .line_height(relative(READING_LEADING))
                            // Until the body loads (one store read), the snippet stands in.
                            .when(m.body.is_empty() && text, |d| d.child(div().text_color(t.ink2).child(m.snippet.clone())))
                            .when(text, |d| d.children(m.body.iter().map(|p| div().child(p.clone()))))
                            .children(m.attachment.clone().map(|(name, kb)| {
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
                                            .line_height(relative(LEADING))
                                            .child(div().font_weight(FontWeight::MEDIUM).child(name))
                                            .child(mono(kb, t.ink2)),
                                    )
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("reply-box")
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_between()
                            .h(px(36.))
                            .px(px(12.))
                            .rounded(px(radius::CONTROL))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.raised)
                            .text_color(t.ink2)
                            .on_click(cx.listener(|this, _, window, cx| this.open_compose(true, window, cx)))
                            .child(format!("Reply to {}", m.from)),
                    ),
                    ),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.prune();
        self.open_current(cx);
        self.window_active = window.is_window_active();
        self.list_focused = self.window_active && self.focus.is_focused(window);
        // A definite pixel width keeps ellipses intact during row motion and live resizing.
        (self.sidebar_w, self.list_w) = self.panes.fit(f32::from(window.viewport_size().width));
        self.web_frame(window, cx);
        // First launch: the add-account flow fills the window; there is nothing for a sidebar yet.
        let welcome = self.live.as_ref().is_some_and(|l| l.accounts.is_empty());
        let title = match &self.page {
            Page::Mail => self.list_title(),
            Page::Settings(_) => "Settings".into(),
            Page::AddAccount(_) => "Add Account".into(),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let empty = matches!(self.page, Page::Mail) && self.inbox.list().is_empty() && self.inbox.open.is_none();
        let reading = matches!(self.page, Page::Mail) && !empty && self.inbox.current().is_some();
        let panel = match &self.page {
            Page::AddAccount(add) => div().flex_1().min_w(px(0.)).child(add.clone()),
            Page::Settings(confirm) => {
                let demo = mail::demo_accounts();
                crate::settings::view(&t, self.live.as_ref().map_or(&demo[..], |l| &l.accounts), confirm.as_deref(), cx)
            }
            // Undo from the empty screen brings the list back. An empty folder keeps its header.
            Page::Mail if empty => crate::empty_inbox::view(&t, self.sync_line().unwrap_or_else(|| "Home · synced 10:12".into())),
            Page::Mail => div().flex_1().min_w(px(0.)).flex().child(self.message_list(cx)).child(self.reading_pane(cx)),
        };
        // Toasts sit bottom-centre in the reading pane, above its reply field, so they never cover
        // the list or the divider. A spacer the list's width stands in for the list.
        let toasts = self.toasts(window, cx);
        let toasts = (!toasts.is_empty()).then(|| {
            let layer = |bottom| crate::compose::toast_layer(bottom).children(toasts);
            match reading {
                true => div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .child(div().w(px(self.list_w)).flex_shrink_0())
                    // Pane padding, reply field and a gap.
                    .child(div().flex_1().relative().child(layer(20. + 36. + 12.))),
                false => layer(24.),
            }
        });
        // The sidebar shows the blurred window background through a frosted tint: a step lighter
        // than the page in dark mode, so it reads as frost, not a hole. At 0.96 only the
        // desktop's colour comes through, not its shapes.
        // shortcut: the blur radius is AppKit's fixed NSVisualEffectView one; a stronger blur needs
        // native code (see spike/liquid-glass).
        let frost = if t.dark { gpui::rgb(0x2A2A2A) } else { gpui::rgb(0xF3F2EF) };
        let tint = Rgba { a: 0.96, ..frost };
        div()
            .key_context("MainWindow")
            // Mouse swipe: the root sees every move inside the window; `on_mouse_up_out` catches a
            // release outside it. shortcut: moves outside the window are not followed (the row waits
            // at the edge); a window-level listener via `window.on_mouse_event` would follow them.
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag_to(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .when(self.mouse_swiping(), |d| d.cursor(CursorStyle::ClosedHand))
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.web_unfocus()))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| if !this.overlay_open(cx) { this.select(Inbox::next, cx) }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| if !this.overlay_open(cx) { this.select(Inbox::prev, cx) }))
            .on_action(cx.listener(|this, _: &Done, _, cx| if !this.overlay_open(cx) { this.act(Move::Done, cx) }))
            .on_action(cx.listener(|this, _: &Later, _, cx| if !this.overlay_open(cx) { this.act(Move::Later, cx) }))
            // The guard hands ⌫ to the palette, where it edits the query.
            .on_action(cx.listener(|this, _: &DeleteMessage, _, cx| if !this.overlay_open(cx) { this.act(Move::Delete, cx) }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                if this.overlay_open(cx) {
                    return;
                }
                // The newest send in its Undo window goes first: its toast is the one on top.
                let sent = this.sending.iter().rev().find(|c| c.read(cx).undoable()).cloned();
                if !sent.is_some_and(|c| c.update(cx, |c, cx| c.undo(window, cx))) {
                    this.undo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                if this.palette.is_none() && matches!(this.page, Page::Mail) { this.open_palette("", window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Reply, window, cx| if !this.overlay_open(cx) { this.open_compose(true, window, cx) }))
            .on_action(cx.listener(|this, _: &NewMessage, window, cx| if !this.overlay_open(cx) { this.open_compose(false, window, cx) }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| if this.palette.is_none() && this.compose.is_none() { this.open_settings(cx) }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.finish_resize(cx);
                this.panes.sidebar_hidden = !this.panes.sidebar_hidden;
                this.save_panes(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| if !this.overlay_open(cx) {
                this.finish_resize(cx);
                this.release(false, cx);
            }))
            .map(|d| exit_actions(d, cx))
            .size_full()
            .relative()
            .flex()
            .bg(tint)
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .line_height(relative(LEADING))
            .when(!welcome && !self.panes.sidebar_hidden, |d| d.child(self.sidebar(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .my(px(8.))
                    .mr(px(8.))
                    .when(welcome, |d| d.ml(px(8.)).mt(px(36.)))
                    .when(!welcome && self.panes.sidebar_hidden && cfg!(target_os = "macos"), |d| d.mt(px(36.)))
                    .bg(t.bg)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(radius::PANEL))
                    .overflow_hidden()
                    .relative()
                    .child(panel)
                    .children(toasts)
                    .when(matches!(self.page, Page::Mail) && !empty, |d| d.child(self.divider(Divider::List, self.list_w - 3., cx))),
            )
            .when(!welcome && !self.panes.sidebar_hidden, |d| d.child(self.divider(Divider::Sidebar, self.sidebar_w - 3., cx)))
            .child(self.resize_events(cx))
            .children(self.palette.clone())
            .children(self.sending.clone())
            .children(self.compose.clone())
    }
}

impl MainWindow {
    /// The window's toast and a "Sending" toast for each send in its Undo window, newest last.
    fn toasts(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = self.theme;
        let undo = || button(&t, "Undo", None);
        // Each toast in its own id scope, so their entry animations stay apart.
        let scoped = |i: usize, el: AnyElement| div().id(("toast", i)).child(el).into_any_element();
        let mut out = Vec::new();
        for (i, c) in self.sending.iter().enumerate().filter(|(_, c)| c.read(cx).undoable()) {
            let c = c.clone();
            let on_undo = window.listener_for(&c, |c, _, window, cx| {
                c.undo(window, cx);
            });
            // Nothing has left yet: it goes when the Undo window ends.
            out.push(scoped(i, crate::compose::toast(&t, "Sending", Some(undo().on_click(on_undo).into_any_element())).into_any_element()));
        }
        if let Some((text, offers_undo, _)) = &self.toast {
            let action = offers_undo.then(|| undo().on_click(cx.listener(|this, _, _, cx| this.undo(cx))).into_any_element());
            out.push(scoped(usize::MAX, crate::compose::toast(&t, text.clone(), action).into_any_element()));
        }
        out
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
/// A long left swipe deletes: about 60 % of the 408 px row in the widest (420 px) list. A
/// narrower list keeps the same distance, so the swipe runs further past the row's width.
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

/// ⌘Q and ⌘W (macOS menu) wait for sends in their Undo window. Menu and key actions reach
/// the window before the app's global handlers, which run when this one propagates.
fn exit_actions<E: InteractiveElement>(el: E, cx: &mut Context<MainWindow>) -> E {
    #[cfg(target_os = "macos")]
    let el = el
        .on_action(cx.listener(|this, _: &crate::menus::Quit, _, cx| if !this.hold_exit(Exit::Quit, cx) { cx.propagate() }))
        .on_action(cx.listener(|this, _: &crate::menus::CloseWindow, _, cx| if !this.hold_exit(Exit::Close, cx) { cx.propagate() }));
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
    el
}

/// How long quit or close waits for sends before it lets the user quit anyway.
const EXIT_WAIT: Duration = Duration::from_secs(90);
/// While one of these toasts shows (5 s), the next quit or close goes and drops what is unsent.
const QUIT_AGAIN_UNSENT: &str = "Unsent mail kept. Quit again to discard it.";
const QUIT_AGAIN_SENDING: &str = "Still sending. Quit again to quit anyway; unsent mail is then lost.";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Exit {
    Close,
    Quit,
}

/// Inbox and folder state as ids into `mail`. Plain Rust, so tests run without a window.
struct Inbox {
    /// Every message the window has shown, by id. Ids stay valid across store reloads, so rows
    /// leaving, coming back and undo history never point at another message.
    mail: Vec<Mail>,
    /// Store id to id in `mail`.
    by_store: HashMap<i64, usize>,
    /// The inbox.
    items: Vec<usize>,
    /// Inbox messages in the store past the loaded page.
    extra: usize,
    folders: Vec<Folder>,
    /// The list on screen: the inbox (`None`) or a folder.
    open: Option<usize>,
    selected: usize,
    later: usize,
    /// Deleted message ids, newest first.
    trash: Vec<usize>,
    // (what, message id, list it left, index it left from).
    history: Vec<(Move, usize, Option<usize>, usize)>,
}

impl Inbox {
    fn new(len: usize, later: usize) -> Self {
        Self { mail: mail::demo(), by_store: HashMap::new(), items: (0..len).collect(), extra: 0, folders: Vec::new(), open: None, selected: 0, later, trash: Vec::new(), history: Vec::new() }
    }

    /// The sample inbox and folders, with their unread counts.
    fn demo(len: usize) -> Self {
        let mut inbox = Self { folders: folders::sample(), ..Self::new(len, LATER_SAMPLE) };
        for f in 0..inbox.folders.len() {
            inbox.folders[f].unread = inbox.folders[f].items.iter().filter(|&&id| inbox.mail[id].unread).count();
        }
        inbox
    }

    /// Empty until the first store read. Later is a local list, not a server folder.
    fn live() -> Self {
        let later = Folder { kind: Kind::Later, ..Folder::new("Later", None, &[]) };
        Self { mail: Vec::new(), folders: vec![later], ..Self::new(0, 0) }
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

    /// The Inbox count in the sidebar: what is loaded plus what is not.
    fn count(&self) -> usize {
        self.items.len() + self.extra
    }

    /// The count for the list on screen.
    fn shown_count(&self) -> usize {
        match self.open {
            None => self.count(),
            Some(_) => self.list().len(),
        }
    }

    fn find_kind(&self, kind: Kind) -> Option<usize> {
        self.folders.iter().position(|f| f.kind == kind && !f.gone)
    }

    fn open_kind(&self) -> Option<Kind> {
        self.open.map(|f| self.folders[f].kind)
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
        self.folders[f].unread
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

    /// Unread counts and the Later count follow a message that leaves the list `from` for
    /// `what`, or comes `back`.
    fn recount(&mut self, id: usize, from: Option<usize>, what: Move, back: bool) {
        let unread = self.mail[id].unread as usize;
        let step = |n: &mut usize, k: usize, up: bool| if up { *n += k } else { *n = n.saturating_sub(k) };
        if let Some(f) = from {
            step(&mut self.folders[f].unread, unread, back);
            if self.folders[f].kind == Kind::Later {
                step(&mut self.later, 1, back);
            }
        }
        match what {
            Move::Later => step(&mut self.later, 1, !back),
            Move::Folder(f) => step(&mut self.folders[f].unread, unread, !back),
            Move::Done | Move::Delete => {}
        }
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
        // Done in Done, Later in Later and Delete in Trash would move a message where it is.
        let here = match what {
            Move::Done => Some(Kind::Done),
            Move::Later => Some(Kind::Later),
            Move::Delete => Some(Kind::Trash),
            Move::Folder(_) => None,
        };
        if here.is_some() && self.open_kind() == here {
            return None;
        }
        let id = *self.list().get(ix)?;
        self.list_mut().remove(ix);
        if ix < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.list().len().saturating_sub(1));
        self.recount(id, self.open, what, false);
        match what {
            Move::Folder(f) => self.folders[f].items.insert(0, id),
            Move::Delete => self.trash.insert(0, id),
            Move::Later | Move::Done => {}
        }
        self.history.push((what, id, self.open, ix));
        Some((id, ix))
    }

    /// Puts the last moved message back where it was, opens that list and selects the message.
    fn undo(&mut self) -> Option<(Move, usize, usize)> {
        let last = self.history.len().checked_sub(1)?;
        self.restore(last, true)
    }

    /// A move of `id` failed on the server: take it back. Returns where the row reappears, if its
    /// list is on screen; otherwise the next store read shows it there.
    fn revert(&mut self, id: usize) -> Option<(Move, usize, usize)> {
        let at = self.history.iter().rposition(|h| h.1 == id)?;
        self.restore(at, false)
    }

    /// `switch`: open the list the message left (undo). Otherwise the selection stays on its message.
    fn restore(&mut self, at: usize, switch: bool) -> Option<(Move, usize, usize)> {
        let (what, id, from, ix) = self.history.remove(at);
        self.recount(id, from, what, true);
        match what {
            Move::Folder(f) => self.folders[f].items.retain(|&i| i != id),
            Move::Delete => self.trash.retain(|&i| i != id),
            Move::Later | Move::Done => {}
        }
        if !switch && self.open != from {
            return None;
        }
        self.open = from;
        let was_empty = self.list().is_empty();
        let list = self.list_mut();
        list.retain(|&i| i != id);
        let ix = ix.min(list.len());
        list.insert(ix, id);
        if switch || was_empty {
            self.selected = ix;
        } else if ix <= self.selected {
            self.selected += 1;
        }
        Some((what, id, ix))
    }

    /// An optimistic move of `id` left its folder for `target` (unknown for a new folder or
    /// Later, where the folder stays).
    fn left(&mut self, id: usize, target: Option<mail::Place>) {
        let m = &mut self.mail[id];
        m.was.push(m.folder.clone());
        if let Some(target) = target {
            m.folder = target;
        }
    }

    /// Undo or a failed move: back to the folder it left last. Returns that folder.
    fn back(&mut self, id: usize) -> mail::Place {
        let m = &mut self.mail[id];
        if let Some(folder) = m.was.pop() {
            m.folder = folder;
        }
        m.folder.clone()
    }

    /// The id in `mail` for a store row, refreshed from the summary. A loaded body stays.
    fn intern(&mut self, m: &lagom_core::store::MessageSummary, seen: &HashSet<i64>, now: chrono::DateTime<chrono::Local>) -> usize {
        let mut fresh = Mail::from_summary(m, now);
        // Read here, and the server has not confirmed it yet.
        if seen.contains(&m.id) {
            fresh.unread = false;
        }
        match self.by_store.get(&m.id) {
            Some(&id) => {
                fresh.body = std::mem::take(&mut self.mail[id].body);
                fresh.html = self.mail[id].html.take();
                fresh.remote = self.mail[id].remote;
                fresh.was = std::mem::take(&mut self.mail[id].was);
                self.mail[id] = fresh;
                id
            }
            None => {
                self.mail.push(fresh);
                self.by_store.insert(m.id, self.mail.len() - 1);
                self.mail.len() - 1
            }
        }
    }

    /// Takes a store read. The selection stays on its message where it can.
    fn apply(&mut self, snap: mail::Snapshot, seen: &HashSet<i64>, now: chrono::DateTime<chrono::Local>) {
        let keep = self.current().and_then(|id| self.mail[id].store);
        self.items = snap.inbox.iter().map(|m| self.intern(m, seen, now)).collect();
        self.extra = (snap.inbox_total as usize).saturating_sub(self.items.len());
        self.later = snap.later_total as usize;
        folders::merge(&mut self.folders, &snap.folders);
        if let Some(f) = self.open {
            if self.folders[f].gone {
                self.open = None;
            } else {
                self.folders[f].items = snap.open.iter().map(|m| self.intern(m, seen, now)).collect();
            }
        }
        let found = keep.and_then(|s| self.list().iter().position(|&id| self.mail[id].store == Some(s)));
        self.selected = found.unwrap_or(self.selected).min(self.list().len().saturating_sub(1));
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
        Inbox { later: 0, ..Inbox::demo(INBOX_LEN) }
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
    fn failed_move_comes_back_and_keeps_the_selection() {
        let mut inbox = with_folders();
        // Archive 1, then 3 (ids), selection on 4 after both.
        inbox.act_at(1, Move::Done);
        inbox.act_at(2, Move::Folder(1));
        assert_eq!(inbox.items, [0, 2, 4, 5, 6]);
        inbox.select(2);
        // The server refuses the first move: 1 comes back at its place, 4 stays selected.
        assert_eq!(inbox.revert(1), Some((Move::Done, 1, 1)));
        assert_eq!((inbox.items.as_slice(), inbox.current()), ([0, 1, 2, 4, 5, 6].as_slice(), Some(4)));
        // The second fails while another list is open: no row now, but the counts go back.
        let before = inbox.unread(1);
        inbox.open(Some(0));
        assert_eq!(inbox.revert(3), None);
        assert_eq!(inbox.unread(1), before - 1);
        assert!(!inbox.folders[1].items.contains(&3));
        // Nothing left to revert or undo.
        assert_eq!(inbox.revert(3), None);
        assert_eq!(inbox.undo(), None);
    }

    #[test]
    fn undo_goes_back_to_the_folder_the_action_left() {
        use mail::Place::{Folder as F, New};
        let mut inbox = with_folders();
        inbox.mail[1].folder = F(10);
        // INBOX 10 to Projects 20, then from Projects to Archive 30, before any store read.
        inbox.left(1, Some(F(20)));
        inbox.left(1, Some(F(30)));
        assert_eq!(inbox.mail[1].folder, F(30));
        assert_eq!(inbox.back(1), F(20));
        assert_eq!(inbox.back(1), F(10));
        // Nothing left: it stays where it is.
        assert_eq!(inbox.back(1), F(10));
        // Later keeps the folder but still stacks, so undo stays in step.
        inbox.left(1, None);
        assert_eq!((inbox.mail[1].folder.clone(), inbox.back(1)), (F(10), F(10)));
        // Into a folder ⌘K is still creating, archived there: undo goes back to it by name.
        inbox.left(1, Some(New("Projects".into())));
        inbox.left(1, Some(F(30)));
        assert_eq!(inbox.back(1), New("Projects".into()));
    }

    #[test]
    fn no_move_into_the_list_it_is_in() {
        let mut inbox = with_folders();
        let done = inbox.folders.len();
        inbox.folders.push(Folder { kind: Kind::Done, ..Folder::new("Archive", None, &[0]) });
        inbox.folders.push(Folder { kind: Kind::Later, ..Folder::new("Later", None, &[1]) });
        inbox.open(Some(done));
        assert_eq!(inbox.act_at(0, Move::Done), None);
        assert!(inbox.act_at(0, Move::Delete).is_some());
        inbox.open(Some(done + 1));
        inbox.later = 1;
        assert_eq!(inbox.act_at(0, Move::Later), None);
        // Done on a message in Later takes it off Later.
        assert!(inbox.act_at(0, Move::Done).is_some());
        assert_eq!(inbox.later, 0);
    }

    #[test]
    fn store_reads_keep_ids_and_selection() {
        use lagom_core::store::{Folder as F, FolderRole, MessageSummary};
        let summary = |id: i64, subject: &str| MessageSummary { id, folder: 1, subject: subject.into(), seen: false, ..Default::default() };
        let snap = |rows: Vec<MessageSummary>, total| mail::Snapshot {
            inbox: rows,
            inbox_total: total,
            folders: vec![F { id: 1, account: "a".into(), name: "INBOX".into(), delimiter: None, role: FolderRole::Inbox, uidvalidity: None, uidnext: None, last_uid: 0, highest_modseq: None, unread: 0 }],
            ..Default::default()
        };
        let now = chrono::Local::now();
        let mut inbox = Inbox::live();
        inbox.apply(snap(vec![summary(10, "a"), summary(11, "b"), summary(12, "c")], 5), &HashSet::new(), now);
        assert_eq!((inbox.count(), inbox.items.len()), (5, 3));
        inbox.select(1);
        inbox.mail[1].body = vec!["loaded".into()];
        // New mail on top, 11 marked read here: same id for 11, body kept, still selected.
        inbox.apply(snap(vec![summary(13, "d"), summary(10, "a"), summary(11, "b")], 6), &[11].into(), now);
        assert_eq!(inbox.items, [3, 0, 1]);
        assert_eq!((inbox.current(), inbox.mail[1].unread, inbox.mail[1].body.len()), (Some(1), false, 1));
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
        cx.add_window_view(|window, cx| MainWindow::new(true, window, cx))
    }

    fn inbox_len(view: &Entity<MainWindow>, cx: &mut VisualTestContext) -> usize {
        view.read_with(cx, |v, _| v.inbox.list().len())
    }

    #[gpui::test]
    fn dividers_resize_reset_collapse_and_leave_swipes_idle(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, cx| { v.panes = Panes::new(1440.); cx.notify(); });
        cx.simulate_resize(gpui::size(px(1440.), px(900.)));
        cx.run_until_parked();
        let at = |x| gpui::point(px(x), px(200.));
        let widths = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| (v.sidebar_w, v.list_w));
        cx.simulate_mouse_down(at(220.), MouseButton::Left, Default::default());
        cx.simulate_mouse_move(at(300.), MouseButton::Left, Default::default());
        assert_eq!(widths(cx).0, 300.);
        cx.simulate_mouse_up(at(300.), MouseButton::Left, Default::default());
        let list_edge = widths(cx).0 + 1. + widths(cx).1;
        cx.simulate_mouse_down(at(list_edge), MouseButton::Left, Default::default());
        cx.simulate_mouse_move(at(list_edge + 80.), MouseButton::Left, Default::default());
        assert_eq!(widths(cx).1, 500.);
        cx.simulate_mouse_up(at(list_edge + 80.), MouseButton::Left, Default::default());
        assert!(view.read_with(cx, |v, _| v.resize.is_none() && v.drag.is_none() && v.swipe == Swipe::Idle));
        let list_edge = widths(cx).0 + 1. + widths(cx).1;
        cx.simulate_event(MouseDownEvent { position: at(list_edge), click_count: 2, ..Default::default() });
        assert_eq!(widths(cx).1, default_list(1440., 300.));
        cx.simulate_event(MouseDownEvent { position: at(300.), click_count: 2, ..Default::default() });
        assert_eq!(widths(cx).0, 220.);
        cx.simulate_mouse_down(at(220.), MouseButton::Left, Default::default());
        cx.simulate_mouse_move(at(100.), MouseButton::Left, Default::default());
        assert_eq!(widths(cx).0, 0.);
        cx.simulate_mouse_up(at(100.), MouseButton::Left, Default::default());
        cx.simulate_keystrokes("alt-secondary-s");
        assert_eq!(widths(cx).0, 220.);
        cx.simulate_keystrokes("alt-secondary-s");
        assert_eq!(widths(cx).0, 0.);
        assert!(view.read_with(cx, |v, _| v.preferences_path.is_none()));
    }

    #[gpui::test]
    fn sidebar_collapse_restores_width_from_drag_start(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, cx| { v.panes.sidebar = 300.; cx.notify(); });
        cx.simulate_resize(gpui::size(px(1440.), px(900.)));
        cx.run_until_parked();
        let at = |x| gpui::point(px(x), px(200.));
        cx.simulate_mouse_down(at(300.), MouseButton::Left, Default::default());
        cx.simulate_mouse_move(at(140.), MouseButton::Left, Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.sidebar_w), 180.);
        cx.simulate_mouse_move(at(119.), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(at(119.), MouseButton::Left, Default::default());
        cx.simulate_keystrokes("alt-secondary-s");
        assert_eq!(view.read_with(cx, |v, _| v.sidebar_w), 300.);
    }

    #[gpui::test]
    fn divider_clicks_preserve_preferences_in_a_narrow_window(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let path = std::env::temp_dir().join(format!("lagom-divider-click-{}.json", std::process::id()));
        let panes = Panes { sidebar: 320., list: 560., sidebar_hidden: false };
        // Valid JSON with trailing whitespace also detects an unnecessary save.
        let bytes = format!("{}\n", serde_json::to_string(&panes).unwrap());
        std::fs::write(&path, &bytes).unwrap();
        view.update(cx, |v, cx| {
            v.panes = panes;
            v.preferences_path = Some(path.clone());
            cx.notify();
        });
        cx.simulate_resize(gpui::size(px(970.), px(900.)));
        cx.run_until_parked();
        let (sidebar, list) = panes.fit(970.);
        for x in [sidebar, sidebar + 1. + list] {
            let at = gpui::point(px(x), px(200.));
            cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
            cx.simulate_mouse_move(at, MouseButton::Left, Default::default());
            cx.simulate_mouse_up(at, MouseButton::Left, Default::default());
            assert_eq!(view.read_with(cx, |v, _| v.panes), panes);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn palette_takes_letters_backspace_and_undo(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = inbox_len(&view, cx);
        cx.simulate_keystrokes("secondary-k");
        cx.simulate_input("elrj");
        cx.simulate_keystrokes("backspace left right");
        let palette = view.read_with(cx, |v, _| v.palette.clone()).expect("palette open");
        assert_eq!(palette.read_with(cx, |p, cx| p.query(cx)), "elr");
        // None of e, l, r, j, ⌫, ←, → reached the inbox.
        assert_eq!(inbox_len(&view, cx), n);
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(palette.read_with(cx, |p, cx| p.query(cx)), "");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.palette.is_none()));
        // Without a field, the same keys act on the inbox again.
        cx.simulate_keystrokes("e");
        assert_eq!(inbox_len(&view, cx), n - 1);
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(inbox_len(&view, cx), n);
    }

    #[gpui::test]
    fn palette_opens_over_compose_and_returns_to_the_body(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("**Hej**");
        cx.simulate_keystrokes("secondary-k");
        let palette = view.read_with(cx, |v, _| v.palette.clone()).expect("palette over compose");
        cx.simulate_input("insert");
        assert_eq!(palette.read_with(cx, |p, cx| p.query(cx)), "insert");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.palette.is_none()));
        cx.simulate_input(" Åsa");
        let compose = view.read_with(cx, |v, _| v.compose.clone()).unwrap();
        assert_eq!(compose.read_with(cx, |c, cx| c.body_text(cx)), "**Hej** Åsa");
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
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(body(&view, cx), "");
        cx.simulate_input("Hello");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("Alex");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.compose.is_none() && v.drafts.len() == 1));
        cx.simulate_keystrokes("r");
        assert_eq!(body(&view, cx), "Hello\nAlex");
        // ⌘⌫ in the body discards the draft, not a message.
        cx.simulate_keystrokes("cmd-backspace");
        assert!(view.read_with(cx, |v, _| v.compose.is_none() && v.drafts.is_empty()));
        assert_eq!(inbox_len(&view, cx), n);
        cx.simulate_keystrokes("r");
        assert_eq!(body(&view, cx), "");
    }

    #[gpui::test]
    fn quit_sends_a_mail_in_its_undo_window_now(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("c");
        cx.simulate_input("ana@example.com");
        cx.simulate_keystrokes("cmd-enter");
        let sheet = view.read_with(cx, |v, _| v.sending[0].clone());
        let id = sheet.read_with(cx, |c, cx| c.outgoing(cx).message_id);
        assert!(id.as_deref().is_some_and(|id| id.starts_with('<') && id.ends_with("@home.example>")));
        // The sheet hands over its mail at once, with the Message-ID a resend would reuse.
        let mail = sheet.update(cx, |c, cx| c.send_now(cx)).expect("mail in its Undo window");
        assert_eq!((mail.to.as_slice(), mail.message_id), (["ana@example.com".to_string()].as_slice(), id));
        assert!(sheet.update(cx, |c, cx| c.send_now(cx)).is_none());
        // Quit flushes every sheet; demo mode has no worker, so nothing holds the quit.
        cx.simulate_keystrokes("c");
        cx.simulate_input("bo@example.com");
        cx.simulate_keystrokes("cmd-enter");
        let second = view.read_with(cx, |v, _| v.sending.last().cloned()).unwrap();
        assert!(!view.update(cx, |v, cx| v.hold_exit(Exit::Quit, cx)));
        assert!(!second.read_with(cx, |c, _| c.undoable()));
    }

    #[gpui::test]
    fn a_waiting_quit_also_sends_mail_sent_meanwhile(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        // Quit is waiting for mail A; mail B was sent after it and is in its Undo window.
        view.update(cx, |v, _| v.exit = Some(Exit::Quit));
        cx.simulate_keystrokes("c");
        cx.simulate_input("bo@example.com");
        cx.simulate_keystrokes("cmd-enter");
        let b = view.read_with(cx, |v, _| v.sending.last().cloned()).unwrap();
        assert!(b.read_with(cx, |c, _| c.undoable()));
        // A has finished: before quitting, B goes to the worker too (demo: nothing to wait for).
        assert_eq!(view.update(cx, |v, cx| v.exit_ready(cx)), Some(Exit::Quit));
        assert!(!b.read_with(cx, |c, _| c.undoable()));
    }

    #[gpui::test]
    fn quit_with_a_draft_asks_once_per_episode(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Not done yet");
        cx.simulate_keystrokes("escape");
        assert_eq!(view.read_with(cx, |v, _| v.drafts.len()), 1);
        // First quit keeps the draft and asks; quitting again while asked discards it.
        assert!(view.update(cx, |v, cx| v.hold_exit(Exit::Quit, cx)));
        assert!(!view.update(cx, |v, cx| v.hold_exit(Exit::Quit, cx)));
        // Once the toast has gone, the question is new: a single quit does not discard.
        cx.executor().advance_clock(Duration::from_secs(6));
        cx.run_until_parked();
        assert!(view.update(cx, |v, cx| v.hold_exit(Exit::Quit, cx)));
        // An open sheet with text counts too: reopening the draft must not let quit through.
        cx.executor().advance_clock(Duration::from_secs(6));
        cx.run_until_parked();
        cx.simulate_keystrokes("r");
        assert!(view.read_with(cx, |v, _| v.compose.is_some() && v.drafts.is_empty()));
        assert!(view.update(cx, |v, cx| v.hold_exit(Exit::Quit, cx)));
    }

    #[test]
    fn an_unsure_send_says_check_sent() {
        assert_eq!(live::sent_note(Delivery::Sent, None), "Sent");
        assert!(live::sent_note(Delivery::Maybe, None).starts_with("May have been sent. Check Sent before sending again."));
        assert!(live::sent_note(Delivery::NotSent, Some("refused".into())).starts_with("Not sent: refused."));
    }

    #[gpui::test]
    fn inbox_keys_work_while_a_send_can_be_undone(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = inbox_len(&view, cx);
        cx.simulate_keystrokes("c");
        cx.simulate_input("ana@example.com");
        cx.simulate_keystrokes("cmd-enter");
        assert!(view.read_with(cx, |v, _| v.compose.is_none() && v.sending.len() == 1));
        // The inbox has the keys during the Undo window.
        cx.simulate_keystrokes("e");
        assert_eq!(inbox_len(&view, cx), n - 1);
        // ⌘Z undoes the send first: the sheet comes back.
        cx.simulate_keystrokes("secondary-z");
        assert!(view.read_with(cx, |v, _| v.compose.is_some() && v.sending.is_empty()));
        assert_eq!(inbox_len(&view, cx), n - 1);
    }

    #[gpui::test]
    fn c_writes_from_the_empty_inbox(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        for _ in 0..inbox_len(&view, cx) {
            cx.simulate_keystrokes("e");
        }
        assert_eq!(inbox_len(&view, cx), 0);
        cx.simulate_keystrokes("c");
        assert!(view.read_with(cx, |v, _| v.compose.is_some()));
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

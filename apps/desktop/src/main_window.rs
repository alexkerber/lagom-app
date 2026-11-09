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
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT, LEADING, READING_LEADING};

actions!(main_window, [SelectNext, SelectPrev, ExtendNext, ExtendPrev, ToggleRead, Done, Later, DeleteMessage, Undo, OpenPalette, Reply, Cancel, NewMessage, OpenSettings, ToggleSidebar, OpenInWindow, NewTask]);

pub fn bind_keys(cx: &mut App) {
    tasks::bind_keys(cx);
    let ctx = Some("MainWindow");
    // Plain keys type into a focused text field instead. The cmd keys stay: the overlay guard
    // hands them on, and a field handles ⌘Z itself because it sits deeper in the dispatch path.
    let keys = Some("MainWindow && !TextInput && !Today");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, keys),
        KeyBinding::new("down", SelectNext, keys),
        KeyBinding::new("k", SelectPrev, keys),
        KeyBinding::new("up", SelectPrev, keys),
        KeyBinding::new("shift-down", ExtendNext, keys),
        KeyBinding::new("shift-up", ExtendPrev, keys),
        KeyBinding::new("secondary-a", crate::text_input::SelectAll, keys),
        KeyBinding::new("secondary-shift-u", ToggleRead, keys),
        KeyBinding::new("e", Done, keys),
        KeyBinding::new("t", NewTask, keys),
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
        KeyBinding::new("secondary-o", OpenInWindow, ctx),
    ]);
    // A message window takes the keys that act on its message. Esc does nothing there.
    let message = Some("MessageWindow && !TextInput");
    cx.bind_keys([
        KeyBinding::new("e", Done, message),
        KeyBinding::new("t", NewTask, message),
        KeyBinding::new("l", Later, message),
        KeyBinding::new("secondary-backspace", DeleteMessage, message),
        KeyBinding::new("backspace", DeleteMessage, message),
        KeyBinding::new("delete", DeleteMessage, message),
        KeyBinding::new("secondary-r", Reply, message),
        KeyBinding::new("r", Reply, message),
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
/// A press while the highlight still glides (a held arrow key) takes this shorter curve, so the
/// highlight keeps up with key repeat instead of trailing rows behind.
const GLIDE_REPEAT: Duration = Duration::from_millis(70);
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

/// Rows that leave, come back or spring back: long enough to read the motion (see `motion::spring`).
const SPRING: Duration = Duration::from_millis(280);

/// The selection highlight on its way between rows: y positions inside the list.
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    at: Instant,
    dur: Duration,
    seq: usize,
}

impl Glide {
    /// A glide to `to` that starts where `prev` is now, so a new key press never jumps.
    fn new(prev: Option<Glide>, from: f32, to: f32, now: Instant, seq: usize) -> Self {
        let dur = if prev.is_some_and(|g| !g.landed(now)) { GLIDE_REPEAT } else { GLIDE };
        Glide { from: prev.map_or(from, |g| g.y(now)), to, at: now, dur, seq }
    }

    fn y(&self, now: Instant) -> f32 {
        let d = (now.saturating_duration_since(self.at).as_secs_f32() / self.dur.as_secs_f32()).min(1.);
        self.from + (self.to - self.from) * ease_out_quint()(d)
    }

    fn landed(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) >= self.dur
    }
}

fn row_y(ix: usize) -> f32 {
    LIST_PAD + ix as f32 * ROW_H
}

mod html;
mod conversation;
mod selection;
mod live;
mod tasks;
mod agenda;
pub(crate) use tasks::TaskChange;
mod popout;
pub use live::Live;
pub(crate) use live::notify_without_window;
use popout::MessageWindow;

/// What fills the main panel.
enum Page {
    Mail,
    Today,
    AddAccount(Entity<AddAccount>),
    /// Settings, with the account whose removal waits for a confirm.
    Settings(Option<String>),
}

/// A removed row that still paints its exit motion. It is not in the inbox any more.
struct Ghost {
    id: usize,
    ix: usize,
    what: Move,
    at: Instant,
    // Where a swipe let go of the row, so the exit motion starts there. 0 for E and L.
    dx: f32,
}

impl Ghost {
    fn progress(&self, now: Instant) -> f32 {
        spring(now.saturating_duration_since(self.at).as_secs_f32() / SPRING.as_secs_f32())
    }
}

fn row_scale(p: f32) -> f32 {
    1. - ((p - 0.3) / 0.7).clamp(0., 1.)
}

struct RowLayout {
    len: usize,
    // Height changes after each index: exiting ghosts add space, entering rows subtract it.
    changes: Vec<(usize, f32)>,
}

impl RowLayout {
    fn y(&self, ix: usize) -> f32 {
        row_y(ix) + self.changes.iter().filter(|&&(i, _)| i < ix).map(|&(_, h)| h).sum::<f32>()
    }

    fn bottom(&self) -> f32 {
        row_y(self.len) + self.changes.iter().map(|&(_, h)| h).sum::<f32>()
    }

    fn index_at(&self, y: f32) -> usize {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.y(mid) <= y { lo = mid; } else { hi = mid - 1; }
        }
        lo
    }

    fn visible(&self, top: f32, height: f32) -> std::ops::Range<usize> {
        let overscan = 2. * ROW_H;
        self.index_at(top - overscan)..(self.index_at(top + height + overscan) + 1).min(self.len)
    }
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
    // (message id, offset it springs back from, start) survives rows leaving the viewport.
    spring: Option<(usize, f32, Instant)>,
    // (move it was, move it is, seq) for the colour behind a swiped row. See `retint`.
    tint: (Option<Move>, Option<Move>, usize),
    // The toast: its text, whether it offers Undo, and its timer. Dropping the task cancels it.
    toast: Option<(SharedString, bool, Task<()>)>,
    palette: Option<gpui::Entity<CommandPalette>>,
    compose: Option<gpui::Entity<Compose>>,
    // Saved drafts, newest last. R and C reopen the newest.
    drafts: Vec<gpui::Entity<Compose>>,
    // Sheets after Send, in their Undo window: they show a toast and are no longer modal.
    sending: Vec<Entity<Compose>>,
    // The store and the mail worker. `None` in demo mode.
    live: Option<live::Live>,
    page: Page,
    today: tasks::Today,
    task_undo: Option<i64>,
    // "N more" was clicked, so every folder row shows.
    all_folders: bool,
    confirm_empty_trash: bool,
    folder_name: Option<Entity<TextInput>>,
    folder_scroll: ScrollHandle,
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
    reading_thread: Option<usize>,
    reading_dirty: bool,
    reading_scroll: ScrollHandle,
    reading_members: Vec<usize>,
    expanded: HashMap<usize, Entity<conversation::MessageBody>>,
    // Messages open in their own windows: (message id, window).
    popouts: Vec<(usize, gpui::WindowHandle<MessageWindow>)>,
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
        // Flush local edits before close; queued sends need no network wait.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| this.update(cx, |this, cx| !this.hold_exit(cx)).unwrap_or(true));
        // `LAGOM_SCREEN` opens one screen at launch, for screenshots.
        let screen = std::env::var("LAGOM_SCREEN").unwrap_or_default();
        let (live, problem) = match demo {
            true => (None, None),
            false => match Live::start(cx) {
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
            live.as_ref().map(|live| live.store.path().to_path_buf())
                .map_or_else(Store::default_path, Ok).map(|p| Some(p.with_file_name("window.json")))
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
            live,
            page: if demo { Page::Mail } else { Page::Today },
            today: tasks::Today::default(),
            task_undo: None,
            all_folders: false,
            confirm_empty_trash: false,
            folder_name: None,
            folder_scroll: ScrollHandle::new(),
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
            reading_thread: None,
            reading_dirty: false,
            reading_scroll: ScrollHandle::new(),
            reading_members: Vec::new(),
            expanded: HashMap::new(),
            popouts: Vec::new(),
            _appearance: appearance,
            _activation: cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        };
        if let Some(problem) = problem.or(preference_problem) {
            this.notify(problem, cx);
        }
        this.inbox.group = this.panes.group_conversations;
        this.inbox.regroup();
        if this.live.is_some() {
            this.reload(cx);
            this.restore_drafts(window, cx);
            if screen.starts_with("add-account") {
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
            // A message window over the main window: plain text, or the HTML newsletter.
            "popout" | "popout-html" => {
                if screen == "popout-html" {
                    this.inbox.select(INBOX_LEN - 1);
                }
                cx.defer_in(window, |this, window, cx| this.open_popout(window, cx));
            }
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
        self.task_undo = None;
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
        self.folder_name = None;
        // The open folder stays listed, so its name never offers "New folder". Moving there is a no-op.
        let on_mail = matches!(self.page, Page::Mail);
        let targets = folders::tree(&self.inbox.folders, true)
            .into_iter()
            .filter(|(f, _)| on_mail && self.inbox.folders[*f].kind == Kind::Custom)
            .map(|(f, _)| (f, folders::path(&self.inbox.folders, f)))
            .collect();
        let subject = self.inbox.single().filter(|_| on_mail).map(|id| self.inbox.mail[id].subject.clone());
        let search = self.live.as_ref().filter(|_| on_mail).and_then(|l| l.search());
        let composing = self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal());
        let palette = cx.new(|cx| CommandPalette::new(query, composing, on_mail, targets, subject, search, window, cx));
        cx.subscribe_in(&palette, window, |this, _, _: &gpui::DismissEvent, window, cx| this.close_palette(window, cx)).detach();
        cx.subscribe(&palette, |this, _, target: &MoveTo, cx| {
            let f = match target {
                MoveTo::Folder(f) => *f,
                MoveTo::New(name) => {
                    let name = name.trim();
                    if let Err(e) = this.validate_folder_name(name) {
                        this.notify(e, cx);
                        return;
                    }
                    this.all_folders = true;
                    this.folder_scroll.scroll_to_bottom();
                    this.inbox.create_folder(name)
                }
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

    /// The keys go back to the sheet under the palette, or to the inbox.
    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        if let Some(compose) = self.compose.as_ref().filter(|c| c.read(cx).is_modal()) {
            compose.update(cx, |c, cx| c.focus_body(window, cx));
        } else {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    fn run(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            Run::NewTask => self.new_task(window, cx),
            Run::Done => self.act(Move::Done, cx),
            Run::Later(when) => self.act_selected(Move::Later, when, cx),
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
        self.compose_for(self.inbox.single().filter(|_| reply), window, cx);
    }

    /// `reply`: to this message id. See `open_compose`.
    fn compose_for(&mut self, reply: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.toast = None;
        let compose = match self.drafts.pop() {
            Some(compose) => {
                compose.update(cx, |c, cx| c.reopen(window, cx));
                compose
            }
            None => {
                let m = reply.map(|id| &self.inbox.mail[id]);
                let to = m.map_or("", |m| m.address.as_str());
                let subject = m.map_or(String::new(), |m| mail::reply_subject(&m.subject));
                let headers = m.map(|m| mail::reply_headers(&m.message_id, &m.thread_key));
                let from = match self.live.as_ref().and_then(|l| l.account()) {
                    Some(a) => a.email.clone(),
                    None => "Home · you@home.example".into(),
                };
                let compose = cx.new(|cx| Compose::new(&from, to, &subject, headers, window, cx));
                if let Some(live) = &self.live {
                    if let Some(account) = live.account().cloned() {
                        compose.update(cx, |c, cx| c.persist(live.store.clone(), account, cx));
                    }
                }
                self.watch_compose(&compose, window, cx);
                compose
            }
        };
        self.compose = Some(compose);
        cx.notify();
    }

    /// Only a failed local write can hold an exit; drafts and sends need no network wait.
    fn hold_exit(&mut self, cx: &mut Context<Self>) -> bool {
        let mut saved = true;
        for compose in self.drafts.iter().chain(self.compose.iter()) {
            if compose.read(cx).is_modal() || compose.read(cx).keeps_draft() {
                saved &= compose.update(cx, |c, cx| c.save(cx));
            }
        }
        if !saved { self.notify("Could not save draft. Check local storage before closing.", cx); }
        !saved
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
                ComposeEvent::Send(id) => this.send_mail(id.clone(), cx),
            }
            cx.notify();
        })
        .detach();
    }

    fn act(&mut self, what: Move, cx: &mut Context<Self>) {
        self.act_selected(what, LaterWhen::Tomorrow, cx);
    }

    fn act_at(&mut self, ix: usize, what: Move, dx: f32, cx: &mut Context<Self>) {
        self.act_with(ix, what, dx, LaterWhen::Tomorrow, cx);
    }

    /// Optimistic: the row leaves at once, then the worker applies the move on the server.
    /// `when`: the time a Later move brings the message back.
    fn act_with(&mut self, ix: usize, what: Move, dx: f32, when: LaterWhen, cx: &mut Context<Self>) {
        if self.emptying_trash() { return; }
        let Some((id, ix)) = self.inbox.act_at(ix, what) else { return };
        let members = self.inbox.batches.last().cloned().unwrap_or_default();
        if matches!(what, Move::Read(_)) {
            let mut sent = true;
            for member in members { sent &= self.server(what, member, when, cx); }
            self.clear_motion();
            if !sent { return; }
            self.notify(if what == Move::Read(true) { "Marked read" } else { "Marked unread" }, cx);
            if let Some(toast) = &mut self.toast { toast.1 = true; }
            cx.notify();
            return;
        }
        // The list changes height under the highlight; rows paint the selection until it settles.
        self.glide = None;
        for g in &mut self.ghosts {
            if g.ix > ix {
                g.ix -= 1;
            }
        }
        self.entering.retain(|g| g.id != id);
        self.spring = self.spring.filter(|s| s.0 != id);
        self.ghosts.push(Ghost { id, ix, what, at: Instant::now(), dx });
        let mut sent = true;
        for member in members {
            sent &= self.server(what, member, when, cx);
            self.close_popouts(Some(member), cx);
        }
        if !sent { return; }
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
        self.reading_thread = None;
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
        self.confirm_empty_trash = false;
        self.glide = None;
        self.ghosts.clear();
        self.entering.clear();
        self.spring = None;
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.toast.as_ref().is_some_and(|t| t.1) {
            if let Some(id) = self.task_undo.take() {
                if self.save_task(TaskChange::Delete(id), cx) { self.toast = None; }
                return;
            }
        }
        if self.emptying_trash() { return; }
        let open = self.inbox.open;
        let members = self.inbox.batches.last().cloned().unwrap_or_default();
        let Some((what, id, ix)) = self.inbox.undo() else { return };
        let members_len = members.len();
        for member in members { self.unserver(what, member, cx); }
        self.inbox.regroup();
        let (id, ix) = if self.inbox.group { (self.inbox.current().unwrap_or(id), self.inbox.selected) } else { (id, ix) };
        self.toast = None;
        if members_len > 1 || matches!(what, Move::Read(_)) {
            self.clear_motion();
            self.reveal_row(self.inbox.selected);
            cx.notify();
        } else {
            self.come_back(what, id, ix, self.inbox.open != open, cx);
        }
    }

    /// The reverse motion of `what` for a row put back at `ix`, after undo or a failed move.
    fn come_back(&mut self, what: Move, id: usize, ix: usize, list_changed: bool, cx: &mut Context<Self>) {
        self.glide = None;
        // Undo goes back to the list the message left, as the reverse motion needs it on screen.
        if list_changed {
            self.swipe = Swipe::Idle;
            self.clear_motion();
        }
        self.ghosts.retain(|g| g.id != id);
        // Otherwise a finished spring on this row would hide the undo motion.
        self.spring = self.spring.filter(|s| s.0 != id);
        for g in &mut self.ghosts {
            if g.ix > ix {
                g.ix += 1;
            }
        }
        self.entering.push(Ghost { id, ix, what, at: Instant::now(), dx: 0. });
        // The row comes back where it was, which may be scrolled off, or in another list.
        if self.inbox.selected == ix {
            self.reveal_row(ix);
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
            _ => self.spring = Some((id, dx, Instant::now())),
        }
        cx.notify();
    }

    /// Finished ghosts paint nothing; drop them, so the next render and `select` agree on the rows.
    fn prune(&mut self) {
        self.ghosts.retain(|g| g.at.elapsed() < SPRING);
        self.entering.retain(|g| g.at.elapsed() < SPRING);
        self.spring = self.spring.filter(|s| s.2.elapsed() < SPRING);
    }

    fn row_layout(&self, now: Instant) -> RowLayout {
        let mut changes: Vec<_> = self.ghosts.iter().map(|g| (g.ix, ROW_H * row_scale(g.progress(now)))).collect();
        for g in &self.entering {
            // Swipe and spring-back render at full height instead of playing the entry motion.
            if self.swipe.offset(g.id).is_some() || self.spring.is_some_and(|s| s.0 == g.id) {
                continue;
            }
            if let Some(ix) = self.inbox.list().iter().position(|&id| id == g.id) {
                changes.push((ix, ROW_H * (row_scale(1. - g.progress(now)) - 1.)));
            }
        }
        RowLayout { len: self.inbox.list().len(), changes }
    }

    fn reveal_row(&self, ix: usize) {
        let now = Instant::now();
        let y = self.row_layout(now).y(ix)
            + self.ghosts.iter().filter(|g| g.ix == ix).map(|g| ROW_H * row_scale(g.progress(now))).sum::<f32>();
        let top = -f32::from(self.scroll.offset().y);
        let height = f32::from(self.scroll.bounds().size.height);
        // Reserve the full row for Undo, even while it is still expanding.
        let top = if y < top { y } else { top.max(y + ROW_H - height) };
        self.scroll.set_offset(gpui::point(px(0.), px(-top.max(0.))));
    }

    fn select(&mut self, f: impl FnOnce(&mut Inbox), cx: &mut Context<Self>) {
        let before = self.inbox.selected;
        f(&mut self.inbox);
        let ix = self.inbox.selected;
        if ix == before || self.inbox.multi() {
            self.glide = None;
            self.reveal_row(ix);
            self.near_end(cx);
            cx.notify();
            return;
        }
        self.prune();
        // While rows leave or come back, the rows paint the selection; a glide would start late.
        self.glide = (self.ghosts.is_empty() && self.entering.is_empty()).then(|| {
            self.seq += 1;
            Glide::new(self.glide, row_y(before), row_y(ix), Instant::now(), self.seq)
        });
        self.reveal_row(ix);
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
    let scale = row_scale(p);
    let row = row
        .relative()
        .h(px(ROW_H * scale))
        .py(px(ROW_PAD * scale))
        .overflow_hidden()
        // The spring overshoots a little past 0 and 1; position follows it, opacity stops at the ends.
        .opacity((1. - p).clamp(0., 1.));
    // Opaque, so the colour of the move shows only where the row has left (see `with_behind`).
    let row = row.bg(t.bg);
    match what {
        Move::Read(_) => row,
        // A move to a folder leaves like Done.
        Move::Inbox | Move::Done | Move::Folder(_) => row.left(px(dx + p * 200.)),
        // Delete leaves the other way, at the same speed.
        Move::Delete => row.left(px(dx - p * 200.)),
        // Towards Later in the sidebar: lift (shadow), shrink and move left.
        Move::Later => row.left(px(dx - p * 80.)).w(relative(1. - 0.3 * p)).shadow_md(),
    }
}

/// The colour behind a row for the move it commits to, and the ink on it. `None`: short of a threshold.
fn swipe_colors(t: &Theme, what: Option<Move>) -> (Rgba, Rgba) {
    match what {
        None | Some(Move::Read(_)) => (t.sel, t.ink2),
        Some(Move::Inbox | Move::Done | Move::Folder(_)) => (t.swipe_done, t.accent_ink),
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
fn with_behind(row: AnyElement, t: Theme, what: Move, p: f32) -> AnyElement {
    let right = !matches!(what, Move::Later | Move::Delete);
    div()
        .relative()
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(radius::CONTROL))
        .child(behind(t, Some(what), right).opacity(if p > 0. { (1. - p).max(0.) } else { 0. }))
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

fn app_mark() -> gpui::Img {
    // The app icon as the Dock shows it. Its plate fills 824 of 1024 px, so 26 px draws a ~21 px tile.
    // Named as embedded: GPUI would read the bare string "app-icon.png" as a URI host and fetch it.
    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded("app-icon.png".into())))
        .size(px(26.))
        .flex_shrink_0()
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
        .debug_selector(move || label.into())
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
    pub(crate) fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        self.panes.notifications = !self.panes.notifications;
        self.save_panes(cx);
        cx.notify();
    }

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
        let active_icon = if self.window_active { t.sel_icon } else { t.ink2 };
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
        let mut account_rows = div().flex().flex_col().flex_shrink_0().gap(px(2.)).child(heading("Accounts"));
        for (i, (name, address, active)) in accounts.into_iter().enumerate() {
            account_rows = account_rows.child(div().flex().flex_col()
                .child(account(i, name, address, active).on_click(cx.listener(move |this, _, _, cx| this.switch_account(i, cx))))
                .children(self.reauth_control(i, "account-reauth", cx)));
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
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(10.))
                    .h(px(40.))
                    .px(px(8.))
                    .rounded(px(radius::CONTROL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(app_mark())
                    .child(div().flex_1().min_w(px(0.)).one_line().child(current))
                    .child(icon("icons/chevron.svg", t.ink2, 14.)),
            )
            .children(self.live.as_ref().and_then(|live| self.reauth_control(live.current, "sidebar-reauth", cx)))
            .child(
                div()
                    .id("search")
                    .flex()
                    .flex_shrink_0()
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
                    .flex_shrink_0()
                    .child(nav("icons/done.svg", "Today", None, matches!(self.page, Page::Today))
                        .on_click(cx.listener(|this, _, window, cx| this.show_today(window, cx))))
                    .child(
                        nav("icons/inbox.svg", "Inbox", Some(self.inbox.count()), on_mail && self.inbox.open.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.open_box(None, cx))),
                    )
                    .child(
                        nav("icons/later.svg", "Later", Some(self.inbox.later), open_kind(Kind::Later))
                            .on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Later, cx))),
                    )
                    .child(nav("icons/sent.svg", "Sent", None, open_kind(Kind::Sent)).on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Sent, cx))))
                    .child(nav("icons/done.svg", "Done", None, open_kind(Kind::Done)).on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Done, cx))))
                    .child(nav("icons/trash.svg", "Trash", None, open_kind(Kind::Trash)).on_click(cx.listener(|this, _, _, cx| this.open_kind(Kind::Trash, cx)))),
            )
            .child(self.folder_group(heading("Folders"), cx))
            .child(account_rows)
            .child(div().flex_1())
            .child(
                pressable(div().id("settings"), t, in_settings)
                    .flex()
                    .flex_shrink_0()
                    .debug_selector(|| "settings".into())
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

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay_open(cx) { return; }
        let input = self.folder_name.get_or_insert_with(|| {
            let input = cx.new(|cx| TextInput::new(window, cx).placeholder("Folder name").bordered());
            cx.subscribe_in(&input, window, |this, input, event: &TextInputEvent, window, cx| {
                match event {
                    TextInputEvent::Enter => {
                        let name = input.read(cx).text().trim().to_string();
                        if let Err(e) = this.create_folder(&name, cx) {
                            this.notify(e, cx);
                            return;
                        }
                    }
                    TextInputEvent::Escape => {}
                    TextInputEvent::Changed => return,
                }
                this.folder_name = None;
                window.focus(&this.focus);
                cx.notify();
            }).detach();
            input
        });
        window.focus(&gpui::Focusable::focus_handle(input, cx));
        cx.notify();
    }

    /// The "Folders" group: the nav row style, subfolders indented, "N more" past six rows.
    fn folder_group(&self, heading: Div, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let all = &self.inbox.folders;
        let row = |id: ElementId, active: bool| {
            pressable(div().id(id), t, active).flex().flex_shrink_0().items_center().gap(px(8.)).h(px(SIDEBAR_ROW)).px(px(10.)).rounded(px(radius::CONTROL))
        };
        let add = pressable(div().id("add-folder"), t, false).size(px(20.)).flex().items_center().justify_center().rounded(px(radius::KEY))
            .child(icon("icons/plus.svg", t.ink2, 14.))
            .on_click(cx.listener(|this, _, window, cx| this.new_folder(window, cx)));
        #[cfg(test)]
        let add = add.debug_selector(|| "add-folder".into());
        let group = div().flex().flex_col().min_h(px(0.)).gap(px(2.)).child(heading.flex_shrink_0().child(add))
            .children(self.folder_name.as_ref().map(|input| div().flex_shrink_0().px(px(10.)).child(input.clone())));
        if !all.iter().any(Folder::listed) {
            let new = row("new-folder".into(), false).text_color(t.ink2).child(icon("icons/plus.svg", t.ink2, 16.)).child("New folder")
                .on_click(cx.listener(|this, _, window, cx| this.new_folder(window, cx)));
            #[cfg(test)]
            let new = new.debug_selector(|| "new-folder".into());
            return group.when(self.folder_name.is_none(), |group| group.child(new));
        }
        // Once any folder has subfolders, every row keeps a chevron slot, so the icons line up.
        let nested = all.iter().any(|f| f.parent.is_some() && f.listed());
        let rows = folders::tree(all, false);
        let shown = folders::shown(rows.len(), self.all_folders);
        let slot = || div().size(px(12.)).flex_shrink_0();
        let active_icon = if self.window_active { t.sel_icon } else { t.ink2 };
        let mut list = div().id("folder-rows").flex().flex_col().min_h(px(0.)).gap(px(2.))
            .overflow_y_scroll().track_scroll(&self.folder_scroll);
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
            list = list.child(
                row(("folder", f).into(), active)
                    .debug_selector(move || format!("folder-{f}"))
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
            list = list.child(
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
        group.child(list)
    }

    fn message_list(&self, height: f32, cx: &mut Context<Self>) -> Div {
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
        let row = |id: usize, bg: bool, on_sel: bool| {
            let m = &self.inbox.mail[id];
            let members = self.inbox.members(id);
            let unread = members.iter().any(|&id| self.inbox.mail[id].unread);
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
                        .child(div().size(px(7.)).flex_shrink_0().rounded_full().when(unread, |d| d.bg(if on_sel { dot } else { t.accent })))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .one_line()
                                .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(m.from.clone()),
                        )
                        .when(members.len() > 1, |d| d.child(mono(members.len().to_string(), ink2).px(px(5.)).rounded(px(radius::CONTROL)).bg(t.subtle)))
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
        let now = Instant::now();
        let layout = self.row_layout(now);
        let top = (-f32::from(self.scroll.offset().y)).clamp(0., (layout.bottom() + LIST_PAD - height).max(0.));
        self.scroll.set_offset(gpui::point(px(0.), px(-top)));
        let visible = layout.visible(top, height);
        let ghosts_at = |ix: usize| {
            self.ghosts.iter().filter(move |g| g.ix == ix).map(move |g| {
                let (what, dx) = (g.what, g.dx);
                let p = g.progress(now);
                let el = away(row(g.id, false, false), theme, what, p, dx).into_any_element();
                with_behind(el, theme, what, p)
            })
        };
        // The selection highlight glides between rows while nothing else moves the list.
        let gliding = self.glide.filter(|_| !self.inbox.multi() && self.inbox.chosen.is_none() && self.ghosts.is_empty() && self.entering.is_empty());
        // One reading of the clock per frame: the highlight, its copies below and the row colours
        // agree. Each with_animation would start when its element first appears instead.
        let (landed, fill_y) = gliding.map_or((true, 0.), |g| (g.landed(now), g.y(now)));
        let mut rows: Vec<AnyElement> = vec![match gliding {
            // First child, so it paints under the rows.
            Some(g) => {
                let fill = div()
                    .absolute()
                    .top(px(fill_y))
                    .left(px(LIST_PAD))
                    .right(px(LIST_PAD))
                    .h(px(ROW_H))
                    .rounded(px(radius::CONTROL))
                    .bg(sel_bg);
                // This render places the highlight; the animation only asks for the next frame. It
                // repeats until a render sees the glide land, so a late frame cannot end it early.
                match landed {
                    true => fill.into_any_element(),
                    false => fill.with_animation(("glide", g.seq), Animation::new(g.dur).repeat(), |el, _| el).into_any_element(),
                }
            }
            None => div().into_any_element(),
        }];
        let dragging = self.mouse_swiping();
        // White text waits for the gliding fill to arrive; until then its copies below show it.
        let spacer = |height| div().flex_shrink_0().h(px(height)).into_any_element();
        rows.push(spacer(layout.y(visible.start) - LIST_PAD));
        for i in visible.clone() {
            let id = self.inbox.list()[i];
            rows.extend(ghosts_at(i));
            let selected = self.inbox.is_selected(id);
            // A row under the fingers or springing back paints its own fill and gets no copy.
            let moving = self.swipe.offset(id).is_some() || self.spring.is_some_and(|s| s.0 == id);
            let el = row(id, selected && gliding.is_none(), selected && (landed || moving))
                .id(("row", id))
                // Arrow at rest, as in native lists.
                // Every row, not only the dragged one: the topmost hovered row's cursor wins.
                .when(dragging, |d| d.cursor(CursorStyle::ClosedHand))
                // A double-click opens the message in its own window instead of starting a swipe.
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| match e.click_count {
                    2 => {
                        this.select(|inbox| inbox.select(i), cx);
                        this.open_popout(window, cx);
                    }
                    _ => this.press(id, e.position),
                }))
                // Runs before the window's mouse up ends the drag: a dragged row is not a click.
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    if this.swipe.offset(id).is_none() {
                        window.focus(&this.focus);
                        let modifiers = e.modifiers();
                        this.select(|inbox| inbox.click(i, modifiers.shift, modifiers.secondary()), cx)
                    }
                }))
                .on_scroll_wheel(cx.listener(move |this, _, _, _| this.under = Some(id)));
            #[cfg(test)]
            let el = el.debug_selector(move || format!("row-{id}"));
            // Under the fingers the row follows dx; after a short swipe it springs back from it.
            let spring_back = self.spring.filter(|s| s.0 == id).map(|(_, dx, at)| (dx, Some(at)));
            if let Some((dx, at)) = self.swipe.offset(id).map(|dx| (dx, None)).or(spring_back) {
                // Opaque, so the action behind shows only where the row has moved away.
                let el = el.relative().bg(if selected { sel_bg } else { t.bg });
                let p = at.map_or(0., |at| spring(now.saturating_duration_since(at).as_secs_f32() / SPRING.as_secs_f32()));
                let el = el.left(px(dx * (1. - p)));
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
                    let p = 1. - g.progress(now);
                    let el = away(el, theme, what, p, 0.).into_any_element();
                    with_behind(el, theme, what, p)
                }
                // At rest. No hover fill, as in native lists; a press tints, the selection keeps its fill.
                None => {
                    let press = t.press;
                    el.when(!selected, |el| el.active(move |s| s.bg(press))).into_any_element()
                }
            });
        }
        if visible.end == layout.len {
            rows.extend(ghosts_at(layout.len));
        }
        rows.push(spacer(if visible.end == layout.len { 0. } else { layout.bottom() - layout.y(visible.end) }));
        // The eased fill reaches the selected row long before the glide ends, but row text turns
        // white only then, and the row it leaves turns dark at once. Until it lands, copies of the
        // rows it crosses, in selection colours, show through the fill, clipped to each row: text
        // keeps its contrast wherever the fill is. Only rows near the viewport, as above.
        if let Some(g) = gliding.filter(|_| !landed) {
            // The fill starts on one row, or across two after a glide was cut short.
            let from = ((g.from - LIST_PAD) / ROW_H).floor() as isize;
            let sel = self.inbox.selected as isize;
            let (lo, hi) = (from.min(sel).max(visible.start as isize), (from + 1).max(sel).min(visible.end as isize - 1));
            for ix in (lo..=hi).map(|i| i as usize) {
                let Some(&id) = self.inbox.list().get(ix) else { continue };
                // A row under the fingers moves sideways; a copy would stay put over it.
                if self.swipe.offset(id).is_some() || self.spring.is_some_and(|s| s.0 == id) {
                    continue;
                }
                let y = row_y(ix);
                let ink = row(id, false, true)
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y - fill_y));
                let fill = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(px(ROW_H))
                    .rounded(px(radius::CONTROL))
                    .overflow_hidden()
                    .bg(sel_bg)
                    .top(px(fill_y - y))
                    .child(ink);
                rows.push(
                    div()
                        .absolute()
                        .top(px(y))
                        .left(px(LIST_PAD))
                        .right(px(LIST_PAD))
                        .h(px(ROW_H))
                        .overflow_hidden()
                        .child(fill)
                        .into_any_element(),
                );
            }
        }

        let filter = div()
            .flex().p(px(2.)).rounded(px(radius::CONTROL)).bg(t.subtle)
            .border_1().border_color(t.line).text_size(px(size::SMALL))
            .children([("All", false, "filter-all"), ("Unread", true, "filter-unread")].into_iter().map(|(label, unread, id)| {
                let control = pressable(div().id(id), t, false)
                    .h(px(22.)).px(px(10.)).flex().items_center().rounded(px(6.))
                    .when(self.inbox.unread_only == unread, |d| d.bg(t.raised))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.filter_unread(unread, cx)));
                #[cfg(test)]
                let control = control.debug_selector(move || id.into());
                control
            }));

        // Title and subtitle as in a macOS toolbar: the count, and a quiet sync status.
        let left = self.inbox.shown_count();
        let subtitle = if self.confirm_empty_trash && self.inbox.open_kind() == Some(Kind::Trash) {
            "Delete all permanently?".to_string()
        } else {
            match self.sync_state() {
                Some(sync) => format!("{left} left · {sync}"),
                None => format!("{left} left"),
            }
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
                    .h(px(self.list_header_height()))
                    .when(self.confirm_empty_trash, |d| d.flex_col().items_start().py(px(8.)))
                    .flex_shrink_0()
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
                    .child(if self.inbox.open_kind() == Some(Kind::Trash) {
                        let controls = div().flex().gap(px(8.));
                        if self.emptying_trash() {
                            controls.child(mono("Emptying…", t.ink2))
                        } else if !self.can_empty_trash() {
                            controls.child(mono("Updating…", t.ink2))
                        } else if self.confirm_empty_trash {
                            controls
                                .child(button(t, "Delete all", Some(t.danger)).on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))))
                                .child(button(t, "Cancel", None).on_click(cx.listener(|this, _, _, cx| { this.confirm_empty_trash = false; cx.notify(); })))
                        } else {
                            controls.child(button(t, "Empty Trash", None).on_click(cx.listener(|this, _, _, cx| { this.confirm_empty_trash = this.can_empty_trash(); cx.notify(); })))
                        }
                    } else { filter }),
            )
            .child(
                {
                    let mut list = div()
                        .id("rows")
                        .h(px(height))
                        .flex_shrink_0()
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

    fn list_header_height(&self) -> f32 {
        HEADER_H + if self.confirm_empty_trash && self.inbox.open_kind() == Some(Kind::Trash) { 32. } else { 0. }
    }

    /// The list on screen, for its header and the window title.
    fn list_title(&self) -> SharedString {
        if self.inbox.open_kind() == Some(Kind::Trash) { return "Trash".into(); }
        self.inbox.open.map_or("Inbox".into(), |f| self.inbox.folders[f].name.clone().into())
    }

    pub(super) fn toggle_conversations(&mut self, cx: &mut Context<Self>) {
        self.panes.group_conversations = !self.panes.group_conversations;
        self.inbox.group = self.panes.group_conversations;
        self.inbox.regroup();
        self.clear_motion();
        self.reading_thread = None;
        self.save_panes(cx);
        self.reload(cx);
        cx.notify();
    }

    fn reading_pane(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        if self.inbox.multi() { return self.selection_pane(cx); }
        let Some(id) = self.inbox.single() else {
            let more = self.has_more();
            return div().flex_1().flex().flex_col().gap(px(12.)).items_center().justify_center().text_color(t.ink2)
                .when(self.inbox.unread_only, |d| d
                    .child(if more { "No unread messages in loaded mail" } else { "No unread messages" })
                    .when(more, |d| d.child(button(t, "Load more", None)
                        .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))))));
        };
        let m = &self.inbox.mail[id];
        let trash = self.inbox.open_kind() == Some(Kind::Trash);
        let this = cx.entity().downgrade();
        let on_download = move |section, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.download_attachment(id, section, cx)).ok();
        };
        // HTML mail: the web view covers only the body. The text shows while an overlay hides it.
        let web = html::SUPPORTED && m.html.is_some();
        let text = !web || self.palette.is_some() || self.compose.is_some();
        // Toasts sit just above the reply field; the web view would cover them.
        let toasts = self.toast.is_some() as usize + self.sending.iter().filter(|c| c.read(cx).undoable()).count()
            + self.live.as_ref().map_or(0, |live| live.failed.len());
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .child(
                toolbar(t)
                    .flex_wrap().h_auto().min_h(px(HEADER_H)).py(px(8.))
                    .px(px(16.))
                    .child(button(t, "Move", None).on_click(cx.listener(|this, _, window, cx| this.open_palette("move ", window, cx))))
                    .child(button(t, if self.inbox.selection_unread() { "Mark read" } else { "Mark unread" }, None).on_click(cx.listener(|this, _, _, cx| this.toggle_read(cx))))
                    .child(button(t, "Reply", None).on_click(cx.listener(|this, _, window, cx| this.open_compose(true, window, cx))))
                    .when(!trash, |d| d.child(
                        button(t, "Later", None).on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx))),
                    ))
                    .when(web && self.web.has_remote && !m.remote, |d| {
                        d.child(button(t, "Load remote images", None).on_click(cx.listener(move |this, _, _, cx| {
                            this.inbox.mail[id].remote = true;
                            cx.notify();
                        })))
                    })
                    // shortcut: no Forward button until forwarding exists.
                    .child(div().flex_1())
                    .child(mono(format!("{} / {}", self.inbox.list().iter().position(|&row| row == id).unwrap_or(0) + 1, self.inbox.shown_count()), t.ink2))
                    .child(
                        icon_button(t, "pop-out", "icons/pop-out.svg", "Open in New Window")
                            .on_click(cx.listener(|this, _, window, cx| this.open_popout(window, cx))),
                    )
                    .child(
                        // The step that finishes a message sits last, as a primary button does in macOS.
                        button(t, if trash { "Move to Inbox" } else { "Done" }, Some(t.accent))
                            .on_click(cx.listener(move |this, _, _, cx| this.act(if trash { Move::Inbox } else { Move::Done }, cx))),
                    ),
            )
            .when(self.inbox.group && self.reading_members.len() > 1, |d| d.child(self.conversation(cx)))
            .when(!self.inbox.group || self.reading_members.len() <= 1, |d| d.child(reading_view(t, (id, m), text, self.web.slot(), toasts, on_download, cx.listener(|this, _, window, cx| this.open_compose(true, window, cx)))))
    }
}

/// The bar above a message: buttons on a macOS toolbar's height.
fn toolbar(t: &Theme) -> Div {
    div().flex().items_center().gap(px(8.)).h(px(HEADER_H)).flex_shrink_0().border_b_1().border_color(t.line)
}

/// A borderless glyph button with a tooltip, as in a macOS toolbar.
fn icon_button(t: &Theme, id: &'static str, path: &'static str, tip: &'static str) -> Stateful<Div> {
    let theme = *t;
    pressable(div().id(id), t, false)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(radius::CONTROL))
        .child(icon(path, t.ink2, 16.))
        .tooltip(move |_, cx| cx.new(|_| Tip(theme, tip)).into())
}

struct Tip(Theme, &'static str);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.0;
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(t.line)
            .bg(t.raised)
            .text_color(t.ink)
            .text_size(px(size::SMALL))
            .font_family(font::ui())
            .shadow_md()
            .child(self.1)
    }
}

fn attachment_size(bytes: u32) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// One message below its toolbar: the trusted header in GPUI, the body as text (`text`) or the
/// web view's `slot`, the attachment and the reply field. The reading pane and message windows
/// both show this. `toasts`: room the web view leaves for toasts above the reply field.
fn reading_view(
    t: &Theme,
    (id, m): (usize, &Mail),
    text: bool,
    slot: impl IntoElement,
    toasts: usize,
    on_download: impl Fn(String, &mut Window, &mut App) + 'static,
    on_reply: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let web = html::SUPPORTED && m.html.is_some();
    let initials: String = m.from.split_whitespace().take(2).filter_map(|w| w.chars().next()).collect();
    let to = if m.to.is_empty() { "you".to_string() } else { m.to.clone() };
    let on_download = std::rc::Rc::new(on_download);
    let attachment = || m.attachments.iter().enumerate().map(|(index, part)| {
        let on_download = on_download.clone();
        let section = part.section.clone();
        pressable(div().id(("attachment", index)), t, false)
            .flex().items_center().gap(px(12.)).min_w(px(0.)).flex_shrink_0()
            .px(px(14.)).py(px(10.)).rounded(px(radius::ATTACHMENT))
            .border_1().border_color(t.line).bg(t.subtle)
            .text_size(px(size::SNIPPET)).line_height(relative(LEADING))
            .child(icon("icons/folder.svg", t.ink2, 16.))
            .child(div().flex_1().min_w(px(0.)).truncate().font_weight(FontWeight::MEDIUM).child(part.filename.clone()))
            .child(mono(attachment_size(part.size), t.ink2))
            .when(m.store.is_some(), |d| d.cursor_pointer().on_click(move |_, window, cx| on_download(section.clone(), window, cx)))
    }).collect::<Vec<_>>();
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
                // The web view's place: the rest of the pane, less room for toasts.
                .when(web, |d| {
                    d.flex_1().min_h(px(0.)).relative().child(
                        div().absolute().top_0().left_0().right_0().bottom(px(48. * toasts as f32)).child(slot),
                    )
                })
                .when(!web, |d| d.children(attachment())),
        )
        .when(web, |d| d.children(attachment()))
        .when(!web, |d| d.child(div().flex_1()))
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
                .on_click(on_reply)
                .child(format!("Reply to {}", m.from)),
        )
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        // Reveal the final height before pruning, even if a late frame skipped the expansion.
        if self.entering.iter().any(|g| Some(g.id) == self.inbox.current()) {
            self.reveal_row(self.inbox.selected);
        }
        self.prune();
        // Off-screen motion must still advance and finish, even without an animated element.
        if !self.ghosts.is_empty() || !self.entering.is_empty() || self.spring.is_some() {
            window.request_animation_frame();
        }
        if matches!(self.page, Page::Mail) { self.open_current(window.is_window_active(), cx); }
        self.window_active = window.is_window_active();
        self.list_focused = self.window_active && self.focus.is_focused(window);
        // A definite pixel width keeps ellipses intact during row motion and live resizing.
        (self.sidebar_w, self.list_w) = self.panes.fit(f32::from(window.viewport_size().width));
        self.web_frame(window, cx);
        // First launch: the add-account flow fills the window; there is nothing for a sidebar yet.
        let welcome = matches!(self.page, Page::AddAccount(_)) && self.live.as_ref().is_some_and(|l| l.accounts.is_empty());
        let panel_top = if welcome || (self.panes.sidebar_hidden && cfg!(target_os = "macos")) { 36. } else { 8. };
        let list_height = (f32::from(window.viewport_size().height) - panel_top - 8. - 2. - self.list_header_height()).max(0.);
        let title = match &self.page {
            Page::Mail => self.list_title(),
            Page::Today => "Today".into(),
            Page::Settings(_) => "Settings".into(),
            Page::AddAccount(_) => "Add Account".into(),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let empty = matches!(self.page, Page::Mail) && self.inbox.list().is_empty() && self.inbox.open.is_none() && !self.inbox.unread_only;
        let reading = matches!(self.page, Page::Mail) && !empty && self.inbox.current().is_some();
        let panel = match &self.page {
            Page::Today => self.tasks_view(cx),
            Page::AddAccount(add) => div().flex_1().min_w(px(0.)).child(add.clone()),
            Page::Settings(confirm) => {
                let demo = mail::demo_accounts();
                crate::settings::view(&t, self.live.as_ref().map_or(&demo[..], |l| &l.accounts), confirm.as_deref(), self.panes.group_conversations, self.panes.notifications, &self.today.lists, self.today.default, cx)
            }
            // Undo from the empty screen brings the list back. An empty folder keeps its header.
            Page::Mail if empty => crate::empty_inbox::view(&t, self.sync_line().unwrap_or_else(|| "Home · synced 10:12".into())),
            Page::Mail => div().flex_1().min_w(px(0.)).flex().child(self.message_list(list_height, cx)).child(self.reading_pane(cx)),
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
            .key_context(if matches!(self.page, Page::Today) { "MainWindow Today" } else { "MainWindow" })
            .map(|d| Self::task_keys(d, cx))
            // Mouse swipe: the root sees every move inside the window; `on_mouse_up_out` catches a
            // release outside it. shortcut: moves outside the window are not followed (the row waits
            // at the edge); a window-level listener via `window.on_mouse_event` would follow them.
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag_to(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.drop_drag(cx)))
            .when(self.mouse_swiping(), |d| d.cursor(CursorStyle::ClosedHand))
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.web.unfocus()))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewTask, window, cx| if this.palette.is_none() && !this.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) && matches!(this.page, Page::Mail | Page::Today) { this.new_task(window, cx); }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| if !this.overlay_open(cx) { this.select(Inbox::next, cx) }))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| if !this.overlay_open(cx) { this.select(Inbox::prev, cx) }))
            .on_action(cx.listener(|this, _: &ExtendNext, _, cx| if !this.overlay_open(cx) { this.select(|inbox| inbox.extend(1), cx) }))
            .on_action(cx.listener(|this, _: &ExtendPrev, _, cx| if !this.overlay_open(cx) { this.select(|inbox| inbox.extend(-1), cx) }))
            .on_action(cx.listener(|this, _: &crate::text_input::SelectAll, _, cx| if !this.overlay_open(cx) { this.select(Inbox::select_all, cx) }))
            .on_action(cx.listener(|this, _: &ToggleRead, _, cx| if !this.overlay_open(cx) { this.toggle_read(cx) }))
            .on_action(cx.listener(|this, _: &Done, _, cx| if !this.overlay_open(cx) { this.act(Move::Done, cx) }))
            .on_action(cx.listener(|this, _: &Later, _, cx| if !this.overlay_open(cx) { this.act(Move::Later, cx) }))
            // The guard hands ⌫ to the palette, where it edits the query.
            .on_action(cx.listener(|this, _: &DeleteMessage, _, cx| if !this.overlay_open(cx) { this.act(Move::Delete, cx) }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                if this.palette.is_some() || this.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) || matches!(this.page, Page::AddAccount(_) | Page::Settings(_)) {
                    cx.propagate();
                    return;
                }
                // The newest send in its Undo window goes first: its toast is the one on top.
                let sent = this.sending.iter().rev().find(|c| c.read(cx).undoable()).cloned();
                if !sent.is_some_and(|c| c.update(cx, |c, cx| c.undo(window, cx))) {
                    if matches!(this.page, Page::Today) && !this.toast.as_ref().is_some_and(|t| t.1) { return; }
                    this.undo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                if this.palette.is_none() && matches!(this.page, Page::Mail | Page::Today) { this.open_palette("", window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Reply, window, cx| if !this.overlay_open(cx) { this.open_compose(true, window, cx) }))
            .on_action(cx.listener(|this, _: &NewMessage, window, cx| if !this.overlay_open(cx) { this.open_compose(false, window, cx) }))
            .on_action(cx.listener(|this, _: &OpenInWindow, window, cx| if !this.overlay_open(cx) { this.open_popout(window, cx) }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| if this.palette.is_none() && this.compose.is_none() { this.open_settings(cx) }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.finish_resize(cx);
                this.panes.sidebar_hidden = !this.panes.sidebar_hidden;
                this.save_panes(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| if !this.overlay_open(cx) {
                this.select(|inbox| inbox.select(inbox.selected), cx);
                this.confirm_empty_trash = false;
                this.finish_resize(cx);
                this.release(false, cx);
                cx.notify();
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
                    .mt(px(panel_top))
                    .mb(px(8.))
                    .mr(px(8.))
                    .when(welcome, |d| d.ml(px(8.)))
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
        if let Some(live) = &self.live {
            for (i, pending) in live.failed.iter().enumerate() {
                let id = pending.id.clone();
                let edit_id = id.clone();
                let status = match pending.state.as_str() {
                    "uncertain" => "Delivery uncertain — sending again may duplicate delivery",
                    "failed" => "Not sent — automatic retry stopped",
                    _ => "Not sent — retry scheduled",
                };
                let actions = div().flex().flex_shrink_0().gap(px(8.))
                    .child(button(&t, "Send again", None).on_click(cx.listener(move |this, _, _, cx| this.retry_failed(&id, cx))))
                    .child(button(&t, "Edit draft", None).on_click(cx.listener(move |this, _, window, cx| this.edit_failed(&edit_id, window, cx))));
                out.push(scoped(self.sending.len() + i, crate::compose::toast(&t,
                    format!("{}: {}", status, if pending.mail.subject.is_empty() { pending.mail.to.join(", ") } else { pending.mail.subject.clone() }), Some(actions.into_any_element())).into_any_element()));
            }
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
    Inbox,
    Done,
    Later,
    /// Into the folder at this index in `Inbox::folders`.
    Folder(usize),
    /// To Trash.
    Delete,
    Read(bool),
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

/// ⌘Q and ⌘W flush local edits. Menu and key actions reach
/// the window before the app's global handlers, which run when this one propagates.
fn exit_actions<E: InteractiveElement>(el: E, cx: &mut Context<MainWindow>) -> E {
    #[cfg(target_os = "macos")]
    let el = el
        .on_action(cx.listener(|this, _: &crate::menus::Quit, _, cx| if !this.hold_exit(cx) { cx.propagate() }))
        .on_action(cx.listener(|this, _: &crate::menus::CloseWindow, _, cx| if !this.hold_exit(cx) { cx.propagate() }));
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
    el
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
    chosen: Option<HashSet<usize>>,
    anchor: Option<usize>,
    unread_only: bool,
    // Explicitly unread messages stay unread until the user selects again.
    keep_unread: HashSet<usize>,
    later: usize,
    /// Deleted message ids, newest first.
    trash: Vec<usize>,
    // (what, message id, list it left, index it left from, worker operation).
    history: Vec<(Move, usize, Option<usize>, usize, Option<u64>)>,
    batches: Vec<Vec<usize>>,
    group: bool,
    rows: Vec<usize>,
    members: HashMap<usize, Vec<usize>>,
}

impl Inbox {
    fn new(len: usize, later: usize) -> Self {
        Self { mail: mail::demo(), by_store: HashMap::new(), items: (0..len).collect(), extra: 0, folders: Vec::new(), open: None, selected: 0, chosen: None, anchor: None, unread_only: false, keep_unread: HashSet::new(), later, trash: Vec::new(), history: Vec::new(), batches: Vec::new(), group: false, rows: Vec::new(), members: HashMap::new() }
    }

    /// The sample inbox and folders, with their unread counts.
    fn demo(len: usize) -> Self {
        let mut inbox = Self { folders: folders::sample(), ..Self::new(len, LATER_SAMPLE) };
        inbox.folders.push(Folder { kind: Kind::Trash, ..Folder::new("Trash", None, &[]) });
        for f in 0..inbox.folders.len() {
            inbox.folders[f].unread = inbox.folders[f].items.iter().filter(|&&id| inbox.mail[id].unread).count();
        }
        inbox
    }

    /// Empty until the first store read. Later is a local list, not a server folder.
    fn live() -> Self {
        let later = Folder { kind: Kind::Later, ..Folder::new("Later", None, &[]) };
        Self { mail: Vec::new(), folders: vec![later, Folder { kind: Kind::Trash, ..Folder::new("Trash", None, &[]) }], ..Self::new(0, 0) }
    }

    fn list(&self) -> &Vec<usize> {
        if self.group || self.unread_only { &self.rows } else { self.raw_list() }
    }

    fn members(&self, id: usize) -> Vec<usize> {
        if self.group { self.members.get(&id).cloned().unwrap_or_else(|| vec![id]) } else { vec![id] }
    }

    fn regroup(&mut self) {
        let keep = self.current();
        let chosen = self.chosen.as_ref().map(|ids| ids.iter().flat_map(|&id| self.members(id)).collect::<HashSet<_>>());
        let mut groups: HashMap<(String, mail::Place, String), usize> = HashMap::new();
        self.members.clear();
        let mut rows = Vec::new();
        for id in self.raw_list().clone() {
            let m = &self.mail[id];
            let key = (m.account.clone(), m.folder.clone(), if m.thread_id.is_empty() { format!("message:{id}") } else { m.thread_id.clone() });
            let rep = *groups.entry(key).or_insert_with(|| { rows.push(id); id });
            self.members.entry(rep).or_default().push(id);
        }
        for rep in &mut rows {
            let mut members = self.members.remove(rep).unwrap();
            members.sort_by_key(|&id| (self.mail[id].date, self.mail[id].store.unwrap_or(id as i64)));
            let latest = *members.last().unwrap();
            self.members.insert(latest, members);
            *rep = latest;
        }
        rows.sort_by_key(|&id| std::cmp::Reverse((self.mail[id].date, self.mail[id].store.unwrap_or(0))));
        if !self.group { rows = self.raw_list().clone(); }
        if self.unread_only {
            rows.retain(|&id| self.members(id).iter().any(|&id| self.mail[id].unread));
        }
        self.rows = rows;
        if self.group || self.unread_only {
            self.selected = keep.and_then(|id| self.rows.iter().position(|&rep| self.members(rep).contains(&id)))
                .unwrap_or(self.selected).min(self.rows.len().saturating_sub(1));
        }
        if let Some(chosen) = chosen {
            self.chosen = Some(self.list().iter().copied().filter(|&rep| self.members(rep).iter().any(|id| chosen.contains(id))).collect());
        }
    }

    fn raw_list(&self) -> &Vec<usize> {
        if self.open_kind() == Some(Kind::Trash) { return &self.trash; }
        self.open.map_or(&self.items, |f| &self.folders[f].items)
    }

    fn list_mut(&mut self) -> &mut Vec<usize> {
        if self.open_kind() == Some(Kind::Trash) { return &mut self.trash; }
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
        if self.group || self.unread_only { return self.rows.len(); }
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
        if self.open_kind() == Some(Kind::Trash) { self.unread_only = false; }
        self.chosen = None;
        self.anchor = None;
        self.keep_unread.clear();
        self.selected = 0;
        self.regroup();
        self.selected = 0;
    }

    fn create_folder(&mut self, name: &str) -> usize {
        self.folders.push(Folder::new(name, None, &[]));
        self.folders.len() - 1
    }

    fn remove_pending_folder(&mut self, name: &str) {
        if let Some(f) = self.folders.iter().position(|f| f.store.is_none() && !f.gone && f.full == name) {
            self.folders[f].gone = true;
            if self.open == Some(f) { self.open(None); }
        }
    }

    fn unread(&self, f: usize) -> usize {
        self.folders[f].unread
    }

    fn current(&self) -> Option<usize> {
        self.list().get(self.selected).copied()
    }

    fn select(&mut self, ix: usize) {
        self.selected = ix.min(self.list().len().saturating_sub(1));
        self.chosen = None;
        self.anchor = self.current();
        self.keep_unread.clear();
    }

    fn next(&mut self) {
        self.select(self.selected + 1);
    }

    fn prev(&mut self) {
        self.select(self.selected.saturating_sub(1));
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
            Move::Inbox | Move::Done | Move::Delete | Move::Read(_) => {}
        }
    }

    /// Removes the message at `ix`. If it was selected, the one below it (above it at the end)
    /// becomes selected. A swipe can hit any row, so otherwise the selection stays on its message.
    fn act_at(&mut self, ix: usize, what: Move) -> Option<(usize, usize)> {
        let id = *self.list().get(ix)?;
        self.act_rows(&[id], what)?;
        Some((id, ix))
    }

    fn act_one(&mut self, ix: usize, what: Move) -> Option<(usize, usize)> {
        if what == Move::Inbox && self.open.is_none() { return None; }
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
            Move::Inbox | Move::Folder(_) | Move::Read(_) => None,
        };
        if here.is_some() && self.open_kind() == here {
            return None;
        }
        let id = *self.raw_list().get(ix)?;
        let unread = self.mail[id].unread;
        if let Move::Read(seen) = what {
            if unread == seen {
                self.set_unread(id, !seen);
                self.keep_unread.insert(id);
                self.history.push((what, id, self.open, ix, None));
                return Some((id, ix));
            }
            return None;
        }
        self.list_mut().remove(ix);
        if ix < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.raw_list().len().saturating_sub(1));
        self.recount(id, self.open, what, false);
        match what {
            Move::Inbox => self.items.insert(0, id),
            Move::Folder(f) => self.folders[f].items.insert(0, id),
            Move::Delete => self.trash.insert(0, id),
            Move::Later | Move::Done | Move::Read(_) => {}
        }
        self.history.push((what, id, self.open, ix, None));
        Some((id, ix))
    }

    /// Puts the last moved message back where it was, opens that list and selects the message.
    fn undo(&mut self) -> Option<(Move, usize, usize)> {
        let batch = self.batches.pop()?;
        let mut result = None;
        self.chosen = None;
        for id in batch.iter().copied().rev() {
            if let Some(at) = self.history.iter().rposition(|h| h.1 == id) {
                result = self.restore(at, true);
            }
        }
        self.regroup();
        self.chosen = Some(self.list().iter().copied().filter(|&rep| self.members(rep).iter().any(|id| batch.contains(id))).collect());
        result.map(|(what, id, ix)| {
            let row = self.list().iter().position(|&rep| self.members(rep).contains(&id)).unwrap_or(ix).min(self.list().len().saturating_sub(1));
            self.selected = row;
            (what, self.list().get(row).copied().unwrap_or(id), row)
        })
    }

    /// A move of `id` failed on the server: take it back. Returns where the row reappears, if its
    /// list is on screen; otherwise the next store read shows it there.
    fn revert(&mut self, id: usize, op: Option<u64>) -> Option<(Move, usize, usize)> {
        let at = self.history.iter().rposition(|h| h.1 == id && (op.is_none() || h.4 == op))?;
        // The same message can belong to several undo batches.
        let newer = self.history[at + 1..].iter().filter(|h| h.1 == id).count();
        if let Some(batch) = self.batches.iter_mut().rev().filter(|batch| batch.contains(&id)).nth(newer) {
            batch.retain(|&member| member != id);
        }
        self.batches.retain(|batch| !batch.is_empty());
        let count = self.list().len();
        let keep = self.current();
        let result = self.restore(at, false);
        self.regroup();
        if let Some(ix) = keep.and_then(|id| self.list().iter().position(|&rep| self.members(rep).contains(&id))) { self.selected = ix; }
        if self.group && self.list().len() == count { return None; }
        result.map(|(what, id, ix)| {
            let row = self.list().iter().position(|&rep| self.members(rep).contains(&id)).unwrap_or(ix);
            (what, self.list().get(row).copied().unwrap_or(id), row)
        })
    }

    /// `switch`: open the list the message left (undo). Otherwise the selection stays on its message.
    fn restore(&mut self, at: usize, switch: bool) -> Option<(Move, usize, usize)> {
        let (what, id, from, ix, _) = self.history.remove(at);
        if let Move::Read(seen) = what {
            self.set_unread(id, seen);
            self.keep_unread.insert(id);
            return Some((what, id, ix));
        }
        self.recount(id, from, what, true);
        match what {
            Move::Inbox => self.items.retain(|&i| i != id),
            Move::Folder(f) => self.folders[f].items.retain(|&i| i != id),
            Move::Delete => self.trash.retain(|&i| i != id),
            Move::Later | Move::Done | Move::Read(_) => {}
        }
        if !switch && self.open != from {
            return None;
        }
        self.open = from;
        let was_empty = self.raw_list().is_empty();
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
        m.was_snoozed.push(m.snoozed_until);
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
        if let Some(until) = m.was_snoozed.pop() { m.snoozed_until = until; }
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
                fresh.attachments = std::mem::take(&mut self.mail[id].attachments);
                if m.has_attachments && fresh.attachments.is_empty() {
                    fresh.body.clear();
                }
                fresh.was = std::mem::take(&mut self.mail[id].was);
                fresh.was_snoozed = std::mem::take(&mut self.mail[id].was_snoozed);
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
                *self.list_mut() = snap.open.iter().map(|m| self.intern(m, seen, now)).collect();
            }
        }
        self.regroup();
        let found = keep.and_then(|s| self.list().iter().position(|&id| self.members(id).iter().any(|&member| self.mail[member].store == Some(s))));
        self.selected = found.unwrap_or(self.selected).min(self.list().len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_actions_and_undo_stay_in_the_folder() {
        for what in [Move::Done, Move::Later, Move::Delete, Move::Folder(0)] {
            let mut inbox = Inbox::demo(4);
            inbox.group = true;
            for id in [0, 1, 7] {
                inbox.mail[id].thread_id = "conversation".into();
                inbox.mail[id].date = id as i64;
            }
            inbox.regroup();
            assert_eq!(inbox.list().len(), 3);
            assert_eq!(inbox.members(1), [0, 1]);
            inbox.selected = 1;
            let keep = inbox.current();
            inbox.act_at(0, what).unwrap();
            assert!(!inbox.items.contains(&0) && !inbox.items.contains(&1));
            assert!(inbox.folders[0].items.contains(&7));
            assert_eq!(inbox.current(), keep);
            assert_eq!(inbox.batches.last().unwrap(), &[0, 1]);
            let (_, id, _) = inbox.undo().unwrap();
            assert_eq!(id, 1);
            assert_eq!(inbox.members(id), [0, 1]);
            assert!(inbox.history.is_empty());
            assert_eq!(inbox.list().len(), 3);
            inbox.group = false;
            assert_eq!(inbox.list().len(), 4);
        }
    }

    #[test]
    fn failed_thread_member_does_not_undo_successful_members_twice() {
        let mut inbox = Inbox::demo(3);
        inbox.group = true;
        inbox.mail[0].thread_id = "thread".into();
        inbox.mail[1].thread_id = "thread".into();
        inbox.regroup();
        inbox.act_at(0, Move::Delete).unwrap();
        inbox.revert(0, None).unwrap();
        assert_eq!(inbox.batches, [vec![1]]);
        inbox.undo().unwrap();
        assert_eq!(inbox.items.iter().filter(|&&id| id == 0).count(), 1);
        assert_eq!(inbox.items.iter().filter(|&&id| id == 1).count(), 1);
        assert!(inbox.trash.is_empty());
        assert_eq!(inbox.list().len(), 2);
    }

    #[test]
    fn virtual_ranges_include_partial_rows_and_trailing_ghosts() {
        let empty = RowLayout { len: 0, changes: Vec::new() };
        assert_eq!(empty.visible(0., 600.), 0..0);
        assert_eq!(empty.bottom(), LIST_PAD);
        let layout = RowLayout { len: 10_000, changes: Vec::new() };
        assert_eq!(layout.visible(row_y(5000) + ROW_H / 2., 5. * ROW_H), 4998..5008);
        assert_eq!(layout.visible(row_y(9999), 5. * ROW_H), 9997..10_000);
        let moving = RowLayout { len: 10, changes: vec![(0, -ROW_H), (1, ROW_H / 2.), (10, ROW_H / 4.)] };
        assert_eq!(moving.y(1), LIST_PAD);
        assert!((moving.y(2) - LIST_PAD - 1.5 * ROW_H).abs() < 0.001);
        assert!((moving.bottom() - LIST_PAD - 9.75 * ROW_H).abs() < 0.001);
        assert_eq!(moving.index_at(moving.y(10) + ROW_H / 8.), 10);
    }

    #[test]
    fn held_arrow_key_highlight_keeps_up() {
        let t0 = Instant::now();
        let single = Glide::new(None, row_y(0), row_y(1), t0, 0);
        assert_eq!(single.dur, GLIDE);
        // macOS repeats a held key after a delay (225 ms at the shortest), then up to 30 times a
        // second. Each repeat starts from where the highlight is.
        let (delay, step) = (Duration::from_millis(225), Duration::from_millis(33));
        let mut glide = single;
        for n in 1..=20 {
            let now = t0 + delay + step * (n - 1);
            let lag = (row_y(n as usize) - glide.y(now)) / ROW_H;
            assert!(lag < 0.5, "press {n}: highlight {lag:.2} rows behind");
            glide = Glide::new(Some(glide), row_y(n as usize), row_y(n as usize + 1), now, n as usize);
            // The first glide lands before the key starts repeating, so the first repeat takes the full curve.
            assert_eq!(glide.dur, if n == 1 { GLIDE } else { GLIDE_REPEAT });
        }
        // Once it lands, the next single press takes the full curve again.
        let later = t0 + delay + step * 20 + GLIDE;
        assert_eq!(Glide::new(Some(glide), row_y(21), row_y(22), later, 21).dur, GLIDE);
    }

    #[test]
    fn attachment_metadata_survives_reload_and_missing_metadata_reloads_body() {
        let mut inbox = Inbox::live();
        let summary = lagom_core::store::MessageSummary { id: 1, has_attachments: true, ..Default::default() };
        let seen = HashSet::new();
        let now = chrono::Local::now();
        let id = inbox.intern(&summary, &seen, now);
        inbox.mail[id].body = vec!["body".into()];
        inbox.intern(&summary, &seen, now);
        assert!(inbox.mail[id].body.is_empty());
        inbox.mail[id].attachments = vec![lagom_core::attachment::Attachment { section: "2".into(), filename: "file.pdf".into(), ..Default::default() }];
        inbox.mail[id].body = vec!["body".into()];
        inbox.intern(&summary, &seen, now);
        assert_eq!(inbox.mail[id].body, ["body"]);
        assert_eq!(inbox.mail[id].attachments[0].filename, "file.pdf");
    }

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
    fn trash_can_restore_to_inbox_and_undo() {
        let mut inbox = Inbox::demo(3);
        inbox.act_at(1, Move::Delete).unwrap();
        inbox.open(inbox.find_kind(Kind::Trash));
        assert_eq!(inbox.list(), &[1]);
        assert_eq!(inbox.act_at(0, Move::Delete), None);
        assert_eq!(inbox.act_at(0, Move::Inbox), Some((1, 0)));
        assert!(inbox.list().is_empty());
        assert_eq!(inbox.items, [1, 0, 2]);
        assert_eq!(inbox.undo(), Some((Move::Inbox, 1, 0)));
        assert_eq!(inbox.list(), &[1]);
        assert_eq!(inbox.items, [0, 2]);
        inbox.open(None);
        assert_eq!(inbox.act_at(0, Move::Inbox), None);
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
        assert_eq!(counts, [3, 0, 1, 0, 0, 0, 0, 1, 2, 0]);
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
        assert_eq!(inbox.revert(1, None), Some((Move::Done, 1, 1)));
        assert_eq!((inbox.items.as_slice(), inbox.current()), ([0, 1, 2, 4, 5, 6].as_slice(), Some(4)));
        // The second fails while another list is open: no row now, but the counts go back.
        let before = inbox.unread(1);
        inbox.open(Some(0));
        assert_eq!(inbox.revert(3, None), None);
        assert_eq!(inbox.unread(1), before - 1);
        assert!(!inbox.folders[1].items.contains(&3));
        // Nothing left to revert or undo.
        assert_eq!(inbox.revert(3, None), None);
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

    pub(super) fn open(cx: &mut TestAppContext) -> (Entity<MainWindow>, &mut VisualTestContext) {
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
    fn conversation_reading_and_setting_keep_thread_rows_virtual(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        view.update(cx, |v, cx| {
            for (id, mail) in v.inbox.mail.iter_mut().enumerate() {
                mail.thread_id = format!("thread-{}", id / 2);
                mail.date = -(id as i64);
                mail.unread = id == 1;
            }
            v.inbox.regroup();
            v.reading_thread = None;
            cx.notify();
        });
        frame(cx);
        view.read_with(cx, |v, _| {
            assert_eq!(v.inbox.list().len(), 5_000);
            assert_eq!(v.reading_members, [1, 0]);
            assert!(v.expanded.contains_key(&1));
        });
        assert!(children(&view, cx) < 15);
        view.update(cx, |v, cx| {
            v.toggle_message(0, true, cx);
            assert_eq!(v.expanded.len(), 2);
            v.toggle_message(1, true, cx);
            assert_eq!(v.expanded.len(), 1);
        });
        cx.simulate_keystrokes("j");
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.inbox.current()), Some(2));
        view.update(cx, |v, cx| v.act(Move::Delete, cx));
        assert_eq!(inbox_len(&view, cx), 4_999);
        view.update(cx, |v, cx| v.undo(cx));
        assert_eq!(inbox_len(&view, cx), 5_000);
        view.update(cx, |v, cx| {
            v.toggle_conversations(cx);
            assert_eq!(v.inbox.list().len(), 10_000);
            v.toggle_conversations(cx);
            assert_eq!(v.inbox.list().len(), 5_000);
            v.inbox.selected = 2500;
            v.scroll.set_offset(gpui::point(px(0.), px(-row_y(2500))));
            v.clear_motion();
        });
        frame(cx);
        assert!(children(&view, cx) < 15);
    }

    #[gpui::test]
    fn today_does_not_undo_hidden_mail_without_a_toast(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update_in(cx, |v, window, cx| {
            v.act(Move::Done, cx);
            v.show_today(window, cx);
        });
        let count = inbox_len(&view, cx);
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(inbox_len(&view, cx), count);
    }

    #[gpui::test]
    fn inline_folder_name_enter_escape_and_validation(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = view.read_with(cx, |v, _| v.inbox.folders.len());
        view.update_in(cx, |v, window, cx| v.new_folder(window, cx));
        cx.simulate_input("Cancelled");
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |v, _| {
            assert!(v.folder_name.is_none());
            assert_eq!(v.inbox.folders.len(), n);
        });
        view.update_in(cx, |v, window, cx| v.new_folder(window, cx));
        cx.simulate_keystrokes("enter");
        assert!(view.read_with(cx, |v, _| v.folder_name.is_some() && v.inbox.folders.len() == n));
        cx.simulate_input("  Taxes  ");
        cx.simulate_keystrokes("enter");
        view.read_with(cx, |v, _| {
            assert!(v.folder_name.is_none());
            assert_eq!(v.inbox.folders.len(), n + 1);
            assert_eq!(v.inbox.folders.last().unwrap().name, "Taxes");
            assert!(v.all_folders);
            assert!(v.toast.is_none());
        });
        view.update_in(cx, |v, window, cx| v.new_folder(window, cx));
        cx.simulate_input("taxes");
        cx.simulate_keystrokes("enter");
        assert!(view.read_with(cx, |v, _| v.folder_name.is_some() && v.inbox.folders.len() == n + 1));
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("j");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected), 1);
    }

    #[gpui::test]
    fn new_folder_stays_reachable_with_many_folders(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, _| {
            v.inbox = Inbox::new(0, 0);
            v.inbox.folders = (0..100).map(|i| Folder::new(&format!("Folder {i}"), None, &[])).collect();
        });
        cx.simulate_resize(gpui::size(px(1440.), px(700.)));
        frame(cx);
        let add = cx.debug_bounds("add-folder").unwrap().center();
        cx.simulate_click(add, Default::default());
        cx.simulate_input("New folder");
        cx.simulate_keystrokes("enter");
        frame(cx);
        let new = cx.debug_bounds("folder-100").unwrap();
        assert_eq!(new.size.height, px(SIDEBAR_ROW));
        assert!(new.top() >= px(0.) && new.bottom() <= px(700.), "new folder outside the window: {new:?}");
        cx.simulate_click(new.center(), Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.open), Some(100));
        cx.simulate_event(ScrollWheelEvent {
            position: new.center(),
            delta: ScrollDelta::Lines(gpui::point(0., 1000.)),
            ..Default::default()
        });
        frame(cx);
        let first = cx.debug_bounds("folder-0").unwrap();
        assert_eq!(first.size.height, px(SIDEBAR_ROW));
        assert!(first.top() >= px(0.) && first.bottom() <= px(700.));
        cx.simulate_click(first.center(), Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.open), Some(0));
        cx.simulate_keystrokes("secondary-k");
        cx.simulate_input("move Palette folder");
        cx.simulate_keystrokes("enter");
        frame(cx);
        let new = cx.debug_bounds("folder-101").unwrap();
        let settings = cx.debug_bounds("settings").unwrap();
        assert_eq!(new.size.height, px(SIDEBAR_ROW));
        assert!(new.top() > add.y && new.bottom() <= settings.top());
        assert!(settings.bottom() <= px(700.));
        cx.simulate_click(new.center(), Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.open), Some(101));
        cx.simulate_click(settings.center(), Default::default());
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Settings(_))));
    }

    #[gpui::test]
    fn both_sidebar_buttons_open_the_folder_field(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, _| v.inbox = Inbox::new(0, 0));
        for button in ["add-folder", "new-folder"] {
            frame(cx);
            let at = cx.debug_bounds(button).unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_input("New folder");
            assert_eq!(view.read_with(cx, |v, cx| v.folder_name.as_ref().unwrap().read(cx).text().to_string()), "New folder");
            cx.simulate_keystrokes("escape");
            assert!(view.read_with(cx, |v, _| v.folder_name.is_none()));
        }
    }

    fn large_list(cx: &mut TestAppContext) -> (Entity<MainWindow>, &mut VisualTestContext) {
        let (view, cx) = open(cx);
        view.update(cx, |v, _| {
            v.inbox.mail = vec![Mail { from: "Sender".into(), subject: "Subject".into(), snippet: "Preview".into(), ..Mail::default() }; 10_000];
            v.inbox.items = (0..10_000).collect();
            v.inbox.regroup();
        });
        cx.simulate_resize(gpui::size(px(1440.), px(600.)));
        (view, cx)
    }

    fn frame(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    fn children(view: &Entity<MainWindow>, cx: &mut VisualTestContext) -> usize {
        view.read_with(cx, |v, _| (0..).take_while(|&i| v.scroll.bounds_for_item(i).is_some()).count())
    }

    #[gpui::test]
    fn ten_thousand_message_rows(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        let mut elapsed = Duration::ZERO;
        for sample in 0..11 {
            let start = Instant::now();
            frame(cx);
            if sample > 0 { elapsed += start.elapsed(); }
        }
        let count = children(&view, cx);
        eprintln!("10,000 messages: {} row elements, mean full frame {:?} (10 frames, debug, headless)", count - 3, elapsed / 10);
        assert!((5..=12).contains(&(count - 3)), "{count} children");
        for ix in [5000, 9999, 0] {
            view.update(cx, |v, cx| {
                v.scroll.set_offset(gpui::point(px(0.), px(-row_y(ix))));
                cx.notify();
            });
            frame(cx);
            assert!(children(&view, cx) <= 15);
            view.read_with(cx, |v, _| {
                let height = f32::from(v.scroll.bounds().size.height);
                let expected = row_y(10_000) + LIST_PAD - height;
                assert!((f32::from(v.scroll.max_offset().height) - expected).abs() < 1.);
            });
        }
    }

    fn selected_in_view(view: &Entity<MainWindow>, cx: &mut VisualTestContext) {
        view.read_with(cx, |v, _| {
            let layout = v.row_layout(Instant::now());
            let top = -f32::from(v.scroll.offset().y);
            let height = f32::from(v.scroll.bounds().size.height);
            let visible = layout.visible(top, height);
            assert!(visible.contains(&v.inbox.selected));
            let child = 2 + v.inbox.selected - visible.start;
            let bounds = v.scroll.bounds_for_item(child).unwrap();
            let y = f32::from(bounds.top() - v.scroll.bounds().top()) - top;
            assert!(y >= -1. && y + ROW_H <= height + 1., "selected y {y}, height {height}");
        });
    }

    #[gpui::test]
    fn virtual_rows_keep_navigation_swipe_and_resize(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        frame(cx);
        view.update(cx, |v, cx| v.select(|inbox| inbox.select(5000), cx));
        frame(cx);
        selected_in_view(&view, cx);
        cx.simulate_keystrokes("down");
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected), 5001);
        selected_in_view(&view, cx);
        assert!(children(&view, cx) < 30, "glide copies stay in the viewport");
        cx.simulate_keystrokes("up");
        frame(cx);
        selected_in_view(&view, cx);

        let at = view.read_with(cx, |v, _| {
            let y = row_y(v.inbox.selected) + ROW_H / 2.;
            v.scroll.bounds().origin + v.scroll.offset() + gpui::point(px(50.), px(y))
        });
        cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(at + gpui::point(px(60.), px(0.)), MouseButton::Left, Default::default());
        assert!(view.read_with(cx, |v, _| v.swipe.offset(5000).is_some()));
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| matches!(v.swipe, Swipe::Idle)));

        let before = view.read_with(cx, |v, _| v.scroll.offset().y);
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(gpui::point(0., -30.)),
            ..Default::default()
        });
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.scroll.offset().y) < before);
        assert!(children(&view, cx) < 30);

        view.update(cx, |v, cx| v.select(|inbox| inbox.select(9999), cx));
        frame(cx);
        selected_in_view(&view, cx);
        cx.simulate_resize(gpui::size(px(1440.), px(1000.)));
        frame(cx);
        selected_in_view(&view, cx);
        assert!(children(&view, cx) < 40);
        view.update(cx, |v, cx| {
            v.inbox.items.truncate(20);
            v.inbox.regroup();
            v.inbox.selected = 19;
            v.clear_motion();
            cx.notify();
        });
        frame(cx);
        selected_in_view(&view, cx);
        view.update(cx, |v, cx| {
            v.act(Move::Done, cx);
            v.ghosts[0].at = Instant::now() - SPRING;
        });
        frame(cx);
        view.update(cx, |v, cx| v.undo(cx));
        frame(cx);
        // A late frame must reveal the restored last row even if its motion has ended.
        view.update(cx, |v, _| v.entering[0].at = Instant::now() - SPRING);
        frame(cx);
        selected_in_view(&view, cx);
        view.update(cx, |v, cx| v.open_box(Some(0), cx));
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.scroll.offset().y), px(0.));
    }

    #[gpui::test]
    fn virtual_undo_swipe_keeps_last_row_height(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        frame(cx);
        let at = Instant::now() + Duration::from_secs(60);
        view.update(cx, |v, cx| {
            v.inbox.selected = 9999;
            v.act(Move::Done, cx);
            v.undo(cx);
            v.entering[0].at = at;
            v.swipe = Swipe::Row { id: 9999, dx: 60. };
        });
        for released in [false, true] {
            view.update(cx, |v, cx| {
                if released { v.release(false, cx); }
                let layout = v.row_layout(at + Duration::from_millis(70));
                assert!((layout.bottom() - layout.y(9999) - ROW_H).abs() < 0.1,
                    "a swiped or springing row renders at full height after Undo");
            });
            frame(cx);
            selected_in_view(&view, cx);
            view.read_with(cx, |v, _| {
                let height = f32::from(v.scroll.bounds().size.height);
                let expected = row_y(10_000) + LIST_PAD - height;
                assert!((f32::from(v.scroll.max_offset().height) - expected).abs() < 1.);
            });
        }
    }

    #[gpui::test]
    fn virtual_spring_keeps_progress_when_remounted(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        frame(cx);
        let rest = cx.debug_bounds("row-0").unwrap().left();
        view.update(cx, |v, cx| {
            v.swipe = Swipe::Row { id: 0, dx: 60. };
            v.release(false, cx);
        });
        frame(cx);
        // Only a lower time bound matters: a delayed frame may have settled completely.
        std::thread::sleep(SPRING / 2);
        frame(cx);
        let before = cx.debug_bounds("row-0").unwrap().left() - rest;
        assert!(f32::from(before).abs() < 10.);
        view.update(cx, |v, _| v.scroll.set_offset(gpui::point(px(0.), px(-row_y(5000)))));
        frame(cx);
        view.read_with(cx, |v, _| {
            let visible = v.row_layout(Instant::now()).visible(-f32::from(v.scroll.offset().y), 600.);
            assert!(!visible.contains(&0));
        });
        assert!(children(&view, cx) < 15);
        view.update(cx, |v, _| v.scroll.set_offset(Point::default()));
        frame(cx);
        let after = cx.debug_bounds("row-0").unwrap().left() - rest;
        assert!(f32::from(after).abs() < 10., "spring restarted at {after:?}, was {before:?}");
        view.update(cx, |v, cx| {
            v.swipe = Swipe::Row { id: 0, dx: 60. };
            v.release(false, cx);
            v.spring.as_mut().unwrap().2 = Instant::now() - SPRING;
            v.scroll.set_offset(gpui::point(px(0.), px(-row_y(5000))));
        });
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.spring.is_none()), "spring finishes off screen");
        view.update(cx, |v, _| v.scroll.set_offset(Point::default()));
        frame(cx);
        assert_eq!(cx.debug_bounds("row-0").unwrap().left(), rest);
    }

    #[gpui::test]
    fn virtual_row_offsets_follow_motion_even_off_screen(cx: &mut TestAppContext) {
        let (view, cx) = large_list(cx);
        let now = Instant::now();
        view.update(cx, |v, cx| {
            v.act_at(0, Move::Done, 0., cx);
            v.ghosts[0].at = now;
            for step in [0., 0.25, 0.5, 1., 2.] {
                let at = now + SPRING.mul_f32(step);
                let height = ROW_H * row_scale(spring(step));
                let layout = v.row_layout(at);
                assert!((layout.y(5000) - row_y(5000) - height).abs() < 0.05);
                assert!((layout.bottom() - row_y(9999) - height).abs() < 0.1);
            }
            v.undo(cx);
            v.entering[0].at = now;
            v.act_at(1, Move::Done, 0., cx);
            v.ghosts[0].at = now;
            let layout = v.row_layout(now);
            assert!((layout.y(1) - LIST_PAD).abs() < 0.01);
            assert!((layout.y(2) - row_y(2)).abs() < 0.01);
            assert!(layout.visible(row_y(5000), 530.).len() <= 12);
            let settled = v.row_layout(now + SPRING);
            assert_eq!(settled.y(5000), row_y(5000));
            assert_eq!(settled.bottom(), row_y(9999));
            // A move above an entering row shifts its index; its id still locates it.
            v.clear_motion();
            v.act_at(1, Move::Done, 0., cx);
            v.undo(cx);
            v.entering[0].at = now;
            v.act_at(0, Move::Done, 0., cx);
            v.ghosts[0].at = now;
            assert_eq!(v.entering[0].ix, 1);
            assert!((v.row_layout(now).y(1) - row_y(1)).abs() < 0.01);
        });
        view.update(cx, |v, cx| {
            v.clear_motion();
            v.act_at(0, Move::Done, 0., cx);
            v.ghosts[0].at = Instant::now() + Duration::from_secs(60);
            v.inbox.selected = 5000;
            v.scroll.set_offset(gpui::point(px(0.), px(-row_y(5000))));
        });
        frame(cx);
        view.read_with(cx, |v, _| {
            let height = f32::from(v.scroll.bounds().size.height);
            let expected = row_y(v.inbox.list().len() + 1) + LIST_PAD - height;
            assert!((f32::from(v.scroll.max_offset().height) - expected).abs() < 1.);
        });
        assert!(children(&view, cx) <= 15);
        view.update(cx, |v, cx| {
            v.undo(cx);
            v.entering[0].at = Instant::now() + Duration::from_secs(60);
            v.inbox.selected = 5000;
            v.scroll.set_offset(Point::default());
        });
        frame(cx);
        view.read_with(cx, |v, _| {
            assert_eq!(v.scroll.bounds_for_item(2).unwrap().size.height, px(0.));
            let height = f32::from(v.scroll.bounds().size.height);
            let expected = row_y(v.inbox.list().len() - 1) + LIST_PAD - height;
            assert!((f32::from(v.scroll.max_offset().height) - expected).abs() < 1.);
        });
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
        let panes = Panes { sidebar: 320., list: 560., sidebar_hidden: false, group_conversations: true, notifications: true };
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
    fn quit_does_not_wait_for_a_send_in_its_undo_window(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("c");
        cx.simulate_input("recipient@example.com");
        cx.simulate_keystrokes("cmd-enter");
        let sheet = view.read_with(cx, |v, _| v.sending[0].clone());
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(sheet.read_with(cx, |c, _| c.undoable()));
    }

    #[gpui::test]
    fn quit_with_a_draft_never_asks_to_discard(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Not done yet");
        cx.simulate_keystrokes("escape");
        assert_eq!(view.read_with(cx, |v, _| v.drafts.len()), 1);
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(view.read_with(cx, |v, _| v.toast.is_none()));
        cx.simulate_keystrokes("r");
        assert!(view.read_with(cx, |v, _| v.compose.is_some() && v.drafts.is_empty()));
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(view.read_with(cx, |v, _| v.toast.is_none()));
    }

    #[test]
    fn a_failed_send_explains_outbox_retry() {
        assert_eq!(live::sent_note(Delivery::Sent, None), "Sent");
        assert!(live::sent_note(Delivery::Maybe, None).contains("outbox"));
        assert!(live::sent_note(Delivery::NotSent, Some("refused".into())).contains("refused"));
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

    /// The middle of list row `ix`, below the window margin and the list header.
    fn row_at(view: &Entity<MainWindow>, ix: usize, cx: &mut VisualTestContext) -> gpui::Point<Pixels> {
        let x = view.read_with(cx, |v, _| v.sidebar_w + v.list_w / 2.);
        gpui::point(px(x), px(8. + 1. + HEADER_H + row_y(ix) + ROW_H / 2.))
    }

    fn windows(cx: &mut VisualTestContext) -> usize {
        cx.update(|_, cx| cx.windows().len())
    }

    #[gpui::test]
    fn double_click_opens_a_message_window_once(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.run_until_parked();
        // A single click selects and opens nothing.
        let at = row_at(&view, 2, cx);
        cx.simulate_click(at, Default::default());
        assert_eq!((view.read_with(cx, |v, _| v.inbox.selected), windows(cx)), (2, 1));
        // The second press of a double-click opens the message, and starts no swipe.
        let at = row_at(&view, 3, cx);
        cx.simulate_click(at, Default::default());
        cx.simulate_event(MouseDownEvent { position: at, button: MouseButton::Left, click_count: 2, ..Default::default() });
        cx.simulate_event(MouseUpEvent { position: at, button: MouseButton::Left, click_count: 2, ..Default::default() });
        cx.run_until_parked();
        assert_eq!(windows(cx), 2);
        assert!(view.read_with(cx, |v, _| v.inbox.selected == 3 && v.swipe == Swipe::Idle && v.drag.is_none()));
        assert_eq!(view.read_with(cx, |v, _| v.popouts.iter().map(|p| p.0).collect::<Vec<_>>()), [3]);
        // The same message again comes to the front instead; ⌘O opens the selection too.
        cx.simulate_event(MouseDownEvent { position: at, button: MouseButton::Left, click_count: 2, ..Default::default() });
        cx.run_until_parked();
        assert_eq!(windows(cx), 2);
        view.update_in(cx, |v, window, _| window.focus(&v.focus));
        cx.simulate_keystrokes("j secondary-o");
        cx.run_until_parked();
        assert_eq!(windows(cx), 3);
    }

    #[gpui::test]
    fn done_in_a_message_window_moves_it_and_closes_the_window(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let n = inbox_len(&view, cx);
        cx.simulate_keystrokes("j secondary-o");
        cx.run_until_parked();
        let (id, popout) = view.read_with(cx, |v, _| v.popouts[0]);
        assert_eq!((id, windows(cx)), (1, 2));
        // Another message selected and another list on screen: Done still moves message 1.
        cx.simulate_keystrokes("j");
        view.update(cx, |v, cx| v.open_box(Some(0), cx));
        popout.update(cx, |p, _, cx| p.act(Move::Done, cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(windows(cx), 1);
        assert!(view.read_with(cx, |v, _| v.inbox.open.is_none() && !v.inbox.items.contains(&1) && v.popouts.is_empty()));
        assert_eq!(inbox_len(&view, cx), n - 1);
        // Undo in the main window puts it back; the window stays closed.
        view.update(cx, |v, cx| v.undo(cx));
        assert_eq!((inbox_len(&view, cx), windows(cx)), (n, 1));
    }

    #[gpui::test]
    fn a_message_window_move_closes_the_palette_and_quit_keeps_drafts(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Not done yet");
        cx.simulate_keystrokes("escape secondary-o");
        cx.run_until_parked();
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        // The palette would act on the selection, which the move changes: it closes.
        view.update_in(cx, |v, window, cx| v.open_palette("move tr", window, cx));
        popout.update(cx, |p, _, cx| p.act(Move::Delete, cx)).unwrap();
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.palette.is_none()));
        // A message window also leaves saved drafts alone on quit.
        #[cfg(target_os = "macos")]
        {
            cx.simulate_keystrokes("secondary-o");
            cx.run_until_parked();
            let popout = view.read_with(cx, |v, _| v.popouts[0].1);
            let focus = popout.update(cx, |p, _, _| p.focus.clone()).unwrap();
            cx.update_window(popout.into(), |_, window, cx| {
                assert!(focus.is_focused(window), "the message window has the keys");
                // Actions reach the elements of the last frame; test windows only draw on request.
                window.draw(cx).clear();
                window.dispatch_action(Box::new(crate::menus::Quit), cx);
            }).unwrap();
            cx.run_until_parked();
            assert!(view.read_with(cx, |v, _| v.toast.as_ref().is_none_or(|t| !t.0.contains("Quit again"))));
            assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        }
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

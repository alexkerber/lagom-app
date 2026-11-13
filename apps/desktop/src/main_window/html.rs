//! HTML mail in the reading view. One platform web view per window (WKWebView on macOS, WebView2
//! on Windows) covers the body area under the GPUI header. The trusted header (subject, sender,
//! date) never goes into the web view: mail CSS such as `position:fixed` paints over anything in
//! the same document, so the document holds only the CSP, the theme CSS and the mail.
//!
//! Linux shows the text part in GPUI instead: wry's child web view there needs GTK's main loop
//! and X11, and GPUI runs neither.
//!
//! HTML mail is untrusted. Nothing is sanitized by hand. Page JavaScript is off, a CSP meta that
//! comes before the mail blocks scripts, frames, fonts and every remote load (images only after
//! "Load remote images"), and the web view navigates nowhere: only the load the app starts is
//! allowed. Links reach the system browser through a click listener the app injects (app
//! scripts run with page scripts off), never through navigation, so a `<meta refresh>` goes
//! nowhere.

use gpui::{Bounds, Pixels, Rgba};
use std::sync::Arc;

use super::*;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;

/// Whether this machine renders HTML mail: macOS always, Windows with the WebView2 runtime.
#[cfg(not(target_os = "windows"))]
pub(super) fn supported() -> bool { cfg!(target_os = "macos") }

#[cfg(target_os = "windows")]
pub(super) fn supported() -> bool {
    static RUNTIME: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RUNTIME.get_or_init(webview2_installed)
}

/// Microsoft's documented check, the same one the installer makes: a `pv` other than empty or
/// 0.0.0.0 under the runtime's EdgeUpdate client, per machine (32-bit view) or per user.
#[cfg(target_os = "windows")]
fn webview2_installed() -> bool {
    use windows::{core::w, Win32::System::Registry::*};
    [(HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6432KEY), (HKEY_CURRENT_USER, REG_ROUTINE_FLAGS(0))].into_iter().any(|(root, view)| {
        let mut buf = [0u16; 64];
        let mut bytes = std::mem::size_of_val(&buf) as u32;
        let found = unsafe {
            RegGetValueW(root, w!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"), w!("pv"),
                RRF_RT_REG_SZ | view, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes))
        }.is_ok();
        let pv = String::from_utf16_lossy(&buf[..(bytes as usize / 2).min(buf.len())]);
        let pv = pv.trim_end_matches('\0');
        found && !pv.is_empty() && pv != "0.0.0.0"
    })
}

/// WebView2 is missing: say so once per run, the first time an HTML message falls back to text.
#[cfg(target_os = "windows")]
static TOLD_NO_WEBVIEW2: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

use lagom_core::html::{csp, link_allowed, styled, has_remote, inert_tags};

fn css(c: Rgba) -> String {
    let ch = |v: f32| (v * 255.).round() as u8;
    format!("rgba({},{},{},{})", ch(c.r), ch(c.g), ch(c.b), c.a)
}

/// The web view's document for one mail: the CSP first, then the theme CSS, then the mail as
/// sent. The mail's own `<html>`, `<head>` and `<body>` merge into this document, as in a
/// browser; its CSS may restyle anything here, as nothing here is trusted UI.
pub(super) fn document(html: &str, t: &Theme, remote: bool) -> String {
    let (bg, ink, accent) = (css(t.raised), css(t.ink), css(t.accent));
    let scheme = if t.dark { "dark" } else { "light" };
    let base = format!("html{{background:{bg}}}body{{margin:0;font:14px/1.4 -apple-system,system-ui,\"Segoe UI\",sans-serif;overflow-wrap:break-word}}");
    // Mail that sets its own colours assumes white paper: in dark mode a rounded white card.
    // Other HTML takes the theme's ink and accent and blends in.
    let look = match (styled(html), t.dark) {
        (true, true) => "body{background:#fff;color:#000;border-radius:10px;padding:16px}".to_string(),
        (true, false) => "body{background:#fff;color:#000}".to_string(),
        (false, _) => format!("body{{color:{ink}}}a{{color:{accent}}}img{{max-width:100%;height:auto}}"),
    };
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{csp}"><meta http-equiv="x-dns-prefetch-control" content="off"><meta name="color-scheme" content="{scheme}"><style>{base}{look}</style></head>{mail}"#,
        csp = csp(remote, if cfg!(target_os = "windows") { "http://lagom-cid.localhost" } else { "lagom-cid:" }),
        mail = inert_tags(html),
    )
}

/// What the web view received: a finished load, a link click, Esc.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(super) enum Msg {
    Loaded,
    Open(String),
    Escape,
    CloseWindow,
    NewTask,
    MailAction(Run),
    Hints { over: bool, command: bool, shift: bool },
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn hint_event(state: &str) -> Option<Msg> {
    match state.split_whitespace().collect::<Vec<_>>().as_slice() {
        [over @ ("0" | "1"), command @ ("0" | "1"), shift @ ("0" | "1")] => Some(Msg::Hints {
            over: *over == "1", command: *command == "1", shift: *shift == "1",
        }),
        _ => None,
    }
}

/// Runs in every document before the mail, as an app script: the CSP and the page-script switch
/// do not apply to it. Clicks inside the shadow root reach `document` retargeted to the host,
/// so the link comes from the event path.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const BRIDGE: &str = r#"
let overMail = false;
const hints = e => {
  if (e.isTrusted) window.ipc.postMessage(`hints ${Number(overMail)} ${Number(!!e.metaKey)} ${Number(!!e.shiftKey)}`);
};
addEventListener('mouseover', e => { overMail = true; hints(e); }, true);
addEventListener('mouseout', e => { if (!e.relatedTarget) { overMail = false; hints(e); } }, true);
addEventListener('keyup', hints, true);
addEventListener('blur', e => { overMail = false; hints(e); });
addEventListener('click', e => {
  const a = e.composedPath().find(n => (n.tagName === 'A' || n.tagName === 'AREA') && typeof n.href === 'string' && n.href);
  if (a) { e.preventDefault(); if (e.isTrusted) window.ipc.postMessage('open ' + a.href); }
}, true);
addEventListener('keydown', e => {
  hints(e);
  if (e.isTrusted && e[closeModifier] && !(e.metaKey && e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'w') {
    e.preventDefault();
    window.ipc.postMessage('close-window');
    return;
  }
  if (e.key === 'Escape') window.ipc.postMessage('escape');
  if (!e.isTrusted || e.metaKey || e.ctrlKey || e.altKey || e.target.closest?.('input,textarea,[contenteditable]')) return;
  const key = e.key.toLowerCase();
  const command = !e.shiftKey && key === 't' ? 'task' : !e.shiftKey && key === 'f' ? 'forward' : e.shiftKey && key === 'r' ? 'reply-all' : e.shiftKey && key === 'u' ? 'toggle-read' : null;
  if (command) { e.preventDefault(); window.ipc.postMessage(command); }
}, true);
"#;

/// A navigation passes only when the app has just started a load and `url` is that load:
/// `loadHTMLString` with no base URL is `about:blank`. Another URL never takes the flag, so a
/// `<meta refresh>` that fires as the user switches mail cannot spend the app's load.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn allow_navigation(ours: &std::cell::Cell<bool>, url: &str) -> bool {
    // shortcut: WebView2 `NavigateToString` is documented as `about:blank` and has been seen as a `data:text/html` URI; both pass on Windows. Not run on Windows: measure it there and keep one.
    (url == "about:blank" || (cfg!(target_os = "windows") && url.starts_with("data:text/html"))) && ours.replace(false)
}

fn placement(bounds: Bounds<Pixels>, mask: Bounds<Pixels>) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let mut clip = bounds.intersect(&mask);
    clip.size.width = clip.size.width.max(px(0.));
    clip.size.height = clip.size.height.max(px(0.));
    (bounds, clip)
}

/// (message id, HTML length, remote images, dark, inline parts): the document on screen.
type Key = (usize, usize, bool, bool, usize);
type Inline = std::sync::Arc<std::sync::Mutex<Option<(i64, std::sync::Arc<Vec<lagom_core::inline::InlinePart>>)>>>;
type Placement = (Bounds<Pixels>, Bounds<Pixels>, f32);

/// The window's web view and what it shows.
#[derive(Default)]
pub(super) struct Web {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    view: Option<Rc<native::View>>,
    /// Set just before the app loads a document: the one navigation the web view may make.
    ours: Rc<std::cell::Cell<bool>>,
    /// The document asked for last.
    shown: Option<Key>,
    inline: Inline,
    /// A document finished loading since the view was last hidden for plain text.
    ready: bool,
    /// The document asked for last did not load: the reading view shows the text part.
    pub(super) failed: bool,
    visible: Rc<std::cell::Cell<bool>>,
    drawable: Rc<std::cell::Cell<bool>>,
    /// The shown mail has remote images.
    pub(super) has_remote: bool,
    /// Bounds and scale factor last applied: a move to a display with another scale needs new
    /// device-pixel bounds on Windows.
    place: Rc<std::cell::Cell<Option<Placement>>>,
    /// The page as it looked when an overlay hid the view, with the request it answers.
    cover: Rc<std::cell::RefCell<Option<(u64, Arc<gpui::Image>)>>>,
    /// The document a snapshot was asked for during this overlay, and that request's number.
    asked: Option<(Key, u64)>,
    /// Snapshot requests so far: a late answer to an earlier overlay never shows in a later one.
    asks: u64,
    /// The document asked for last has finished loading; until then the view may still show the
    /// previous one, so no snapshot is taken.
    // shortcut: a Loaded for the previous document that is still queued when the next load starts
    // counts for the new one; wry's load event names no document. Tag loads if a wrong picture shows.
    settled: bool,
}

/// What stands in for the web view while an overlay hides it.
pub(super) enum Cover {
    /// A picture of the page, so the mail looks unchanged under the overlay.
    Picture(Arc<gpui::Image>),
    /// The picture is on its way; draw nothing rather than switch to text for a frame.
    Pending,
}

/// A window with a mail web view: the main window and each message window.
pub(super) trait WebHost: 'static + Sized {
    fn web(&mut self) -> &mut Web;
    /// Where the keys go when Esc leaves the web view.
    fn keys(&self) -> FocusHandle;
    fn new_task(&mut self, cx: &mut Context<Self>);
    fn mail_action(&mut self, action: Run, window: &mut Window, cx: &mut Context<Self>);
    fn hints(&mut self, over: bool, command: bool, shift: bool, cx: &mut Context<Self>);
}

impl WebHost for MainWindow {
    fn hints(&mut self, over: bool, command: bool, shift: bool, cx: &mut Context<Self>) {
        self.update_hints(over, gpui::Modifiers { platform: command, shift, ..Default::default() }, cx);
    }
    fn mail_action(&mut self, action: Run, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.inbox.current() { self.mail_action_on(id, action, window, cx); }
    }
    fn new_task(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.inbox.single() { self.task_from_mail(id, cx); }
    }
    fn web(&mut self) -> &mut Web {
        &mut self.web
    }

    fn keys(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl MainWindow {
    /// Before a frame: the selected HTML message. Overlays draw in GPUI, under any native view,
    /// so the web view hides while one is open.
    pub(super) fn web_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.inbox.list().is_empty() && self.inbox.open.is_none();
        let want = self.inbox.single().filter(|_| matches!(self.page, Page::Mail) && !empty && (!self.inbox.group || self.reading_members.len() <= 1)).map(|id| (id, &self.inbox.mail[id]));
        let overlay = self.message_menu.is_some() || self.palette.is_some() || self.compose.is_some() || !self.sending.is_empty();
        #[cfg(target_os = "windows")]
        let text_fallback = !supported() && want.is_some_and(|(_, m)| m.html.is_some());
        self.web.frame(want, &self.theme, overlay, window, cx);
        #[cfg(target_os = "windows")]
        if text_fallback && !TOLD_NO_WEBVIEW2.swap(true, std::sync::atomic::Ordering::Relaxed) {
            self.notify("Formatted mail needs Microsoft Edge WebView2. Showing plain text.", cx);
        }
        let overlay = overlay || !matches!(self.page, Page::Mail) || self.toast.is_some() || self.live.as_ref().is_some_and(|l| !l.failed.is_empty());
        for body in self.expanded.values() {
            body.update(cx, |body, cx| body.prepare(self.theme, overlay, window, cx));
        }
    }
}

impl Web {
    /// Before a frame: load `want` if it is HTML, and show the web view once it has loaded.
    /// `overlay`: something in GPUI covers the body, so the web view hides.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_variables))]
    pub(super) fn frame<T: WebHost>(&mut self, want: Option<(usize, &Mail)>, theme: &Theme, overlay: bool, window: &mut Window, cx: &mut Context<T>) {
        if !supported() {
            return;
        }
        let Some((id, m, html)) = want.and_then(|(id, m)| m.html.as_ref().map(|h| (id, m, h))) else {
            // A cold web content process takes about a second to start: start it with the window,
            // on an empty page, so the first HTML message opens as fast as the rest.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if self.view.is_none() {
                self.view = Self::create(self.ours.clone(), self.inline.clone(), theme, window, cx).map(Rc::new);
                if let Some(view) = &self.view {
                    self.ours.set(view.load_html("<!DOCTYPE html><title></title>").is_ok());
                }
            }
            // Plain text: the next HTML message waits for its own load, never shows a stale one.
            *self.inline.lock().unwrap() = None;
            self.shown = None;
            self.ready = false;
            self.drawable.set(false);
            self.set_visible(false);
            return;
        };
        let key = (id, html.len(), m.remote, theme.dark, m.inline.len());
        if self.shown != Some(key) {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                if self.view.is_none() {
                    self.view = Self::create(self.ours.clone(), self.inline.clone(), theme, window, cx).map(Rc::new);
                }
                let Some(view) = self.view.clone() else { return };
                let message = m.store.unwrap_or(-(id as i64) - 1);
                *self.inline.lock().unwrap() = Some((message, m.inline.clone()));
                let html = lagom_core::inline::rewrite(html, message, cfg!(target_os = "windows"));
                let doc = document(&html, theme, m.remote);
                self.ours.set(true);
                let _ = view.set_background_color(rgba8(theme.bg));
                // WebView2 refuses documents over 2 MB: the text part shows, never the last mail's body.
                self.failed = view.load_html(&doc).is_err();
                if self.failed {
                    self.ours.set(false);
                    self.ready = false;
                }
                self.settled = false;
            }
            self.shown = Some(key);
            self.has_remote = has_remote(html);
            // Shown already: the previous message stays until this one has loaded, no flash.
            self.ready &= self.visible.get();
        }
        self.drawable.set(self.ready && !overlay);
        self.snapshot(overlay, cx);
        // Native children outside the scroll viewport must stay hidden until their slot is painted.
        self.set_visible(false);
    }

    /// While an overlay is open, ask once for a picture of the page the reader was looking at.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_variables))]
    fn snapshot<T: WebHost>(&mut self, overlay: bool, cx: &mut Context<T>) {
        if !overlay {
            self.asked = None;
            self.cover.borrow_mut().take();
            return;
        }
        // `visible`: the view was on screen in the last frame, so the picture matches what was seen.
        if self.asked.is_some() || !self.visible.get() || !self.settled { return; }
        let Some(key) = self.shown else { return };
        self.asks += 1;
        let ask = self.asks;
        self.asked = Some((key, ask));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(view) = &self.view {
            let (tx, rx) = tokio::sync::oneshot::channel();
            view.snapshot(move |png| { let _ = tx.send(png); });
            cx.spawn(async move |this, cx| {
                let png = rx.await.ok().flatten();
                this.update(cx, |this, cx| {
                    let web = this.web();
                    if web.asked.map(|(_, a)| a) != Some(ask) { return; }
                    match png {
                        Some(png) => *web.cover.borrow_mut() = Some((ask, Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)))),
                        // No picture (Windows, or WebKit failed): the text part stands in, as before.
                        None => web.asked = None,
                    }
                    cx.notify();
                }).ok();
            }).detach();
        }
    }

    /// The body area: the web view's slot, or its picture while an overlay hides it.
    pub(super) fn surface(&self) -> gpui::AnyElement {
        match self.cover() {
            Some(Cover::Picture(picture)) => gpui::img(picture).size_full().into_any_element(),
            Some(Cover::Pending) => div().into_any_element(),
            None => self.slot().into_any_element(),
        }
    }

    /// What to draw in the web view's place while an overlay hides it; `None` means the text part.
    pub(super) fn cover(&self) -> Option<Cover> {
        let (key, ask) = self.asked?;
        // Another message or theme under the same overlay: no picture of it exists, so the text.
        if self.shown != Some(key) { return None; }
        Some(match &*self.cover.borrow() {
            Some((answer, picture)) if *answer == ask => Cover::Picture(picture.clone()),
            _ => Cover::Pending,
        })
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible.replace(visible) == visible {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(view) = &self.view {
            let _ = view.set_visible(visible);
        }
    }

    /// The element that keeps the web view over the body area: its bounds every frame.
    pub(super) fn slot(&self) -> impl IntoElement {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let view = self.view.clone();
        let place = self.place.clone();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let drawable = self.drawable.clone();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let visible = self.visible.clone();
        gpui::canvas(
            move |bounds, window, _| {
                let (bounds, clip) = placement(bounds, window.content_mask().bounds);
                let now = Some((bounds, clip, window.scale_factor()));
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                if let Some(view) = &view {
                    let show = drawable.get() && clip.size.width > px(0.) && clip.size.height > px(0.);
                    visible.set(show);
                    let _ = view.set_visible(show);
                }
                if place.get() == now {
                    return;
                }
                place.set(now);
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                if let Some(view) = &view {
                    if view.place(bounds, clip, window.scale_factor()).is_err() {
                        visible.set(false);
                        let _ = view.set_visible(false);
                        place.set(None);
                    }
                }
            },
            |_, _, _, _| {},
        )
        .size_full()
    }

    /// A click anywhere in GPUI takes the keys back from the web view. GPUI's view does not take
    /// first responder on a click by itself.
    pub(super) fn unfocus(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(view) = &self.view {
            let _ = view.focus_parent();
        }
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    fn event<T: WebHost>(host: &mut T, msg: Msg, window: &mut Window, cx: &mut Context<T>) {
        match msg {
            Msg::Hints { over, command, shift } => host.hints(over, command, shift, cx),
            Msg::MailAction(action) => {
                host.web().unfocus();
                window.focus(&host.keys());
                host.mail_action(action, window, cx);
            }
            Msg::NewTask => {
                host.web().unfocus();
                window.focus(&host.keys());
                host.new_task(cx);
            }
            Msg::Loaded => {
                host.web().ready = true;
                host.web().settled = true;
            }
            Msg::CloseWindow => {
                host.web().unfocus();
                window.focus(&host.keys());
                window.dispatch_action(Box::new(super::CloseWindow), cx);
            }
            Msg::Open(url) if link_allowed(&url) => cx.open_url(&url),
            Msg::Open(_) => {}
            Msg::Escape => {
                host.web().unfocus();
                window.focus(&host.keys());
            }
        }
        cx.notify();
    }

    /// The web view, hidden and unfocused, with page scripts, navigation and downloads off and
    /// nothing kept on disk.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn create<T: WebHost>(ours: Rc<std::cell::Cell<bool>>, inline: Inline, theme: &Theme, window: &mut Window, cx: &mut Context<T>) -> Option<native::View> {
        // GPUI's test windows have no native handle.
        if cfg!(test) {
            return None;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (load_tx, ipc_tx) = (tx.clone(), tx);
        let view = wry::WebViewBuilder::new()
            .with_javascript_disabled()
            .with_incognito(true)
            .with_focused(false)
            .with_visible(false)
            .with_background_color(rgba8(theme.bg))
            .with_custom_protocol("lagom-cid".into(), move |_, request| {
                let state = inline.lock().unwrap();
                let part = state.as_ref().and_then(|(message, parts)| {
                    (request.method() == "GET").then(|| lagom_core::inline::resolve(*message, parts, &request.uri().to_string())).flatten()
                });
                let response = wry::http::Response::builder()
                    .header("Content-Security-Policy", "default-src 'none'; sandbox")
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Cache-Control", "no-store");
                match part {
                    Some(part) => response.header("Content-Type", &part.mime_type).body(std::borrow::Cow::Owned(part.data.clone()))
                        .unwrap_or_else(|_| wry::http::Response::builder().status(404).body(std::borrow::Cow::Owned(Vec::new())).unwrap()),
                    None => response.status(404).body(std::borrow::Cow::Owned(Vec::new())).unwrap(),
                }
            })
            .with_initialization_script(format!("const closeModifier = '{}';\n{BRIDGE}\n{}",
                if cfg!(target_os = "macos") { "metaKey" } else { "ctrlKey" }, include_str!("html/fit.js")))
            .with_navigation_handler(move |url| allow_navigation(&ours, &url))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_on_page_load_handler(move |e, _| {
                if matches!(e, wry::PageLoadEvent::Finished) {
                    let _ = load_tx.send(Msg::Loaded);
                }
            })
            .with_ipc_handler(move |req| {
                let body = req.body();
                let msg = match body.split_once(' ') {
                    Some(("open", url)) => Msg::Open(url.to_string()),
                    Some(("hints", state)) => match hint_event(state) { Some(msg) => msg, None => return },
                    _ if body == "escape" => Msg::Escape,
                    _ if body == "close-window" => Msg::CloseWindow,
                    _ if body == "task" => Msg::NewTask,
                    _ if body == "forward" => Msg::MailAction(Run::Forward),
                    _ if body == "reply-all" => Msg::MailAction(Run::ReplyAll),
                    _ if body == "toggle-read" => Msg::MailAction(Run::ToggleRead),
                    _ => return,
                };
                let _ = ipc_tx.send(msg);
            })
            .build_as_child(window)
            .ok()?;
        cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| Self::event(this, msg, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        native::View::new(view)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn rgba8(c: Rgba) -> (u8, u8, u8, u8) {
    let ch = |v: f32| (v * 255.).round() as u8;
    (ch(c.r), ch(c.g), ch(c.b), ch(c.a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn cover_shows_only_the_picture_of_the_document_asked_for() {
        let mut web = Web::default();
        assert!(web.cover().is_none(), "no overlay: the slot or the text");
        let (shown, dark) = ((1, 10, false, true, 0), (1, 10, false, false, 0));
        web.shown = Some(shown);
        web.asked = Some((shown, 2));
        assert!(matches!(web.cover(), Some(Cover::Pending)));
        let picture = Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![]));
        *web.cover.borrow_mut() = Some((1, picture.clone()));
        assert!(matches!(web.cover(), Some(Cover::Pending)), "an earlier overlay's picture never shows");
        *web.cover.borrow_mut() = Some((2, picture));
        assert!(matches!(web.cover(), Some(Cover::Picture(_))));
        web.shown = Some(dark);
        assert!(web.cover().is_none(), "a theme or message change under the overlay falls back to text");
    }

    #[gpui::test]
    fn html_slot_stays_inside_the_reading_card(cx: &mut gpui::TestAppContext) {
        if !supported() { return; }
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "html-simple", window, cx));
        cx.run_until_parked();
        let card = cx.debug_bounds("message-card").unwrap();
        let subject = cx.debug_bounds("reading-subject").unwrap();
        let bounds = view.read_with(cx, |v, _| v.web.place.get().unwrap().0);
        assert!(subject.bottom() <= card.top());
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(bounds.left() > card.left() && bounds.right() < card.right());
        assert!(bounds.top() > card.top() && bounds.bottom() < card.bottom());
    }

    #[test]
    fn html_modifier_messages_are_bounded_flags() {
        assert!(matches!(hint_event("1 1 0"), Some(Msg::Hints { over: true, command: true, shift: false })));
        assert!(matches!(hint_event("0 0 0"), Some(Msg::Hints { over: false, command: false, shift: false })));
        for invalid in ["", "1 0", "1 1 1 0", "1 NaN 0", "2 0 0", "1 true false"] { assert!(hint_event(invalid).is_none()); }
    }

    #[gpui::test]
    fn close_window_from_html_uses_the_window_action(cx: &mut gpui::TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.simulate_keystrokes("secondary-o");
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        cx.update_window(popout.into(), |_, window, cx| window.draw(cx).clear()).unwrap();
        popout.update(cx, |v, window, cx| {
            Web::event(v, Msg::CloseWindow, window, cx);
        }).unwrap();
        cx.run_until_parked();
        assert!(popout.read_with(cx, |_, _| ()).is_err());
        assert_eq!(cx.cx.update(|cx| cx.windows().len()), 1);
        view.update_in(cx, |v, window, cx| Web::event(v, Msg::CloseWindow, window, cx));
        cx.run_until_parked();
        assert_eq!(cx.cx.update(|cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn html_mail_actions_use_compose_and_unread_paths(cx: &mut gpui::TestAppContext) {
        for action in [Run::Forward, Run::ReplyAll, Run::ToggleRead] {
            let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
            view.update_in(cx, |v, window, cx| {
                v.open_box(None, cx);
                let id = v.inbox.current().unwrap();
                let unread = v.inbox.mail[id].unread;
                Web::event(v, Msg::MailAction(action), window, cx);
                if action == Run::ToggleRead { assert_eq!(v.inbox.mail[id].unread, !unread); }
                else { assert!(v.compose.is_some()); }
            });
        }
    }

    #[gpui::test]
    fn review_cmd_toggle_keeps_sole_selected_html(cx: &mut gpui::TestAppContext) {
        if !supported() { return; }
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        view.update_in(cx, |v, window, cx| {
            v.open_box(None, cx);
            v.inbox.group = false;
            v.inbox.select(0);
            let id = v.inbox.current().unwrap();
            v.inbox.mail[id].html = Some("<p>Selected</p>".into());
            v.web.shown = Some((id, v.inbox.mail[id].html.as_ref().unwrap().len(), false, v.theme.dark, v.inbox.mail[id].inline.len()));
            v.web.ready = true;
            v.inbox.click(1, false, true);
            v.inbox.click(1, false, true);
            v.open_current(false, cx);
            v.web_frame(window, cx);
            assert_eq!(v.web.shown.map(|key| key.0), Some(id));
        });
    }

    #[gpui::test]
    fn review_task_toast_keeps_html_drawable(cx: &mut gpui::TestAppContext) {
        if !supported() { return; }
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        view.update_in(cx, |v, window, cx| {
            v.open_box(None, cx);
            let id = v.inbox.current().unwrap();
            v.inbox.mail[id].html = Some("<p>Still visible</p>".into());
            v.web.shown = Some((id, v.inbox.mail[id].html.as_ref().unwrap().len(), v.inbox.mail[id].remote, v.theme.dark, v.inbox.mail[id].inline.len()));
            v.web.ready = true;
            v.notify("Task created", cx);
            v.task_undo = Some(1);
            v.toast.as_mut().unwrap().1 = true;
            v.web_frame(window, cx);
            assert!(v.web.drawable.get(), "task toast must not blank the HTML body");
        });
    }

    #[test]
    fn scrolling_clips_without_resizing_or_reanchoring_the_document() {
        let mask = Bounds::new(point(px(20.), px(100.)), size(px(300.), px(400.)));
        for (x, y) in [(20., 100.), (10., 50.), (20., 400.), (20., -500.)] {
            let full = Bounds::new(point(px(x), px(y)), size(px(320.), px(480.)));
            let (document, clip) = placement(full, mask);
            assert_eq!(document, full);
            assert!(clip.size.width >= px(0.) && clip.size.height >= px(0.));
            if clip.size.height > px(0.) {
                assert!(clip.origin.y >= mask.origin.y);
                assert!(clip.bottom() <= mask.bottom());
            }
        }
    }

    #[test]
    fn the_document_blocks_scripts_and_remote_loads() {
        let doc = document("<p>Hi</p>", &Theme::light(DEFAULT_ACCENT), false);
        // The CSP comes before anything from the mail, and allows no network source.
        assert!(doc.find("Content-Security-Policy").unwrap() < doc.find("<p>Hi</p>").unwrap());
        let local = if cfg!(target_os = "windows") { "http://lagom-cid.localhost" } else { "lagom-cid:" };
        assert_eq!(csp_of(&doc), format!("default-src 'none'; img-src data: {local}; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"));
        // The wrapper itself loads nothing and runs nothing.
        assert!(!doc.contains("src=") && !doc.contains("<script") && !doc.contains("<link"));
        // Remote images, when asked for: images only.
        let doc = document("", &Theme::light(DEFAULT_ACCENT), true);
        assert_eq!(csp_of(&doc), format!("default-src 'none'; img-src data: {local} https: http:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"));
    }

    fn csp_of(doc: &str) -> &str {
        let start = doc.find("Content-Security-Policy\" content=\"").unwrap() + 34;
        &doc[start..start + doc[start..].find('"').unwrap()]
    }

    #[test]
    fn mail_cannot_reach_the_header() {
        // Breaking out of a wrapper, or painting over the viewport, finds no header: the
        // document is the CSP, the theme CSS and the mail, nothing the app vouches for.
        for evil in [
            "</template><h1>Your bank</h1>",
            "</body><script>alert(1)</script></html><p>after</p>",
            r#"<div style="position:fixed;inset:0;z-index:2147483647;background:#fff">From: ceo@bank.example</div>"#,
        ] {
            let doc = document(evil, &Theme::dark(DEFAULT_ACCENT), false);
            let head = &doc[..doc.find(evil).unwrap()];
            assert!(head.ends_with("</head>"), "only the head comes before the mail: {head}");
            for header in ["<main", "<h1", "avatar", "class=\"from\"", "<template"] {
                assert!(!head.contains(header), "{header} in {head}");
            }
        }
    }

    #[test]
    fn styled_mail_gets_white_paper_in_dark_mode() {
        let dark = Theme::dark(DEFAULT_ACCENT);
        assert!(document(r##"<table bgcolor="#f4f4f4">"##, &dark, false).contains("body{background:#fff;color:#000;border-radius:10px"));
        // Simple HTML blends: theme ink, accent links.
        let doc = document("<div dir=ltr>Lunch?</div>", &dark, false);
        assert!(!doc.contains("background:#fff") && doc.contains("a{color:rgba(138,176,245,1)}"));
    }

    #[test]
    fn resource_hints_are_inert() {
        let mail = r#"<LINK rel="preconnect" href="https://t.example"><link rel=dns-prefetch href=//t.example><p>&lt;link</p>"#;
        let doc = document(mail, &Theme::light(DEFAULT_ACCENT), false);
        assert!(doc.contains(r#"<x-LINK rel="preconnect""#) && doc.contains("<x-link rel=dns-prefetch") && doc.contains("<p>&lt;link</p>"));
        assert!(!doc.to_ascii_lowercase().contains("<link"));
        assert!(doc.contains(r#"<meta http-equiv="x-dns-prefetch-control" content="off">"#));
    }

    #[test]
    fn only_the_apps_own_load_navigates() {
        let ours = std::cell::Cell::new(false);
        assert!(!allow_navigation(&ours, "about:blank"), "no load started");
        ours.set(true);
        for evil in ["https://evil.example/", "about:srcdoc", "lagom-cid:x"] {
            assert!(!allow_navigation(&ours, evil), "{evil}");
        }
        assert_eq!(allow_navigation(&ours, "data:text/html,x"), cfg!(target_os = "windows"));
        ours.set(true);
        assert!(ours.get(), "a denied URL leaves the app's load its flag");
        assert!(allow_navigation(&ours, "about:blank"));
        assert!(!allow_navigation(&ours, "about:blank"), "one load, one navigation");
    }

    #[test]
    fn links_open_only_for_web_and_mail() {
        for ok in ["https://example.com", "HTTP://x", "mailto:a@b.se"] {
            assert!(link_allowed(ok), "{ok}");
        }
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,x", "about:blank", "ftp://x", "lagom://x", "nocolon"] {
            assert!(!link_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn remote_content_is_detected() {
        assert!(has_remote(r#"<img src="https://t.example/p.gif">"#));
        assert!(has_remote("<td background='http://x/bg.png'>"));
        assert!(has_remote(r#"<div style="background:url( //cdn.example/a.png)">"#));
        assert!(!has_remote(r#"<a href="https://example.com">x</a><img src="data:image/png;base64,AA" alt="">"#));
    }
}

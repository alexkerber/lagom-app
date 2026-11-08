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

use super::*;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;

/// Whether this platform renders HTML mail.
pub(super) const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The CSP for a mail document. `base-uri` and `form-action` do not fall back to `default-src`.
pub(super) fn csp(remote: bool) -> String {
    let remote = if remote { " https: http:" } else { "" };
    format!("default-src 'none'; img-src data: cid:{remote}; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'")
}

/// Links open outside only for these schemes.
pub(super) fn link_allowed(url: &str) -> bool {
    let scheme = url.split_once(':').map_or("", |s| s.0);
    ["http", "https", "mailto"].iter().any(|s| s.eq_ignore_ascii_case(scheme))
}

/// Whether the mail sets its own colours or backgrounds. Such mail assumes white paper; other
/// HTML takes the theme's ink and background.
pub(super) fn styled(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("color") || lower.contains("background")
}

/// Whether the mail has remote images or backgrounds, which "Load remote images" would fetch.
pub(super) fn has_remote(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    ["src=", "srcset=", "background=", "url("].iter().any(|k| {
        lower.match_indices(k).any(|(i, _)| {
            let rest = lower[i + k.len()..].trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
            rest.starts_with("http:") || rest.starts_with("https:") || rest.starts_with("//")
        })
    })
}

/// `<link>` becomes an unknown, inert element. Resource hints (preconnect, dns-prefetch) open
/// connections that the CSP does not cover; stylesheets are blocked by the CSP anyway.
fn inert_links(html: &str) -> std::borrow::Cow<'_, str> {
    let bytes = html.as_bytes();
    let hit = |i: usize| bytes[i] == b'<' && bytes.get(i + 1..i + 5).is_some_and(|w| w.eq_ignore_ascii_case(b"link"));
    if !(0..bytes.len()).any(hit) {
        return html.into();
    }
    let mut out = String::with_capacity(html.len() + 16);
    let mut last = 0;
    for i in (0..bytes.len()).filter(|&i| hit(i)) {
        out.push_str(&html[last..=i]);
        out.push_str("x-");
        last = i + 1;
    }
    out.push_str(&html[last..]);
    out.into()
}

fn css(c: Rgba) -> String {
    let ch = |v: f32| (v * 255.).round() as u8;
    format!("rgba({},{},{},{})", ch(c.r), ch(c.g), ch(c.b), c.a)
}

/// The web view's document for one mail: the CSP first, then the theme CSS, then the mail as
/// sent. The mail's own `<html>`, `<head>` and `<body>` merge into this document, as in a
/// browser; its CSS may restyle anything here, as nothing here is trusted UI.
pub(super) fn document(html: &str, t: &Theme, remote: bool) -> String {
    let (bg, ink, accent) = (css(t.bg), css(t.ink), css(t.accent));
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
        csp = csp(remote),
        mail = inert_links(html),
    )
}

/// What the web view received: a finished load, a link click, Esc.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(super) enum Msg {
    Loaded,
    Open(String),
    Escape,
}

/// Runs in every document before the mail, as an app script: the CSP and the page-script switch
/// do not apply to it. Clicks inside the shadow root reach `document` retargeted to the host,
/// so the link comes from the event path.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const BRIDGE: &str = r#"
addEventListener('click', e => {
  const a = e.composedPath().find(n => (n.tagName === 'A' || n.tagName === 'AREA') && typeof n.href === 'string' && n.href);
  if (a) { e.preventDefault(); if (e.isTrusted) window.ipc.postMessage('open ' + a.href); }
}, true);
addEventListener('keydown', e => { if (e.key === 'Escape') window.ipc.postMessage('escape'); }, true);
"#;

fn placement(bounds: Bounds<Pixels>, mask: Bounds<Pixels>) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let mut clip = bounds.intersect(&mask);
    clip.size.width = clip.size.width.max(px(0.));
    clip.size.height = clip.size.height.max(px(0.));
    (bounds, clip)
}

/// (message id, HTML length, remote images, dark): the document on screen.
type Key = (usize, usize, bool, bool);

/// The window's web view and what it shows.
#[derive(Default)]
pub(super) struct Web {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    view: Option<Rc<native::View>>,
    /// Set just before the app loads a document: the one navigation the web view may make.
    ours: Rc<std::cell::Cell<bool>>,
    /// The document asked for last.
    shown: Option<Key>,
    /// A document finished loading since the view was last hidden for plain text.
    ready: bool,
    visible: Rc<std::cell::Cell<bool>>,
    drawable: Rc<std::cell::Cell<bool>>,
    /// The shown mail has remote images.
    pub(super) has_remote: bool,
    /// Bounds and scale factor last applied: a move to a display with another scale needs new
    /// device-pixel bounds on Windows.
    place: Rc<std::cell::Cell<Option<(Bounds<Pixels>, Bounds<Pixels>, f32)>>>,
}

/// A window with a mail web view: the main window and each message window.
pub(super) trait WebHost: 'static {
    fn web(&mut self) -> &mut Web;
    /// Where the keys go when Esc leaves the web view.
    fn keys(&self) -> FocusHandle;
}

impl WebHost for MainWindow {
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
        let want = self.inbox.current().filter(|_| matches!(self.page, Page::Mail) && !empty && (!self.inbox.group || self.reading_members.len() <= 1)).map(|id| (id, &self.inbox.mail[id]));
        let overlay = self.palette.is_some() || self.compose.is_some() || !self.sending.is_empty();
        self.web.frame(want, &self.theme, overlay, window, cx);
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
        if !SUPPORTED {
            return;
        }
        let Some((id, m, html)) = want.and_then(|(id, m)| m.html.as_ref().map(|h| (id, m, h))) else {
            // A cold web content process takes about a second to start: start it with the window,
            // on an empty page, so the first HTML message opens as fast as the rest.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if self.view.is_none() {
                self.view = Self::create(self.ours.clone(), theme, window, cx).map(Rc::new);
                if let Some(view) = &self.view {
                    self.ours.set(view.load_html("<!DOCTYPE html><title></title>").is_ok());
                }
            }
            // Plain text: the next HTML message waits for its own load, never shows a stale one.
            self.shown = None;
            self.ready = false;
            self.drawable.set(false);
            self.set_visible(false);
            return;
        };
        let key = (id, html.len(), m.remote, theme.dark);
        if self.shown != Some(key) {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                if self.view.is_none() {
                    self.view = Self::create(self.ours.clone(), theme, window, cx).map(Rc::new);
                }
                let Some(view) = self.view.clone() else { return };
                let doc = document(html, theme, m.remote);
                self.ours.set(true);
                let _ = view.set_background_color(rgba8(theme.bg));
                if view.load_html(&doc).is_err() {
                    self.ours.set(false);
                }
            }
            self.shown = Some(key);
            self.has_remote = has_remote(html);
            // Shown already: the previous message stays until this one has loaded, no flash.
            self.ready &= self.visible.get();
        }
        self.drawable.set(self.ready && !overlay);
        // Native children outside the scroll viewport must stay hidden until their slot is painted.
        self.set_visible(false);
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
            Msg::Loaded => host.web().ready = true,
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
    fn create<T: WebHost>(ours: Rc<std::cell::Cell<bool>>, theme: &Theme, window: &mut Window, cx: &mut Context<T>) -> Option<native::View> {
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
            .with_initialization_script(BRIDGE)
            .with_navigation_handler(move |_| ours.replace(false))
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
                    _ if body == "escape" => Msg::Escape,
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
        assert_eq!(csp_of(&doc), "default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'");
        // The wrapper itself loads nothing and runs nothing.
        assert!(!doc.contains("://") && !doc.contains("<script") && !doc.contains("<link"));
        // Remote images, when asked for: images only.
        let doc = document("", &Theme::light(DEFAULT_ACCENT), true);
        assert_eq!(csp_of(&doc), "default-src 'none'; img-src data: cid: https: http:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'");
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
        assert!(!doc.contains("background:#fff") && doc.contains("a{color:rgba(235,106,85,1)}"));
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

//! HTML mail in the reading pane. One platform web view per window (WKWebView on macOS, WebView2
//! on Windows) covers the pane between its toolbar and the reply field. The document holds the
//! message header and the mail, so the pane scrolls as one, natively.
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

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn css(c: Rgba) -> String {
    let ch = |v: f32| (v * 255.).round() as u8;
    format!("rgba({},{},{},{})", ch(c.r), ch(c.g), ch(c.b), c.a)
}

/// The document for one message: the header as the GPUI pane draws it, then the mail in a
/// shadow root, so its CSS cannot restyle the header.
// shortcut: a `</template>` in the mail ends the shadow root early; the rest renders unscoped,
// still under the CSP. Parse the mail into a fragment if that matters.
pub(super) fn document(m: &Mail, html: &str, t: &Theme, remote: bool) -> String {
    let initials: String = m.from.split_whitespace().take(2).filter_map(|w| w.chars().next()).collect();
    let to = if m.to.is_empty() { "you" } else { &m.to };
    let card = styled(html);
    let (bg, ink, ink2, line, subtle, accent) = (css(t.bg), css(t.ink), css(t.ink2), css(t.line), css(t.subtle), css(t.accent));
    let scheme = if t.dark { "dark" } else { "light" };
    // Mail that sets its own colours keeps white paper; in dark mode that is a rounded card.
    let mail_css = match (card, t.dark) {
        (true, true) => "background:#fff;color:#000;border-radius:10px;padding:16px;",
        (true, false) => "color:#000;",
        (false, _) => "",
    };
    // Unstyled HTML reads like the plain-text pane: theme ink, accent links, images to width.
    let inner_css = if card { String::new() } else { format!("a{{color:{accent}}}img{{max-width:100%;height:auto}}") };
    let attachment = m.attachment.as_ref().map_or(String::new(), |(name, kb)| {
        format!(r#"<div class="att"><span class="name">{}</span><span class="small">{}</span></div>"#, escape(name), escape(kb))
    });
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{csp}"><meta name="color-scheme" content="{scheme}"><style>
html,body{{margin:0;background:{bg};color:{ink};font:13px/1.25 -apple-system,system-ui,"Segoe UI",sans-serif}}
main{{box-sizing:border-box;max-width:760px;padding:24px 32px 20px}}
h1{{margin:0;font-size:20px;line-height:25px;font-weight:600}}
.from{{display:flex;align-items:center;gap:12px;margin-top:12px}}
.avatar{{box-sizing:border-box;width:32px;height:32px;flex:none;border-radius:50%;background:{subtle};border:1px solid {line};display:flex;align-items:center;justify-content:center;font-size:11px;font-weight:600}}
.who{{flex:1;min-width:0;display:flex;flex-direction:column;gap:1px}}
.name{{font-weight:600}}
.small{{font-size:11px;color:{ink2};font-variant-numeric:tabular-nums}}
.addr{{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
.mail{{display:block;margin-top:20px;font-size:14px;line-height:1.4;overflow-wrap:break-word;{mail_css}}}
.att{{display:inline-flex;align-items:center;gap:12px;margin-top:12px;padding:10px 14px;border-radius:10px;border:1px solid {line};background:{subtle};font-size:12px}}
.att .name{{font-weight:500}}
</style></head><body><main><h1>{subject}</h1><div class="from"><div class="avatar">{initials}</div><div class="who"><div class="name">{from}</div><div class="small addr">{address} → {to}</div></div><div class="small">{time}</div></div><div class="mail"><template shadowrootmode="open"><style>{inner_css}</style>{html}</template></div>{attachment}</main></body></html>"#,
        csp = csp(remote),
        subject = escape(&m.subject),
        initials = escape(&initials.to_uppercase()),
        from = escape(&m.from),
        address = escape(&m.address),
        to = escape(to),
        time = escape(&m.time),
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
  if (a) { e.preventDefault(); window.ipc.postMessage('open ' + a.href); }
}, true);
addEventListener('keydown', e => { if (e.key === 'Escape') window.ipc.postMessage('escape'); }, true);
"#;

/// (message id, HTML length, remote images, dark): the document on screen.
type Key = (usize, usize, bool, bool);

/// The window's web view and what it shows.
#[derive(Default)]
pub(super) struct Web {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    view: Option<Rc<wry::WebView>>,
    /// Set just before the app loads a document: the one navigation the web view may make.
    ours: Rc<std::cell::Cell<bool>>,
    /// The document asked for last.
    shown: Option<Key>,
    /// A document finished loading since the view was last hidden for plain text.
    ready: bool,
    visible: bool,
    /// The shown mail has remote images.
    pub(super) has_remote: bool,
    place: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

impl MainWindow {
    /// Before a frame: load the selected HTML message, and show the web view once it has loaded.
    /// Overlays draw in GPUI, under any native view, so the web view hides while one is open.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_variables))]
    pub(super) fn web_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !SUPPORTED {
            return;
        }
        let empty = self.inbox.list().is_empty() && self.inbox.open.is_none();
        let want = self
            .inbox
            .current()
            .filter(|_| matches!(self.page, Page::Mail) && !empty)
            .and_then(|id| self.inbox.mail[id].html.as_ref().map(|h| (id, h.len())));
        let overlay = self.palette.is_some() || self.compose.is_some() || !self.sending.is_empty();
        let Some((id, len)) = want else {
            // A cold web content process takes about a second to start: start it with the window,
            // on an empty page, so the first HTML message opens as fast as the rest.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if self.web.view.is_none() {
                self.web.view = self.web_create(window, cx).map(Rc::new);
                if let Some(view) = &self.web.view {
                    self.web.ours.set(view.load_html("<!DOCTYPE html><title></title>").is_ok());
                }
            }
            // Plain text: the next HTML message waits for its own load, never shows a stale one.
            self.web.shown = None;
            self.web.ready = false;
            self.web_visible(false);
            return;
        };
        let key = (id, len, self.inbox.mail[id].remote, self.theme.dark);
        if self.web.shown != Some(key) {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                if self.web.view.is_none() {
                    self.web.view = self.web_create(window, cx).map(Rc::new);
                }
                let Some(view) = self.web.view.clone() else { return };
                let m = &self.inbox.mail[id];
                let doc = document(m, m.html.as_ref().map_or("", |h| h.as_ref()), &self.theme, m.remote);
                self.web.ours.set(true);
                let _ = view.set_background_color(rgba8(self.theme.bg));
                if view.load_html(&doc).is_err() {
                    self.web.ours.set(false);
                }
            }
            self.web.shown = Some(key);
            self.web.has_remote = has_remote(self.inbox.mail[id].html.as_ref().map_or("", |h| h.as_ref()));
            // Shown already: the previous message stays until this one has loaded, no flash.
            self.web.ready &= self.web.visible;
        }
        self.web_visible(self.web.ready && !overlay);
    }

    fn web_visible(&mut self, visible: bool) {
        if self.web.visible == visible {
            return;
        }
        self.web.visible = visible;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(view) = &self.web.view {
            let _ = view.set_visible(visible);
        }
    }

    /// The element that keeps the web view over the body area: its bounds every frame.
    pub(super) fn web_slot(&self) -> impl IntoElement {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let view = self.web.view.clone();
        let place = self.web.place.clone();
        gpui::canvas(
            move |bounds, _, _| {
                if place.get() == Some(bounds) {
                    return;
                }
                place.set(Some(bounds));
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                if let Some(view) = &view {
                    use wry::dpi::{LogicalPosition, LogicalSize};
                    let (o, s) = (bounds.origin, bounds.size);
                    let _ = view.set_bounds(wry::Rect {
                        position: LogicalPosition::new(f64::from(f32::from(o.x)), f64::from(f32::from(o.y))).into(),
                        size: LogicalSize::new(f64::from(f32::from(s.width)), f64::from(f32::from(s.height))).into(),
                    });
                }
            },
            |_, _, _, _| {},
        )
        .size_full()
    }

    /// A click anywhere in GPUI takes the keys back from the web view. GPUI's view does not take
    /// first responder on a click by itself.
    pub(super) fn web_unfocus(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(view) = &self.web.view {
            let _ = view.focus_parent();
        }
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    fn web_event(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Loaded => self.web.ready = true,
            Msg::Open(url) if link_allowed(&url) => cx.open_url(&url),
            Msg::Open(_) => {}
            Msg::Escape => {
                self.web_unfocus();
                window.focus(&self.focus);
            }
        }
        cx.notify();
    }

    /// The web view, hidden and unfocused, with page scripts, navigation and downloads off and
    /// nothing kept on disk.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn web_create(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<wry::WebView> {
        // GPUI's test windows have no native handle.
        if cfg!(test) {
            return None;
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ours = self.web.ours.clone();
        let (load_tx, ipc_tx) = (tx.clone(), tx);
        let view = wry::WebViewBuilder::new()
            .with_javascript_disabled()
            .with_incognito(true)
            .with_focused(false)
            .with_visible(false)
            .with_background_color(rgba8(self.theme.bg))
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
                if this.update_in(cx, |this, window, cx| this.web_event(msg, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Some(view)
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

    fn sample() -> Mail {
        Mail { from: "Ann <b>Bold</b>".into(), address: "ann@example.com".into(), subject: "<script>alert(1)</script>".into(), ..Default::default() }
    }

    #[test]
    fn the_document_blocks_scripts_and_remote_loads() {
        let doc = document(&sample(), "<p>Hi</p>", &Theme::light(DEFAULT_ACCENT), false);
        // The CSP comes before anything from the mail, and allows no network source.
        let csp = doc.find("Content-Security-Policy").unwrap();
        assert!(csp < doc.find("<p>Hi</p>").unwrap());
        assert!(doc.contains("default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'"));
        assert!(!csp_of(&doc).contains("http"));
        // The wrapper itself loads nothing and runs nothing.
        assert!(!doc.contains("://") && !doc.contains("<script") && !doc.contains("<link"));
        // Header text is escaped.
        assert!(doc.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && doc.contains("Ann &lt;b&gt;Bold&lt;/b&gt;"));
        // Remote images, when asked for: images only.
        let doc = document(&sample(), "", &Theme::light(DEFAULT_ACCENT), true);
        assert_eq!(csp_of(&doc), "default-src 'none'; img-src data: cid: https: http:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'");
    }

    fn csp_of(doc: &str) -> &str {
        let start = doc.find("Content-Security-Policy\" content=\"").unwrap() + 34;
        &doc[start..start + doc[start..].find('"').unwrap()]
    }

    #[test]
    fn styled_mail_gets_white_paper_in_dark_mode() {
        let dark = Theme::dark(DEFAULT_ACCENT);
        assert!(document(&sample(), r##"<table bgcolor="#f4f4f4">"##, &dark, false).contains("background:#fff;color:#000;border-radius:10px"));
        // Simple HTML blends: theme ink, accent links.
        let doc = document(&sample(), "<div dir=ltr>Lunch?</div>", &dark, false);
        assert!(!doc.contains("background:#fff") && doc.contains("a{color:rgba(235,106,85,1)}"));
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

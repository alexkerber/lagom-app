//! Shared privacy policy for native mail web views.

/// The CSP for a mail document. `base-uri` and `form-action` do not fall back to `default-src`.
/// `local` is the origin that serves the message's own inline parts (desktop: the lagom-cid protocol).
pub fn csp(remote: bool, local: &str) -> String {
    let remote = if remote { " https: http:" } else { "" };
    format!("default-src 'none'; img-src data: {local}{remote}; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'")
}

/// Links open outside only for these schemes.
pub fn link_allowed(url: &str) -> bool {
    let scheme = url.split_once(':').map_or("", |s| s.0);
    ["http", "https", "mailto"].iter().any(|s| s.eq_ignore_ascii_case(scheme))
}

/// The URL a native view fetches for a remote `<img src>` after "Load remote images": the
/// `https: http:` of `csp`, with a scheme-relative `//host/x` as `https:`. `None` for others.
pub fn remote_image(src: &str) -> Option<String> {
    let scheme = src.split_once(':').map_or("", |s| s.0);
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") { return Some(src.into()); }
    src.starts_with("//").then(|| format!("https:{src}"))
}

/// The bytes of a base64 `data:` image, the `data:` of `csp`. At most about 10 MiB.
pub fn data_image(src: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let (meta, data) = src.get(..5).filter(|s| s.eq_ignore_ascii_case("data:")).and_then(|_| src[5..].split_once(','))?;
    if !meta.to_ascii_lowercase().ends_with(";base64") || data.len() > crate::inline::MAX_INLINE_BYTES / 3 * 4 + 4096 { return None; }
    let data: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// The media type of raster image bytes, by their signature. SVG and anything else is `None`.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'B', b'M', ..] => "image/bmp",
        _ => return None,
    })
}

/// Whether a remote image may come from this address: not this machine, the local network,
/// link-local, multicast or reserved space. Mail must not reach a router or a local service.
fn public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
                || v4.is_documentation() || v4.is_multicast() || a == 0 || a >= 240 || (a == 100 && (64..128).contains(&b)) || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let v4 = |hi: u16, lo: u16| std::net::Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
            match s {
                // IPv4-mapped, and NAT64 (64:ff9b::/96) and 6to4 (2002::/16), which reach the IPv4 address they hold.
                [0, 0, 0, 0, 0, 0xffff, hi, lo] | [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => public_ip(v4(hi, lo).into()),
                [0x2002, hi, lo, ..] => public_ip(v4(hi, lo).into()),
                // IPv4-compatible (::/96, loopback and unspecified too) and local-use NAT64 64:ff9b:1::/48.
                [0, 0, 0, 0, 0, 0, ..] | [0x64, 0xff9b, 1, ..] => false,
                // Multicast, unique local fc00::/7, link-local fe80::/10, site-local fec0::/10.
                [first, ..] => !((first & 0xff00) == 0xff00 || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || (first & 0xffc0) == 0xfec0),
            }
        }
    }
}

/// A URL a remote image may load from, first or after a redirect: http or https, a public IP
/// when the host is one, never `localhost`. Names resolve through `PublicDns`.
fn image_url_allowed(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https") && match url.host() {
        Some(url::Host::Ipv4(ip)) => public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => public_ip(ip.into()),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name != "localhost" && !name.ends_with(".localhost")
        }
        None => false,
    }
}

/// A redirect for a remote image: at most five, never from https down to http. `previous`
/// holds the first URL too, as reqwest gives it: five redirects leave five there.
fn redirect_allowed(previous: &[reqwest::Url], next: &reqwest::Url) -> bool {
    previous.len() <= 5 && image_url_allowed(next) && !(next.scheme() == "http" && previous.iter().any(|u| u.scheme() == "https"))
}

/// DNS for remote images that drops private addresses at every lookup, redirects included, so
/// a name cannot rebind to the local network between checks.
struct PublicDns;

impl reqwest::dns::Resolve for PublicDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<_> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| public_ip(a.ip())).collect();
            if addrs.is_empty() { return Err("no public address".into()); }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A remote image for a native view, after "Load remote images": no cookies, referrer or
/// proxy, at most five redirects, 20 seconds and `limit` bytes (at most 10 MiB), public
/// addresses only, raster formats only. `used` counts the bytes received, also when the fetch
/// fails. Needs a Tokio runtime.
pub async fn fetch_image(url: &str, limit: usize, used: &mut usize) -> Option<(&'static str, Vec<u8>)> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| reqwest::Client::builder().use_rustls_tls()
        .timeout(std::time::Duration::from_secs(20))
        .referer(false)
        // A proxy would resolve the name itself, past `PublicDns`.
        .no_proxy()
        .dns_resolver(std::sync::Arc::new(PublicDns))
        .redirect_policy(reqwest::redirect::Policy::custom(|attempt| {
            if redirect_allowed(attempt.previous(), attempt.url()) { attempt.follow() } else { attempt.error("redirect refused") }
        }))
        .build().ok()).as_ref()?;
    let url = reqwest::Url::parse(url).ok().filter(image_url_allowed)?;
    let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let limit = limit.min(crate::inline::MAX_INLINE_BYTES);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        *used += chunk.len();
        if bytes.len() + chunk.len() > limit { return None; }
        bytes.extend_from_slice(&chunk);
    }
    Some((image_type(&bytes)?, bytes))
}

/// Whether the mail sets its own colours or backgrounds. Such mail assumes white paper; other
/// HTML takes the theme's ink and background.
pub fn styled(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("color") || lower.contains("background")
}

/// Whether the mail has remote images or backgrounds, which "Load remote images" would fetch.
pub fn has_remote(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    ["src=", "srcset=", "background=", "url("].iter().any(|k| {
        lower.match_indices(k).any(|(i, _)| {
            let rest = lower[i + k.len()..].trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
            rest.starts_with("http:") || rest.starts_with("https:") || rest.starts_with("//")
        })
    })
}

/// `<link>`, `<iframe>`, `<frame>`, `<object>` and `<embed>` (open and close tags) become unknown,
/// inert elements. Resource hints (preconnect, dns-prefetch) open connections that the CSP does
/// not cover. A nested document (`srcdoc`, entity-encoded) or a plugin parses markup that this
/// scan never sees as a tag. The CSP blocks stylesheets and frame loads anyway.
pub fn inert_tags(html: &str) -> std::borrow::Cow<'_, str> {
    const INERT: [&[u8]; 5] = [b"link", b"iframe", b"frame", b"object", b"embed"];
    let bytes = html.as_bytes();
    // The index after `<` or `</` when an inert tag name follows: always an ASCII boundary.
    let hit = |i: usize| {
        let at = if bytes.get(i + 1) == Some(&b'/') { i + 2 } else { i + 1 };
        (bytes[i] == b'<' && INERT.iter().any(|n| bytes.get(at..at + n.len()).is_some_and(|w| w.eq_ignore_ascii_case(n)))).then_some(at)
    };
    if !(0..bytes.len()).any(|i| hit(i).is_some()) {
        return html.into();
    }
    let mut out = String::with_capacity(html.len() + 16);
    let mut last = 0;
    for at in (0..bytes.len()).filter_map(hit) {
        out.push_str(&html[last..at]);
        out.push_str("x-");
        last = at;
    }
    out.push_str(&html[last..]);
    out.into()
}

/// Text for HTML: `&`, `<`, `>` and `"` escaped, so it is safe in text and quoted attributes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Someone else's mail HTML, made safe to quote in mail lagom sends and in the compose preview.
/// An allowlist rebuilt from html5ever's tokens: nothing from the input reaches the output
/// unescaped. Scripts, styles, frames, forms, plugins, SVG and MathML go with their content;
/// unknown tags go and keep their text. Attributes are an allowlist: no event handlers, ids or
/// classes; links only http, https and mailto; images only http, https and raster `data:`.
/// Every element is closed, so the quote cannot close the markup around it.
pub fn sanitize(html: &str) -> String {
    clean(html, None).0
}

/// `sanitize`, with each `data:` image moved to an inline part that `src` names as `cid:`.
pub fn sanitize_parts(html: &str) -> (String, Vec<crate::inline::InlinePart>) {
    clean(html, Some(Vec::new()))
}

const KEEP: &[&str] = &["a", "abbr", "address", "article", "aside", "b", "bdi", "bdo", "big", "blockquote", "br", "caption", "center",
    "cite", "code", "col", "colgroup", "dd", "del", "details", "dfn", "div", "dl", "dt", "em", "figcaption", "figure", "font", "footer",
    "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "i", "img", "ins", "kbd", "li", "main", "mark", "nav", "ol", "p", "pre", "q", "s",
    "samp", "section", "small", "span", "strike", "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead",
    "time", "tr", "tt", "u", "ul", "var", "wbr"];
const VOID: &[&str] = &["br", "col", "hr", "img", "wbr"];
/// Dropped with everything inside them.
const DROP: &[&str] = &["script", "style", "title", "textarea", "select", "iframe", "frameset", "object", "applet", "noscript",
    "noembed", "noframes", "template", "svg", "math", "xmp", "plaintext", "audio", "video", "canvas", "map"];
/// Deeper elements go and keep their text: WebKit's parser stops nesting there too, and the end-tag scan stays linear.
const MAX_DEPTH: usize = 512;
/// Distinct `data:` images a quote keeps: the byte cap alone let 80,000 tiny images cost quadratic work and as many
/// MIME parts (Codex review 2026-11-20). Newsletters stay well under it.
const MAX_DATA_IMAGES: usize = 100;
const ATTRS: &[&str] = &["align", "valign", "width", "height", "bgcolor", "color", "face", "size", "border", "cellpadding",
    "cellspacing", "colspan", "rowspan", "dir", "lang", "title", "alt", "start", "type", "reversed", "span", "nowrap", "scope",
    "headers", "abbr", "datetime", "hspace", "vspace", "frame", "rules", "summary", "clear", "noshade", "compact", "open"];

#[derive(Default)]
struct Clean {
    out: String,
    open: Vec<String>,
    /// Inside a dropped element: its name and how deep.
    skip: Option<(String, usize)>,
    parts: Option<Vec<crate::inline::InlinePart>>,
    /// The `data:` image sources kept so far, so the quote stays under `MAX_INLINE_BYTES`.
    data_bytes: usize,
    /// The distinct `data:` images kept so far and the source each got, at most `MAX_DATA_IMAGES`.
    kept: Vec<(Vec<u8>, String)>,
}

struct CleanSink(std::cell::RefCell<Clean>);

fn clean(html: &str, parts: Option<Vec<crate::inline::InlinePart>>) -> (String, Vec<crate::inline::InlinePart>) {
    use html5ever::tokenizer::{BufferQueue, Tokenizer};
    let queue = BufferQueue::default();
    queue.push_back(html.into());
    let tokenizer = Tokenizer::new(CleanSink(Clean { parts, ..Default::default() }.into()), Default::default());
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    let state = tokenizer.sink.0.into_inner();
    (state.out, state.parts.unwrap_or_default())
}

impl html5ever::tokenizer::TokenSink for CleanSink {
    type Handle = ();
    fn process_token(&self, token: html5ever::tokenizer::Token, _line: u64) -> html5ever::tokenizer::TokenSinkResult<()> {
        use html5ever::tokenizer::{Token, TokenSinkResult};
        let mut state = self.0.borrow_mut();
        match token {
            Token::TagToken(tag) => return state.tag(tag),
            Token::CharacterTokens(text) if state.skip.is_none() => {
                let text = escape(&text);
                state.out.push_str(&text);
            }
            Token::EOFToken => {
                while let Some(name) = state.open.pop() { state.out.push_str(&format!("</{name}>")); }
            }
            // Comments (IE conditional comments too), doctypes, NUL.
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

impl Clean {
    fn tag(&mut self, tag: html5ever::tokenizer::Tag) -> html5ever::tokenizer::TokenSinkResult<()> {
        use html5ever::tokenizer::{TagKind, TokenSinkResult};
        let name: &str = &tag.name;
        let start = tag.kind == TagKind::StartTag;
        if let Some((skip, depth)) = &mut self.skip {
            if skip == name {
                if start && !tag.self_closing { *depth += 1 } else if !start { *depth -= 1 }
                if *depth == 0 { self.skip = None; }
            }
            return TokenSinkResult::Continue;
        }
        if DROP.contains(&name) {
            // `<svg/>` closes itself; `<script/>` does not, in HTML.
            if start && !(tag.self_closing && matches!(name, "svg" | "math")) {
                self.skip = Some((name.to_owned(), 1));
                return match crate::mime::raw_kind(&tag.name) {
                    Some(Some(kind)) => TokenSinkResult::RawData(kind),
                    Some(None) => TokenSinkResult::Plaintext,
                    None => TokenSinkResult::Continue,
                };
            }
            return TokenSinkResult::Continue;
        }
        if !KEEP.contains(&name) { return TokenSinkResult::Continue; }
        if !start {
            if let Some(at) = self.open.iter().rposition(|n| n == name) {
                for open in self.open.drain(at..).rev() { self.out.push_str(&format!("</{open}>")); }
            }
            return TokenSinkResult::Continue;
        }
        let mut attrs = String::new();
        let mut has_src = false;
        for attr in &tag.attrs {
            if !attr.name.ns.is_empty() { continue; }
            let (key, value): (&str, &str) = (&attr.name.local, &attr.value);
            let value = match key {
                "href" if name == "a" => Some(value.trim()).filter(|v| link_allowed(v)).map(str::to_owned),
                "src" if name == "img" => self.image(value.trim()),
                "background" => web_url(value.trim()),
                "style" => style(value),
                _ if ATTRS.contains(&key) => Some(value.to_owned()),
                _ => None,
            };
            if let Some(value) = value {
                has_src |= key == "src";
                attrs.push_str(&format!(" {key}=\"{}\"", escape(&value)));
            }
        }
        // An image with nothing safe to show goes, rather than a broken-image box.
        if name == "img" && !has_src { return TokenSinkResult::Continue; }
        if !VOID.contains(&name) && self.open.len() >= MAX_DEPTH { return TokenSinkResult::Continue; }
        self.out.push_str(&format!("<{name}{attrs}>"));
        if !VOID.contains(&name) { self.open.push(name.to_owned()); }
        TokenSinkResult::Continue
    }

    fn image(&mut self, src: &str) -> Option<String> {
        if src.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:")) {
            let data = data_image(src)?;
            let mime = image_type(&data)?;
            // One source per distinct image, however often the quote shows it, on both paths; at most
            // MAX_DATA_IMAGES distinct ones, so this scan stays short.
            if let Some((_, kept)) = self.kept.iter().find(|(seen, _)| *seen == data) { return Some(kept.clone()); }
            if self.kept.len() >= MAX_DATA_IMAGES || self.data_bytes + data.len() > crate::inline::MAX_INLINE_BYTES { return None; }
            self.data_bytes += data.len();
            let source = match &mut self.parts {
                Some(parts) => {
                    let cid = format!("quote{}@lagommail.app", parts.len() + 1);
                    parts.push(crate::inline::InlinePart { cid: cid.clone(), mime_type: mime.into(), data: data.clone() });
                    format!("cid:{cid}")
                }
                // The sniffed type, never the one the sender wrote.
                None => { use base64::Engine; format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&data)) }
            };
            self.kept.push((data, source.clone()));
            return Some(source);
        }
        web_url(src)
    }
}

/// An absolute http or https URL, as written.
fn web_url(url: &str) -> Option<String> {
    let scheme = url.split_once(':').map_or("", |s| s.0);
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")).then(|| url.to_owned())
}

/// Inline CSS without script hooks (`expression`, `behavior`, bindings, escapes) or `position`,
/// which would let the quote paint over the reply in the recipient's mail. Images in CSS are web
/// URLs only: a `data:` SVG background would draw what `<img>` refuses.
fn style(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if lower.contains('\\') || ["expression", "javascript:", "vbscript:", "behavior", "-moz-binding", "@import", "/*"].iter().any(|b| lower.contains(b)) {
        return None;
    }
    let kept: Vec<_> = value.split(';').filter(|d| d.split_once(':').is_some_and(|(p, _)| {
        let p = p.trim().to_ascii_lowercase();
        !p.is_empty() && p != "position" && css_images_web(d)
    })).map(str::trim).collect();
    (!kept.is_empty()).then(|| kept.join(";"))
}

/// Every `url(…)` in a declaration is http or https, and it has no other image function.
fn css_images_web(declaration: &str) -> bool {
    let lower = declaration.to_ascii_lowercase();
    !["image-set(", "image(", "cross-fade(", "element("].iter().any(|f| lower.contains(f))
        && lower.split("url(").skip(1).all(|rest| {
            let url = rest.trim_start().trim_start_matches(['"', '\'']);
            url.starts_with("http://") || url.starts_with("https://")
        })
}

#[cfg(test)]
mod tests {
    use super::{data_image, image_type, image_url_allowed, inert_tags, public_ip, redirect_allowed, remote_image, sanitize, sanitize_parts};

    #[test]
    fn sanitize_drops_scripts_handlers_frames_forms_and_refresh() {
        let evil = r#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.example"><title>T</title><style>p{color:red}</style><base href="https://evil.example/"></head>
<body onload="alert(1)"><script>alert(2)</script><p onclick="x()" id="a" class="b" style="color:#333;position:fixed">Hi <b>there</b></p>
<iframe src="https://evil.example"></iframe><iframe srcdoc="&lt;script&gt;"></iframe><form action="https://evil.example"><input name=q><button>Go</button></form>
<a href="javascript:alert(3)">js</a><a href=" https://ok.example/x?a=1&amp;b=2 ">ok</a><a href="java&#x09;script:alert(4)">tab</a>
<img src="x" onerror="alert(5)"><img src="https://img.example/a.png" alt="A"><svg><script>alert(6)</script></svg><object data="x"></object><embed src="x">
<div style="background:url(javascript:alert(7))">bg</div><div style="width:expression(alert(8))">ie</div><script/>alert(9)</script>after"#;
        let out = sanitize(evil);
        let lower = out.to_ascii_lowercase();
        for bad in ["<script", "alert(", "onload", "onclick", "onerror", "<iframe", "srcdoc", "<form", "<input", "<meta", "refresh", "<base",
            "<style", "<object", "<embed", "<svg", "javascript", "expression", "position", " id=", " class=", "<title", "action="] {
            assert!(!lower.contains(bad), "{bad} in {out}");
        }
        for good in ["<p style=\"color:#333\">Hi <b>there</b></p>", "<a href=\"https://ok.example/x?a=1&amp;b=2\">ok</a>",
            "<img src=\"https://img.example/a.png\" alt=\"A\">", "Go", "after", "<div>bg</div>"] {
            assert!(out.contains(good), "{good} missing from {out}");
        }
        assert!(!out.contains("src=\"x\""), "a relative image goes: {out}");
    }

    #[test]
    fn sanitize_keeps_tables_and_closes_what_it_opens() {
        let table = r##"<table width="600" cellpadding="0" style="border-collapse:collapse"><tr><td bgcolor="#f4f4f4" align="center" colspan="2">Cell &lt;1&gt;</td></tr></table>"##;
        assert_eq!(sanitize(table), r##"<table width="600" cellpadding="0" style="border-collapse:collapse"><tr><td bgcolor="#f4f4f4" align="center" colspan="2">Cell &lt;1&gt;</td></tr></table>"##);
        // A quote cannot close the blockquote it is placed in, nor leave an element open after it.
        assert_eq!(sanitize("</blockquote></div><div><b>open"), "<div><b>open</b></div>");
        assert_eq!(sanitize("<p title='\"><script>x</script>'>t</p>"), "<p title=\"&quot;&gt;&lt;script&gt;x&lt;/script&gt;\">t</p>");
        assert_eq!(sanitize(&sanitize(table)), sanitize(table), "sanitizing twice changes nothing");
    }

    #[test]
    fn sanitize_moves_data_images_to_parts() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        let html = format!("<img src=\"data:image/png;base64,{png}\" alt=logo><img src=\"data:image/svg+xml;base64,PHN2Zy8+\">");
        assert_eq!(sanitize(&html), format!("<img src=\"data:image/png;base64,{png}\" alt=\"logo\">"), "SVG data goes");
        let (out, parts) = sanitize_parts(&html);
        assert_eq!(out, "<img src=\"cid:quote1@lagommail.app\" alt=\"logo\">");
        assert_eq!((parts.len(), parts[0].mime_type.as_str(), parts[0].data[..4].to_vec()), (1, "image/png", vec![0x89, b'P', b'N', b'G']));
        assert_eq!(sanitize("<img src=\"cid:logo@x\">"), "", "an unresolved inline image goes");
        let (out, parts) = sanitize_parts(&format!("<img src=\"data:image/png;base64,{png}\">").repeat(3));
        assert_eq!((parts.len(), out.matches("cid:quote1@lagommail.app").count()), (1, 3), "one part per image, however often it shows");
        let claimed = sanitize(&format!("<img src=\"data:text/html;base64,{png}\">"));
        assert!(claimed.contains("data:image/png;base64,"), "the sniffed type, not the claimed one: {claimed}");
    }

    #[test]
    fn many_tiny_data_images_are_capped_in_count() {
        use base64::Engine;
        // Codex review 2026-11-20: 80,000 distinct eight-byte "PNG"s, under the byte cap.
        let html: String = (0u32..80_000).map(|i| {
            let mut png = b"\x89PNG".to_vec();
            png.extend_from_slice(&i.to_be_bytes());
            format!("<img src=\"data:image/png;base64,{}\">", base64::engine::general_purpose::STANDARD.encode(png))
        }).collect();
        let (out, parts) = sanitize_parts(&html);
        assert_eq!((parts.len(), out.matches("<img").count()), (super::MAX_DATA_IMAGES, super::MAX_DATA_IMAGES));
        assert_eq!(sanitize(&html).matches("<img").count(), super::MAX_DATA_IMAGES, "the preview keeps the same cap");
        // The cap bounds the work too: each image is compared with at most MAX_DATA_IMAGES kept ones.
        // Repeats count once, on both paths (Codex re-review): 150 copies of A, then B, keep B.
        let image = |byte: u8| format!("<img src=\"data:image/png;base64,{}\">", base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G', byte]));
        let repeated = format!("{}{}", image(1).repeat(150), image(2));
        assert_eq!(sanitize(&repeated).matches("<img").count(), 151, "the preview keeps the second image");
        let (out, parts) = sanitize_parts(&repeated);
        assert_eq!((parts.len(), out.matches("<img").count()), (2, 151), "sending keeps both, as two parts");
    }

    #[test]
    fn sanitize_bounds_depth_css_images_and_keeps_bodies_after_an_open_head() {
        assert_eq!(sanitize("<html><head><meta charset=utf-8><body><p>Hello</p>"), "<p>Hello</p>", "an unclosed head keeps the body");
        assert_eq!(sanitize("<svg><svg/></svg><p>after</p>"), "<p>after</p>", "a self-closed child ends no skip early or late");
        let deep = sanitize(&format!("{}x{}", "<div>".repeat(2000), "</b>".repeat(2000)));
        assert_eq!(deep.matches("<div>").count(), 512);
        assert_eq!(deep.matches("</div>").count(), 512, "balanced");
        let css = sanitize(r#"<div style="color:red;background:url(data:image/svg+xml;base64,PHN2Zy8+);background-image:image-set('x.svg' 1x)">a</div><div style="background:url( 'https://img.example/a.png' )">b</div>"#);
        assert!(css.contains("style=\"color:red\"") && !css.contains("data:") && !css.contains("image-set"), "{css}");
        assert!(css.contains("https://img.example/a.png"), "{css}");
    }
    use std::borrow::Cow;

    #[test]
    fn remote_images_reach_only_public_hosts() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "fe80::1", "fd00::1", "::ffff:192.168.1.1", "::",
            "64:ff9b::7f00:1", "64:ff9b::a00:1", "2002:c0a8:101::1", "2002:7f00:1::", "::7f00:1", "::808:808", "fec0::1", "64:ff9b:1::808:808", "ff02::1"] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "8.8.8.8", "2606:4700::1111", "64:ff9b::808:808", "2002:808:808::1"] { assert!(public_ip(ip.parse().unwrap()), "{ip}"); }
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        for bad in ["http://127.0.0.1/x.png", "https://[::1]/x", "http://localhost:8080/x", "http://LOCALHOST./x", "http://a.localhost/x", "https://192.168.0.1/x", "http://0x7f000001/x", "file:///etc/passwd", "ftp://example.com/x"] {
            assert!(!image_url_allowed(&url(bad)), "{bad}");
        }
        assert!(image_url_allowed(&url("https://cdn.example.com/x.png")) && image_url_allowed(&url("http://8.8.8.8/x")));
        let (https, http) = (url("https://a.example/x"), url("http://b.example/y"));
        assert!(redirect_allowed(&[https.clone()], &url("https://b.example/y")));
        assert!(redirect_allowed(&[http.clone()], &url("http://c.example/z")), "http mail images may stay on http");
        assert!(!redirect_allowed(&[https.clone()], &http), "no downgrade from https");
        assert!(!redirect_allowed(&[http.clone(), https.clone()], &url("http://c.example/z")));
        assert!(!redirect_allowed(&[https.clone()], &url("https://10.0.0.1/x")));
        assert!(redirect_allowed(&vec![https.clone(); 5], &url("https://b.example/y")), "the fifth redirect: the first URL and four redirects before it");
        assert!(!redirect_allowed(&vec![https.clone(); 6], &url("https://b.example/y")), "no sixth");
    }

    #[test]
    fn native_images_follow_the_csp() {
        for (src, url) in [("https://a.example/x.png", Some("https://a.example/x.png")), ("HTTP://a/x", Some("HTTP://a/x")), ("//cdn.example/x", Some("https://cdn.example/x")),
            ("cid:logo", None), ("data:image/png;base64,AA==", None), ("file:///etc/passwd", None), ("javascript:x", None), ("/relative.png", None), ("x.png", None)] {
            assert_eq!(remote_image(src).as_deref(), url, "{src}");
        }
        assert_eq!(data_image("DATA:image/png;BASE64,iVBO\n Rw=="), Some(vec![0x89, b'P', b'N', b'G']));
        assert_eq!(data_image("data:image/svg+xml,<svg/>"), None, "not base64");
        assert_eq!(data_image("https://a.example/x.png"), None);
        assert_eq!(image_type(&[0x89, b'P', b'N', b'G', 0x0d]), Some("image/png"));
        assert_eq!(image_type(b"GIF89a"), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None, "SVG never draws");
    }

    #[test]
    fn srcdoc_and_plugins_are_inert() {
        let mail = r#"<iframe srcdoc="&lt;link rel=preconnect href=https://u123.t.example&gt;" style="display:none"></iframe><p>after</p>"#;
        assert_eq!(inert_tags(mail), r#"<x-iframe srcdoc="&lt;link rel=preconnect href=https://u123.t.example&gt;" style="display:none"></x-iframe><p>after</p>"#);
        let mixed = "<IFrame src=x></iFRAME><FRAME src=y><Object data=z></OBJECT><eMbEd src=w><LiNk rel=preconnect>";
        assert_eq!(inert_tags(mixed), "<x-IFrame src=x></x-iFRAME><x-FRAME src=y><x-Object data=z></x-OBJECT><x-eMbEd src=w><x-LiNk rel=preconnect>");
        assert!(matches!(inert_tags("<p>&lt;iframe é</p><"), Cow::Borrowed(_)));
    }

    #[test]
    fn any_input_survives_and_only_gains_prefixes() {
        let pieces = ["<", "/", "link", "iFrame", "frame", "OBJ", "ect", "embed", "é", "日", "😀", "x", " ", ">", "\0", "\u{80}"];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        for _ in 0..5000 {
            let input: String = (0..next() % 24).map(|_| pieces[(next() % pieces.len() as u64) as usize]).collect();
            let out = inert_tags(&input);
            // "x-" is not in the alphabet, so it comes only from a rename.
            assert_eq!(out.replace("x-", ""), input);
            assert_eq!(matches!(out, Cow::Borrowed(_)), !out.contains("x-"));
            let lower = out.to_ascii_lowercase();
            for tag in ["link", "iframe", "frame", "object", "embed"] {
                assert!(!lower.contains(&format!("<{tag}")) && !lower.contains(&format!("</{tag}")), "{input:?} -> {out:?}");
            }
        }
    }
}

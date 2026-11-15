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
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_ip(v4.into()),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
            }
        },
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

/// A redirect for a remote image: at most five, never from https down to http.
fn redirect_allowed(previous: &[reqwest::Url], next: &reqwest::Url) -> bool {
    previous.len() < 5 && image_url_allowed(next) && !(next.scheme() == "http" && previous.iter().any(|u| u.scheme() == "https"))
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

#[cfg(test)]
mod tests {
    use super::{data_image, image_type, image_url_allowed, inert_tags, public_ip, redirect_allowed, remote_image};
    use std::borrow::Cow;

    #[test]
    fn remote_images_reach_only_public_hosts() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "::1", "fe80::1", "fd00::1", "::ffff:192.168.1.1", "::"] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "8.8.8.8", "2606:4700::1111"] { assert!(public_ip(ip.parse().unwrap()), "{ip}"); }
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
        assert!(!redirect_allowed(&vec![https.clone(); 5], &url("https://b.example/y")), "five redirects at most");
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

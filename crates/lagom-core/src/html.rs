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

/// A remote image for a native view, after "Load remote images": no cookies or referrer, at
/// most five redirects, 20 seconds and 10 MiB, raster formats only. Needs a Tokio runtime.
pub async fn fetch_image(url: &str) -> Option<(&'static str, Vec<u8>)> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| reqwest::Client::builder().use_rustls_tls()
        .timeout(std::time::Duration::from_secs(20))
        .referer(false)
        .redirect_policy(reqwest::redirect::Policy::limited(5))
        .build().ok()).as_ref()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = crate::oauth::body_limit(response, crate::inline::MAX_INLINE_BYTES).await.ok()?;
    Some((image_type(&bytes)?, bytes.to_vec()))
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
    use super::{data_image, image_type, inert_tags, remote_image};
    use std::borrow::Cow;

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

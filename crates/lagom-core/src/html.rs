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
    use super::inert_tags;
    use std::borrow::Cow;

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

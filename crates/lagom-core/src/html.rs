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

/// `<link>` becomes an unknown, inert element. Resource hints (preconnect, dns-prefetch) open
/// connections that the CSP does not cover; stylesheets are blocked by the CSP anyway.
pub fn inert_links(html: &str) -> std::borrow::Cow<'_, str> {
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

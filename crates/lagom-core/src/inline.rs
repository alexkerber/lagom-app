use crate::{Error, Result, store::Store};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::params;

pub const MAX_INLINE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlinePart {
    pub cid: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

pub(crate) fn content_id(value: &str) -> &str {
    let value = value.trim();
    value.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(value)
}

fn decode_id(value: &str) -> Option<String> {
    let value = mail_parser::decoders::html::html_to_text(value);
    let mut bytes = value.bytes();
    let mut out = Vec::new();
    while let Some(b) = bytes.next() {
        out.push(if b == b'%' {
            let hi = (bytes.next()? as char).to_digit(16)?;
            let lo = (bytes.next()? as char).to_digit(16)?;
            (hi * 16 + lo) as u8
        } else { b });
    }
    String::from_utf8(out).ok()
}

// shortcut: CSS-escaped CID spellings are not decoded; use a CSS tokenizer if encountered.
fn references(html: &str) -> impl Iterator<Item = (std::ops::Range<usize>, String)> {
    let mut out = Vec::new();
    let mut add = |range: std::ops::Range<usize>| {
        let value = &html[range.clone()];
        // Bound both individual spellings and per-message work before decoding or allocating IDs.
        if out.len() >= 1024 || value.len() > 4096 || !value.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:")) { return; }
        if let Some(cid) = decode_id(&value[4..]).filter(|s| !s.is_empty()) { out.push((range, cid)); }
    };
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(start) = html[i..].find('<') else { break };
        i += start + 1;
        if html[i..].starts_with("!--") {
            i = html[i + 3..].find("-->").map_or(bytes.len(), |end| i + 3 + end + 3);
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() { i += 1; }
        if i == start { continue; }
        let tag = &html[start..i];
        while i < bytes.len() && bytes[i] != b'>' {
            if bytes[i].is_ascii_whitespace() || bytes[i] == b'/' { i += 1; continue; }
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !b"=>/".contains(&bytes[i]) { i += 1; }
            let name = &html[start..i];
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            if bytes.get(i) != Some(&b'=') { continue; }
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            let quote = bytes.get(i).copied().filter(|b| b"\"'".contains(b));
            if quote.is_some() { i += 1; }
            let start = i;
            while i < bytes.len() && if let Some(quote) = quote { bytes[i] != quote } else { !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' } { i += 1; }
            let end = i;
            if quote.is_some() && i < bytes.len() { i += 1; }
            if name.eq_ignore_ascii_case("src") || name.eq_ignore_ascii_case("background") || name.eq_ignore_ascii_case("poster") {
                let value = &html[start..end];
                let trimmed = value.trim_ascii();
                let start = start + value.len() - value.trim_ascii_start().len();
                add(start..start + trimmed.len());
            } else if name.eq_ignore_ascii_case("srcset") {
                let mut at = start;
                while at < end {
                    while at < end && (bytes[at].is_ascii_whitespace() || bytes[at] == b',') { at += 1; }
                    let start = at;
                    while at < end && !bytes[at].is_ascii_whitespace() { at += 1; }
                    let url_end = start + html[start..at].trim_end_matches(',').len();
                    add(start..url_end);
                    if url_end == at {
                        while at < end && bytes[at] != b',' { at += 1; }
                    }
                }
            } else if name.eq_ignore_ascii_case("style") {
                css_urls(&html[start..end], start, &mut add);
            }
        }
        i = (i + 1).min(bytes.len());
        if tag.eq_ignore_ascii_case("plaintext") { break; }
        if ["style", "script", "textarea", "title", "xmp", "iframe", "noembed", "noframes"].iter().any(|t| tag.eq_ignore_ascii_case(t)) {
            let end = bytes[i..].windows(tag.len() + 2).enumerate().find(|(at, s)| s.starts_with(b"</") && s[2..].eq_ignore_ascii_case(tag.as_bytes())
                && bytes.get(i + at + s.len()).is_none_or(|b| b.is_ascii_whitespace() || b">/".contains(b))).map(|(at, _)| at);
            let end = end.map_or(bytes.len(), |end| i + end);
            if tag.eq_ignore_ascii_case("style") { css_urls(&html[i..end], i, &mut add); }
            i = end;
        }
    }
    out.into_iter()
}

fn css_urls(css: &str, offset: usize, add: &mut impl FnMut(std::ops::Range<usize>)) {
    let bytes = css.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            i = css[i + 2..].find("*/").map_or(bytes.len(), |end| i + 2 + end + 2);
        } else if b"\"'".contains(&bytes[i]) {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != quote { i += if bytes[i] == b'\\' { 2 } else { 1 }; }
            i = (i + 1).min(bytes.len());
        } else if bytes[i].is_ascii_alphabetic() || b"_-".contains(&bytes[i]) {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || b"_-".contains(&bytes[i])) { i += 1; }
            if !css[start..i].eq_ignore_ascii_case("url") || bytes.get(i) != Some(&b'(') { continue; }
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            let quote = bytes.get(i).copied().filter(|b| b"\"'".contains(b));
            if quote.is_some() { i += 1; }
            let start = i;
            while i < bytes.len() && if let Some(quote) = quote { bytes[i] != quote } else { bytes[i] != b')' } { i += 1; }
            let end = i;
            if quote.is_some() && i < bytes.len() { i += 1; }
            while i < bytes.len() && bytes[i].is_ascii_whitespace() { i += 1; }
            if bytes.get(i) == Some(&b')') {
                let end = start + css[start..end].trim_ascii_end().len();
                add(offset + start..offset + end);
                i += 1;
            }
        } else {
            i += 1;
        }
    }
}

pub fn referenced_ids(html: &str) -> std::collections::HashSet<String> {
    references(html).map(|(_, cid)| cid).collect()
}

pub fn url(message: i64, cid: &str, windows: bool) -> String {
    let origin = if windows { "http://lagom-cid.localhost" } else { "lagom-cid://localhost" };
    format!("{origin}/{message}/{}", URL_SAFE_NO_PAD.encode(cid))
}

pub fn rewrite(html: &str, message: i64, windows: bool) -> String {
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for (range, cid) in references(html) {
        out.push_str(&html[last..range.start]);
        out.push_str(&url(message, &cid, windows));
        last = range.end;
    }
    out.push_str(&html[last..]);
    out
}

/// Only the current document's parts are available, including for forged protocol URLs.
pub fn resolve<'a>(message: i64, parts: &'a [InlinePart], request: &str) -> Option<&'a InlinePart> {
    parts.iter().find(|p| request == url(message, &p.cid, false) || request == url(message, &p.cid, true))
}

impl Store {
    pub fn inline_loaded(&self, message: i64) -> Result<bool> {
        Ok(self.db.query_row("SELECT inline_loaded FROM messages WHERE id=?1", [message], |r| r.get(0))?)
    }

    pub fn inline_parts(&self, message: i64) -> Result<Vec<InlinePart>> {
        let mut stmt = self.db.prepare("SELECT cid,mime_type,data FROM inline_parts WHERE message=?1 ORDER BY cid")?;
        let rows = stmt.query_map([message], |r| Ok(InlinePart { cid: r.get(0)?, mime_type: r.get(1)?, data: r.get(2)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub(crate) fn save_inline_parts(&self, message: i64, parts: &[InlinePart]) -> Result<()> {
        if parts.iter().try_fold(0usize, |n, p| n.checked_add(p.data.len())).is_none_or(|n| n > MAX_INLINE_BYTES) {
            return Err(Error::Invalid("inline parts exceed 10 MiB"));
        }
        let tx = self.db.unchecked_transaction()?;
        self.db.execute("DELETE FROM inline_parts WHERE message=?1", [message])?;
        for part in parts {
            self.db.execute("INSERT INTO inline_parts VALUES (?1,?2,?3,?4)", params![message, part.cid, part.mime_type, part.data])?;
        }
        self.db.execute("UPDATE messages SET inline_loaded=1 WHERE id=?1", [message])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_fixture_matches_content_id_and_rewrites_only_cid_urls() {
        let raw = include_bytes!("../tests/fixtures/related.eml");
        let parsed = mail_parser::MessageParser::default().parse(raw).unwrap();
        let html = crate::mime::parse(raw).unwrap().body_html.unwrap();
        let image = parsed.attachments().next().unwrap();
        let cid = image.content_id().unwrap();
        assert_eq!(cid, "Logo+1@example.test");
        assert_eq!(referenced_ids(&html), [cid.to_owned()].into());
        assert_eq!(content_id(" <Logo+1@example.test> "), cid);
        for windows in [false, true] {
            let rewritten = rewrite(&html, 42, windows);
            assert!(rewritten.contains(&format!("src=\"{}\"", url(42, cid, windows))));
            assert!(rewritten.contains("src=\"https://example.test/remote.png\""));
            assert!(!rewritten.contains("cid:Logo"));
        }
    }

    #[test]
    fn cid_matching_preserves_case_and_handles_css_entities_and_unquoted_urls() {
        let html = "<img src=CID:Logo%2B1%40example.test><div background='cid:a&amp;b' style='background:url(cid:x%2Fy)'></div>";
        assert_eq!(referenced_ids(html), ["Logo+1@example.test", "a&b", "x/y"].map(str::to_owned).into());
        assert_ne!(referenced_ids("<img src='cid:Logo'>"), referenced_ids("<img src='cid:logo'>"));
        assert!(referenced_ids("<img src='cid:%xx'>").is_empty());
        assert_eq!(rewrite("<img src='https://example.test/cid:other'>", 1, false), "<img src='https://example.test/cid:other'>");
    }

    #[test]
    fn repeated_cid_prefixes_are_one_bounded_reference() {
        let html = format!("<img src='{}'>", "cid:".repeat(256));
        let ids = referenced_ids(&html);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.iter().next().unwrap().len(), 1020);
        assert_eq!(rewrite(&html, 1, false), format!("<img src='{}'>", url(1, &"cid:".repeat(255), false)));
        let oversized = format!("<img src='{}'>", "cid:".repeat(25_000));
        assert!(referenced_ids(&oversized).is_empty());
        assert_eq!(rewrite(&oversized, 1, false), oversized);
        assert_eq!(referenced_ids(&format!("{oversized}<img src=cid:next>")), ["next".into()].into());
        for (length, count) in [(4092, 1), (4093, 0)] {
            assert_eq!(referenced_ids(&format!("<img src='cid:{}'>", "a".repeat(length))).len(), count);
        }
        let many = (0..2048).map(|i| format!("<img src='cid:{i}'>")).collect::<String>();
        assert_eq!(referenced_ids(&many).len(), 1024);
        assert_eq!(rewrite(&many, 1, false).matches("lagom-cid://").count(), 1024);
    }

    #[test]
    fn cid_urls_only_change_in_resource_attributes_and_css_urls() {
        let unchanged = r#"<img src="https://example.test/image?token=cid:logo" srcset="https://example.test/?token=cid:logo 2x"><a href="cid:logo">cid:logo</a>
            <p title="cid:logo">src='cid:logo' url(cid:logo)</p><!-- <img src='cid:logo'> -->
            <script>"<img src='cid:logo'>"</script>
            <style>.a { content: 'url(cid:logo)'; background: url(https://example.test/?token=cid:logo) }
            /* url(cid:logo) */</style>"#;
        assert!(referenced_ids(unchanged).is_empty());
        assert_eq!(rewrite(unchanged, 1, false), unchanged);
        let html = r#"<IMG SRC = 'cid:logo'><table background=cid:logo><video poster="cid:logo"></video>
            <div style="background: URL( 'cid:logo' )"></div><style>.a { background: url(cid:logo) }</style>
            <img srcset="cid:logo, cid:logo 2x, cid:logo 3x">"#;
        assert_eq!(referenced_ids(html), ["logo".into()].into());
        assert_eq!(rewrite(html, 1, false).matches(&url(1, "logo", false)).count(), 8);
    }

    #[test]
    fn protocol_refuses_other_messages_hosts_and_paths() {
        let parts = [InlinePart { cid: "logo@test".into(), mime_type: "image/png".into(), data: vec![0, 255] }];
        for windows in [false, true] {
            assert_eq!(resolve(42, &parts, &url(42, "logo@test", windows)), Some(&parts[0]));
            assert!(resolve(42, &parts, &url(43, "logo@test", windows)).is_none());
            assert!(resolve(42, &parts, &url(42, "other@test", windows)).is_none());
            assert!(resolve(42, &parts, &format!("{}?message=43", url(42, "logo@test", windows))).is_none());
        }
        for uri in ["lagom-cid://evil/42/bG9nb0B0ZXN0", "lagom-cid://localhost/43/../42/bG9nb0B0ZXN0", "file:///42/bG9nb0B0ZXN0"] {
            assert!(resolve(42, &parts, uri).is_none());
        }
    }

    #[test]
    fn store_cap_rollback_reopen_and_message_cascade() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mail.db");
        let store = Store::open(&path).unwrap();
        let (imap, smtp) = crate::presets::servers("fixture@fastmail.com").unwrap();
        let account = crate::account::Account::new("fixture@example.test".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let folder = store.save_folder(&account.id, "INBOX", None, crate::store::FolderRole::Inbox).unwrap();
        let id = store.save_message(&crate::store::Message {
            summary: crate::store::MessageSummary { account: account.id, folder, uid: 1, ..Default::default() },
            ..Default::default()
        }).unwrap();
        assert!(!store.inline_loaded(id).unwrap());
        let mut parts = vec![InlinePart { cid: "large".into(), mime_type: "image/png".into(), data: vec![0; MAX_INLINE_BYTES] }];
        store.save_inline_parts(id, &parts).unwrap();
        parts.push(InlinePart { cid: "small".into(), mime_type: "image/png".into(), data: vec![1] });
        assert!(store.save_inline_parts(id, &parts).is_err());
        assert_eq!(store.inline_parts(id).unwrap(), parts[..1]);
        parts[0].data.clear();
        parts[1].cid = "large".into();
        assert!(store.save_inline_parts(id, &parts).is_err());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.inline_loaded(id).unwrap());
        assert_eq!(store.inline_parts(id).unwrap()[0].data.len(), MAX_INLINE_BYTES);
        store.delete_message(id).unwrap();
        assert!(store.inline_parts(id).unwrap().is_empty());
    }
}

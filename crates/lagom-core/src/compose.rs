//! Shared reply and forwarding fields for native clients.
use crate::contacts::{recipient_address, split_recipients};

pub struct Original<'a> {
    pub from: &'a str, pub address: &'a str, pub date: &'a str,
    pub subject: &'a str, pub to: &'a str, pub text: &'a str,
}

/// "Re: subject", without stacking "Re: Re:".
pub fn reply_subject(subject: &str) -> String {
    if subject.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) { subject.into() } else { format!("Re: {subject}") }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposeKind { Reply, ReplyAll, Forward }

pub fn forward_subject(subject: &str) -> String {
    if subject.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("fwd:")) { subject.into() } else { format!("Fwd: {subject}") }
}

pub fn reply_all(sender: &str, to: &str, cc: &str, own: &[&str]) -> (String, String) {
    let mut used: std::collections::HashSet<String> = own.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let mut unique = |field: &str| split_recipients(field).into_iter().filter(|address| used.insert(recipient_address(address).unwrap_or_else(|| address.clone()).to_ascii_lowercase())).collect::<Vec<_>>().join(", ");
    (unique(&format!("{sender}, {to}")), unique(cc))
}

pub fn compose_body(signature: &str, original: Option<Original<'_>>, kind: ComposeKind) -> String {
    let mut body = if signature.trim().is_empty() { String::new() } else { format!("-- \n{signature}") };
    if let Some(m) = original {
        if !body.is_empty() { body.push_str("\n\n"); }
        let header = if kind == ComposeKind::Forward {
            format!("Forwarded message\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\n\n", m.from, m.address, m.date, m.subject, m.to)
        } else { format!("On {}, {} wrote:\n\n", m.date, m.address) };
        let quote = crate::mime::clean_lines(&format!("{header}{}", crate::mime::clean_lines(m.text)));
        body.push_str(&quote.lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n"));
    }
    if !body.is_empty() { body.insert_str(0, "\n\n"); }
    body
}

/// A new message from a `mailto:` link (RFC 6068). Bcc is visible in Compose, so a link cannot add a hidden recipient.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailto { pub to: Vec<String>, pub cc: Vec<String>, pub bcc: Vec<String>, pub subject: String, pub body: String }

/// Links longer than this are refused, not truncated: a cut link could drop a recipient or end mid-escape.
pub const MAILTO_MAX: usize = 64 * 1024;

/// `None` when `url` is not a mailto link or is longer than `MAILTO_MAX`. Header values lose control
/// characters, so CRLF cannot add headers; the body keeps its line breaks as `\n`. Unknown headers are ignored.
pub fn parse_mailto(url: &str) -> Option<Mailto> {
    let url = url.trim();
    if url.len() > MAILTO_MAX || !url.get(..7)?.eq_ignore_ascii_case("mailto:") { return None; }
    // An unescaped '#' starts a fragment, which mailto does not use.
    let rest = url[7..].split('#').next().unwrap_or_default();
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let header = |value: &str| decode(value).chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_owned();
    let mut mail = Mailto { to: split_recipients(&header(path)), ..Default::default() };
    for field in query.split('&') {
        let (name, value) = field.split_once('=').unwrap_or((field, ""));
        match decode(name).to_ascii_lowercase().as_str() {
            "to" => mail.to.extend(split_recipients(&header(value))),
            "cc" => mail.cc.extend(split_recipients(&header(value))),
            "bcc" => mail.bcc.extend(split_recipients(&header(value))),
            "subject" => mail.subject = header(value),
            "body" => mail.body = decode(value).replace("\r\n", "\n").replace('\r', "\n")
                .chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect(),
            _ => {}
        }
    }
    Some(mail)
}

/// Percent-decoding as RFC 3986: '+' stays '+', a broken escape stays literal, invalid UTF-8 becomes U+FFFD.
fn decode(value: &str) -> String {
    let (bytes, mut out, mut at) = (value.as_bytes(), Vec::with_capacity(value.len()), 0);
    while at < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[at], bytes.get(at + 1).copied().and_then(hex), bytes.get(at + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => { out.push((high * 16 + low) as u8); at += 3; }
            (b, _, _) => { out.push(b); at += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_trim_and_collapse_legacy_blank_lines() {
        for kind in [ComposeKind::Reply, ComposeKind::ReplyAll, ComposeKind::Forward] {
            let body = compose_body("", Some(Original { from: "Sender", address: "sender@example.test",
                date: "2026-01-01", subject: "News", to: "reader@example.test",
                text: "\n \n\nFirst\n\n \n\nSecond\n \n\n" }), kind);
            assert!(body.ends_with("> First\n> \n> Second"), "{body:?}");
            assert!(!body.contains("\n> \n> \n"), "{body:?}");
        }
    }

    #[test]
    fn reply_all_compares_mailboxes_and_keeps_display_names() {
        let (to, cc) = reply_all("Sender <sender@example.test>",
            "Me <ME@example.test>, sender@example.test, Other <other@example.test>",
            "Alias <alias@example.test>, OTHER@example.test, Copy <copy@example.test>",
            &["me@example.test", "alias@example.test"]);
        assert_eq!(to, "Sender <sender@example.test>, Other <other@example.test>");
        assert_eq!(cc, "Copy <copy@example.test>");
        assert_eq!(reply_all("me@example.test", "", "ME@example.test", &["me@example.test"]), (String::new(), String::new()));
    }

    #[test]
    fn quotes_and_signatures_share_desktop_format() {
        for kind in [ComposeKind::Reply, ComposeKind::ReplyAll, ComposeKind::Forward] {
            let body = compose_body("**Team**", Some(Original { from: "Sender", address: "sender@example.test",
                date: "2026-01-01 12:00", subject: "Plans", to: "me@example.test", text: "First\n\nSecond" }), kind);
            assert!(body.starts_with("\n\n-- \n**Team**\n\n> "));
            assert!(body.ends_with("> First\n> \n> Second"));
            assert_eq!(body.matches("-- \n**Team**").count(), 1);
            if kind == ComposeKind::Forward {
                assert!(body.contains("> Forwarded message\n> From: Sender <sender@example.test>\n> Date: "));
                assert!(body.contains("> Subject: Plans\n> To: me@example.test"));
            } else { assert!(body.contains("sender@example.test wrote:")); }
        }
        assert_eq!(compose_body("", None, ComposeKind::Reply), "");
        assert_eq!(reply_subject("RE: Plans"), "RE: Plans");
        assert_eq!(forward_subject("FWd: Plans"), "FWd: Plans");
        assert_eq!(reply_subject("日"), "Re: 日");
        assert_eq!(forward_subject("日"), "Fwd: 日");
    }
    #[test]
    fn mailto_follows_rfc_6068_examples() {
        let m = |url: &str| parse_mailto(url).unwrap();
        assert_eq!(m("mailto:chris@example.com").to, ["chris@example.com"]);
        assert_eq!(m("mailto:infobot@example.com?subject=current-issue").subject, "current-issue");
        assert_eq!(m("mailto:infobot@example.com?body=send%20current-issue%0D%0Asend%20index").body, "send current-issue\nsend index");
        let list = m("mailto:list@example.org?In-Reply-To=%3C3469A91.D10AF4C@example.com%3E");
        assert_eq!((list.to.as_slice(), list.subject.as_str(), list.body.as_str()), (&["list@example.org".to_owned()][..], "", ""));
        let joe = m("mailto:joe@example.com?cc=bob@example.com&body=hello");
        assert_eq!((joe.cc, joe.body), (vec!["bob@example.com".to_owned()], "hello".to_owned()));
        assert_eq!(m("mailto:gorby%25kremvax@example.com").to, ["gorby%kremvax@example.com"]);
        assert_eq!(m("mailto:unlikely%3Faddress@example.com?blat=foop").to, ["unlikely?address@example.com"]);
        assert_eq!(m("mailto:Mike%26family@example.org").to, ["Mike&family@example.org"]);
        assert_eq!(m("mailto:%22not%40me%22@example.org").to, ["\"not@me\"@example.org"]);
        assert_eq!(m("mailto:?to=addr1@an.example,addr2@an.example").to, ["addr1@an.example", "addr2@an.example"]);
        assert_eq!(m("mailto:addr1@an.example?to=addr2@an.example").to, ["addr1@an.example", "addr2@an.example"]);
        assert_eq!(m("mailto:user@example.org?subject=caf%C3%A9&body=caf%C3%A9").subject, "café");
        assert_eq!(m("MAILTO:a@example.org?SUBJECT=x+y&Bcc=b@example.org").subject, "x+y");
        assert_eq!(m("MAILTO:a@example.org?Bcc=b@example.org&bcc=c@example.org").bcc, ["b@example.org", "c@example.org"]);
        assert_eq!(m("mailto:a@example.org?subject=%ZZ%4").subject, "%ZZ%4");
        assert_eq!(m("mailto:a@example.org?body=fr%E9").body, "fr\u{FFFD}");
    }

    #[test]
    fn mailto_rejects_header_injection_and_huge_links() {
        let m = parse_mailto("mailto:a@example.org%0D%0ABcc:%20evil@example.org?subject=Hi%0D%0ABcc:%20evil@example.org&cc=c@example.org%0Abcc@example.org&body=a%00b%0Dc").unwrap();
        assert!(m.bcc.is_empty(), "{m:?}");
        assert!(m.to.iter().chain(&m.cc).chain([&m.subject]).all(|v| !v.contains(['\r', '\n'])), "{m:?}");
        assert_eq!(m.subject, "Hi  Bcc: evil@example.org");
        assert_eq!(m.body, "ab\nc");
        assert_eq!(parse_mailto("https://example.org"), None);
        assert_eq!(parse_mailto("mail"), None);
        assert!(parse_mailto(&format!("mailto:a@example.org?body={}", "x".repeat(MAILTO_MAX))).is_none());
        assert!(parse_mailto(&format!("mailto:a@example.org?body={}", "%41".repeat(1000))).is_some());
    }
}

use crate::{
    store::{Message, MessageSummary},
    Error, Result,
};
use mail_parser::{AddressList, MessageParser, PartKind};

pub const MAX_MIME_BYTES: usize = 4 * 1024 * 1024;
/// The stored body of a message whose text is over `MAX_MIME_BYTES`: the headers are kept, the
/// text is not downloaded. Readers compare against this to show why the body is missing.
pub const BODY_UNAVAILABLE: &str = "The text of this message is larger than 4 MiB and was not downloaded.";

/// A row for a message whose headers do not parse: the list shows it, the body says why.
pub fn placeholder() -> Message {
    Message {
        summary: MessageSummary { subject: "(headers too large to read)".into(), ..Default::default() },
        body_text: BODY_UNAVAILABLE.into(),
        body_html: None,
    }
}

/// Headers only, with the body marked unavailable, so one huge message never stops a sync.
pub fn metadata_only(header: &[u8]) -> Result<Message> {
    let mut message = parse(header)?;
    message.body_text = BODY_UNAVAILABLE.into();
    message.body_html = None;
    message.summary.snippet = String::new();
    Ok(message)
}
/// Zero-width characters that newsletters repeat after the preheader to push the body text
/// out of list previews (`&#847;`, `&zwnj;`, soft hyphens).
const INVISIBLE: &[char] = &['\u{34F}', '\u{AD}', '\u{180E}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'];

pub fn html_to_text(html: &str) -> String {
    let document = dom_query::Document::from(html);
    document.select("head, style, script, title, template, [hidden]").remove();
    // shortcut: visibility uses inline styles only; add a CSS cascade if class-based preheaders need filtering.
    for node in document.select("[style]").iter() {
        let hidden = node.attr_or("style", "").split(';').any(|declaration| {
            let Some((name, value)) = declaration.split_once(':') else { return false; };
            let value = value.split('!').next().unwrap_or_default().trim();
            (name.trim().eq_ignore_ascii_case("display") && value.eq_ignore_ascii_case("none"))
                || (name.trim().eq_ignore_ascii_case("visibility") && value.eq_ignore_ascii_case("hidden"))
        });
        if hidden { node.remove(); }
    }
    clean_lines(&document.formatted_text())
}

/// Repair an older HTML fallback without replacing a sender's plain-text alternative.
pub fn readable_text<'a>(text: &'a str, html: Option<&str>) -> std::borrow::Cow<'a, str> {
    // shortcut: legacy rows lack provenance; repair exact converter matches until bodies can be re-fetched.
    match html {
        Some(html) if !text.starts_with(html) && text == mail_parser::decoders::html::html_to_text(html) => html_to_text(html).into(),
        _ => text.into(),
    }
}

pub(crate) fn clean_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        if line.trim().is_empty() { blank = !out.is_empty(); continue; }
        if !out.is_empty() {
            out.push('\n');
            if blank { out.push('\n'); }
        }
        out.push_str(line.trim_end());
        blank = false;
    }
    out
}

/// The list preview, at most 200 characters on one line. From the HTML when there is one: text
/// parts are often generated, with CSS and "[Image]" lines in them.
pub fn snippet(text: &str, html: Option<&str>) -> String {
    match html {
        Some(html) => clean_snippet(&html_to_text(html)),
        None => clean_snippet(text),
    }
}

/// Invisible padding dropped, white space collapsed, cut at 200 characters. Also for snippets
/// stored before this cleanup.
pub fn clean_snippet(text: &str) -> String {
    let visible: String = text.chars().filter(|c| !INVISIBLE.contains(c)).collect();
    let mut out = String::new();
    for word in visible.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() >= 800 {
            break;
        }
    }
    out.chars().take(200).collect()
}

fn addresses(list: Option<AddressList<'_>>) -> String {
    list.map(|l| {
        l.mailboxes()
            .filter_map(|m| m.address().map(|address| crate::contacts::format_recipient(m.name().unwrap_or_default(), address)))
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}
pub fn parse(raw: &[u8]) -> Result<Message> {
    if raw.len() > MAX_MIME_BYTES {
        return Err(Error::Invalid("MIME input exceeds 4 MiB"));
    }
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or(Error::Invalid("invalid MIME message"))?;
    let from = parsed.from().and_then(|a| a.first());
    let subject = parsed.subject().unwrap_or_default().to_owned();
    let message_id = parsed.message_id().unwrap_or_default().to_owned();
    let thread_key = parsed
        .references()
        .and_then(|v| v.first())
        .or_else(|| parsed.in_reply_to().and_then(|v| v.first()))
        .unwrap_or(&message_id)
        .to_owned();
    let body_text = parsed
        .text_body()
        .filter_map(|part| match part.kind() {
            PartKind::Text => part.text().map(|text| text.into_owned()),
            PartKind::Html => part.text().map(|html| html_to_text(&html)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body_html = parsed
        .html_body()
        .find(|part| matches!(part.kind(), PartKind::Html))
        .and_then(|part| part.text())
        .map(|v| v.into_owned());
    let snippet = snippet(&body_text, body_html.as_deref());
    Ok(Message {
        summary: MessageSummary {
            message_id,
            thread_key,
            thread_refs: parsed.references().into_iter().flat_map(|ids| ids.iter())
                .chain(parsed.in_reply_to().into_iter().flat_map(|ids| ids.iter())).collect::<Vec<_>>().join(" "),
            from_name: from.and_then(|m| m.name()).unwrap_or_default().into(),
            from_address: from.and_then(|m| m.address()).unwrap_or_default().into(),
            to: addresses(parsed.to()),
            cc: addresses(parsed.cc()),
            subject,
            date: parsed.date().map(|d| d.to_timestamp()).unwrap_or_default(),
            snippet,
            has_attachments: parsed.has_attachments(),
            size: raw.len() as u32,
            ..Default::default()
        },
        body_text,
        body_html,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multipart_reply_keeps_plain_text_and_quote_levels() {
        let plain = "My reply\n\n> Previous answer\n\nPlain-only detail";
        let raw = format!("Content-Type: multipart/alternative; boundary=reply\r\n\r\n--reply\r\nContent-Type: text/plain\r\n\r\n{plain}\r\n--reply\r\nContent-Type: text/html\r\n\r\n<p>My reply</p><blockquote>Previous answer</blockquote>\r\n--reply--\r\n");
        let m = parse(raw.as_bytes()).unwrap();
        let text = readable_text(&m.body_text, m.body_html.as_deref());
        assert_eq!(text, plain);
        let quote = crate::compose::compose_body("", Some(crate::compose::Original {
            from: "Sender", address: "sender@example.test", date: "", subject: "", to: "", text: &text,
        }), crate::compose::ComposeKind::Reply);
        assert!(quote.contains("> > Previous answer"), "{quote}");
        assert!(quote.ends_with("> Plain-only detail"));
    }

    #[test]
    fn outlook_button_keeps_non_outlook_text() {
        let html = include_str!("../tests/fixtures/outlook-button.html");
        let m = parse(format!("Content-Type: text/html\r\n\r\n{html}").as_bytes()).unwrap();
        assert_eq!(m.body_text, "Read the update");
        assert_eq!(m.summary.snippet, "Read the update");
    }

    #[test]
    fn legacy_generated_text_is_cleaned_but_current_text_is_borrowed() {
        let html = include_str!("../tests/fixtures/newsletter.html");
        let legacy = mail_parser::decoders::html::html_to_text(html);
        let clean = html_to_text(html);
        assert_eq!(readable_text(&legacy, Some(html)), clean);
        assert!(matches!(readable_text(&clean, Some(html)), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn newsletter_body_has_only_visible_text() {
        let html = include_str!("../tests/fixtures/newsletter.html");
        let m = parse(format!("Content-Type: text/html; charset=utf-8\r\n\r\n{html}").as_bytes()).unwrap();
        assert_eq!(m.body_text, "Hello & welcome\n\nRead the news");
        assert_eq!(m.body_html.as_deref(), Some(html));
    }

    #[test]
    fn html_text_keeps_visible_links_and_plain_mail() {
        let html = "<title>Title</title><div hidden=false><p>Hidden</p></div><div style='mso-hide:ALL;display:none'>Hidden</div><!-- Hidden --><p>Read <a href='https://tracking.example.test' title='Hidden'>this &amp; that</a>.</p><p>https://visible.example.test</p>";
        assert_eq!(html_to_text(html), "Read this & that.\n\nhttps://visible.example.test");
        assert_eq!(html_to_text("<p>Keep</p><style>unclosed { color: red }"), "Keep");
        assert_eq!(html_to_text("<div hidden><span style='display:none'>Nested</span></div><p>Keep</p>"), "Keep");
        let plain = "A <literal> & URL https://visible.example.test\n\n\n  indented";
        let m = parse(format!("Content-Type: text/plain\r\n\r\n{plain}").as_bytes()).unwrap();
        assert_eq!(m.body_text, plain);
        assert!(m.body_html.is_none());
        assert_eq!(readable_text(&m.body_text, m.body_html.as_deref()), plain);
        // Older parsers also stored the plain body in the HTML slot.
        assert_eq!(readable_text(plain, Some(plain)), plain);
        let multipart = format!("{plain}\nSecond plain part");
        assert_eq!(readable_text(&multipart, Some(plain)), multipart);
    }

    #[test]
    fn recipient_names_survive_sync_headers() {
        let message = parse(b"From: Sender <sender@example.com>\r\nTo: \"Team, West\" <west@example.com>, east@example.com\r\nCc: Team <team@example.com>\r\n\r\nbody").unwrap();
        assert_eq!(crate::contacts::split_recipients(&message.summary.to), ["\"Team, West\" <west@example.com>", "east@example.com"]);
        assert_eq!(message.summary.cc, "Team <team@example.com>");
    }

    #[test]
    fn multipart_and_attachment() {
        let raw=b"From: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: =?utf-8?Q?H=C3=A4lsning?=\r\nMessage-ID: <child@test>\r\nReferences: <root@test> <parent@test>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: multipart/alternative; boundary=y\r\n\r\n--y\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello plain\r\n--y\r\nContent-Type: text/html\r\n\r\n<b>Hello HTML</b>\r\n--y--\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let m = parse(raw).unwrap();
        assert_eq!(m.summary.subject, "Hälsning");
        assert_eq!(m.summary.from_address, "alice@example.com");
        assert_eq!(m.summary.thread_key, "root@test");
        assert!(m.summary.has_attachments);
        assert_eq!(m.body_text, "Hello plain");
        assert!(m.body_html.unwrap().contains("Hello HTML"));
        assert!(!m.body_text.contains("aGVsbG8="));
    }
    #[test]
    fn html_only_and_charset() {
        let m=parse(b"Content-Type: text/html; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<p>H=E4lsning &amp; hej</p>").unwrap();
        assert!(m.body_text.contains("Hälsning & hej"));
        assert!(!m.body_text.contains("<p>"));
        assert!(m.body_html.is_some());
        assert!(!m.summary.has_attachments);
        let m = parse(b"Content-Type: text/plain; charset=windows-1252\r\n\r\nPrice \x80").unwrap();
        assert!(m.body_text.contains('€'));
        assert!(m.body_html.is_none());
    }
    #[test]
    fn newsletter_snippet_has_no_css_or_padding() {
        let raw = "Content-Type: text/html; charset=utf-8\r\n\r\n<!DOCTYPE html><html><head><title>News</title><style>@media only screen and (max-width: 600px) { .container { width: 100% !important; } }</style></head><body><div style=\"display:none\">Autumn sale &#847; &zwnj; &nbsp; &#847; &zwnj;&nbsp;&shy;</div><table><tr><td><style>.x { color: red }</style><p>Fifty&nbsp;kinds of tomato &amp; two&nbsp;&nbsp; kinds of soil</p></td></tr></table></body></html>";
        let m = parse(raw.as_bytes()).unwrap();
        assert_eq!(m.summary.snippet, "Fifty kinds of tomato & two kinds of soil");
        // A generated text part with junk loses to the HTML part.
        assert_eq!(snippet("[Image] Mobile px) { .conta", Some("<p>Real text</p>")), "Real text");
        assert_eq!(snippet("a\u{200C}\u{200C}  b\n\nc", None), "a b c");
        assert_eq!(snippet(&"word ".repeat(100), None).chars().count(), 200);
    }
    #[test]
    fn rejects_empty_and_oversized() {
        assert!(parse(b"").is_err());
        assert!(parse(&vec![0; MAX_MIME_BYTES + 1]).is_err());
        let m = metadata_only(b"From: Big <big@example.com>\r\nSubject: Huge\r\nMessage-ID: <big@x>\r\n\r\n").unwrap();
        assert_eq!((m.summary.subject.as_str(), m.summary.from_address.as_str(), m.body_text.as_str()), ("Huge", "big@example.com", BODY_UNAVAILABLE));
    }
}

use crate::{
    store::{Message, MessageSummary},
    Error, Result,
};
use mail_parser::{AddressList, MessageParser};

pub const MAX_MIME_BYTES: usize = 4 * 1024 * 1024;
/// The stored body of a message whose text is over `MAX_MIME_BYTES`: the headers are kept, the
/// text is not downloaded. Readers compare against this to show why the body is missing.
pub const BODY_UNAVAILABLE: &str = "The text of this message is larger than 4 MiB and was not downloaded.";

/// Headers only, with the body marked unavailable, so one huge message never stops a sync.
pub fn metadata_only(header: &[u8]) -> Result<Message> {
    let mut message = parse(header)?;
    message.body_text = BODY_UNAVAILABLE.into();
    message.body_html = None;
    message.summary.snippet = String::new();
    Ok(message)
}
fn addresses(list: Option<AddressList<'_>>) -> String {
    list.map(|l| {
        l.mailboxes()
            .filter_map(|m| m.address().map(str::to_owned))
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
        .text_bodies()
        .map(|v| v.into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let body_html = parsed
        .html_body()
        .next()
        .and_then(|part| part.text())
        .map(|v| v.into_owned());
    let snippet = body_text
        .split_whitespace()
        .take(200)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    Ok(Message {
        summary: MessageSummary {
            message_id,
            thread_key,
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
    fn multipart_and_attachment() {
        let raw=b"From: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: =?utf-8?Q?H=C3=A4lsning?=\r\nMessage-ID: <child@test>\r\nReferences: <root@test> <parent@test>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: multipart/alternative; boundary=y\r\n\r\n--y\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello plain\r\n--y\r\nContent-Type: text/html\r\n\r\n<b>Hello HTML</b>\r\n--y--\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let m = parse(raw).unwrap();
        assert_eq!(m.summary.subject, "Hälsning");
        assert_eq!(m.summary.from_address, "alice@example.com");
        assert_eq!(m.summary.thread_key, "root@test");
        assert!(m.summary.has_attachments);
        assert!(m.body_text.contains("Hello plain"));
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
    }
    #[test]
    fn rejects_empty_and_oversized() {
        assert!(parse(b"").is_err());
        assert!(parse(&vec![0; MAX_MIME_BYTES + 1]).is_err());
        let m = metadata_only(b"From: Big <big@example.com>\r\nSubject: Huge\r\nMessage-ID: <big@x>\r\n\r\n").unwrap();
        assert_eq!((m.summary.subject.as_str(), m.summary.from_address.as_str(), m.body_text.as_str()), ("Huge", "big@example.com", BODY_UNAVAILABLE));
    }
}

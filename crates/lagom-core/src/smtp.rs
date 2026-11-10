use tokio::time::timeout;

use crate::{
    account::{Account, Auth, Security},
    imap_sync::ImapSync,
    presets,
    store::Store,
    Error, Result,
};
use lettre::{
    message::{header, Mailbox, MultiPart, SinglePart},
    transport::smtp::{
        authentication::{Credentials, Mechanism},
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outgoing {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<OutgoingAttachment>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Kept by the caller across retries, so clients can spot a duplicate after `MaybeSent`; it
    /// does not prevent one. `None`: a new one from `message_id`.
    pub message_id: Option<String>,
}
/// A new Message-ID on the sender's domain.
pub fn message_id(email: &str) -> String {
    let domain = email.rsplit_once('@').map_or("localhost", |(_, d)| d);
    format!("<{}@{domain}>", uuid::Uuid::new_v4().simple())
}
fn mailbox(value: &str) -> Result<Mailbox> {
    value
        .parse()
        .map_err(|_| Error::Invalid("invalid email address"))
}
fn header_value(value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        Err(Error::Invalid("invalid mail header"))
    } else {
        Ok(())
    }
}
pub fn compose(account: &Account, outgoing: &Outgoing) -> Result<Message> {
    compose_message(account, outgoing, None)
}

pub(crate) fn compose_draft(account: &Account, draft: &crate::outbox::Draft, token: &str) -> Result<Vec<u8>> {
    let (in_reply_to, references) = draft.reply.clone().unwrap_or_default();
    let outgoing = Outgoing { subject: draft.subject.clone(), text: draft.text.clone(),
        attachments: draft.attachments.clone(), in_reply_to, references, message_id: Some(format!("<{token}@draft.lagommail.app>")),
        ..Default::default() };
    Ok(compose_message(account, &outgoing, Some(draft))?.formatted())
}

fn compose_message(account: &Account, outgoing: &Outgoing, draft: Option<&crate::outbox::Draft>) -> Result<Message> {
    account.validate()?;
    if draft.is_none() && outgoing.to.is_empty() && outgoing.cc.is_empty() && outgoing.bcc.is_empty() {
        return Err(Error::Invalid("at least one recipient required"));
    }
    header_value(&outgoing.subject)?;
    let from = Mailbox::new(
        Some(account.display_name.clone()),
        account
            .email
            .parse()
            .map_err(|_| Error::Invalid("invalid sender address"))?,
    );
    // Replies, threads and finding a message after a move all key on Message-ID; lettre adds none.
    let id = match &outgoing.message_id {
        Some(id) => {
            header_value(id)?;
            id.clone()
        }
        None => message_id(&account.email),
    };
    let mut message = Message::builder()
        .from(from)
        .subject(&outgoing.subject)
        .message_id(Some(id));
    if let Some(draft) = draft {
        // Drafts can have no recipients or unfinished addresses; the envelope is never submitted.
        message = message.keep_bcc().envelope(lettre::address::Envelope::new(None, vec![account.email.parse()
            .map_err(|_| Error::Invalid("invalid sender address"))?])
            .map_err(|_| Error::Invalid("invalid draft envelope"))?);
        for (name, value) in [("To", &draft.to), ("Cc", &draft.cc), ("Bcc", &draft.bcc)] {
            header_value(value)?;
            if !value.is_empty() {
                message = if let Ok(mailboxes) = value.parse::<lettre::message::Mailboxes>() {
                    match name {
                        "To" => message.header(header::To::from(mailboxes)),
                        "Cc" => message.header(header::Cc::from(mailboxes)),
                        _ => message.header(header::Bcc::from(mailboxes)),
                    }
                } else {
                    // Unfinished input is kept as text, after rejecting header control characters.
                    message.raw_header(header::HeaderValue::new(header::HeaderName::new_from_ascii_str(name), value.clone()))
                };
            }
        }
    }
    for to in &outgoing.to {
        message = message.to(mailbox(to)?);
    }
    for cc in &outgoing.cc {
        message = message.cc(mailbox(cc)?);
    }
    for bcc in &outgoing.bcc {
        message = message.bcc(mailbox(bcc)?);
    }
    if let Some(reply) = &outgoing.in_reply_to {
        header_value(reply)?;
        message = message.in_reply_to(reply.clone());
    }
    for reference in &outgoing.references {
        header_value(reference)?;
    }
    if !outgoing.references.is_empty() {
        message = message.references(outgoing.references.join(" "));
    }
    let html = markdown_html(&outgoing.text);
    let alternative = || MultiPart::alternative()
        .singlepart(SinglePart::plain(outgoing.text.clone()))
        .singlepart(SinglePart::html(html.clone().unwrap_or_default()));
    let result = if outgoing.attachments.is_empty() {
        match html {
            Some(_) => message.multipart(alternative()),
            None => message.header(header::ContentType::TEXT_PLAIN).body(outgoing.text.clone()),
        }
    } else {
        let mut mixed = if html.is_some() { MultiPart::mixed().multipart(alternative()) }
            else { MultiPart::mixed().singlepart(SinglePart::plain(outgoing.text.clone())) };
        for part in &outgoing.attachments {
            header_value(&part.filename)?;
            let mime = part.mime_type.parse().map_err(|_| Error::Invalid("invalid attachment MIME type"))?;
            mixed = mixed.singlepart(lettre::message::Attachment::new(part.filename.clone()).body(part.data.clone(), mime));
        }
        message.multipart(mixed)
    };
    result.map_err(|_| Error::Invalid("could not construct message"))
}
/// Render only parser-generated markup; images become their labels so mail loads no resources.
fn markdown_html(source: &str) -> Option<String> {
    use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};
    let mut formatted = false;
    let mut links = Vec::new();
    let events = Parser::new(source).filter_map(|event| {
        if matches!(&event, Event::Start(tag) if !matches!(tag, Tag::Paragraph))
            || matches!(&event, Event::Code(_) | Event::HardBreak | Event::Rule | Event::Html(_) | Event::InlineHtml(_)) {
            formatted = true;
        }
        match event {
            Event::Html(text) | Event::InlineHtml(text) => Some(Event::Text(text)),
            Event::Start(Tag::HtmlBlock | Tag::Image { .. }) | Event::End(TagEnd::HtmlBlock | TagEnd::Image) => None,
            Event::Start(Tag::Link { ref dest_url, .. }) => {
                let allowed = url::Url::parse(dest_url).is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "mailto"));
                links.push(allowed);
                allowed.then_some(event)
            }
            Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then_some(event),
            event => Some(event),
        }
    });
    let mut body = String::new();
    html::push_html(&mut body, events);
    formatted.then(|| format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"></head><body style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;font-size:14px;line-height:1.45\">{}</body></html>",
        body.replace("<blockquote>", "<blockquote style=\"margin:0 0 0 12px;padding-left:12px;border-left:3px solid #ccc\">")
    ))
}
/// Whole-step deadlines. lettre's own timeout bounds the TCP connect, not a server that
/// answers and then stalls; without these the worker would hang.
const LOGIN_DEADLINE: Duration = Duration::from_secs(30);
const SUBMIT_DEADLINE: Duration = Duration::from_secs(60);
const SENT_COPY_DEADLINE: Duration = Duration::from_secs(60);

/// SMTP success is irreversible. `SentCopy` means delivery succeeded; retry only
/// saving the copy, never the SMTP submission. This synchronous API leaves no queue entry.
pub async fn send(
    account: &Account,
    password: &str,
    outgoing: &Outgoing,
    store: &mut Store,
) -> Result<()> {
    send_within(account, password, outgoing, store, LOGIN_DEADLINE, SUBMIT_DEADLINE).await
}
async fn send_within(
    account: &Account,
    password: &str,
    outgoing: &Outgoing,
    store: &mut Store,
    login: Duration,
    submit: Duration,
) -> Result<()> {
    let message = compose(account, outgoing)?;
    let raw = message.formatted();
    submit_within(account, password, &message, &raw, login, submit).await?;
    save_sent(account, password, &raw, store, None).await
}

async fn submit_within(account: &Account, password: &str, message: &Message, raw: &[u8], login: Duration, submit: Duration) -> Result<()> {
    let oauth = matches!(account.auth, Auth::OAuth(_));
    let mut credential = if oauth { crate::oauth::access_token(account, None).await? } else { zeroize::Zeroizing::new(password.to_owned()) };
    for attempt in 0..2 {
        let transport = transport(account, &credential)?;
        // Authentication failures precede DATA; only those can safely refresh and retry.
        let result = async {
            match timeout(login, transport.test_connection()).await {
                Ok(Ok(true)) => {},
                Ok(Err(e)) if auth_failure(&e) => return Err(Error::Authentication),
                Ok(Err(e)) if e.is_permanent() => return Err(Error::Rejected),
                _ => return Err(Error::Network("could not reach the SMTP server")),
            }
            match timeout(submit, transport.send_raw(message.envelope(), raw)).await {
                Err(_) => Err(Error::MaybeSent),
                Ok(Err(e)) if auth_failure(&e) => Err(Error::Authentication),
                Ok(result) => result.map_err(submission_error),
            }
        }.await;
        match result {
            Err(Error::Authentication) if oauth && attempt == 0 => {
                credential = crate::oauth::access_token(account, Some(&credential)).await?;
            }
            result => { result?; break; }
        }
    }
    Ok(())
}

/// Resumes at Sent APPEND once SMTP has acknowledged delivery.
pub async fn send_queued(account: &Account, password: &str, pending: &crate::outbox::Pending, store: &mut Store) -> Result<()> {
    if pending.account != account.id {
        return Err(Error::Invalid("outbox account mismatch"));
    }
    let result = async {
        if !pending.submitted {
            let message = compose(account, &pending.mail)?;
            submit_within(account, password, &message, &pending.raw, LOGIN_DEADLINE, SUBMIT_DEADLINE).await?;
            store.submitted(&pending.id).map_err(|_| Error::MaybeSent)?;
        }
        if !pending.copied {
            save_sent(account, password, &pending.raw, store, Some(&pending.id)).await?;
        }
        store.sent(&pending.id)
    }.await;
    if let Err(error) = &result {
        store.send_failed(&pending.id, error, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64)?;
    }
    result
}

async fn save_sent(account: &Account, password: &str, raw: &[u8], store: &mut Store, queued: Option<&str>) -> Result<()> {
    if !presets::is_gmail(account) {
        let copy = async {
            let mut imap = ImapSync::login(account, password).await?;
            // shortcut: a lost APPEND acknowledgement can repeat the Sent copy; reconcile Message-ID before APPEND when duplicate copies need prevention.
            let folder = imap.append_sent(store, raw).await?;
            if let Some(id) = queued { store.copied(id)?; }
            // Refresh is best effort after the acknowledged copy has been checkpointed.
            let _ = imap.sync_folder(store, folder, crate::imap_sync::DEFAULT_LIMIT).await;
            // A failed LOGOUT cannot turn an already submitted and saved message into a send failure.
            let _ = imap.logout().await;
            Ok::<_, Error>(())
        };
        match timeout(SENT_COPY_DEADLINE, copy).await {
            Ok(Ok(())) => {}
            _ => return Err(Error::SentCopy),
        }
    }
    Ok(())
}
/// A reply code means the server refused the message. Anything else (a dropped connection, a
/// timeout) can happen after the server took it.
fn submission_error(e: lettre::transport::smtp::Error) -> Error {
    if e.is_permanent() {
        Error::Rejected
    } else if e.status().is_some() {
        Error::Network("the SMTP server refused the message")
    } else {
        Error::MaybeSent
    }
}
/// Connects and authenticates to SMTP, then quits. Sends nothing.
pub async fn verify(account: &Account, password: &str) -> Result<()> {
    let oauth = matches!(account.auth, Auth::OAuth(_));
    let mut credential = if oauth { crate::oauth::access_token(account, None).await? } else { zeroize::Zeroizing::new(password.to_owned()) };
    for attempt in 0..2 {
        let Ok(result) = timeout(LOGIN_DEADLINE, transport(account, &credential)?.test_connection()).await else {
            return Err(Error::Network("SMTP server timed out"));
        };
        match result {
            Ok(true) => return Ok(()),
            Err(e) if auth_failure(&e) && oauth && attempt == 0 => {
                credential = crate::oauth::access_token(account, Some(&credential)).await?;
            }
            Err(e) if auth_failure(&e) && oauth => return Err(Error::Authentication),
            Err(e) if e.is_permanent() => return Err(Error::Network("SMTP login rejected: check the address and app password")),
            Err(e) if e.is_tls() => return Err(Error::Network("SMTP TLS handshake failed")),
            Err(e) if e.is_timeout() => return Err(Error::Network("SMTP server timed out")),
            _ => return Err(Error::Network("could not reach the SMTP server")),
        }
    }
    Err(Error::Authentication)
}
fn auth_failure(error: &lettre::transport::smtp::Error) -> bool {
    error.status().is_some_and(|status| u16::from(status) == 535)
}

fn transport(account: &Account, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let config = &account.smtp;
    config.validate()?;
    let tls = match config.security {
        Security::Plain => Tls::None,
        Security::Tls => Tls::Wrapper(
            TlsParameters::new(config.host.clone())
                .map_err(|_| Error::Invalid("invalid SMTP TLS host"))?,
        ),
        Security::StartTls => Tls::Required(
            TlsParameters::new(config.host.clone())
                .map_err(|_| Error::Invalid("invalid SMTP TLS host"))?,
        ),
    };
    let host =
        if config.security == Security::Plain && config.host.eq_ignore_ascii_case("localhost") {
            "127.0.0.1"
        } else {
            &config.host
        };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(config.port).tls(tls)
        .credentials(Credentials::new(config.username.clone(), password.to_owned()))
        .timeout(Some(Duration::from_secs(60)));
    if matches!(account.auth, Auth::OAuth(_)) {
        builder = builder.authentication(vec![Mechanism::Xoauth2]);
    }
    Ok(builder.build())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    fn account() -> Account {
        let (i, s) = crate::presets::servers("a@gmail.com").unwrap();
        Account::new("a@gmail.com".into(), i, s).unwrap()
    }

    fn parts(text: &str) -> (String, Option<String>) {
        let message = compose(&account(), &Outgoing { to: vec!["b@example.com".into()], text: text.into(), ..Default::default() }).unwrap();
        let raw = message.formatted();
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        let wire = String::from_utf8_lossy(&raw);
        assert!(wire.contains("multipart/alternative"));
        assert!(wire.contains("text/plain"));
        assert!(wire.contains("text/html"));
        (parsed.body_text(0).unwrap().replace("\r\n", "\n"), parsed.body_html(0).map(|h| h.replace("\r\n", "\n")))
    }
    #[test]
    fn draft_recipient_fallback_keeps_unfinished_text_and_rejects_injection() {
        let mut draft = crate::outbox::Draft::default();
        for value in ["unfinished address", "Tëam <unfinished"] {
            draft.to = value.into();
            draft.cc = value.into();
            draft.bcc = value.into();
            let raw = compose_draft(&account(), &draft, "fixture").unwrap();
            let parsed = mail_parser::MessageParser::default()
                .header("To", mail_parser::HeaderForm::Text).header("Cc", mail_parser::HeaderForm::Text)
                .header("Bcc", mail_parser::HeaderForm::Text).parse(&raw).unwrap();
            for name in ["To", "Cc", "Bcc"] {
                assert_eq!(parsed.headers().get(name).unwrap().value().as_text(), Some(value));
            }
        }
        for field in 0..3 {
            let mut draft = crate::outbox::Draft::default();
            [&mut draft.to, &mut draft.cc, &mut draft.bcc][field].push_str("unfinished\r\nX-Injected: yes");
            assert!(compose_draft(&account(), &draft, "fixture").is_err());
        }
    }

    #[test]
    fn attachments_survive_mime_encoding_with_plain_and_markdown_bodies() {
        for text in ["Body", "**Body**\n\n> Forwarded message"] {
            let part = OutgoingAttachment { filename: "report.bin".into(), mime_type: "application/octet-stream".into(), data: vec![0, 255, 10, 13] };
            let mail = Outgoing { to: vec!["recipient@example.com".into()], text: text.into(), attachments: vec![part.clone()], ..Default::default() };
            let raw = compose(&account(), &mail).unwrap().formatted();
            let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
            assert_eq!(parsed.body_text(0).unwrap().replace("\r\n", "\n"), text);
            let attachment = parsed.attachments().next().unwrap();
            assert_eq!(attachment.decoded(), part.data);
            assert_eq!(parsed.attachments().count(), 1);
            assert!(String::from_utf8_lossy(&raw).contains("report.bin"));
        }
    }

    #[test]
    fn markdown_styles_and_plain_source() {
        for (source, expected) in [
            ("- ett\n- två", "<ul>"),
            ("- ett\n  - två", "<ul>\n<li>ett\n<ul>"),
            ("1. ett\n2. två", "<ol>"),
            ("**fet** och _kursiv_", "<strong>fet</strong> och <em>kursiv</em>"),
            ("[Lagom](https://example.com)", "href=\"https://example.com\""),
            ("# Rubrik", "<h1>Rubrik</h1>"),
            ("> citat", "border-left:3px solid #ccc"),
            ("`kod`", "<code>kod</code>"),
            ("```rust\nlet x = 1;\n```", "<pre><code"),
            ("**Åäö – hälsningar**", "<strong>Åäö – hälsningar</strong>"),
        ] {
            let (plain, html) = parts(source);
            assert_eq!(plain.replace("\r\n", "\n"), source);
            assert!(html.as_ref().unwrap().contains(expected), "{html:?}");
        }
    }
    #[test]
    fn markdown_escapes_html_and_drops_unsafe_links_and_images() {
        let html = markdown_html("<script>alert(1)</script>\n\n<b>hej</b>").unwrap();
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;hej&lt;/b&gt;"));
        assert!(!html.contains("<script>"));
        for target in ["javascript:alert(1)", "data:text/html,evil", "file:///tmp/a", "//example.com", "/relative"] {
            let html = markdown_html(&format!("[etikett]({target})")).unwrap();
            assert!(html.contains("etikett"));
            assert!(!html.contains("<a "), "{html}");
        }
        for target in ["http://example.com", "HTTPS://example.com", "mailto:a@example.com"] {
            assert!(markdown_html(&format!("[etikett]({target})")).unwrap().contains("<a "));
        }
        assert!(!markdown_html("![bild](https://example.com/pixel)").unwrap().contains("<img"));
    }
    #[test]
    fn plain_body_stays_single_part() {
        let source = "Hej Åsa!\nEn rad till.\n\nHälsningar, Örjan";
        assert!(markdown_html(source).is_none());
        let message = compose(&account(), &Outgoing { to: vec!["b@example.com".into()], text: source.into(), ..Default::default() }).unwrap();
        let raw = message.formatted();
        assert!(!String::from_utf8_lossy(&raw).contains("multipart/alternative"));
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.parts().len(), 1);
        assert_eq!(parsed.body_text(0).unwrap().replace("\r\n", "\n"), source);
    }

    #[tokio::test]
    async fn xoauth2_smtp_wire() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut account = account();
        account.auth = Auth::OAuth(crate::oauth::Provider::Google);
        account.smtp.host = "127.0.0.1".into();
        account.smtp.port = listener.local_addr().unwrap().port();
        account.smtp.security = Security::Plain;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"220 fake\r\n").await.unwrap();
            let mut line = String::new();
            let mut authenticated = false;
            loop {
                line.clear();
                if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                let reply: &[u8] = if line.starts_with("EHLO ") {
                    b"250-fake\r\n250 AUTH PLAIN LOGIN XOAUTH2\r\n"
                } else if let Some(value) = line.strip_prefix("AUTH XOAUTH2 ") {
                    assert_eq!(STANDARD.decode(value.trim()).unwrap(), b"user=a@gmail.com\x01auth=Bearer access-token\x01\x01");
                    authenticated = true;
                    b"235 authenticated\r\n"
                } else if line.starts_with("QUIT") { b"221 bye\r\n" }
                else { assert!(authenticated, "password mechanism used"); b"250 ok\r\n" };
                socket.get_mut().write_all(reply).await.unwrap();
            }
            assert!(authenticated);
        });
        assert!(transport(&account, "access-token").unwrap().test_connection().await.unwrap());
        server.await.unwrap();
    }

    /// A local SMTP server that answers until `stall_at` (a command prefix, or "greeting"), then
    /// says nothing more and keeps the connection open.
    async fn stalling_server(stall_at: &'static str) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    let stall = || std::future::pending::<()>();
                    if stall_at == "greeting" {
                        return stall().await;
                    }
                    write.write_all(b"220 fake\r\n").await.unwrap();
                    let mut data = false;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if data {
                            if line == "." {
                                return stall().await;
                            }
                            continue;
                        }
                        let reply: &[u8] = match line.split(' ').next().unwrap_or("").to_ascii_uppercase().as_str() {
                            "EHLO" => b"250-fake\r\n250 AUTH PLAIN\r\n",
                            "AUTH" => b"235 ok\r\n",
                            "MAIL" | "RCPT" => b"250 ok\r\n",
                            "DATA" => {
                                data = true;
                                b"354 go\r\n"
                            }
                            "QUIT" => b"221 bye\r\n",
                            _ => b"250 ok\r\n",
                        };
                        write.write_all(reply).await.unwrap();
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn a_stalling_server_ends_in_not_sent_or_maybe_sent() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("s.db")).unwrap();
        let mail = Outgoing { to: vec!["b@example.com".into()], subject: "Hi".into(), text: "x".into(), ..Default::default() };
        let account_on = |port| {
            let config = crate::account::ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "a@lagom.test".into() };
            Account::new("a@lagom.test".into(), config.clone(), config).unwrap()
        };
        let short = Duration::from_millis(300);
        // Silent before the greeting: nothing was handed over.
        let silent = account_on(stalling_server("greeting").await);
        let e = send_within(&silent, "pw", &mail, &mut store, short, short).await.unwrap_err();
        assert!(matches!(e, Error::Network(_)), "{e:?}");
        // Silent after the message body: it may have been taken.
        let after_data = account_on(stalling_server("data").await);
        let e = send_within(&after_data, "pw", &mail, &mut store, short, short).await.unwrap_err();
        assert!(matches!(e, Error::MaybeSent), "{e:?}");
    }
    #[test]
    fn reply_and_bcc_envelope() {
        let a = account();
        let m = compose(
            &a,
            &Outgoing {
                to: vec!["b@example.com".into()],
                cc: vec!["c@example.com".into()],
                bcc: vec!["hidden@example.com".into()],
                subject: "Reply".into(),
                text: "Hello".into(),
                in_reply_to: Some("<parent@test>".into()),
                references: vec!["<root@test>".into(), "<parent@test>".into()],
                message_id: Some("<kept@gmail.com>".into()),
                attachments: Vec::new(),
            },
        )
        .unwrap();
        let raw = String::from_utf8(m.formatted()).unwrap();
        assert!(!raw.to_ascii_lowercase().contains("bcc:"));
        assert!(!raw.contains("hidden@example.com"));
        assert!(raw.contains("In-Reply-To: <parent@test>"));
        assert!(raw.contains("References: <root@test> <parent@test>"));
        // A retry keeps its Message-ID; a new message gets one on the sender's domain.
        assert!(raw.contains("Message-ID: <kept@gmail.com>"));
        let fresh = compose(&a, &Outgoing { to: vec!["b@example.com".into()], ..Default::default() }).unwrap();
        let fresh = String::from_utf8(fresh.formatted()).unwrap();
        assert!(fresh.lines().any(|l| l.starts_with("Message-ID: <") && l.ends_with("@gmail.com>")));
        assert_eq!(m.envelope().to().len(), 3);
    }
    #[test]
    fn rejects_invalid_headers_and_recipients() {
        let a = account();
        assert!(compose(&a, &Outgoing::default()).is_err());
        let mut m = Outgoing {
            to: vec!["b@example.com".into()],
            subject: "bad\r\nBcc: evil@example.com".into(),
            ..Default::default()
        };
        assert!(compose(&a, &m).is_err());
        m.subject = "ok".into();
        m.to = vec!["bad address".into()];
        assert!(compose(&a, &m).is_err());
    }
}

#[cfg(test)]
#[path = "smtp_outbox_tests.rs"]
mod outbox_tests;

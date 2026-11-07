use crate::{
    account::{Account, Security},
    imap_sync::ImapSync,
    presets,
    store::Store,
    Error, Result,
};
use lettre::{
    message::{header, Mailbox},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Outgoing {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
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
    account.validate()?;
    if outgoing.to.is_empty() && outgoing.cc.is_empty() && outgoing.bcc.is_empty() {
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
    message
        .header(header::ContentType::TEXT_PLAIN)
        .body(outgoing.text.clone())
        .map_err(|_| Error::Invalid("could not construct message"))
}
/// SMTP success is irreversible. `SentCopy` means delivery succeeded; retry only
/// saving the copy, never the SMTP submission. No connection pool or idle timers.
pub async fn send(
    account: &Account,
    password: &str,
    outgoing: &Outgoing,
    store: &mut Store,
) -> Result<()> {
    let message = compose(account, outgoing)?;
    let transport = transport(account, password)?;
    let raw = message.formatted();
    // Log in first: a failure here certainly sent nothing. One extra handshake per message.
    if !matches!(transport.test_connection().await, Ok(true)) {
        return Err(Error::Network("could not reach the SMTP server"));
    }
    transport
        .send_raw(message.envelope(), &raw)
        .await
        .map_err(submission_error)?;
    if !presets::is_gmail(account) {
        let mut imap = ImapSync::login(account, password)
            .await
            .map_err(|_| Error::SentCopy)?;
        imap.append_sent(store, &raw)
            .await
            .map_err(|_| Error::SentCopy)?;
        // A failed LOGOUT cannot turn an already submitted and saved message into a send failure.
        let _ = imap.logout().await;
    }
    Ok(())
}
/// A reply code means the server refused the message. Anything else (a dropped connection, a
/// timeout) can happen after the server took it.
fn submission_error(e: lettre::transport::smtp::Error) -> Error {
    if e.status().is_some() {
        Error::Network("the SMTP server refused the message")
    } else {
        Error::MaybeSent
    }
}
/// Connects and authenticates to SMTP, then quits. Sends nothing.
pub async fn verify(account: &Account, password: &str) -> Result<()> {
    match transport(account, password)?.test_connection().await {
        Ok(true) => Ok(()),
        Err(e) if e.is_permanent() => Err(Error::Network(
            "SMTP login rejected: check the address and app password",
        )),
        Err(e) if e.is_tls() => Err(Error::Network("SMTP TLS handshake failed")),
        Err(e) if e.is_timeout() => Err(Error::Network("SMTP server timed out")),
        _ => Err(Error::Network("could not reach the SMTP server")),
    }
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
    Ok(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(config.port)
            .tls(tls)
            .credentials(Credentials::new(
                config.username.clone(),
                password.to_owned(),
            ))
            .timeout(Some(Duration::from_secs(60)))
            .build(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn account() -> Account {
        let (i, s) = crate::presets::servers("a@gmail.com").unwrap();
        Account::new("a@gmail.com".into(), i, s).unwrap()
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

use tokio::time::timeout;

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
/// Whole-step deadlines. lettre's own timeout bounds the TCP connect, not a server that
/// answers and then stalls; without these the worker, and a quit waiting on it, would hang.
const LOGIN_DEADLINE: Duration = Duration::from_secs(30);
const SUBMIT_DEADLINE: Duration = Duration::from_secs(60);
const SENT_COPY_DEADLINE: Duration = Duration::from_secs(60);

/// SMTP success is irreversible. `SentCopy` means delivery succeeded; retry only
/// saving the copy, never the SMTP submission. No connection pool or idle timers.
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
    let transport = transport(account, password)?;
    let raw = message.formatted();
    // Log in first: a failure or stall here certainly sent nothing. One extra handshake per message.
    if !matches!(timeout(login, transport.test_connection()).await, Ok(Ok(true))) {
        return Err(Error::Network("could not reach the SMTP server"));
    }
    match timeout(submit, transport.send_raw(message.envelope(), &raw)).await {
        Err(_) => return Err(Error::MaybeSent),
        Ok(result) => result.map_err(submission_error)?,
    };
    if !presets::is_gmail(account) {
        let copy = async {
            let mut imap = ImapSync::login(account, password).await?;
            imap.append_sent(store, &raw).await?;
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
    if e.status().is_some() {
        Error::Network("the SMTP server refused the message")
    } else {
        Error::MaybeSent
    }
}
/// Connects and authenticates to SMTP, then quits. Sends nothing.
pub async fn verify(account: &Account, password: &str) -> Result<()> {
    let Ok(result) = timeout(LOGIN_DEADLINE, transport(account, password)?.test_connection()).await else {
        return Err(Error::Network("SMTP server timed out"));
    };
    match result {
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
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    fn account() -> Account {
        let (i, s) = crate::presets::servers("a@gmail.com").unwrap();
        Account::new("a@gmail.com".into(), i, s).unwrap()
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

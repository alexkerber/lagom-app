use super::*;
use std::sync::{Arc, Mutex};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, net::TcpListener};

async fn smtp_fixture(mut first_reply: Option<&'static [u8]>) -> (u16, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let copies = received.clone();
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            write.write_all(b"220 fixture\r\n").await.unwrap();
            let mut body: Option<String> = None;
            while let Some(line) = lines.next_line().await.unwrap() {
                let reply: &[u8] = if let Some(raw) = &mut body {
                    if line != "." {
                        raw.push_str(&line);
                        raw.push_str("\r\n");
                        continue;
                    }
                    copies.lock().unwrap().push(body.take().unwrap());
                    match first_reply.take() { Some(b"") => break, Some(reply) => reply, None => b"250 accepted\r\n" }
                } else if line.starts_with("EHLO") { b"250-fixture\r\n250 AUTH PLAIN\r\n" }
                else if line.starts_with("AUTH") { b"235 ok\r\n" }
                else if line == "DATA" { body = Some(String::new()); b"354 continue\r\n" }
                else if line == "QUIT" { b"221 bye\r\n" }
                else { b"250 ok\r\n" };
                write.write_all(reply).await.unwrap();
                if line == "QUIT" { break; }
            }
        }
    });
    (port, received, server)
}

async fn imap_fixture(reject_first: bool, fail_sync: bool) -> (u16, Arc<Mutex<Vec<Vec<u8>>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let copies = received.clone();
    let server = tokio::spawn(async move {
        let mut reject = reject_first;
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            write.write_all(b"* OK fixture\r\n").await.unwrap();
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap() == 0 { break; }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let response = if command.starts_with("LOGIN ") { format!("{tag} OK login\r\n") }
                else if command == "CAPABILITY" { format!("* CAPABILITY IMAP4rev1\r\n{tag} OK capability\r\n") }
                else if command.starts_with("LIST ") { format!("* LIST (\\Sent) \"/\" \"Sent\"\r\n{tag} OK list\r\n") }
                else if command.starts_with("APPEND ") {
                    let len: usize = command.rsplit_once('{').unwrap().1.trim_end_matches('}').parse().unwrap();
                    write.write_all(b"+ continue\r\n").await.unwrap();
                    let mut raw = vec![0; len];
                    read.read_exact(&mut raw).await.unwrap();
                    let mut end = String::new();
                    read.read_line(&mut end).await.unwrap();
                    assert_eq!(end, "\r\n");
                    copies.lock().unwrap().push(raw);
                    if reject { reject = false; format!("{tag} NO try later\r\n") }
                    else { format!("{tag} OK appended\r\n") }
                }
                else if command.starts_with("SELECT ") && fail_sync { format!("{tag} NO refresh failed\r\n") }
                else if command.starts_with("SELECT ") { format!("* 0 EXISTS\r\n* OK [UIDVALIDITY 1] valid\r\n* OK [UIDNEXT 1] next\r\n{tag} OK selected\r\n") }
                else if command == "UID SEARCH ALL" { format!("* SEARCH\r\n{tag} OK search\r\n") }
                else if command == "LOGOUT" {
                    write.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes()).await.unwrap();
                    break;
                } else { panic!("unexpected fixture command"); };
                write.write_all(response.as_bytes()).await.unwrap();
            }
        }
    });
    (port, received, server)
}

async fn retry(reject_smtp: bool) {
    let (smtp_port, submissions, smtp_server) = smtp_fixture(reject_smtp.then_some(&b"451 try later\r\n"[..])).await;
    let (imap_port, copies, imap_server) = imap_fixture(!reject_smtp, false).await;
    let config = |port| crate::account::ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "sender@example.com".into() };
    let account = Account::new("sender@example.com".into(), config(imap_port), config(smtp_port)).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox.db");
    let mut store = Store::open(&path).unwrap();
    store.save_account(&account).unwrap();
    let mail = Outgoing { to: vec!["recipient@example.com".into()], bcc: vec!["hidden@example.com".into()], text: "**Persistent** mail".into(), ..Default::default() };
    let id = store.enqueue(&account, &mail, 0).unwrap();
    assert!(store.claim_send(&id, 0).unwrap());
    let pending = store.pending(&id).unwrap();
    let result = send_queued(&account, "fixture", &pending, &mut store).await;
    assert!(result.is_err());
    assert_eq!(matches!(result, Err(Error::SentCopy)), !reject_smtp);
    assert_eq!(store.pending(&id).unwrap().submitted, !reject_smtp);
    assert!(store.pending(&id).unwrap().error.is_some());
    drop(store);
    let mut store = Store::open(&path).unwrap();
    store.recover_outbox().unwrap();
    assert!(store.due_outbox(&account.id, 0).unwrap().is_empty());
    for id in store.due_outbox(&account.id, i64::MAX).unwrap() {
        assert!(store.claim_send(&id, i64::MAX).unwrap());
        send_queued(&account, "fixture", &store.pending(&id).unwrap(), &mut store).await.unwrap();
    }
    assert!(matches!(store.pending(&id), Err(Error::NotFound)));
    assert_eq!(submissions.lock().unwrap().len(), if reject_smtp { 2 } else { 1 });
    for raw in submissions.lock().unwrap().iter() {
        // lettre adds the CRLF before the DATA terminator even when the message ends in CRLF.
        assert_eq!(raw.as_bytes(), [pending.raw.as_slice(), b"\r\n"].concat());
    }
    assert_eq!(copies.lock().unwrap().len(), if reject_smtp { 1 } else { 2 });
    for raw in copies.lock().unwrap().iter() { assert_eq!(raw, &pending.raw); }
    smtp_server.abort();
    imap_server.abort();
}

#[tokio::test]
async fn outbox_retries_refused_smtp_after_reopen() {
    tokio::time::timeout(Duration::from_secs(10), retry(true)).await.unwrap();
}

#[tokio::test]
async fn outbox_retries_sent_copy_without_resubmitting_smtp() {
    tokio::time::timeout(Duration::from_secs(10), retry(false)).await.unwrap();
}

#[tokio::test]
async fn synchronous_failure_leaves_no_queued_delivery() {
    let (port, _, server) = smtp_fixture(Some(b"451 try later\r\n")).await;
    let config = crate::account::ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "sender@example.com".into() };
    let account = Account::new("sender@example.com".into(), config.clone(), config).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    store.save_account(&account).unwrap();
    let mail = Outgoing { to: vec!["recipient@example.com".into()], ..Default::default() };
    assert!(send(&account, "fixture", &mail, &mut store).await.is_err());
    assert!(store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn acknowledged_append_survives_cache_refresh_failure() {
    let (smtp_port, submissions, smtp_server) = smtp_fixture(None).await;
    let (imap_port, copies, imap_server) = imap_fixture(false, true).await;
    let config = |port| crate::account::ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "sender@example.com".into() };
    let account = Account::new("sender@example.com".into(), config(imap_port), config(smtp_port)).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    store.save_account(&account).unwrap();
    let mail = Outgoing { to: vec!["recipient@example.com".into()], ..Default::default() };
    let id = store.enqueue(&account, &mail, 0).unwrap();
    assert!(store.claim_send(&id, 0).unwrap());
    send_queued(&account, "fixture", &store.pending(&id).unwrap(), &mut store).await.unwrap();
    assert!(matches!(store.pending(&id), Err(Error::NotFound)));
    assert_eq!(submissions.lock().unwrap().len(), 1);
    assert_eq!(copies.lock().unwrap().len(), 1);
    smtp_server.abort();
    imap_server.abort();
}

#[tokio::test]
async fn permanent_and_ambiguous_smtp_failures_require_deliberate_retry() {
    for (reply, state) in [(&b"550 bad recipient\r\n"[..], "failed"), (&b""[..], "uncertain")] {
        let (port, submissions, server) = smtp_fixture(Some(reply)).await;
        let config = crate::account::ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "sender@example.com".into() };
        let account = Account::new("sender@example.com".into(), config.clone(), config).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("mail.db")).unwrap();
        store.save_account(&account).unwrap();
        let id = store.enqueue(&account, &Outgoing { to: vec!["recipient@example.com".into()], ..Default::default() }, 0).unwrap();
        assert!(store.claim_send(&id, 0).unwrap());
        let result = send_queued(&account, "fixture", &store.pending(&id).unwrap(), &mut store).await;
        assert!(result.is_err());
        assert_eq!(store.pending(&id).unwrap().state, state);
        store.recover_outbox().unwrap();
        assert!(store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
        assert_eq!(submissions.lock().unwrap().len(), 1);
        assert!(store.retry_send(&id, 100).unwrap());
        assert_eq!(store.due_outbox(&account.id, 100).unwrap(), [id]);
        server.abort();
    }
}

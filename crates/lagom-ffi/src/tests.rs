use super::*;
use lagom_core::store::{Message, MessageSummary};
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};
use std::{any::Any, collections::HashMap};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// The test binary replaces the backend before any credential operation; it never opens a keychain.
#[derive(Clone, Default, Debug)]
struct MemorySecrets(Arc<Mutex<HashMap<String, Vec<u8>>>>);
#[derive(Debug)]
struct MemoryEntry { store: MemorySecrets, id: String }
impl CredentialBuilderApi for MemorySecrets {
    fn build(&self, _: Option<&str>, service: &str, user: &str) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(MemoryEntry { store: self.clone(), id: format!("{service}/{user}") }))
    }
    fn as_any(&self) -> &dyn Any { self }
}
impl CredentialApi for MemoryEntry {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> { self.store.0.lock().unwrap().insert(self.id.clone(), secret.to_vec()); Ok(()) }
    fn get_secret(&self) -> keyring::Result<Vec<u8>> { self.store.0.lock().unwrap().get(&self.id).cloned().ok_or(keyring::Error::NoEntry) }
    fn delete_credential(&self) -> keyring::Result<()> { self.store.0.lock().unwrap().remove(&self.id); Ok(()) }
    fn as_any(&self) -> &dyn Any { self }
}
fn fixture() -> (tempfile::TempDir, Arc<LagomCore>) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| keyring::set_default_credential_builder(Box::<MemorySecrets>::default()));
    let dir = tempfile::tempdir().unwrap();
    let core = LagomCore::open(dir.path().to_string_lossy().into()).unwrap();
    (dir, core)
}
fn fixture_account(store: &Store, email: &str, port: u16) -> Account {
    let server = ServerConfig { host: "127.0.0.1".into(), port, username: email.into(), security: Security::Plain };
    let a = Account::new(email.into(), server.clone(), server).unwrap();
    a.set_password("fixture").unwrap();
    store.save_account(&a).unwrap();
    a
}
fn mail(store: &mut Store, a: &Account, uid: u32, date: i64) -> i64 {
    let folder = store.save_folder(&a.id, "INBOX", None, FolderRole::Inbox).unwrap();
    store.reset_uidvalidity(folder, 7).unwrap();
    store.save_message(&Message { summary: MessageSummary { account: a.id.clone(), folder, uid, date,
        from_address: "sender@example.test".into(), subject: "Hello".into(), message_id: format!("m{uid}@example.test"),
        thread_refs: "root@example.test".into(), has_attachments: true, ..Default::default() },
        body_text: "line one\nline two".into(), body_html: Some("<p>Hello</p>".into()) }).unwrap()
}

#[tokio::test]
async fn unified_pages_snooze_details_and_account_removal() {
    let (dir, core) = fixture();
    let (a, b, first, second, third) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "a@example.test", 1);
        let b = fixture_account(&s, "b@example.test", 1);
        let first = mail(&mut s, &a, 1, 20);
        let second = mail(&mut s, &b, 1, 30);
        let third = mail(&mut s, &a, 2, 30);
        (a, b, first, second, third)
    };
    assert_eq!(core.inbox(2, 0).await.unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [third, second]);
    assert_eq!(core.inbox(2, 2).await.unwrap()[0].id, first);
    assert!(core.inbox(501, 0).await.is_err());
    assert!(core.inbox(0, 0).await.unwrap().is_empty());
    assert_eq!(core.mailbox(Some(b.id.clone()), Mailbox::Inbox, 20, 0).await.unwrap().len(), 1);
    assert!(core.later(vec![first, -1], now() + 100).await.is_err());
    assert_eq!(core.inbox(20, 0).await.unwrap().len(), 3);
    core.later(vec![third], now() + 100).await.unwrap();
    assert_eq!(core.mailbox(None, Mailbox::Later, 20, 0).await.unwrap()[0].id, third);
    assert_eq!(core.inbox(20, 0).await.unwrap()[0].id, second);
    core.stores.local.lock().unwrap().snooze(third, Some(now() - 1)).unwrap();
    assert_eq!(core.inbox(20, 0).await.unwrap()[0].id, third);
    let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
    db.execute("INSERT INTO attachments VALUES (?1,'2','fixture.txt','text/plain',8,'base64')", [first]).unwrap();
    let message = core.message(first).await.unwrap();
    assert_eq!(message.attachments[0].filename, "fixture.txt");
    assert_eq!(message.attachments[0].size, 8);
    assert_eq!(message.text_body, "line one\nline two");
    assert_eq!(message.html_body.as_deref(), Some("<p>Hello</p>"));
    assert!(message.row.unread);
    assert!(core.message(-1).await.unwrap_err().to_string().contains("not found"));
    core.remove_account(a.id.clone()).await.unwrap();
    assert_eq!(core.accounts().await.unwrap()[0].id, b.id);
    assert_eq!(core.inbox(20, 0).await.unwrap().len(), 1);
    assert!(a.password().is_err());
}

#[tokio::test]
async fn replies_queue_markdown_durably_without_network_or_cross_account_sends() {
    let (dir, core) = fixture();
    let (a, b, id) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "a@example.test", 1);
        let b = fixture_account(&s, "b@example.test", 1);
        let id = mail(&mut s, &a, 1, 1);
        (a, b, id)
    };
    let reply = core.reply_draft(id).await.unwrap();
    assert_eq!(reply.account, a.id);
    assert_eq!(reply.subject, "Re: Hello");
    assert!(reply.body.contains("> line two"));
    assert!(core.send(b.id, vec![reply.to.clone()], vec![], reply.subject.clone(), "body".into(), Some(id), vec![]).await.is_err());
    assert!(core.send(a.id.clone(), vec!["invalid".into()], vec![], "subject".into(), "body".into(), None, vec![]).await.is_err());
    let queued = core.send(a.id.clone(), vec![reply.to], vec![], reply.subject, "**bold**".into(), Some(id), vec![]).await.unwrap();
    let s = core.stores.local.lock().unwrap();
    let pending = s.pending(&queued).unwrap();
    assert_eq!(pending.mail.in_reply_to.as_deref(), Some("<m1@example.test>"));
    assert_eq!(pending.mail.references, ["<root@example.test>", "<m1@example.test>"]);
    assert!(String::from_utf8_lossy(&pending.raw).contains("<strong>bold</strong>"));
    assert!(s.claim_send(&queued, i64::MAX).unwrap());
    drop(s);
    let same = LagomCore::open(dir.path().to_string_lossy().into()).unwrap();
    assert!(Arc::ptr_eq(&same.stores, &core.stores));
    assert!(same.send_failures().await.unwrap().is_empty(), "reopening must not recover an active send");
    drop(same); drop(core);
    let reopened = LagomCore::open(dir.path().to_string_lossy().into()).unwrap();
    assert!(reopened.send_failures().await.unwrap()[0].message.contains("uncertain"));
    assert!(reopened.stores.local.lock().unwrap().due_outbox(&a.id, i64::MAX).unwrap().is_empty());
}

#[tokio::test]
async fn imap_fixture_sync_and_rejected_flags_preserve_confirmed_state() {
    // SQLite migrations are setup, not part of the protocol hang guard.
    let (_dir, core) = fixture();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let a = fixture_account(&core.stores.local.lock().unwrap(), "fixture@example.test", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let mut flags = 0;
            for _ in 0..3 {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                    let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                    let data = if cmd.starts_with("LOGIN ") { String::new() }
                    else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
                    else if cmd.starts_with("LIST ") { "* LIST () NIL \"INBOX\"\r\n".into() }
                    else if cmd == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into() }
                    else if cmd == "UID SEARCH ALL" { "* SEARCH 1\r\n".into() }
                    else if cmd.contains("BODYSTRUCTURE") {
                        let header = "From: Sender <sender@example.test>\r\nSubject: Fixture\r\nMessage-ID: <fixture@example.test>\r\nContent-Type: text/plain\r\n\r\n";
                        format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 200 BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
                    } else if cmd == "UID FETCH 1 (UID BODY.PEEK[TEXT])" { "* 1 FETCH (UID 1 BODY[TEXT] {4}\r\nbody)\r\n".into() }
                    else if cmd == "UID FETCH 1 (UID FLAGS)" { "* 1 FETCH (UID 1 FLAGS ())\r\n".into() }
                    else if cmd.starts_with("UID STORE 1 ") {
                        flags += 1;
                        if flags == 2 {
                            socket.get_mut().write_all(format!("{tag} NO refused\r\n").as_bytes()).await.unwrap();
                            continue;
                        }
                        assert_eq!(cmd, "UID STORE 1 +FLAGS.SILENT (\\Seen)");
                        String::new()
                    } else if cmd == "LOGOUT" { "* BYE bye\r\n".into() }
                    else { panic!("unexpected command {cmd}"); };
                    socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                    if cmd == "LOGOUT" { break; }
                }
            }
            assert_eq!(flags, 2);
        });
        core.sync(a.id).await.unwrap();
        let row = core.inbox(10, 0).await.unwrap().remove(0);
        assert_eq!(row.sender_name, "Sender");
        assert_eq!(core.message(row.id).await.unwrap().text_body, "body");
        core.mark_read(row.id, true).await.unwrap();
        assert!(!core.message(row.id).await.unwrap().row.unread);
        assert!(core.mark_read(row.id, false).await.is_err());
        assert!(!core.message(row.id).await.unwrap().row.unread);
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn folder_failure_keeps_inbox_first_and_reconnects_for_remaining_folders() {
    for all in [false, true] {
        // Each independent fixture gets a protocol budget after its SQLite migrations.
        let (_dir, core) = fixture();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let a = fixture_account(&core.stores.local.lock().unwrap(), "fixture@example.test", listener.local_addr().unwrap().port());
            let server = tokio::spawn(async move {
                let mut selected = Vec::new();
                for connection in 0..2 {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = BufReader::new(socket);
                    socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                    let mut failed = false;
                    loop {
                        let mut line = String::new();
                        if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                        assert!(!failed, "a failed session must be dropped without another command");
                        let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                        let data = if cmd.starts_with("LOGIN ") { String::new() }
                        else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
                        else if cmd.starts_with("LIST ") {
                            "* LIST (\\Archive) NIL \"Archive\"\r\n* LIST () NIL \"INBOX\"\r\n* LIST (\\Sent) NIL \"Sent\"\r\n".into()
                        } else if let Some(name) = cmd.strip_prefix("SELECT ") {
                            selected.push((connection, name.to_owned()));
                            if name == "\"Archive\"" {
                                failed = true;
                                socket.get_mut().write_all(format!("{tag} NO refused\r\n").as_bytes()).await.unwrap();
                                continue;
                            }
                            "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into()
                        } else if cmd == "UID SEARCH ALL" { "* SEARCH 1\r\n".into() }
                        else if cmd.contains("BODYSTRUCTURE") {
                            let header = "From: sender@example.test\r\nSubject: Fixture\r\nMessage-ID: <fixture@example.test>\r\nContent-Type: text/plain\r\n\r\n";
                            format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 200 BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
                        } else if cmd == "UID FETCH 1 (UID BODY.PEEK[TEXT])" { "* 1 FETCH (UID 1 BODY[TEXT] {4}\r\nbody)\r\n".into() }
                        else if cmd == "UID FETCH 1 (UID FLAGS)" { "* 1 FETCH (UID 1 FLAGS ())\r\n".into() }
                        else if cmd == "LOGOUT" { "* BYE bye\r\n".into() }
                        else { panic!("unexpected command {cmd}"); };
                        socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                        if cmd == "LOGOUT" { break; }
                    }
                }
                selected
            });
            let result = if all { core.sync_all().await } else { core.sync(a.id.clone()).await };
            assert!(result.unwrap_err().to_string().contains("Archive"));
            let inbox = core.inbox(10, 0).await.unwrap();
            assert_eq!(inbox.len(), 1, "Archive failure must not prevent incoming mail");
            assert_eq!(core.message(inbox[0].id).await.unwrap().text_body, "body");
            assert_eq!(core.mailbox(Some(a.id), Mailbox::Sent, 10, 0).await.unwrap().len(), 1);
            assert_eq!(server.await.unwrap(), [(0, "\"INBOX\"".into()), (0, "\"Archive\"".into()), (1, "\"Sent\"".into())]);
        }).await.unwrap();
    }
}

#[test]
fn local_later_is_tomorrow_at_eight() {
    use chrono::{Local, TimeZone, Timelike};
    let timestamp = tomorrow_at_eight().unwrap();
    let next = Local.timestamp_opt(timestamp, 0).unwrap();
    assert_eq!(next.date_naive(), Local::now().date_naive().succ_opt().unwrap());
    assert_eq!((next.hour(), next.minute(), next.second()), (8, 0, 0));
}

#[test]
fn html_privacy_and_recipients_share_core_policy() {
    let html = "<LINK rel=preconnect href='https://tracker.example'><img src='https://tracker.example/pixel'><script>fetch('/secret')</script><iframe srcdoc='&lt;link rel=preconnect href=https://tracker.example&gt;'></iframe><object data='https://tracker.example'></object><embed src='https://tracker.example'><frame src='https://tracker.example'>";
    for dark in [false, true] {
        let document = mail_document(html.into(), dark, 42, false);
        assert!(document.contains("default-src 'none'; img-src data: lagom-cid:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"));
        assert!(document.find("Content-Security-Policy").unwrap() < document.find("<img").unwrap());
        assert!(document.contains("<x-LINK"));
        assert!(!document.contains("<LINK"));
        for tag in ["iframe", "object", "embed", "frame"] {
            assert!(document.contains(&format!("<x-{tag}")));
            assert!(!document.contains(&format!("<{tag}")));
            assert!(!document.contains(&format!("</{tag}")));
        }
        assert!(document.contains("system-ui,-apple-system,sans-serif"));
    }
    assert_eq!(split_recipients("\"Example, Sender\" <sender@example.test>, next@example.test".into()), ["\"Example, Sender\" <sender@example.test>", "next@example.test"]);
}

#[tokio::test]
async fn done_and_delete_move_through_imap_without_crossing_accounts() {
    // SQLite migrations are setup, not part of the protocol hang guard.
    let (_dir, core) = fixture();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let (a, other, id, untouched) = {
            let mut s = core.stores.local.lock().unwrap();
            let a = fixture_account(&s, "moving@example.test", listener.local_addr().unwrap().port());
            let other = fixture_account(&s, "other@example.test", 1);
            let id = mail(&mut s, &a, 1, 1);
            let untouched = mail(&mut s, &other, 1, 1);
            (a, other, id, untouched)
        };
        let server = tokio::spawn(async move {
            let mut location = "INBOX".to_string();
            for destination in ["Archive", "Trash"] {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                let mut selected = String::new();
                let mut moved = false;
                loop {
                    let mut line = String::new();
                    assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                    let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                    let data = if cmd.starts_with("LOGIN ") { String::new() }
                    else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1 MOVE\r\n".into() }
                    else if cmd.starts_with("LIST ") { "* LIST () NIL \"INBOX\"\r\n* LIST (\\Archive) NIL \"Archive\"\r\n* LIST (\\Trash) NIL \"Trash\"\r\n".into() }
                    else if let Some(name) = cmd.strip_prefix("SELECT ") {
                        selected = name.trim_matches('"').into();
                        "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into()
                    } else if cmd.starts_with("UID MOVE ") {
                        assert_eq!(cmd, format!("UID MOVE 1 \"{destination}\""));
                        assert_eq!(selected, location);
                        location = destination.into(); moved = true; String::new()
                    } else if cmd == "UID SEARCH ALL" {
                        if selected == location { "* SEARCH 1\r\n".into() } else { "* SEARCH\r\n".into() }
                    } else if cmd.contains("BODYSTRUCTURE") {
                        assert_eq!(selected, location);
                        let header = "From: sender@example.test\r\nSubject: Moved\r\nMessage-ID: <m1@example.test>\r\nContent-Type: text/plain\r\n\r\n";
                        format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 200 BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
                    } else if cmd == "UID FETCH 1 (UID BODY.PEEK[TEXT])" { "* 1 FETCH (UID 1 BODY[TEXT] {4}\r\nbody)\r\n".into() }
                    else if cmd == "UID FETCH 1 (UID FLAGS)" { "* 1 FETCH (UID 1 FLAGS ())\r\n".into() }
                    else if cmd == "LOGOUT" { "* BYE bye\r\n".into() }
                    else { panic!("unexpected command {cmd}"); };
                    socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                    if cmd == "LOGOUT" { break; }
                }
                assert!(moved);
            }
        });
        core.done(vec![id]).await.unwrap();
        assert_eq!(core.inbox(20, 0).await.unwrap()[0].id, untouched);
        let archived = core.mailbox(Some(a.id.clone()), Mailbox::Done, 20, 0).await.unwrap();
        assert_eq!(archived.len(), 1);
        core.delete(vec![archived[0].id]).await.unwrap();
        assert!(core.mailbox(Some(a.id.clone()), Mailbox::Done, 20, 0).await.unwrap().is_empty());
        let s = core.stores.local.lock().unwrap();
        assert_eq!(s.mailbox_messages(Some(&a.id), Some(FolderRole::Trash), now(), 20, 0).unwrap().len(), 1);
        assert_eq!(s.message_summary(untouched).unwrap().account, other.id);
        drop(s);
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn manual_add_verifies_both_servers_before_saving() {
    // SQLite migrations are setup, not part of the protocol hang guard.
    let (_dir, core) = fixture();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for accepted in [false, true] {
            let imap = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let smtp = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let config = |port| MailServer { host: "127.0.0.1".into(), port, username: "fixture@example.test".into(), security: TransportSecurity::Plain };
            let (imap_config, smtp_config) = (config(imap.local_addr().unwrap().port()), config(smtp.local_addr().unwrap().port()));
            let imap_server = tokio::spawn(async move {
                let (socket, _) = imap.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                    let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                    let data = if cmd.starts_with("LOGIN ") { "" }
                    else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" }
                    else if cmd == "LOGOUT" { "* BYE bye\r\n" }
                    else { panic!("unexpected command {cmd}"); };
                    socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                    if cmd == "LOGOUT" { break; }
                }
            });
            let smtp_server = tokio::spawn(async move {
                let (socket, _) = smtp.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"220 fixture ESMTP\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                    let data = if line.starts_with("EHLO ") { "250-fixture\r\n250 AUTH PLAIN\r\n" }
                    else if line.starts_with("AUTH PLAIN ") { if accepted { "235 authenticated\r\n" } else { "535 rejected\r\n" } }
                    else if line == "NOOP\r\n" { "250 OK\r\n" }
                    else if line == "QUIT\r\n" { "221 bye\r\n" }
                    else { panic!("unexpected SMTP command"); };
                    socket.get_mut().write_all(data.as_bytes()).await.unwrap();
                    if line == "QUIT\r\n" || !accepted && line.starts_with("AUTH ") { break; }
                }
            });
            let result = core.add_account_manual("fixture@example.test".into(), "fixture".into(), imap_config, smtp_config).await;
            assert_eq!(result.is_ok(), accepted);
            assert_eq!(core.accounts().await.unwrap().len(), usize::from(accepted));
            if let Ok(a) = result { assert_eq!(&*account(&core.stores.local.lock().unwrap(), &a.id).unwrap().password().unwrap(), "fixture"); }
            imap_server.await.unwrap(); smtp_server.await.unwrap();
        }
        assert!(core.add_account("not-an-address".into(), "fixture".into()).await.is_err());
        assert!(core.sync("missing-account".into()).await.is_err());
    }).await.unwrap();
}

#[tokio::test]
async fn stalled_sync_keeps_cached_mail_and_durable_enqueue_available() {
    let (dir, core) = fixture();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let (a, id) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "slow@example.test", listener.local_addr().unwrap().port());
        let id = mail(&mut s, &a, 1, 1);
        (a, id)
    };
    let sync = tokio::spawn({ let core = core.clone(); async move { core.sync_all().await } });
    let (socket, _) = listener.accept().await.unwrap();
    let local = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let same = LagomCore::open(dir.path().to_string_lossy().into()).unwrap();
        assert_eq!(same.message(id).await.unwrap().text_body, "line one\nline two");
        assert_eq!(same.mailbox(Some(a.id.clone()), Mailbox::Inbox, 100, 0).await.unwrap()[0].id, id);
        assert!(same.mailbox(None, Mailbox::Done, 100, 0).await.unwrap().is_empty());
        let queued = same.send(a.id.clone(), vec!["to@example.test".into()], vec![], "Queued".into(), "body".into(), None, vec![]).await.unwrap();
        let disk = Store::open(dir.path().join("lagom.sqlite3")).unwrap();
        assert_eq!(disk.pending(&queued).unwrap().mail.subject, "Queued");
    }).await;
    drop(socket);
    assert!(sync.await.unwrap().is_err());
    assert!(local.is_ok(), "cached mail and enqueue waited for the IMAP greeting");
}

#[tokio::test]
async fn done_clears_snooze_when_already_in_archive_but_not_on_failure() {
    let (_dir, core) = fixture();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let id = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "archived@example.test", listener.local_addr().unwrap().port());
        let id = mail(&mut s, &a, 1, 1);
        let mut message = s.message(id).unwrap();
        s.delete_message(id).unwrap();
        message.summary.folder = s.save_folder(&a.id, "Archive", None, FolderRole::Archive).unwrap();
        s.save_message(&message).unwrap()
    };
    let server = tokio::spawn(async move {
        for accepted in [false, true] {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
            loop {
                let mut line = String::new();
                if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                let data = if cmd.starts_with("LOGIN ") {
                    if !accepted {
                        socket.get_mut().write_all(format!("{tag} NO refused\r\n").as_bytes()).await.unwrap();
                        break;
                    }
                    ""
                }
                else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" }
                else if cmd.starts_with("LIST ") {
                    "* LIST (\\Archive) NIL \"Archive\"\r\n"
                } else if cmd == "LOGOUT" { "* BYE bye\r\n" }
                else { panic!("unexpected command {cmd}"); };
                socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                if cmd == "LOGOUT" { break; }
            }
        }
    });
    core.later(vec![id], now() + 3600).await.unwrap();
    assert!(core.done(vec![id]).await.is_err());
    assert_eq!(core.mailbox(None, Mailbox::Later, 100, 0).await.unwrap()[0].id, id);
    core.done(vec![id]).await.unwrap();
    server.await.unwrap();
    assert!(core.mailbox(None, Mailbox::Later, 100, 0).await.unwrap().is_empty());
    assert_eq!(core.mailbox(None, Mailbox::Done, 100, 0).await.unwrap()[0].id, id);
}

#[tokio::test]
async fn next_snooze_uses_earliest_future_deadline_across_accounts() {
    let (_dir, core) = fixture();
    let (first, second) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "a@example.test", 1);
        let b = fixture_account(&s, "b@example.test", 1);
        (mail(&mut s, &a, 1, 1), mail(&mut s, &b, 1, 2))
    };
    assert_eq!(core.next_snooze().await.unwrap(), None);
    let due = now() + 3600;
    core.later(vec![first], due + 100).await.unwrap();
    core.later(vec![second], due).await.unwrap();
    assert_eq!(core.next_snooze().await.unwrap(), Some(due));
    core.stores.local.lock().unwrap().snooze(second, Some(now() - 1)).unwrap();
    assert_eq!(core.next_snooze().await.unwrap(), Some(due + 100));
    core.stores.local.lock().unwrap().snooze(first, None).unwrap();
    assert_eq!(core.next_snooze().await.unwrap(), None);
}

#[tokio::test]
async fn a_panic_in_one_action_leaves_the_store_usable() {
    let (_dir, core) = fixture();
    for network in [false, true] {
        assert!(core.run_on(network, |_| -> lagom_core::Result<()> { panic!("action panicked") }).await.is_err());
        assert!(core.run_on(network, |s| s.accounts()).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn reply_all_uses_source_identity_cc_signature_and_thread() {
    let (_dir, core) = fixture();
    let (a, id) = {
        let mut s = core.stores.local.lock().unwrap();
        let mut a = fixture_account(&s, "me@example.test", 1);
        a.imap.username = "alias@example.test".into();
        s.save_account(&a).unwrap();
        s.set_signature(&a.id, "Team").unwrap();
        let id = mail(&mut s, &a, 1, 1);
        let mut message = s.message(id).unwrap();
        message.summary.to = "Me <ME@example.test>, Alias <alias@example.test>, Other <other@example.test>".into();
        message.summary.cc = "other@example.test, Copy <copy@example.test>".into();
        s.save_message(&message).unwrap();
        (a, id)
    };
    let reply = core.compose_draft(id, ComposeKind::ReplyAll).await.unwrap();
    assert_eq!(reply.account, a.id);
    assert_eq!(reply.to, "sender@example.test, Other <other@example.test>");
    assert_eq!(reply.cc, "Copy <copy@example.test>");
    assert_eq!(reply.reply_to, Some(id));
    assert!(reply.body.starts_with("\n\n-- \nTeam\n\n> On "));
    assert!(reply.body.ends_with("> line one\n> line two"));
    assert!(reply.attachments.is_empty());
    let queued = core.send(reply.account, split_recipients(reply.to), split_recipients(reply.cc), reply.subject,
        reply.body, reply.reply_to, reply.attachments).await.unwrap();
    let pending = core.stores.local.lock().unwrap().pending(&queued).unwrap();
    assert_eq!(pending.mail.cc, ["Copy <copy@example.test>"]);
    assert_eq!(pending.mail.in_reply_to.as_deref(), Some("<m1@example.test>"));
    assert_eq!(pending.mail.references, ["<root@example.test>", "<m1@example.test>"]);
    for kind in [ComposeKind::Reply, ComposeKind::ReplyAll, ComposeKind::Forward] {
        assert!(core.compose_draft(-1, kind).await.is_err());
    }
}

#[tokio::test]
async fn forward_preserves_cached_attachments_offline_and_starts_a_new_thread() {
    let (dir, core) = fixture();
    let (a, id) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "me@example.test", 1);
        let id = mail(&mut s, &a, 1, 1);
        (a, id)
    };
    let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
    db.execute("UPDATE messages SET attachments_loaded=1 WHERE id=?1", [id]).unwrap();
    db.execute("INSERT INTO attachments VALUES (?1,'2','fixture.txt','text/plain',4,'base64')", [id]).unwrap();
    db.execute("INSERT INTO attachment_cache VALUES (?1,'2',?2)", rusqlite::params![id, b"file".as_slice()]).unwrap();
    let forward = core.compose_draft(id, ComposeKind::Forward).await.unwrap();
    assert_eq!(forward.account, a.id);
    assert!(forward.to.is_empty() && forward.cc.is_empty());
    assert_eq!(forward.subject, "Fwd: Hello");
    assert!(forward.reply_to.is_none());
    assert!(forward.body.contains("> Forwarded message\n> From: "));
    assert!(forward.body.ends_with("> line one\n> line two"));
    assert_eq!(forward.attachments.len(), 1);
    assert_eq!(forward.attachments[0].data, b"file");
    let queued = core.send(forward.account, vec!["next@example.test".into()], vec![], forward.subject,
        forward.body, forward.reply_to, forward.attachments).await.unwrap();
    let pending = core.stores.local.lock().unwrap().pending(&queued).unwrap();
    assert!(pending.mail.in_reply_to.is_none() && pending.mail.references.is_empty());
    assert_eq!(pending.mail.attachments[0].filename, "fixture.txt");
    assert_eq!(pending.mail.attachments[0].mime_type, "text/plain");
    assert_eq!(pending.mail.attachments[0].data, b"file");
    assert!(String::from_utf8_lossy(&pending.raw).contains("filename=\"fixture.txt\""));
}

#[tokio::test]
async fn forward_refuses_unknown_or_unavailable_attachments_instead_of_dropping_them() {
    let (dir, core) = fixture();
    let id = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "me@example.test", 1);
        mail(&mut s, &a, 1, 1)
    };
    assert!(core.compose_draft(id, ComposeKind::Forward).await.unwrap_err().to_string().contains("sync attachments"));
    let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
    db.execute("UPDATE messages SET attachments_loaded=1 WHERE id=?1", [id]).unwrap();
    assert!(core.compose_draft(id, ComposeKind::Forward).await.unwrap().attachments.is_empty());
    db.execute("INSERT INTO attachments VALUES (?1,'2','fixture.txt','text/plain',4,'base64')", [id]).unwrap();
    assert!(core.compose_draft(id, ComposeKind::Forward).await.is_err());
    assert!(core.stores.local.lock().unwrap().failed_outbox().unwrap().is_empty());
}

#[tokio::test]
async fn forward_fetches_only_missing_parts_and_reuses_the_cache() {
    // SQLite migrations are setup, not part of the protocol hang guard.
    let (dir, core) = fixture();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let id = {
            let mut s = core.stores.local.lock().unwrap();
            let a = fixture_account(&s, "fixture@example.test", listener.local_addr().unwrap().port());
            mail(&mut s, &a, 1, 1)
        };
        let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
        db.execute("UPDATE messages SET attachments_loaded=1 WHERE id=?1", [id]).unwrap();
        db.execute("INSERT INTO attachments VALUES (?1,'2','cached.txt','text/plain',4,'7bit')", [id]).unwrap();
        db.execute("INSERT INTO attachment_cache VALUES (?1,'2',?2)", rusqlite::params![id, b"file".as_slice()]).unwrap();
        db.execute("INSERT INTO attachments VALUES (?1,'3','data.bin','application/octet-stream',8,'base64')", [id]).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
            let mut downloads = 0;
            loop {
                let mut line = String::new();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                let data = if cmd.starts_with("LOGIN ") { "" }
                else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" }
                else if cmd == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n" }
                else if cmd == "UID FETCH 1 (UID BODY.PEEK[3])" {
                    downloads += 1;
                    "* 1 FETCH (UID 1 BODY[3] {8}\r\nAP9oaQ==)\r\n"
                } else if cmd == "LOGOUT" { "* BYE bye\r\n" }
                else { panic!("unexpected command {cmd}"); };
                socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                if cmd == "LOGOUT" { break; }
            }
            assert_eq!(downloads, 1);
        });
        for _ in 0..2 {
            let draft = core.compose_draft(id, ComposeKind::Forward).await.unwrap();
            assert_eq!(draft.attachments.len(), 2);
            assert_eq!(draft.attachments[0].data, b"file");
            assert_eq!(draft.attachments[1].data, b"\0\xffhi");
        }
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn inline_resources_are_scoped_to_the_open_message() {
    use lagom_core::inline::url;
    let (dir, core) = fixture();
    let (first, second) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "a@example.test", 1);
        let b = fixture_account(&s, "b@example.test", 1);
        (mail(&mut s, &a, 1, 1), mail(&mut s, &b, 1, 2))
    };
    let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
    for (id, bytes) in [(first, b"first".as_slice()), (second, b"second".as_slice())] {
        db.execute("INSERT INTO inline_parts VALUES (?1,'same','image/png',?2)", rusqlite::params![id, bytes]).unwrap();
        db.execute("UPDATE messages SET inline_loaded=1 WHERE id=?1", [id]).unwrap();
        core.load_inline(id).await.unwrap();
        let resource = core.inline_resource(id, url(id, "same", false)).await.unwrap();
        assert_eq!(resource.data, bytes);
        assert_eq!(resource.mime_type, "image/png");
    }
    db.execute("INSERT INTO inline_parts VALUES (?1,'private','image/png',?2)", rusqlite::params![second, b"private".as_slice()]).unwrap();
    for request in [url(second, "same", false), url(first, "private", false), url(second, "private", false), url(first, "missing", false),
        format!("{}?other=1", url(first, "same", false)), "lagom-cid://evil/1/c2FtZQ".into(),
        "file:///1/c2FtZQ".into(), "https://example.test/pixel".into()] {
        assert!(core.inline_resource(first, request).await.is_err());
    }
    assert!(core.inline_resource(-1, url(first, "same", false)).await.is_err());
    assert!(core.load_inline(-1).await.is_err());
    core.stores.local.lock().unwrap().delete_message(first).unwrap();
    assert!(core.inline_resource(first, url(first, "same", false)).await.is_err());
}

#[tokio::test]
async fn inline_resource_reads_only_the_requested_blob() {
    use lagom_core::inline::url;
    let (dir, core) = fixture();
    let id = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "fixture@example.test", 1);
        mail(&mut s, &a, 1, 1)
    };
    let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
    db.execute("WITH RECURSIVE ids(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM ids WHERE n<1023)
        INSERT INTO inline_parts SELECT ?1, CAST(n AS TEXT), 'image/png', zeroblob(10240) FROM ids", [id]).unwrap();
    // Evaluating any unrelated BLOB fails deterministically instead of timing cache amplification.
    db.execute_batch("ALTER TABLE inline_parts RENAME TO cached_parts;
        CREATE VIEW inline_parts AS SELECT message,cid,mime_type,
        CASE WHEN cid='0' THEN data ELSE abs(-9223372036854775808) END AS data FROM cached_parts;").unwrap();
    for windows in [false, true] {
        let resource = core.inline_resource(id, url(id, "0", windows)).await.unwrap();
        assert_eq!(resource.mime_type, "image/png");
        assert_eq!(resource.data, vec![0; 10240]);
    }
    for request in [url(id + 1, "0", false), url(id, "missing", false),
        format!("{}?query=1", url(id, "0", false)), "https://example.test/pixel".into()] {
        assert_eq!(core.inline_resource(id, request).await.unwrap_err().to_string(), "not found");
    }
}

#[test]
fn mobile_document_rewrites_cids_and_only_opts_in_remote_images() {
    let html = "<img src='cid:logo'><img src='https://example.test/pixel'><link rel=prefetch href='https://example.test'>";
    for remote in [false, true] {
        let document = mail_document(html.into(), false, 42, remote);
        assert!(document.contains(&lagom_core::inline::url(42, "logo", false)));
        assert!(!document.contains("src='cid:logo'"));
        assert!(document.contains(&lagom_core::html::csp(remote, "lagom-cid:")));
        assert_eq!(document.contains("lagom-cid: https: http:"), remote);
        assert!(document.contains("<x-link"));
    }
}

#[tokio::test]
async fn unified_search_uses_desktop_prefixes_and_pages_across_accounts() {
    let (_dir, core) = fixture();
    let (a, first, second, snoozed) = {
        let mut s = core.stores.local.lock().unwrap();
        let a = fixture_account(&s, "a@example.test", 1);
        let b = fixture_account(&s, "b@example.test", 1);
        let first = mail(&mut s, &a, 1, 30);
        let second = mail(&mut s, &b, 1, 20);
        let snoozed = mail(&mut s, &a, 2, 10);
        s.snooze(snoozed, Some(now() + 3600)).unwrap();
        (a, first, second, snoozed)
    };
    assert_eq!(core.search(None, "hel".into(), 1, 0).await.unwrap()[0].id, first);
    assert_eq!(core.search(None, "hello".into(), 1, 1).await.unwrap()[0].id, second);
    assert!(core.search(None, "hello".into(), 1, 2).await.unwrap().is_empty());
    assert_eq!(core.search(Some(a.id), "line on".into(), 100, 0).await.unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [first]);
    for query in ["", "   ", "no-match", "\" OR nonsense"] {
        assert!(core.search(None, query.into(), 100, 0).await.unwrap().is_empty());
    }
    assert!(core.search(None, "hello".into(), 501, 0).await.is_err());
    assert!(core.search(None, "a".repeat(4097), 100, 0).await.is_err());
    assert!(core.search(Some("missing".into()), "hello".into(), 100, 0).await.is_err());
    core.stores.local.lock().unwrap().snooze(snoozed, None).unwrap();
    assert_eq!(core.search(None, "hello".into(), 100, 0).await.unwrap().len(), 3);
}

#[tokio::test]
async fn inline_loading_fetches_legacy_mail_and_reuses_cache() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let (dir, core) = fixture();
        let id = {
            let mut s = core.stores.local.lock().unwrap();
            let a = fixture_account(&s, "fixture@example.test", listener.local_addr().unwrap().port());
            mail(&mut s, &a, 1, 1)
        };
        // HTML without CID references needs no network, even before the cache was classified.
        core.load_inline(id).await.unwrap();
        let db = rusqlite::Connection::open(dir.path().join("lagom.sqlite3")).unwrap();
        db.execute("UPDATE messages SET body_html='<img src=\"cid:logo\">' WHERE id=?1", [id]).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
            loop {
                let mut line = String::new();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                let data = if cmd.starts_with("LOGIN ") { "" }
                else if cmd == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" }
                else if cmd == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n" }
                else if cmd == "UID FETCH 1 (UID BODYSTRUCTURE)" {
                    "* 1 FETCH (UID 1 BODYSTRUCTURE ((\"TEXT\" \"HTML\" NIL NIL NIL \"7BIT\" 20 1)(\"IMAGE\" \"PNG\" NIL \"<logo>\" NIL \"BASE64\" 8) \"RELATED\"))\r\n"
                } else if cmd == "UID FETCH 1 (UID BODY.PEEK[2])" { "* 1 FETCH (UID 1 BODY[2] {8}\r\nAP9oaQ==)\r\n" }
                else if cmd == "LOGOUT" { "* BYE bye\r\n" }
                else { panic!("unexpected command {cmd}"); };
                socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                if cmd == "LOGOUT" { break; }
            }
        });
        core.load_inline(id).await.unwrap();
        server.await.unwrap();
        core.load_inline(id).await.unwrap();
        assert_eq!(core.inline_resource(id, lagom_core::inline::url(id, "logo", false)).await.unwrap().data, b"\0\xffhi");
    }).await.unwrap();
}

use futures_util::TryStreamExt;
use lagom_core::{
    account::{Account, Security, ServerConfig},
    imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp::{self, Outgoing},
    store::{FolderRole, Store},
};
use tokio::net::TcpStream;

fn account() -> Account {
    let email = format!("core-{}@lagom.test", uuid::Uuid::new_v4().simple());
    let config = |port| ServerConfig {
        host: "127.0.0.1".into(),
        port,
        security: Security::Plain,
        username: email.clone(),
    };
    Account::new(email.clone(), config(3143), config(3025)).unwrap()
}
async fn raw(account: &Account) -> async_imap::Session<TcpStream> {
    let mut client =
        async_imap::Client::new(TcpStream::connect(("127.0.0.1", 3143)).await.unwrap());
    client.read_response().await.unwrap().unwrap();
    client.login(&account.imap.username, "test").await.unwrap()
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
async fn deliver(account: &Account, store: &mut Store, subject: &str) {
    smtp::send(
        account,
        "test",
        &Outgoing {
            to: vec![account.email.clone()],
            subject: subject.into(),
            text: format!("Body for {subject}"),
            ..Default::default()
        },
        store,
    )
    .await
    .unwrap();
}
fn folder(store: &Store, account: &Account, role: FolderRole) -> i64 {
    store
        .folders(&account.id, now())
        .unwrap()
        .into_iter()
        .find(|f| f.role == role)
        .unwrap()
        .id
}

#[tokio::test]
#[cfg_attr(not(lagom_greenmail), ignore = "set LAGOM_GREENMAIL=1; see README")]
async fn mail_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let account = account();
    store.save_account(&account).unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    let folders = imap.list_folders(&store).await.unwrap();
    assert!(folders.iter().any(|f| f.role == FolderRole::Inbox));
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    deliver(&account, &mut store, "First").await;
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    assert_eq!(messages.len(), 1);
    let first = messages[0].clone();
    assert_eq!(first.subject, "First");
    assert!(store
        .message(first.id)
        .unwrap()
        .body_text
        .contains("Body for First"));
    assert!(!first.seen);
    assert_eq!(
        store
            .list_messages(folder(&store, &account, FolderRole::Sent), now(), 20, 0)
            .unwrap()
            .len(),
        1
    );
    imap.mark_seen(&mut store, first.id, true).await.unwrap();
    assert!(store.message(first.id).unwrap().summary.seen);
    imap.mark_seen(&mut store, first.id, false).await.unwrap();
    assert!(!store.message(first.id).unwrap().summary.seen);
    let mut external = raw(&account).await;
    external.select("INBOX").await.unwrap();
    external
        .uid_store(first.uid.to_string(), "+FLAGS (\\Seen \\Flagged)")
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    imap.sync_folder(&mut store, inbox, DEFAULT_LIMIT)
        .await
        .unwrap();
    assert!(store.message(first.id).unwrap().summary.seen);
    assert!(store.message(first.id).unwrap().summary.flagged);
    store.snooze(first.id, Some(now() + 3600)).unwrap();
    assert!(store.list_messages(inbox, now(), 20, 0).unwrap().is_empty());
    assert_eq!(store.counts(&account.id, now()).unwrap().later, 1);
    store.snooze(first.id, None).unwrap();
    deliver(&account, &mut store, "Second").await;
    imap.sync_folder(&mut store, inbox, DEFAULT_LIMIT)
        .await
        .unwrap();
    assert_eq!(store.list_messages(inbox, now(), 20, 0).unwrap().len(), 2);
    assert_eq!(store.message(first.id).unwrap().summary.subject, "First");
    let custom = imap.create_folder(&store, "Receipts").await.unwrap();
    imap.move_message(&mut store, first.id, custom)
        .await
        .unwrap();
    assert_eq!(store.list_messages(inbox, now(), 20, 0).unwrap().len(), 1);
    let moved = store.list_messages(custom, now(), 20, 0).unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].subject, "First");
    imap.archive(&mut store, moved[0].id).await.unwrap();
    let archive = folder(&store, &account, FolderRole::Archive);
    let archived = store.list_messages(archive, now(), 20, 0).unwrap();
    assert_eq!(archived.len(), 1);
    assert!(store
        .list_messages(custom, now(), 20, 0)
        .unwrap()
        .is_empty());
    imap.trash(&mut store, archived[0].id).await.unwrap();
    assert_eq!(
        store
            .list_messages(folder(&store, &account, FolderRole::Trash), now(), 20, 0)
            .unwrap()
            .len(),
        1
    );
    let second = store.list_messages(inbox, now(), 20, 0).unwrap().remove(0);
    external.select("INBOX").await.unwrap();
    external
        .uid_store(second.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    external
        .uid_expunge(second.uid.to_string())
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    imap.sync_folder(&mut store, inbox, DEFAULT_LIMIT)
        .await
        .unwrap();
    assert!(store.list_messages(inbox, now(), 20, 0).unwrap().is_empty());
    assert!(store
        .search(&account.id, "Second", now(), 20, 0)
        .unwrap()
        .iter()
        .all(|m| m.folder != inbox));
    external.logout().await.unwrap();
    imap.logout().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(lagom_greenmail), ignore = "set LAGOM_GREENMAIL=1; see README")]
async fn bounded_initial_sync_multipart_and_idle() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let account = account();
    store.save_account(&account).unwrap();
    let mut external = raw(&account).await;
    for index in 0..4 {
        let raw = format!(
            "From: alice@example.com\r\nTo: {}\r\nSubject: Older {index}\r\n\r\nOld body",
            account.email
        );
        external.append("INBOX", None, None, raw).await.unwrap();
    }
    let raw=b"From: Alice <alice@example.com>\r\nSubject: Multipart\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: multipart/alternative; boundary=y\r\n\r\n--y\r\nContent-Type: text/plain\r\n\r\n[Image] Plain part\r\n--y\r\nContent-Type: text/html; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<style>p{color:red}</style><p>H=E4lsning</p>\r\n--y--\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\n\r\nAttachment bytes must not be fetched\r\n--x--\r\n";
    external.append("INBOX", None, None, raw).await.unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    imap.sync(&mut store, 2).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| !m.thread_key.is_empty()));
    let mime = messages.iter().find(|m| m.subject == "Multipart").unwrap();
    assert!(mime.has_attachments);
    let body = store.message(mime.id).unwrap();
    // Both parts of multipart/alternative; the snippet comes from the HTML.
    assert!(body.body_text.contains("Plain part"));
    assert!(body.body_html.unwrap().contains("Hälsning"));
    assert_eq!(mime.snippet, "Hälsning");
    assert!(!body.body_text.contains("Attachment bytes"));
    let idle = ImapSync::login(&account, "test").await.unwrap();
    let change = async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        external
            .append("INBOX", None, None, b"Subject: IDLE change\r\n\r\nWake up")
            .await
            .unwrap();
    };
    let wait = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), idle.idle_inbox())
            .await
            .unwrap()
            .unwrap()
    };
    let (changed, ()) = tokio::join!(wait, change);
    assert!(changed);
    imap.sync_folder(&mut store, inbox, 2).await.unwrap();
    assert_eq!(store.list_messages(inbox, now(), 20, 0).unwrap().len(), 3);
    external.logout().await.unwrap();
    imap.logout().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(lagom_greenmail), ignore = "set LAGOM_GREENMAIL=1; see README")]
async fn copy_uid_expunge_fallback_preserves_other_deleted_messages() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    // GreenMail supports MOVE. Hide only that capability to exercise its real UIDPLUS fallback.
    let proxy = tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let server = TcpStream::connect(("127.0.0.1", 3143)).await.unwrap();
        let (mut cr, mut cw) = client.into_split();
        let (sr, mut sw) = server.into_split();
        let upstream = async {
            tokio::io::copy(&mut cr, &mut sw).await?;
            sw.shutdown().await
        };
        let downstream = async {
            let mut read = BufReader::new(sr);
            loop {
                let mut line = Vec::new();
                if read.read_until(b'\n', &mut line).await? == 0 {
                    break;
                }
                if line.starts_with(b"* CAPABILITY ") {
                    line = String::from_utf8_lossy(&line)
                        .replace(" MOVE", "")
                        .into_bytes();
                }
                cw.write_all(&line).await?;
            }
            cw.shutdown().await
        };
        tokio::try_join!(upstream, downstream).unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("fallback.db")).unwrap();
    let mut account = account();
    let mut external = raw(&account).await;
    for subject in ["Move me", "Keep deleted"] {
        let raw = format!("Subject: {subject}\r\n\r\nBody");
        external.append("INBOX", None, None, raw).await.unwrap();
    }
    account.imap.port = port;
    store.save_account(&account).unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    let moved = messages.iter().find(|m| m.subject == "Move me").unwrap();
    let kept = messages
        .iter()
        .find(|m| m.subject == "Keep deleted")
        .unwrap();
    external.select("INBOX").await.unwrap();
    external
        .uid_store(kept.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let target = imap.create_folder(&store, "Fallback").await.unwrap();
    imap.move_message(&mut store, moved.id, target)
        .await
        .unwrap();
    let present = external.uid_search("ALL").await.unwrap();
    assert!(present.contains(&kept.uid));
    assert!(!present.contains(&moved.uid));
    assert_eq!(store.list_messages(target, now(), 20, 0).unwrap().len(), 1);
    imap.logout().await.unwrap();
    external.logout().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), proxy)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[cfg_attr(not(lagom_greenmail), ignore = "set LAGOM_GREENMAIL=1; see README")]
async fn oversized_body_is_stored_as_metadata_and_sync_goes_on() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let account = account();
    store.save_account(&account).unwrap();
    let mut external = raw(&account).await;
    let line = "x".repeat(76) + "\r\n";
    let huge = format!(
        "From: Big <big@example.com>\r\nTo: {}\r\nSubject: Huge\r\n\r\n{}",
        account.email,
        line.repeat(5 * 1024 * 1024 / line.len())
    );
    external.append("INBOX", None, None, huge).await.unwrap();
    external
        .append("INBOX", None, None, b"Subject: After\r\n\r\nSmall body")
        .await
        .unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    let huge = messages.iter().find(|m| m.subject == "Huge").unwrap();
    assert_eq!(store.message(huge.id).unwrap().body_text, lagom_core::mime::BODY_UNAVAILABLE);
    assert!(messages.iter().any(|m| m.subject == "After"));
    // The checkpoint moved past both: a second sync keeps the two rows and adds none.
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    assert_eq!(store.list_messages(inbox, now(), 20, 0).unwrap().len(), 2);
    external.logout().await.unwrap();
    imap.logout().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(lagom_greenmail), ignore = "set LAGOM_GREENMAIL=1; see README")]
async fn oversized_header_is_stored_and_sync_goes_on() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let account = account();
    store.save_account(&account).unwrap();
    let mut external = raw(&account).await;
    let pad = "X-Pad: ".to_string() + &"y".repeat(990) + "\r\n";
    let huge = format!("Subject: Big header\r\n{}\r\nBody", pad.repeat(300));
    external.append("INBOX", None, None, huge).await.unwrap();
    external
        .append("INBOX", None, None, b"Subject: Later UID\r\n\r\nSmall body")
        .await
        .unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    assert_eq!(messages.len(), 2);
    let big = messages.iter().find(|m| m.subject != "Later UID").unwrap();
    assert_eq!(store.message(big.id).unwrap().body_text, lagom_core::mime::BODY_UNAVAILABLE);
    external.logout().await.unwrap();
    imap.logout().await.unwrap();
}

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
    let raw=b"From: Alice <alice@example.com>\r\nSubject: Multipart\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<p>H=E4lsning</p>\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\n\r\nAttachment bytes must not be fetched\r\n--x--\r\n";
    external.append("INBOX", None, None, raw).await.unwrap();
    let mut imap = ImapSync::login(&account, "test").await.unwrap();
    imap.sync(&mut store, 2).await.unwrap();
    let inbox = folder(&store, &account, FolderRole::Inbox);
    let messages = store.list_messages(inbox, now(), 20, 0).unwrap();
    assert_eq!(messages.len(), 2);
    let mime = messages.iter().find(|m| m.subject == "Multipart").unwrap();
    assert!(mime.has_attachments);
    let body = store.message(mime.id).unwrap();
    assert!(body.body_text.contains("Hälsning"));
    assert!(body.body_html.is_some());
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

use lagom_core::{
    account::{Account, Security, ServerConfig},
    imap_sync::ImapSync,
    store::{FolderRole, Store},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn sync_only_fetches_text_then_downloads_selected_parts_and_guards_uidvalidity() {
    attachment_roundtrip(false).await;
}

#[tokio::test]
async fn cached_messages_gain_attachment_metadata_on_sync() {
    attachment_roundtrip(true).await;
}

async fn attachment_roundtrip(cached: bool) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        write.write_all(b"* OK fixture\r\n").await.unwrap();
        let mut synced = false;
        let mut searches = 0;
        let mut selections = 0;
        let mut downloads = 0;
        loop {
            let mut line = String::new();
            assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = if command.starts_with("LOGIN ") {
                String::new()
            } else if command == "CAPABILITY" {
                "* CAPABILITY IMAP4rev1\r\n".into()
            } else if command == "SELECT \"INBOX\"" {
                selections += 1;
                let validity = if selections > 4 { 8 } else { 7 };
                format!("* 1 EXISTS\r\n* OK [UIDVALIDITY {validity}] valid\r\n* OK [UIDNEXT 2] next\r\n")
            } else if command == "UID SEARCH ALL" {
                searches += 1;
                synced = searches >= 2;
                "* SEARCH 1\r\n".into()
            } else if command.contains("BODYSTRUCTURE") {
                assert!(!synced);
                assert_eq!(command, "UID FETCH 1 (UID ENVELOPE FLAGS BODYSTRUCTURE RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER]<0.262144>)");
                let header = "Subject: Files\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n";
                let structure = r#"(("TEXT" "PLAIN" NIL NIL NIL "7BIT" 5 1)(("APPLICATION" "OCTET-STREAM" ("NAME" "fallback.bin") NIL NIL "BASE64" 10 NIL ("ATTACHMENT" ("FILENAME" "data.bin")))("TEXT" "PLAIN" NIL NIL NIL "QUOTED-PRINTABLE" 9 1 NIL ("ATTACHMENT" ("FILENAME" "note.txt"))) "MIXED") "MIXED")"#;
                format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 300 BODYSTRUCTURE {structure} BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[1])" {
                assert!(!synced);
                "* 1 FETCH (UID 1 BODY[1] {5}\r\nhello)\r\n".into()
            } else if command == "UID FETCH 1 (UID FLAGS)" {
                "* 1 FETCH (UID 1 FLAGS ())\r\n".into()
            } else if command == "UID FETCH 1 (UID BODY.PEEK[2.1])" {
                assert!(synced);
                downloads += 1;
                if downloads == 3 {
                    "* 1 FETCH (UID 1 BODY[2.1] {4}\r\nAP9o)\r\n".into()
                } else {
                    "* 1 FETCH (UID 1 BODY[2.1] {10}\r\nAP9o\r\naQ==)\r\n".into()
                }
            } else if command == "UID FETCH 1 (UID BODY.PEEK[2.2])" {
                assert!(synced);
                downloads += 1;
                "* 1 FETCH (UID 1 BODY[2.2] {9}\r\n=00=FFhi!)\r\n".into()
            } else if command == "LOGOUT" {
                assert_eq!(downloads, 3);
                write.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes()).await.unwrap();
                break;
            } else {
                panic!("unexpected fixture command: {command}");
            };
            write.write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let config = ServerConfig { host: "127.0.0.1".into(), port, username: "fixture".into(), security: Security::Plain };
    let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
    store.save_account(&account).unwrap();
    let inbox = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    if cached {
        store.reset_uidvalidity(inbox, 7).unwrap();
        store.save_message(&lagom_core::store::Message {
            summary: lagom_core::store::MessageSummary {
                account: account.id.clone(), folder: inbox, uid: 1, has_attachments: true, ..Default::default()
            },
            ..Default::default()
        }).unwrap();
        store.finish_sync(inbox, &[1].into_iter().collect(), 1, Some(2), None).unwrap();
    }
    let mut imap = ImapSync::login(&account, "fixture").await.unwrap();
    imap.sync_folder(&mut store, inbox, 20).await.unwrap();
    let message = store.list_messages(inbox, 0, 20, 0).unwrap().remove(0);
    assert!(message.has_attachments);
    assert_eq!(store.message(message.id).unwrap().body_text, "hello");
    let parts = store.attachments(message.id).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!((&*parts[0].section, &*parts[0].filename, &*parts[0].mime_type, parts[0].size, &*parts[0].encoding),
        ("2.1", "data.bin", "application/octet-stream", 10, "base64"));
    assert!(imap.fetch_attachment(&mut store, message.id, "2.1] UID FETCH 2").await.is_err());
    assert_eq!(imap.fetch_attachment(&mut store, message.id, "2.1").await.unwrap(), b"\0\xffhi");
    assert_eq!(imap.fetch_attachment(&mut store, message.id, "2.2").await.unwrap(), b"\0\xffhi!");
    assert!(imap.fetch_attachment(&mut store, message.id, "2.1").await.is_err());
    assert!(imap.fetch_attachment(&mut store, message.id, "2.1").await.is_err());
    assert!(store.attachments(message.id).unwrap().is_empty());
    imap.logout().await.unwrap();
    server.await.unwrap();
}

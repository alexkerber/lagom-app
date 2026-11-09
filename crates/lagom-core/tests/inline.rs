use lagom_core::{
    account::{Account, Security, ServerConfig},
    imap_sync::ImapSync,
    inline::{self, MAX_INLINE_BYTES},
    store::{FolderRole, Store},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn related_images_sync_and_legacy_mail_fetches_them_once_on_open() {
    for legacy in [false, true] { roundtrip(legacy, false).await; }
}

#[tokio::test]
async fn inline_fetch_rejects_a_server_literal_over_the_budget() {
    roundtrip(true, true).await;
}

async fn roundtrip(legacy: bool, oversized: bool) {
    let raw = include_str!("fixtures/related.eml").replace('\n', "\r\n");
    let chunks: Vec<_> = raw.split("--related\r\n").collect();
    let header = chunks[0].to_owned();
    let html = chunks[1].split_once("\r\n\r\n").unwrap().1.trim_end().to_owned();
    let image = chunks[2].split_once("\r\n\r\n").unwrap().1.split("\r\n--related--").next().unwrap().to_owned();
    let expected = mail_parser::MessageParser::default().parse(raw.as_bytes()).unwrap();
    let expected = expected.attachments().next().unwrap().decoded().into_owned();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        write.write_all(b"* OK fixture\r\n").await.unwrap();
        let mut images = 0;
        loop {
            let mut line = String::new();
            assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = if command.starts_with("LOGIN ") { String::new() }
            else if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
            else if command == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into() }
            else if command == "UID SEARCH ALL" { "* SEARCH 1\r\n".into() }
            else if command.contains("BODYSTRUCTURE") {
                let structure = format!(r#"(("TEXT" "HTML" ("CHARSET" "utf-8") NIL NIL "7BIT" {} 1)("IMAGE" "PNG" NIL "<Logo+1@example.test>" NIL "BASE64" {} NIL ("INLINE" NIL)) "RELATED")"#, html.len(), image.len());
                format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 500 BODYSTRUCTURE {structure} BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[1])" {
                format!("* 1 FETCH (UID 1 BODY[1] {{{}}}\r\n{html})\r\n", html.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[2])" {
                images += 1;
                if oversized {
                    write.write_all(format!("* 1 FETCH (UID 1 BODY[2] {{{}}}\r\n", MAX_INLINE_BYTES + 1).as_bytes()).await.unwrap();
                    return;
                }
                // FLAGS-only and unrelated UID/section replies must not supply the image.
                format!("* 1 FETCH (UID 1 FLAGS ())\r\n* 2 FETCH (UID 2 BODY[2] {{4}}\r\nAAAA)\r\n* 1 FETCH (UID 1 BODY[3] {{4}}\r\nAAAA)\r\n* 1 FETCH (UID 1 BODY[2] {{{}}}\r\n{image})\r\n", image.len())
            } else if command == "UID FETCH 1 (UID FLAGS)" { "* 1 FETCH (UID 1 FLAGS ())\r\n".into() }
            else if command == "LOGOUT" {
                assert_eq!(images, 1);
                write.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes()).await.unwrap();
                break;
            } else { panic!("unexpected fixture command: {command}"); };
            write.write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mail.db");
    let mut store = Store::open(&path).unwrap();
    let config = ServerConfig { host: "127.0.0.1".into(), port, username: "fixture".into(), security: Security::Plain };
    let account = Account::new("fixture@example.test".into(), config.clone(), config).unwrap();
    store.save_account(&account).unwrap();
    let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    store.reset_uidvalidity(folder, 7).unwrap();
    let mut imap = ImapSync::login(&account, "fixture").await.unwrap();
    if legacy {
        let mut message = lagom_core::mime::parse(raw.as_bytes()).unwrap();
        message.summary.account = account.id.clone();
        message.summary.folder = folder;
        message.summary.uid = 1;
        let id = store.save_message(&message).unwrap();
        assert!(!store.inline_loaded(id).unwrap());
        let result = imap.fetch_inline(&mut store, id).await;
        if oversized {
            assert!(result.is_err());
            assert!(!store.inline_loaded(id).unwrap());
            assert!(store.inline_parts(id).unwrap().is_empty());
            server.await.unwrap();
            return;
        }
        result.unwrap();
    } else {
        imap.sync_folder(&mut store, folder, 20).await.unwrap();
    }
    let id = store.list_messages(folder, 0, 20, 0).unwrap()[0].id;
    imap.fetch_inline(&mut store, id).await.unwrap();
    imap.logout().await.unwrap();
    server.await.unwrap();
    drop(store);
    let store = Store::open(path).unwrap();
    assert!(store.inline_loaded(id).unwrap());
    let parts = store.inline_parts(id).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].cid, "Logo+1@example.test");
    assert_eq!(parts[0].mime_type, "image/png");
    assert_eq!(parts[0].data, expected);
    assert_eq!(&parts[0].data[..8], b"\x89PNG\r\n\x1a\n");
    let html = inline::rewrite(&store.message(id).unwrap().body_html.unwrap(), id, false);
    assert!(html.contains(&inline::url(id, &parts[0].cid, false)));
    assert!(inline::resolve(id + 1, &parts, &inline::url(id, &parts[0].cid, false)).is_none());
}

use lagom_core::{
    account::{Account, Security, ServerConfig},
    imap_sync::ImapSync,
    inline::{self, MAX_INLINE_BYTES},
    store::{FolderRole, Store},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn related_images_sync_and_legacy_fetch_preserves_cached_attachments() {
    for legacy in [false, true] { roundtrip(legacy, false, false).await; }
}

#[tokio::test]
async fn inline_fetch_rejects_a_server_literal_over_the_budget() {
    roundtrip(true, true, false).await;
}

#[tokio::test]
async fn inline_fetch_uses_source_folder_and_uid_during_pending_move() {
    roundtrip(true, false, true).await;
}

async fn roundtrip(legacy: bool, oversized: bool, pending_move: bool) {
    // Windows checks the fixture out with CRLF; normalise before building the wire form.
    let raw = include_str!("fixtures/related.eml").replace("\r\n", "\n").replace('\n', "\r\n");
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
        let mut target_selected = false;
        loop {
            let mut line = String::new();
            assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = if command.starts_with("LOGIN ") { String::new() }
            else if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
            else if command == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into() }
            else if pending_move && command == "SELECT \"Archive\"" {
                target_selected = true;
                "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into()
            }
            else if command == "UID SEARCH ALL" { "* SEARCH 1\r\n".into() }
            else if command.contains("BODYSTRUCTURE") {
                if legacy { assert_eq!(command, "UID FETCH 1 (UID BODYSTRUCTURE)"); }
                let structure = format!(r#"(("TEXT" "HTML" ("CHARSET" "utf-8") NIL NIL "7BIT" {} 1)("IMAGE" "PNG" NIL "<Logo+1@example.test>" NIL "BASE64" {} NIL ("INLINE" NIL)) "RELATED")"#, html.len(), image.len());
                format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 500 BODYSTRUCTURE {structure} BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[1])" {
                format!("* 1 FETCH (UID 1 BODY[1] {{{}}}\r\n{html})\r\n", html.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[2])" {
                images += 1;
                if oversized {
                    write.write_all(format!("* 1 FETCH (UID 1 BODY[2] {{{}}}\r\n", MAX_INLINE_BYTES + 1).as_bytes()).await.unwrap();
                    std::future::pending::<()>().await;
                    unreachable!();
                }
                let image = if target_selected { image.replace("iVBOR", "AAAAA") } else { image.clone() };
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
    let id = if legacy {
        let mut message = lagom_core::mime::parse(raw.as_bytes()).unwrap();
        message.summary.account = account.id.clone();
        message.summary.folder = folder;
        message.summary.uid = 1;
        let id = store.save_message(&message).unwrap();
        let target = if pending_move {
            let target = store.save_folder(&account.id, "Archive", None, FolderRole::Archive).unwrap();
            store.reset_uidvalidity(target, 7).unwrap();
            let mut other = message.clone();
            other.summary.folder = target;
            other.summary.message_id = "other@example.test".into();
            let other_id = store.save_message(&other).unwrap();
            store.begin_move(id, target).unwrap();
            assert_eq!(store.message_summary(id).unwrap().folder, target);
            Some((target, other_id))
        } else { None };
        assert!(!store.inline_loaded(id).unwrap());
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("PRAGMA foreign_keys=ON", []).unwrap();
        db.execute("INSERT INTO attachments VALUES (?1,'3','cached.txt','text/plain',6,'7bit')", [id]).unwrap();
        db.execute("INSERT INTO attachment_cache VALUES (?1,'3',x'636163686564')", [id]).unwrap();
        let attachments = store.attachments(id).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), imap.fetch_inline(&mut store, id)).await;
        if oversized { server.abort(); }
        let result = result.expect("must reject the literal header without waiting for its body");
        if oversized {
            assert!(result.is_err());
            assert!(!store.inline_loaded(id).unwrap());
            assert!(store.inline_parts(id).unwrap().is_empty());
            assert!(server.await.unwrap_err().is_cancelled());
            return;
        }
        result.unwrap();
        assert_eq!(store.cached_attachment(id, "3").unwrap().as_deref(), Some(b"cached".as_slice()));
        assert_eq!(store.attachments(id).unwrap(), attachments);
        assert_eq!(store.message(id).unwrap().body_html, message.body_html);
        if let Some((target, other_id)) = target {
            assert_eq!(store.message_summary(id).unwrap().folder, target);
            assert_eq!(store.message(id).unwrap().summary.folder, folder);
            assert!(!store.inline_loaded(other_id).unwrap());
            assert!(store.inline_parts(other_id).unwrap().is_empty());
        }
        id
    } else {
        imap.sync_folder(&mut store, folder, 20).await.unwrap();
        store.list_messages(folder, 0, 20, 0).unwrap()[0].id
    };
    imap.fetch_inline(&mut store, id).await.unwrap();
    imap.logout().await.unwrap();
    server.await.unwrap();
    drop(store);
    let store = Store::open(path).unwrap();
    assert!(store.inline_loaded(id).unwrap());
    if legacy { assert_eq!(store.cached_attachment(id, "3").unwrap().as_deref(), Some(b"cached".as_slice())); }
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

#[derive(Clone, Copy, PartialEq)]
enum Body { Large, Legacy, TooLarge, Lying, Refused }

#[tokio::test]
async fn opening_a_body_over_4_mib_fetches_it_once() { open_body(Body::Large).await; }

#[tokio::test]
async fn opening_legacy_text_only_mail_fetches_its_html_once() { open_body(Body::Legacy).await; }

#[tokio::test]
async fn opening_a_body_over_25_mib_is_not_downloaded_and_not_retried() { open_body(Body::TooLarge).await; }

#[tokio::test]
async fn opening_a_body_stops_at_the_budget_when_the_server_lies_about_its_size() { open_body(Body::Lying).await; }

#[tokio::test]
async fn opening_a_body_the_server_refuses_is_retried_on_a_later_open() { open_body(Body::Refused).await; }

async fn open_body(case: Body) {
    use lagom_core::mime::{BODY_UNAVAILABLE, MAX_MIME_BYTES, MAX_OPEN_BYTES};
    let line = "x".repeat(76) + "\r\n";
    let text = if case == Body::Legacy { "Plain text".to_owned() } else { line.repeat(MAX_MIME_BYTES / line.len() + 1) };
    let html = "<p>Rich</p>";
    let header = "From: Big <big@example.test>\r\nSubject: Big\r\nMessage-ID: <big@example.test>\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n";
    // Sizes the server claims; the client must not trust them.
    let claimed = if case == Body::TooLarge { MAX_OPEN_BYTES + 1 } else { text.len() };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let wire_text = text.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        write.write_all(b"* OK fixture\r\n").await.unwrap();
        let mut bodies = 0;
        loop {
            let mut line = String::new();
            assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = if command.starts_with("LOGIN ") { String::new() }
            else if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
            else if command == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 2] next\r\n".into() }
            else if command == "UID SEARCH ALL" { "* SEARCH 1\r\n".into() }
            else if command == "UID FETCH 1 (UID FLAGS)" { "* 1 FETCH (UID 1 FLAGS ())\r\n".into() }
            else if command.contains("BODYSTRUCTURE") {
                let structure = format!(r#"(("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "7BIT" {claimed} 1)("TEXT" "HTML" ("CHARSET" "utf-8") NIL NIL "7BIT" {} 1) "ALTERNATIVE")"#, html.len());
                format!("* 1 FETCH (UID 1 FLAGS () RFC822.SIZE {claimed} BODYSTRUCTURE {structure} BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
            } else if command == "UID FETCH 1 (UID BODY.PEEK[1] BODY.PEEK[2])" {
                assert!(case != Body::TooLarge, "a part over the limit must not be requested");
                bodies += 1;
                if case == Body::Lying {
                    write.write_all(format!("* 1 FETCH (UID 1 BODY[1] {{{}}}\r\n", MAX_OPEN_BYTES + 1024 * 1024 + 1).as_bytes()).await.unwrap();
                    std::future::pending::<()>().await;
                    unreachable!();
                }
                if case == Body::Refused {
                    write.write_all(format!("{tag} NO busy\r\n").as_bytes()).await.unwrap();
                    continue;
                }
                format!("* 1 FETCH (UID 1 BODY[1] {{{}}}\r\n{wire_text} BODY[2] {{{}}}\r\n{html})\r\n", wire_text.len(), html.len())
            } else if command == "LOGOUT" {
                assert_eq!(bodies, if case == Body::TooLarge { 0 } else { 1 });
                write.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes()).await.unwrap();
                break;
            } else { panic!("unexpected fixture command: {command}"); };
            write.write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    let config = ServerConfig { host: "127.0.0.1".into(), port, username: "fixture".into(), security: Security::Plain };
    let account = Account::new("fixture@example.test".into(), config.clone(), config).unwrap();
    store.save_account(&account).unwrap();
    let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    store.reset_uidvalidity(folder, 7).unwrap();
    let mut imap = ImapSync::login(&account, "fixture").await.unwrap();
    let id = if case == Body::Legacy {
        // Synced before HTML parts were kept: only the text part is stored.
        let mut message = lagom_core::mime::parse(format!("{header}{text}").as_bytes()).unwrap();
        message.body_html = None;
        message.summary.account = account.id.clone();
        message.summary.folder = folder;
        message.summary.uid = 1;
        store.save_message(&message).unwrap()
    } else {
        imap.sync_folder(&mut store, folder, 20).await.unwrap();
        store.list_messages(folder, 0, 20, 0).unwrap()[0].id
    };
    let before = store.message(id).unwrap();
    assert!(before.body_html.is_none());
    if case != Body::Legacy { assert_eq!(before.body_text, BODY_UNAVAILABLE); }
    assert!(!store.inline_loaded(id).unwrap());
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), imap.fetch_inline(&mut store, id)).await
        .expect("must reject the literal header without waiting for its body");
    if matches!(case, Body::Lying | Body::Refused) {
        assert!(result.is_err());
        // Not marked: a later open tries again.
        assert!(!store.inline_loaded(id).unwrap());
        assert_eq!(store.message(id).unwrap().body_text, BODY_UNAVAILABLE);
        if case == Body::Lying { server.abort(); } else {
            imap.logout().await.unwrap();
            server.await.unwrap();
        }
        return;
    }
    result.unwrap();
    assert!(store.inline_loaded(id).unwrap());
    let after = store.message(id).unwrap();
    if case == Body::TooLarge {
        assert_eq!(after.body_text, BODY_UNAVAILABLE);
        assert!(after.body_html.is_none());
    } else {
        assert_eq!(after.body_text.trim_end(), text.trim_end());
        assert_eq!(after.body_html.as_deref(), Some(html));
        assert!(!after.summary.snippet.is_empty());
    }
    // Loaded once: a second open does not go to the server (the fixture panics on other commands).
    imap.fetch_inline(&mut store, id).await.unwrap();
    imap.logout().await.unwrap();
    server.await.unwrap();
}

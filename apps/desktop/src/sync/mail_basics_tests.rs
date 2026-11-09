use super::*;
use lagom_core::{account::{Security, ServerConfig}, store::{Message, MessageSummary}};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn forward_downloads_once_and_read_flags_reach_the_server() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
            let mut downloads = 0;
            let mut flags = Vec::new();
            loop {
                let mut line = String::new();
                if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let data = if command.starts_with("LOGIN ") { String::new() }
                else if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n".into() }
                else if command == "SELECT \"INBOX\"" { "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n".into() }
                else if command == "UID FETCH 1 (UID BODY.PEEK[2])" {
                    downloads += 1;
                    "* 1 FETCH (UID 1 BODY[2] {8}\r\nAP9oaQ==)\r\n".into()
                } else if command.starts_with("UID STORE 1 ") {
                    flags.push(command.to_string());
                    if flags.len() == 3 {
                        socket.get_mut().write_all(format!("{tag} NO fixture refusal\r\n").as_bytes()).await.unwrap();
                        continue;
                    }
                    String::new()
                } else { panic!("unexpected fixture command: {command}"); };
                socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
            }
            assert_eq!(downloads, 1);
            assert_eq!(flags, ["UID STORE 1 -FLAGS.SILENT (\\Seen)", "UID STORE 1 +FLAGS.SILENT (\\Seen)", "UID STORE 1 -FLAGS.SILENT (\\Seen)"]);
        });
        let path = std::env::temp_dir().join(format!("lagom-mail-basics-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut store = Store::open(&path).unwrap();
        store.save_account(&account).unwrap();
        let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        store.reset_uidvalidity(folder, 7).unwrap();
        let id = store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid: 1, seen: true, flagged: true, ..Default::default() }, ..Default::default() }).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("INSERT INTO attachments VALUES (?1,'2','data.bin','application/octet-stream',8,'base64')", [id]).unwrap();
        db.execute("UPDATE messages SET attachments_loaded=1 WHERE id=?1", [id]).unwrap();
        drop(db);
        let session = ImapSync::login(&account, "fixture").await.unwrap();
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store, sessions: HashMap::from([(account.id.clone(), session)]), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), reauth_required: HashSet::new(), discovery: None, me, events };
        for _ in 0..2 {
            worker.quick(Cmd::ForwardAttachments { draft: "draft".into(), id }).await;
            let Some(Event::ForwardAttachments { draft, result }) = out.recv().await else { panic!("missing forward result") };
            assert_eq!(draft, "draft");
            let parts = result.unwrap();
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].filename, "data.bin");
            assert_eq!(parts[0].mime_type, "application/octet-stream");
            assert_eq!(parts[0].data, b"\0\xffhi");
        }
        for seen in [false, true] {
            worker.quick(Cmd::Op { op: 42, id, act: Act::Read(seen) }).await;
            assert!(matches!(out.recv().await, Some(Event::Op { op: 42, error: None })));
            assert!(matches!(out.recv().await, Some(Event::Changed)));
            let m = worker.store.message_summary(id).unwrap();
            assert_eq!(m.seen, seen);
            assert!(m.flagged);
        }
        worker.quick(Cmd::Op { op: 43, id, act: Act::Read(false) }).await;
        assert!(matches!(out.recv().await, Some(Event::Op { op: 43, error: Some(_) })));
        assert!(matches!(out.recv().await, Some(Event::Changed)));
        assert!(worker.store.message_summary(id).unwrap().seen, "a failed server flag preserves the confirmed state even without a window");
        assert!(worker.forward_attachments(-1).await.is_err());
        drop(worker);
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn review_two_rejected_toggles_keep_last_confirmed_flags() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        let server = tokio::spawn(async move {
            for (seen, status) in [(false, "NO"), (true, "BAD")] {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                    let (tag, command) = line.trim_end().split_once(' ').unwrap();
                    let data = match command {
                        "LOGIN \"fixture\" \"fixture\"" => "",
                        "CAPABILITY" => "* CAPABILITY IMAP4rev1\r\n",
                        "SELECT \"INBOX\"" => "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n",
                        _ => {
                            assert_eq!(command, format!("UID STORE 1 {}FLAGS.SILENT (\\Seen)", if seen { "+" } else { "-" }));
                            socket.get_mut().write_all(format!("{tag} {status} refused\r\n").as_bytes()).await.unwrap();
                            break;
                        }
                    };
                    socket.get_mut().write_all(format!("{data}{tag} OK fixture\r\n").as_bytes()).await.unwrap();
                }
            }
        });
        let path = std::env::temp_dir().join(format!("lagom-queued-read-{}.db", account.id));
        let mut store = Store::open(&path).unwrap();
        store.save_account(&account).unwrap();
        let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        store.reset_uidvalidity(folder, 7).unwrap();
        let id = store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid: 1, seen: true, flagged: true, ..Default::default() }, ..Default::default() }).unwrap();
        let (me, mut queue) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        me.send(Cmd::Op { op: 1, id, act: Act::Read(false) }).unwrap();
        me.send(Cmd::Op { op: 2, id, act: Act::Read(true) }).unwrap();
        let mut worker = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), reauth_required: HashSet::new(), discovery: None, me, events };
        for expected in [1, 2] {
            let session = ImapSync::login(&account, "fixture").await.unwrap();
            worker.sessions.insert(account.id.clone(), session);
            worker.quick(queue.recv().await.unwrap()).await;
            assert!(matches!(out.recv().await, Some(Event::Op { op, error: Some(_) }) if op == expected));
            assert!(matches!(out.recv().await, Some(Event::Changed)));
            let m = worker.store.message_summary(id).unwrap();
            assert!(m.seen && m.flagged, "neither failure can replace confirmed flags with optimistic flags");
        }
        drop(worker);
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }).await.unwrap();
}

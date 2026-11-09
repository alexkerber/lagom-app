use super::*;
use lagom_core::{account::{Security, ServerConfig}, store::{Message, MessageSummary}};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn archive_notifies_mail_fetched_by_its_inbox_sync() {
    roundtrip(false, false, false).await;
}

#[tokio::test]
async fn move_notifies_completed_inbox_sync_even_if_source_sync_fails() {
    roundtrip(true, false, true).await;
}

#[tokio::test]
async fn resolving_a_moved_message_notifies_new_inbox_mail() {
    roundtrip(true, true, false).await;
}

async fn roundtrip(into_inbox: bool, resolving: bool, fail_source: bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            write.write_all(b"* OK fixture\r\n").await.unwrap();
            let mut inbox = false;
            let mut moved = false;
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap() == 0 { break; }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let data = if command.starts_with("LOGIN ") || command.starts_with("UID MOVE ") {
                    moved |= command.starts_with("UID MOVE ");
                    String::new()
                } else if command == "CAPABILITY" {
                    "* CAPABILITY IMAP4rev1 MOVE\r\n".into()
                } else if command.starts_with("LIST ") {
                    "* LIST () NIL \"INBOX\"\r\n* LIST (\\Archive) NIL \"Archive\"\r\n".into()
                } else if command.starts_with("SELECT ") {
                    inbox = command == "SELECT \"INBOX\"";
                    if !inbox && moved && fail_source {
                        write.write_all(format!("{tag} NO fixture source unavailable\r\n").as_bytes()).await.unwrap();
                        continue;
                    }
                    "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 3] next\r\n".into()
                } else if command == "UID SEARCH ALL" {
                    if inbox { "* SEARCH 2\r\n".into() } else { "* SEARCH\r\n".into() }
                } else if command.contains("BODYSTRUCTURE") {
                    assert!(inbox);
                    let header = "From: sender@example.test\r\nSubject: Incoming\r\nMessage-ID: <incoming@example.test>\r\nContent-Type: text/plain\r\n\r\n";
                    format!("* 1 FETCH (UID 2 FLAGS () RFC822.SIZE 200 BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())
                } else if command == "UID FETCH 2 (UID BODY.PEEK[TEXT])" {
                    "* 1 FETCH (UID 2 BODY[TEXT] {4}\r\nbody)\r\n".into()
                } else if command == "UID FETCH 2 (UID FLAGS)" {
                    "* 1 FETCH (UID 2 FLAGS ())\r\n".into()
                } else {
                    panic!("unexpected fixture command: {command}");
                };
                write.write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
            }
        });
        let path = std::env::temp_dir().join(format!("lagom-notify-{into_inbox}-{resolving}-{fail_source}-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let config = ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.test".into(), config.clone(), config).unwrap();
        let mut store = Store::open(&path).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let archive = store.save_folder(&account.id, "Archive", None, FolderRole::Archive).unwrap();
        for folder in [inbox, archive] {
            store.reset_uidvalidity(folder, 7).unwrap();
            store.finish_sync(folder, &Default::default(), 1, Some(2), None).unwrap();
        }
        let id = store.save_message(&Message { summary: MessageSummary {
            account: account.id.clone(), folder: if into_inbox { archive } else { inbox },
            uid: 1, message_id: "<old@example.test>".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
        let (me, mut rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let session = ImapSync::login(&account, "fixture").await.unwrap();
        let mut worker = Worker { store, sessions: [(account.id.clone(), session)].into(), idle: HashMap::new(),
            moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), reauth_required: HashSet::new(), discovery: None, me, events };
        if resolving {
            worker.moved.insert(id, (inbox, "incoming@example.test".into()));
            worker.resolve(id).await.unwrap();
        } else {
            let act = if into_inbox { Act::Move(inbox) } else { Act::Archive };
            worker.quick(Cmd::Op { op: 1, id, act }).await;
        }
        let mut messages = Vec::new();
        while let Ok(event) = out.try_recv() {
            match event {
                Event::NewMail(mail) => messages.extend(mail),
                Event::Op { error, .. } => assert_eq!(error.is_some(), fail_source),
                _ => {},
            }
        }
        assert_eq!(worker.store.folder(inbox).unwrap().last_uid, 2);
        assert_eq!(messages.len(), 1, "internal Inbox sync must emit new mail");
        assert_eq!(messages[0].subject, "Incoming");
        assert_eq!(messages[0].account, account.id);
        if !fail_source {
            worker.sync_account(&account, true, &mut rx).await.unwrap();
            while let Ok(event) = out.try_recv() {
                assert!(!matches!(event, Event::NewMail(_)), "next poll must not repeat the notice");
            }
        }
        drop(worker);
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }).await.expect("fixture completes without keychain or external servers");
}

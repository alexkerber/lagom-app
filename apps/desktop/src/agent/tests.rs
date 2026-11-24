use super::*;
use crate::agent::Server;
use lagom_core::{
    account::{Security, ServerConfig},
    agent::{
        id::ObjectId,
        protocol::{Code, Response},
    },
    calendar::{Calendar, Event as CalendarEvent},
    store::{Message, MessageSummary},
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn wait(mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
}

fn wire(path: &Path, request: &[u8]) -> Vec<u8> {
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    socket.write_all(request).unwrap();
    let mut bytes = Vec::new();
    socket.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    bytes
}

fn fixed(code: Code) -> Vec<u8> {
    let mut line = serde_json::to_vec(&Response::Failure(code)).unwrap();
    line.push(b'\n');
    line
}

fn digest(db: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
    let mut result = Vec::new();
    for table in ["messages", "drafts", "outbox", "events"] {
        let mut query = db
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let n = query.column_count();
        result.extend(
            query
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(Result::unwrap),
        );
    }
    result
}

#[test]
fn account_scoped_reads_over_the_socket_are_pure_and_audited() {
    let imap = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let config = ServerConfig {
        host: "127.0.0.1".into(),
        port: imap.local_addr().unwrap().port(),
        security: Security::Plain,
        username: "fixture".into(),
    };
    let a = Account::new("a@example.com".into(), config.clone(), config.clone()).unwrap();
    let b = Account::new("b@example.com".into(), config.clone(), config).unwrap();
    let dir = PathBuf::from("/tmp").join(format!("lagom-agent-{}", a.id));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lagom.sqlite3");
    let sock = dir.join("agent.sock");
    let mut store = Store::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    let now = chrono::Utc::now().timestamp();
    for (n, account) in [&a, &b].into_iter().enumerate() {
        store.save_account(account).unwrap();
        let folder = store
            .save_folder(&account.id, "INBOX", None, FolderRole::Inbox)
            .unwrap();
        store.reset_uidvalidity(folder, 7).unwrap();
        let row = store
            .save_message(&Message {
                summary: MessageSummary {
                    account: account.id.clone(),
                    folder,
                    uid: 3,
                    message_id: "<fixture>".into(),
                    subject: format!("needle-{n}"),
                    from_address: format!("writer{n}@example.com"),
                    flagged: true,
                    has_attachments: true,
                    ..Default::default()
                },
                body_text: "x".repeat(100001),
                ..Default::default()
            })
            .unwrap();
        db.execute(
            "INSERT INTO attachments VALUES(?,'2','file.txt','text/plain',3,'7bit')",
            [row],
        )
        .unwrap();
        db.execute(
            "INSERT INTO drafts(id,account,content) VALUES(?,?,'{}')",
            params![account.id, account.id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO outbox(id,account,content,raw,ready_at) VALUES(?,?,'{}',x'12',9999999999)",
            params![account.id, account.id],
        )
        .unwrap();
        store
            .replace_calendar_window(
                &account.id,
                now - 1,
                now + 1000,
                &[(
                    Calendar {
                        remote_id: "cal".into(),
                        name: "Cal".into(),
                        color: None,
                        shown: true,
                        writable: false,
                    },
                    vec![CalendarEvent {
                        remote_id: "e".into(),
                        title: format!("event-{n}"),
                        start: "start".into(),
                        end: "end".into(),
                        start_utc: now + 10 + 20 * n as i64,
                        end_utc: now + 20 + 20 * n as i64,
                        all_day: false,
                        location: None,
                        html_link: None,
                        join_url: None,
                    }],
                )],
            )
            .unwrap();
    }
    let before = digest(&db);
    let reader = Store::agent_reader(&path, || false).unwrap();
    assert!(
        matches!(reader.set_signature(&a.id, "changed"), Err(Error::Store(e))
        if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ReadOnly))
    );
    drop(reader);
    let scripted = thread::spawn(move || {
        let (socket, _) = imap.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(12)))
            .unwrap();
        let mut peer = BufReader::new(socket);
        peer.get_mut().write_all(b"* OK fixture\r\n").unwrap();
        let mut peek = 0;
        loop {
            let mut line = String::new();
            if peer.read_line(&mut line).unwrap() == 0 {
                break;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = match command {
                "LOGIN \"fixture\" \"fixture\"" => "",
                "CAPABILITY" => "* CAPABILITY IMAP4rev1\r\n",
                "SELECT \"INBOX\"" => "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n",
                "UID FETCH 3 (UID BODY.PEEK[2])" => {
                    peek += 1;
                    "* 1 FETCH (UID 3 BODY[2] {3}\r\nabc)\r\n"
                }
                _ => panic!("unexpected IMAP command {command}"),
            };
            write!(peer.get_mut(), "{data}{tag} OK fixture\r\n").unwrap();
        }
        assert_eq!(peek, 1);
    });
    let (bridge, mut commands) = Bridge::detached();
    let worker = thread::spawn({
        let path = path.clone();
        let account = a.clone();
        move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let session = ImapSync::login(&account, "fixture").await.unwrap();
                    let (me, _) = unbounded_channel();
                    let (events, _) = unbounded_channel();
                    let mut worker = Worker::new(Store::open(path).unwrap(), me, events);
                    worker.sessions.insert(account.id, session);
                    while let Some(cmd) = commands.recv().await {
                        assert!(matches!(cmd, Cmd::AgentAttachment(_)));
                        worker.quick(cmd).await;
                    }
                })
        }
    });
    drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
    let server = Server::start(&path, bridge);
    wait(|| !sock.exists());
    db.execute_batch(
        "UPDATE agent_settings SET value='1' WHERE key='enabled';
        INSERT INTO agent_access VALUES('','mail','',1),('','calendar','',1)",
    )
    .unwrap();
    let agent = store
        .create_agent("fixture", "Remote", "fixture", true)
        .unwrap();
    let token = store
        .issue_token(
            &agent,
            lagom_core::agent::token::TokenScope::Remote {
                accounts: std::slice::from_ref(&a.id),
                level: lagom_core::agent::policy::Level::Read,
                ranges: &[],
                expires: None,
            },
        )
        .unwrap();
    wait(|| sock.exists());
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let count = || {
        db.query_row("SELECT count(*) FROM agent_audit", [], |r| {
            r.get::<_, usize>(0)
        })
        .unwrap()
    };
    let call = |cmd: &str, args: Value| {
        let n = count();
        let bytes = wire(
            &sock,
            format!(
                "{}\n",
                json!({"v":1,"cmd":cmd,"args":args,"token":token.as_str(),"source_ip":"127.0.0.1"})
            )
            .as_bytes(),
        );
        assert_eq!(count(), n + 1);
        bytes
    };
    let ok = |cmd, args| {
        let bytes = call(cmd, args);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["ok"], true, "{cmd} {value}");
        value["result"].clone()
    };
    for cmd in [
        "accounts",
        "folders",
        "contacts",
        "emails",
        "search",
        "events",
        "availability",
    ] {
        let args = match cmd {
            "search" => json!({"query":"needle","in":"inbox"}),
            "events" | "availability" => json!({"start":now,"end":now+100}),
            _ => json!({}),
        };
        let data = ok(cmd, args.clone());
        assert!(!data.to_string().contains(&b.id));
        match cmd {
            "availability" => assert_eq!(
                data,
                json!({"busy":[[now+10,now+20]],"free":[[now,now+10],[now+20,now+100]]})
            ),
            "emails" | "search" => assert_eq!(data["messages"][0]["subject"], "needle-0"),
            "events" => assert_eq!(data["events"][0]["title"], "event-0"),
            _ => assert_eq!(data[cmd].as_array().unwrap().len(), 1),
        }
        let mut args = args;
        args["account"] = json!(b.email);
        assert_eq!(call(cmd, args), fixed(Code::NotFound));
    }
    let list = ok(
        "emails",
        json!({"filter":"subject:needle-0","page":1,"limit":1,"account":a.email}),
    );
    let mail = &list["messages"][0];
    let thread_id = mail["thread"].as_str().unwrap();
    let thread = ok("thread", json!({"id":thread_id}));
    let first = &thread["messages"][0];
    assert_eq!(first["id"], mail["id"]);
    assert_eq!(first["account"], a.id);
    assert_eq!(first["body"].as_str().unwrap().len(), 100000);
    assert_eq!(first["truncated"], true);
    let part = ok("attachment", json!({"id":first["attachments"][0]["id"]}));
    assert_eq!(part["data"], "YWJj");
    db.execute("UPDATE attachment_cache SET data=zeroblob(20971521)", [])
        .unwrap();
    assert_eq!(
        call("attachment", json!({"id":first["attachments"][0]["id"]})),
        fixed(Code::TooLarge)
    );
    assert_eq!(
        call("availability", json!({"start":now-8*86400,"end":now})),
        fixed(Code::Unsupported)
    );
    assert_eq!(before, digest(&db));
    let archive = store
        .save_folder(&a.id, "Archive", None, FolderRole::Archive)
        .unwrap();
    let inbox = store
        .folder_metadata(&a.id)
        .unwrap()
        .into_iter()
        .find(|f| f.role == FolderRole::Inbox)
        .unwrap();
    let mut copy = store
        .message(store.find_uid(inbox.id, 3).unwrap().unwrap())
        .unwrap();
    copy.summary.folder = archive;
    store.save_message(&copy).unwrap();
    let copies = ok("thread", json!({"id":thread_id}));
    assert_eq!(copies["messages"].as_array().unwrap().len(), 1);
    assert_eq!(copies["messages"][0]["folder"], "INBOX");
    let missing = call(
        "thread",
        json!({"id":ObjectId::Thread(a.id.clone(),"missing".into()).encode()}),
    );
    let ObjectId::Thread(_, key) = ObjectId::decode(thread_id).unwrap() else {
        panic!()
    };
    let foreign = ObjectId::Thread(b.id.clone(), key).encode();
    assert_eq!(missing, fixed(Code::NotFound));
    assert_eq!(call("thread", json!({"id":foreign})), missing);
    // Removing the underlying table makes any thread query observable.
    db.execute("ALTER TABLE messages RENAME TO hidden_fixture_messages", [])
        .unwrap();
    assert_eq!(call("thread", json!({"id":foreign})), missing);
    assert_eq!(
        call("thread", json!({"id":thread_id})),
        fixed(Code::Internal)
    );
    assert_eq!(
        call(
            "emails",
            json!({"filter":format!("subject:private-subject {}",token.as_str())})
        ),
        fixed(Code::Internal)
    );
    assert_eq!(
        call("emails", json!({"unexpected":true})),
        fixed(Code::InvalidRequest)
    );
    let n = count();
    assert_eq!(wire(&sock, b"{broken}\n"), fixed(Code::InvalidRequest));
    assert_eq!(
        wire(&sock, b"{\"v\":1,\"cmd\":\"accounts\",\"args\":{}}\n"),
        fixed(Code::Unauthorized)
    );
    assert_eq!(count(), n + 2);
    let mut logs = db.prepare("SELECT * FROM agent_audit").unwrap();
    let columns = logs.column_count();
    let logged = logs
        .query_map([], |row| {
            Ok((0..columns)
                .filter_map(|i| row.get::<_, String>(i).ok())
                .collect::<Vec<_>>()
                .join(" "))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    for private in ["private-subject", "needle-0", token.as_str()] {
        assert!(!logged.contains(private));
    }
    assert!(logged.contains("not_found:outside_scope"));
    assert!(logged.contains("unauthorized:missing"));
    store.revoke_agent_tokens(&agent).unwrap();
    assert_eq!(call("accounts", json!({})), fixed(Code::Unauthorized));
    let mut pending = UnixStream::connect(&sock).unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    pending.write_all(b"{").unwrap();
    store.disable_agents().unwrap();
    wait(|| !sock.exists());
    match pending.read(&mut [0]) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("open request survived disable: {other:?}"),
    }
    drop(server);
    worker.join().unwrap();
    scripted.join().unwrap();
    drop(logs);
    drop(db);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn socket_ownership_framing_deadlines_and_limits() {
    let config = ServerConfig {
        host: "localhost".into(),
        port: 143,
        security: Security::Plain,
        username: "fixture".into(),
    };
    let account = Account::new("limits@example.com".into(), config.clone(), config).unwrap();
    let dir = PathBuf::from("/tmp").join(format!("lagom-limits-{}", account.id));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lagom.sqlite3");
    let sock = dir.join("agent.sock");
    let store = Store::open(&path).unwrap();
    store.save_account(&account).unwrap();
    let token = store.provision_read_agent(&account.id).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
    let (bridge, _) = Bridge::detached();
    let first = Server::start(&path, bridge.clone());
    wait(|| UnixStream::connect(&sock).is_ok());
    let second = Server::start(&path, bridge);
    thread::sleep(Duration::from_millis(120));
    drop(second);
    let request = |v| {
        format!(
            "{}\n",
            json!({"v":v,"token":token.as_str(),"cmd":"accounts","args":{}})
        )
    };
    assert_eq!(wire(&sock, request(2).as_bytes()), fixed(Code::Unsupported));
    let mut oversized = vec![b' '; 1024 * 1024];
    oversized.push(b'\n');
    assert_eq!(wire(&sock, &oversized), fixed(Code::TooLarge));

    let pending: Vec<_> = (0..8)
        .map(|_| {
            let socket = UnixStream::connect(&sock).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(7)))
                .unwrap();
            socket
        })
        .collect();
    let mut ninth = UnixStream::connect(&sock).unwrap();
    ninth
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reply = Vec::new();
    ninth.read_to_end(&mut reply).unwrap();
    assert_eq!(reply, fixed(Code::Busy));
    for mut socket in pending {
        reply.clear();
        socket.read_to_end(&mut reply).unwrap();
        assert_eq!(reply, fixed(Code::InvalidRequest));
    }
    let mut old = UnixStream::connect(&sock).unwrap();
    old.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    old.write_all(b"{").unwrap();
    thread::sleep(Duration::from_millis(100));
    store.disable_agents().unwrap();
    let _replacement = store.provision_read_agent(&account.id).unwrap();
    assert_eq!(old.read(&mut [0]).unwrap(), 0);
    let req = request(1);
    for _ in 1..300 {
        assert!(matches!(
            serde_json::from_slice(&wire(&sock, req.as_bytes())),
            Ok(Response::Success(_))
        ));
    }
    assert_eq!(wire(&sock, req.as_bytes()), fixed(Code::Busy));
    drop(first);
    assert!(!sock.exists());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

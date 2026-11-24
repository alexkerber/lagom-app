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
                    message_id: if n == 0 {
                        "x".repeat(1500)
                    } else {
                        "<fixture>".into()
                    },
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
        db.query_row(
            "SELECT coalesce(sum(max(refusals,1)),0) FROM agent_audit",
            [],
            |r| r.get::<_, usize>(0),
        )
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
    db.execute("DELETE FROM agent_audit WHERE refusals>0", [])
        .unwrap();
    assert_eq!(call("draft.send", json!({})), fixed(Code::InvalidRequest));
    let classified: (String, bool) = db
        .query_row(
            "SELECT command,read_seq IS NOT NULL FROM agent_audit WHERE refusals>0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(classified, ("unknown".to_owned(), true));
    let list = ok(
        "emails",
        json!({"filter":"subject:needle-0","page":1,"limit":1,"account":a.email}),
    );
    let mail = &list["messages"][0];
    assert!(mail["id"].as_str().unwrap().len() <= 2048);
    let thread_id = mail["thread"].as_str().unwrap();
    let thread = ok("thread", json!({"id":thread_id}));
    let first = &thread["messages"][0];
    assert_eq!(first["id"], mail["id"]);
    assert_eq!(first["account"], a.id);
    assert_eq!(first["body"].as_str().unwrap().len(), 100000);
    assert_eq!(first["truncated"], true);
    let part = ok("attachment", json!({"id":first["attachments"][0]["id"]}));
    assert_eq!(part["data"], "YWJj");
    let audited: String = db.query_row(
        "SELECT target FROM agent_audit WHERE command='attachment' AND result='ok' ORDER BY rowid DESC LIMIT 1",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(audited, mail["id"]);
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
    db.execute("DELETE FROM agent_audit WHERE refusals>0", [])
        .unwrap();
    let missing = call(
        "thread",
        json!({"id":ObjectId::Thread(a.id.clone(),"missing".into()).encode()}),
    );
    let foreign = match ObjectId::decode(thread_id).unwrap() {
        ObjectId::ThreadHash(_, hash) => ObjectId::ThreadHash(b.id.clone(), hash).encode(),
        id => panic!("expected compact thread ID, got {id:?}"),
    };
    assert_eq!(missing, fixed(Code::NotFound));
    assert_eq!(call("thread", json!({"id":foreign})), missing);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM agent_audit WHERE refusals>0 AND account IS NOT NULL",
            [],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        0
    );
    db.execute("DELETE FROM agent_audit WHERE refusals>0", [])
        .unwrap();
    let missing_part = ObjectId::Attachment(
        lagom_core::agent::id::MessageId::Header(a.id.clone(), "missing".into()),
        "1".into(),
    );
    assert_eq!(
        call("attachment", json!({"id":missing_part.encode()})),
        fixed(Code::NotFound)
    );
    assert_eq!(
        db.query_row(
            "SELECT account FROM agent_audit WHERE refusals>0",
            [],
            |r| r.get::<_, Option<String>>(0)
        )
        .unwrap(),
        None
    );
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
    assert!(logged.contains("not_found"));
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
    for resource in [
        lagom_core::agent::policy::Resource::Mail,
        lagom_core::agent::policy::Resource::Calendar,
    ] {
        store
            .set_agent_access(
                "",
                &lagom_core::agent::policy::Access {
                    resource,
                    account: String::new(),
                    level: lagom_core::agent::policy::Level::Read,
                },
            )
            .unwrap();
    }
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
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut ninth = UnixStream::connect(&sock).unwrap();
    ninth
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reply = Vec::new();
    ninth.read_to_end(&mut reply).unwrap();
    assert_eq!(reply, fixed(Code::Busy));
    lock.execute_batch("ROLLBACK").unwrap();
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
    let wrong_source = format!(
        "{}\n",
        json!({"v":1,"cmd":"draft.send","args":{},"token":token.as_str(),"source_ip":"::1"})
    );
    for _ in 1..300 {
        assert_eq!(
            wire(&sock, wrong_source.as_bytes()),
            fixed(Code::Unauthorized)
        );
    }
    assert_eq!(wire(&sock, req.as_bytes()), fixed(Code::Busy));
    assert_eq!(wire(&sock, wrong_source.as_bytes()), fixed(Code::Busy));
    let (buckets, total): (usize, usize) = lock
        .query_row(
            "SELECT count(*),sum(refusals) FROM agent_audit
        WHERE agent IS NOT NULL AND refusals>0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(buckets <= 2); // The requests can cross a minute boundary.
    assert_eq!(total, 302);
    drop(first);
    assert!(!sock.exists());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

struct WriteFixture {
    _dir: tempfile::TempDir,
    store: Store,
    account: Account,
    agent: String,
    token: String,
    socket: PathBuf,
    commands: UnboundedReceiver<Cmd>,
    worker: Worker,
    _server: Server,
    rt: tokio::runtime::Runtime,
}

impl WriteFixture {
    fn new() -> Self {
        use lagom_core::agent::{policy::Setting, token::TokenScope};
        let dir = tempfile::Builder::new()
            .prefix("lagom-write")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().join("db");
        let store = Store::open(&path).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let agent = store.create_agent("fixture", "Test Agent", "host", false).unwrap();
        let token = store.issue_token(&agent, TokenScope::Local).unwrap().to_string();
        store.set_agent_setting(Setting::Enabled, true).unwrap();
        let (bridge, commands) = Bridge::detached();
        let server = Server::start(&path, bridge);
        let socket = dir.path().join("agent.sock");
        wait(|| socket.exists());
        let worker = Worker::new(
            Store::open(&path).unwrap(),
            unbounded_channel().0,
            unbounded_channel().0,
        );
        let f = Self {
            _dir: dir,
            store,
            account,
            agent,
            token,
            socket,
            commands,
            worker,
            _server: server,
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        };
        f.level(3);
        f
    }
    fn level(&self, level: i64) {
        Connection::open(self.store.path()).unwrap().execute("INSERT OR REPLACE INTO agent_access VALUES (?1,'mail','',?2),('','mail','',3),( ?1,'calendar','',?2),('','calendar','',3)",params![self.agent,level]).unwrap();
    }
    fn start(&self, cmd: &str, args: Value) -> UnixStream {
        let mut socket = UnixStream::connect(&self.socket).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        writeln!(socket, "{}", json!({"v":1,"cmd":cmd,"args":args,"token":self.token})).unwrap();
        socket
    }
    fn recv(&mut self) -> Cmd {
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(4), self.commands.recv())
                .await
                .unwrap()
                .unwrap()
        })
    }
    fn run(&mut self, cmd: Cmd) {
        self.rt.block_on(self.worker.quick(cmd));
    }
    fn pump(&mut self) {
        let cmd = self.recv();
        self.run(cmd);
    }
    fn write(&mut self, cmd: &str, args: Value) -> Value {
        let s = self.start(cmd, args);
        self.pump();
        response(s)
    }
    fn sending(&mut self, id: &Value) -> (UnixStream, crate::agent::confirm::Sheet) {
        let s = self.start("draft.send", json!({"id":id}));
        let Cmd::AgentConfirm(sheet) = self.recv() else {
            panic!("missing confirmation")
        };
        (s, sheet)
    }
}
fn response(s: UnixStream) -> Value {
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("invalid reply: {line}"))
}

#[test]
fn draft_confirmation_sends_only_the_reviewed_revision_and_undo_restores_ownership() {
    let mut f = WriteFixture::new();
    let created = f.write(
        "draft.create",
        json!({"to":"a@example.com","subject":"private","body":"first"}),
    );
    let id = &created["result"]["id"];
    let ObjectId::Draft(_, raw) = ObjectId::decode(id.as_str().unwrap()).unwrap() else {
        panic!()
    };
    let (s, sheet) = f.sending(id);
    assert!(sheet.rows.iter().any(|r| r.label == "Agent" && r.value.contains("Test Agent")) && sheet.rows.iter().any(|r| r.label == "To" && r.value == "a@example.com"));
    assert_eq!(sheet.body, "first");
    assert!(f.store.pending(&raw).is_err());
    sheet.answer.send(Err(Code::Denied)).unwrap();
    assert_eq!(response(s)["error"]["code"], "denied");
    let (s, sheet) = f.sending(id);
    let mut waiting = crate::agent::confirm::Waiting::default();
    let mut denials = crate::agent::confirm::Denials::default();
    assert!(waiting.push(sheet, &mut denials));
    drop(s);
    let sheet = waiting.sheets.front_mut().unwrap();
    wait(|| sheet.answer.is_closed());
    assert!(sheet.withdrawn_at.get().is_some());
    // Simulate a window reopening after the entire cooldown has elapsed.
    sheet.withdrawn_at = std::sync::Arc::new(std::sync::OnceLock::from(
        std::time::Instant::now() - Duration::from_secs(31)));
    let (next, replacement) = f.sending(id);
    assert!(waiting.push(replacement, &mut denials));
    assert!(!waiting.withdrawn);
    waiting.sheets.pop_front().unwrap().answer.send(Err(Code::Denied)).unwrap();
    assert_eq!(response(next)["error"]["code"], "denied");
    let (s, sheet) = f.sending(id);
    let edited = f.write(
        "draft.edit",
        json!({"id":id,"revision":created["result"]["revision"],"body":"second"}),
    );
    assert_eq!(edited["ok"], true);
    sheet.answer.send(Ok(())).unwrap();
    f.pump();
    assert_eq!(response(s)["error"]["code"], "conflict");
    assert_eq!(f.store.draft(&raw).unwrap().text, "second");
    assert!(f.store.pending(&raw).is_err());
    let (s, sheet) = f.sending(id);
    sheet.answer.send(Ok(())).unwrap();
    f.pump();
    assert_eq!(response(s)["ok"], true);
    assert_eq!(f.store.pending(&raw).unwrap().mail.text, "second");
    assert_eq!(
        response(f.start("draft.show", json!({"id":id})))["error"]["code"],
        "not_found"
    );
    assert!(f.store.undo_send(&raw).unwrap());
    assert_eq!(
        f.store.agent_draft_meta(&f.account.id, &raw).unwrap().1,
        Some(f.agent.clone())
    );
    let (s, sheet) = f.sending(id);
    assert_eq!(f.write("draft.delete", json!({"id":id}))["ok"], true);
    sheet.answer.send(Ok(())).unwrap();
    f.pump();
    assert_eq!(response(s)["error"]["code"], "conflict");
    assert!(f.store.pending(&raw).is_err());
    f.store
        .set_agent_setting(lagom_core::agent::policy::Setting::Confirm, false)
        .unwrap();
    let draft = f.write("draft.create", json!({"to":"a@example.com"}));
    assert_eq!(f.write("draft.send", json!({"id":draft["result"]["id"]}))["ok"], true);
    let db = Connection::open(f.store.path()).unwrap();
    let (target,detail):(String,String)=db.query_row("SELECT target,detail FROM agent_audit WHERE command='draft.send' AND result='ok' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(target, draft["result"]["id"]);
    assert_eq!(detail, r#"["a@example.com"]"#);
    assert_eq!(
        response(f.start("approve", json!({})))["error"]["code"],
        "invalid_request"
    );
}

#[test]
fn queued_writes_and_approved_sheets_reauthorize_in_the_worker() {
    for waiting in [false, true] {
        for change in ["level", "expiry", "revoke"] {
            let mut f = WriteFixture::new();
            let d = f.write("draft.create", json!({"to":"a@example.com"}));
            let id = &d["result"]["id"];
            let (s, sheet) = if waiting {
                let (s, sheet) = f.sending(id);
                (s, Some(sheet))
            } else {
                (f.start("draft.delete", json!({"id":id})), None)
            };
            let queued = (!waiting).then(|| f.recv());
            match change {
                "level" => f.level(1),
                "expiry" => {
                    Connection::open(f.store.path())
                        .unwrap()
                        .execute("UPDATE agent_tokens SET expires=1", [])
                        .unwrap();
                }
                _ => f.store.revoke_agent_tokens(&f.agent).unwrap(),
            }
            if let Some(sheet) = sheet {
                sheet.answer.send(Ok(())).unwrap();
            }
            if let Some(cmd) = queued {
                f.run(cmd);
            } else {
                f.pump();
            }
            assert_eq!(
                response(s)["error"]["code"],
                if change == "level" { "forbidden" } else { "unauthorized" }
            );
            assert_eq!(f.store.drafts().unwrap().len(), 1);
            assert!(f.store.due_outbox(&f.account.id, i64::MAX).unwrap().is_empty());
        }
    }
}

#[test]
fn writes_scope_all_inputs_and_keep_other_agents_and_personal_drafts_read_only() {
    let mut f = WriteFixture::new();
    let foreign = f.write("draft.create", json!({}));
    let other = f.store.create_agent("test", "Other", "host", false).unwrap();
    f.token = f
        .store
        .issue_token(&other, lagom_core::agent::token::TokenScope::Local)
        .unwrap()
        .to_string();
    f.agent = other;
    f.level(3);
    let personal = lagom_core::outbox::Draft {
        id: "personal".into(),
        account: f.account.id.clone(),
        ..Default::default()
    };
    f.store.save_draft(&personal).unwrap();
    f.level(1);
    let listing = response(f.start("draft.list",json!({})));
    let drafts = listing["result"]["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(),2);
    for draft in drafts {
        assert!(draft.get("revision").is_none());
        let shown = response(f.start("draft.show",json!({"id":draft["id"]})));
        assert_eq!(shown["ok"],true);
        assert!(shown["result"].get("revision").is_none());
    }
    f.level(3);
    for id in [
        foreign["result"]["id"].clone(),
        json!(ObjectId::Draft(f.account.id.clone(), personal.id).encode()),
    ] {
        for cmd in ["draft.edit", "draft.delete", "draft.send"] {
            let args = if cmd == "draft.edit" {
                json!({"id":id,"revision":"x"})
            } else {
                json!({"id":id})
            };
            let s = f.start(cmd, args);
            assert_eq!(response(s)["error"]["code"], "forbidden");
        }
    }
    let (imap, smtp) = lagom_core::presets::servers("other@gmail.com").unwrap();
    let b = Account::new("other@gmail.com".into(), imap, smtp).unwrap();
    f.store.save_account(&b).unwrap();
    let foreign_message = lagom_core::agent::id::MessageId::Header(b.id.clone(), "other".into()).encode();
    for (cmd, args) in [
        (
            "draft.show",
            json!({"id":ObjectId::Draft(b.id.clone(),"personal".into()).encode()}),
        ),
        (
            "draft.delete",
            json!({"id":ObjectId::Draft(b.id.clone(),"personal".into()).encode()}),
        ),
        (
            "draft.send",
            json!({"id":ObjectId::Draft(b.id.clone(),"personal".into()).encode()}),
        ),
        ("draft.create", json!({"forward":foreign_message})),
        ("draft.create", json!({"reply":foreign_message})),
        (
            "event.delete",
            json!({"id":ObjectId::Event(b.id.clone(),"primary".into(),"other".into()).encode()}),
        ),
        ("event.create", json!({"calendar":"only-B"})),
        ("draft.create", json!({"file":"/tmp/file"})),
    ] {
        let mut args = args;
        args["account"] = json!(&f.account.id);
        let expected = if args.get("file").is_some() {
            "invalid_request"
        } else {
            "not_found"
        };
        assert_eq!(response(f.start(cmd, args))["error"]["code"], expected);
    }
    assert_eq!(f.store.drafts().unwrap().len(), 2);
}

#[test]
fn archive_requires_triage_and_every_move_requires_targeted_expunge() {
    for mode in ["unsupported", "move", "lost", "revoked_after_copy", "revoked_after_store"] {
        let supported = mode != "unsupported";
        let mut f = WriteFixture::new();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        f.account.imap = ServerConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            security: Security::Plain,
            username: "fixture".into(),
        };
        f.store.save_account(&f.account).unwrap();
        let inbox = f
            .store
            .save_folder(&f.account.id, "INBOX", Some("/"), FolderRole::Inbox)
            .unwrap();
        f.store.reset_uidvalidity(inbox, 7).unwrap();
        f.store
            .save_folder(&f.account.id, "Archive", Some("/"), FolderRole::Archive)
            .unwrap();
        let row = f
            .store
            .save_message(&Message {
                summary: MessageSummary {
                    account: f.account.id.clone(),
                    folder: inbox,
                    uid: 1,
                    message_id: "m".into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        let id = f.store.agent_message_key(&f.account.id, row).unwrap().0.encode();
        f.level(1);
        assert_eq!(
            response(f.start("action.archive", json!({"id":id})))["error"]["code"],
            "forbidden"
        );
        assert_eq!(f.store.message_summary(row).unwrap().folder, inbox);
        f.level(3);
        Connection::open(f.store.path())
            .unwrap()
            .execute(
                "UPDATE agent_access SET level=2 WHERE kind='mail' AND agent=?",
                [&f.agent],
            )
            .unwrap();
        let (imap, smtp) = lagom_core::presets::servers("b@gmail.com").unwrap();
        let b = Account::new("b@gmail.com".into(), imap, smtp).unwrap();
        f.store.save_account(&b).unwrap();
        f.store.save_folder(&b.id, "B only", None, FolderRole::Other).unwrap();
        assert_eq!(
            f.write("action.move", json!({"id":id,"folder":"B only"}))["error"]["code"],
            "not_found"
        );
        for role in [FolderRole::Sent, FolderRole::Drafts] {
            f.store.save_folder(&f.account.id, role.as_str(), None, role).unwrap();
            assert_eq!(f.write("action.move", json!({"id":id,"folder":role.as_str()}))["error"]["code"], "forbidden");
        }
        let path = f.store.path().to_owned();
        let peer = thread::spawn(move || {
            let mut moves = 0;
            for _ in 0..if supported { 1 } else { 3 } {
                let (s, _) = listener.accept().unwrap();
                let mut p = BufReader::new(s);
                writeln!(p.get_mut(), "* OK fixture\r").unwrap();
                loop {
                    let mut line = String::new();
                    if p.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                    let data = match cmd {
                        "LOGIN \"fixture\" \"fixture\"" => "",
                        "CAPABILITY" if mode.starts_with("revoked_after_") => "* CAPABILITY IMAP4rev1 UIDPLUS\r\n",
                        "CAPABILITY" if supported => "* CAPABILITY IMAP4rev1 MOVE\r\n",
                        "CAPABILITY" => "* CAPABILITY IMAP4rev1\r\n",
                        _ if supported && cmd.starts_with("LIST ") => {
                            "* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Archive) \"/\" \"Archive\"\r\n"
                        }
                        _ if supported && cmd.starts_with("SELECT ") => {
                            "* 0 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 1] next\r\n"
                        }
                        "UID COPY 1 \"Archive\"" if mode == "revoked_after_store" => {
                            moves += 1;
                            ""
                        }
                        "UID STORE 1 +FLAGS.SILENT (\\Deleted)" if mode == "revoked_after_store" => {
                            Connection::open(&path).unwrap().execute("DELETE FROM agent_tokens", []).unwrap();
                            ""
                        }
                        "UID COPY 1 \"Archive\"" if mode == "revoked_after_copy" => {
                            Connection::open(&path).unwrap().execute("DELETE FROM agent_tokens", []).unwrap();
                            moves += 1;
                            ""
                        }
                        "UID MOVE 1 \"Archive\"" if mode == "lost" => {
                            moves += 1;
                            break;
                        }
                        "UID MOVE 1 \"Archive\"" if supported => {
                            moves += 1;
                            ""
                        }
                        "UID SEARCH ALL" if supported => "* SEARCH\r\n",
                        _ => panic!("unexpected IMAP command: {cmd}"),
                    };
                    write!(p.get_mut(), "{data}{tag} OK fixture\r\n").unwrap();
                }
            }
            assert_eq!(moves, usize::from(supported));
        });
        for cmd in if supported {
            vec!["action.archive"]
        } else {
            vec!["action.archive", "action.move", "action.trash"]
        } {
            let session = f.rt.block_on(ImapSync::login(&f.account, "fixture")).unwrap();
            f.worker.sessions.insert(f.account.id.clone(), session);
            let args = if cmd == "action.move" {
                json!({"id":id,"folder":"Archive"})
            } else {
                json!({"id":id})
            };
            let result = f.write(cmd, args);
            if matches!(mode, "lost" | "revoked_after_copy" | "revoked_after_store") {
                assert_eq!(result["error"]["code"], "uncertain");
                assert_eq!(f.store.agent_audit(1).unwrap()[0].result, "uncertain");
            } else if supported {
                assert_eq!(result["ok"], true, "{result}");
                assert!(f.store.message_summary(row).is_err());
            } else {
                assert_eq!(result["error"]["code"], "unsupported");
                assert_eq!(f.store.message_summary(row).unwrap().folder, inbox);
            }
        }
        f.worker.sessions.clear();
        peer.join().unwrap();
    }
}

#[test]
fn calendar_writes_confirm_existing_guests_and_reject_stale_responses() {
    use lagom_core::agent::policy::Setting;
    let mut f = WriteFixture::new();
    f.account.auth = lagom_core::account::Auth::OAuth(Provider::Google);
    f.store
        .save_authorized_account(&f.account, oauth::GOOGLE_CALENDAR_SCOPES)
        .unwrap();
    let cal = Calendar {
        remote_id: "primary".into(),
        name: "Fixture".into(),
        shown: true,
        writable: true,
        color: None,
    };
    let old = NewEvent {
        title: "Old event".into(),
        start: "2026-11-24".into(),
        end: "2026-11-25".into(),
        all_day: true,
        time_zone: String::new(),
        location: None,
        description: None,
        attendees: vec![],
        meet: false,
    }
    .event("existing".into())
    .unwrap();
    let (start, end) = (old.start_utc, old.end_utc);
    let stale_rows = vec![(cal, vec![old.clone()])];
    f.store
        .replace_calendar_window(&f.account.id, start, end, &stale_rows)
        .unwrap();
    f.store.set_agent_setting(Setting::Confirm, false).unwrap();
    let id = ObjectId::Event(f.account.id.clone(), "primary".into(), "existing".into()).encode();
    let create = json!({"guests":"guest@xn--bcher-kva.example,GUEST@bücher.example","title":"New event","start":"2026-11-24","end":"2026-11-25","calendar":"primary"});
    let update = json!({"id":id,"title":"Updated","description":r"\u{202e}"});
    let delete = json!({"id":id});
    f.level(1);
    for (cmd, args) in [
        ("event.create", create.clone()),
        ("event.update", update.clone()),
        ("event.delete", delete.clone()),
    ] {
        assert_eq!(response(f.start(cmd, args))["error"]["code"], "forbidden");
    }
    f.level(3);
    for bad in [r#""quoted"@example.org"#, "å@example.org"] {
        let mut invalid = create.clone();
        invalid["guests"] = json!(bad);
        assert_eq!(response(f.start("event.create",invalid))["error"]["code"],"invalid_request");
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    crate::agent::write::CALENDAR_FIXTURES
        .lock()
        .unwrap()
        .insert(f.account.id.clone(), base);
    let policy_path = f.store.path().to_owned();
    let peer = thread::spawn(move || {
        let remote = json!({"id":"existing","etag":"\"old\"","summary":"Old event","description":"\u{202e}",
            "attendees":[{"email":"hidden@example.org"}],"start":{"date":"2026-11-24"},"end":{"date":"2026-11-25"}});
        let created = json!({"id":"created","summary":"New event","start":{"date":"2026-11-24"},"end":{"date":"2026-11-25"}});
        let updated =
            json!({"id":"existing","summary":"Updated","start":{"date":"2026-11-24"},"end":{"date":"2026-11-25"}});
        let consent = json!({"error":{"errors":[{"reason":"insufficientPermissions"}]}});
        let steps = [
            ("POST", 200, &created),
            ("GET", 200, &remote),
            ("GET", 200, &remote),
            ("PATCH", 412, &remote),
            ("GET", 200, &remote),
            ("GET", 200, &remote),
            ("PATCH", 0, &remote),
            ("GET", 200, &remote),
            ("GET", 200, &remote),
            ("PATCH", 200, &updated),
            ("GET", 200, &remote),
            ("GET", 200, &remote),
            ("DELETE", 204, &Value::Null),
            ("POST", 403, &consent),
        ];
        for (method, status, data) in steps {
            let (s, _) = listener.accept().unwrap();
            let mut io = BufReader::new(s);
            let mut headers = String::new();
            while !headers.ends_with("\r\n\r\n") {
                assert!(io.read_line(&mut headers).unwrap() > 0);
            }
            assert!(
                headers.starts_with(&format!("{method} /calendars/primary/events")),
                "{headers}"
            );
            if matches!(method, "PATCH" | "DELETE") {
                assert!(headers.to_lowercase().contains("if-match: \"old\""));
            }
            let length = headers
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(0);
            let mut bytes = vec![0; length];
            io.read_exact(&mut bytes).unwrap();
            if method == "POST" {
                let payload: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(payload["attendees"],json!([{"email":"guest@xn--bcher-kva.example"}]));
            }
            if status != 0 {
                if method == "PATCH" && status == 200 {
                    Connection::open(&policy_path).unwrap().execute("UPDATE agent_access SET level=1 WHERE kind='calendar'", []).unwrap();
                }
                let body = if method == "PATCH" && status == 200 { "{}".into() } else if status == 204 { String::new() } else { data.to_string() };
                write!(
                    io.get_mut(),
                    "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        }
    });
    for (cmd, args, expected) in [
        ("event.create", &create, None),
        ("event.update", &update, Some("conflict")),
        ("event.update", &update, Some("uncertain")),
        ("event.update", &update, None),
        ("event.delete", &delete, None),
    ] {
        let s = f.start(cmd, args.clone());
        let Cmd::AgentConfirm(sheet) = f.recv() else {
            panic!("calendar skipped approval")
        };
        let row = |label: &str, value: &str| sheet.rows.iter().find(|r| r.label == label && r.value == value);
        assert!(row("Calendar", "Fixture").is_some());
        if cmd == "event.create" { assert_eq!(sheet.rows.iter().filter(|r| r.label == "Guest").count(),1); }
        if cmd != "event.create" {
            // Google's current guest shows once, untagged.
            assert!(row("Guest", "hidden@example.org").is_some_and(|r| r.tag.is_none()));
            if cmd == "event.update" {
                assert_eq!(sheet.previous_description.as_deref(), Some(r"\u{202e}"));
                assert_eq!(sheet.body, r"\\u{202e}");
            } else {
                assert!(sheet.previous_description.is_none());
                assert_eq!(sheet.body, r"\u{202e}");
            }
        }
        if cmd == "event.update" {
            assert_eq!(row("Title", "Updated").and_then(|r| r.old.as_deref()), Some("Old event"));
        }
        let (release, held) = tokio::sync::oneshot::channel();
        let stale = stale_rows.clone();
        f.rt.block_on(async {
            f.worker.start_calendar(f.account.clone(), start, end, async move {
                let _ = held.await;
                Ok(stale)
            });
        });
        sheet.answer.send(Ok(())).unwrap();
        f.pump();
        let result = response(s);
        if let Some(code) = expected {
            assert_eq!(result["error"]["code"], code, "{result}");
        } else {
            assert_eq!(result["ok"], true, "{result}");
        }
        let _ = release.send(());
        let stale = f.rt.block_on(f.worker.calendars.join_next_with_id()).unwrap();
        f.worker.calendar_finished(stale);
        let rows = f.store.agent_events(&f.account.id, start, end).unwrap();
        if cmd == "event.delete" {
            assert!(!rows.iter().any(|r| r.event.remote_id == "existing"));
        }
        if cmd == "event.update" && expected.is_none() {
            assert!(!rows.iter().any(|r| r.event.remote_id == "existing"));
            f.level(3);
            f.store.insert_calendar_event(&f.account.id, "primary", &old).unwrap();
        }
        assert!(rows.iter().any(|r| r.event.remote_id == "created"));
    }
    let s = f.start("event.create", create.clone());
    let Cmd::AgentConfirm(sheet) = f.recv() else {
        panic!("calendar skipped approval")
    };
    sheet.decide(true);
    f.pump();
    assert_eq!(response(s)["error"]["code"], "forbidden");
    assert!(!oauth::calendar_writable(
        &f.store.account_scopes(&f.account.id).unwrap()
    ));
    assert_eq!(response(f.start("event.create", create))["error"]["code"], "forbidden");
    peer.join().unwrap();
    crate::agent::write::CALENDAR_FIXTURES
        .lock()
        .unwrap()
        .remove(&f.account.id);
}

#[test]
fn unanswered_send_expires_after_two_minutes_without_enqueuing() {
    let mut f = WriteFixture::new();
    let d = f.write("draft.create", json!({"to":"a@example.com"}));
    let (s, sheet) = f.sending(&d["result"]["id"]);
    s.set_read_timeout(Some(Duration::from_secs(130))).unwrap();
    assert_eq!(response(s)["error"]["code"], "denied");
    assert!(sheet.answer.is_closed());
    assert_eq!(f.store.drafts().unwrap().len(), 1);
    assert!(f.store.due_outbox(&f.account.id, i64::MAX).unwrap().is_empty());
}

#[test]
fn reply_and_forward_keep_the_original_account_headers_and_cached_files() {
    let mut f = WriteFixture::new();
    let folder = f
        .store
        .save_folder(&f.account.id, "INBOX", None, FolderRole::Inbox)
        .unwrap();
    let row = f
        .store
        .save_message(&Message {
            summary: MessageSummary {
                account: f.account.id.clone(),
                folder,
                uid: 1,
                from_address: "sender@example.org".into(),
                message_id: "source@example.org".into(),
                subject: "Original".into(),
                ..Default::default()
            },
            body_text: "quoted text".into(),
            body_html: Some("<p>secret HTML only</p>".into()),
            ..Default::default()
        })
        .unwrap();
    let db = Connection::open(f.store.path()).unwrap();
    db.execute("UPDATE messages SET attachments_loaded=1 WHERE id=?", [row])
        .unwrap();
    db.execute(
        "INSERT INTO attachments VALUES(?,'2','original.txt','text/plain',3,'7bit')",
        [row],
    )
    .unwrap();
    db.execute(
        "INSERT INTO attachment_cache(message,section,data) VALUES(?,'2',x'616263')",
        [row],
    )
    .unwrap();
    let id = f.store.agent_message_key(&f.account.id, row).unwrap().0.encode();
    db.execute("INSERT INTO attachments VALUES(?,'3','large','text/plain',20971520,'7bit')", [row]).unwrap();
    db.execute("INSERT INTO attachment_cache(message,section,data) VALUES(?,'3',zeroblob(20971520))", [row]).unwrap();
    assert_eq!(f.write("draft.create", json!({"forward":id}))["error"]["code"], "too_large");
    db.execute("DELETE FROM attachments WHERE message=? AND section='3'", [row]).unwrap();
    for kind in ["reply", "forward"] {
        let result = f.write("draft.create", json!({(kind):id,"body":"new text"}));
        let ObjectId::Draft(a, id) = ObjectId::decode(result["result"]["id"].as_str().unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(a, f.account.id);
        let d = f.store.draft(&id).unwrap();
        assert!(d.quoted.is_none());
        assert!(d.text.starts_with("new text") && d.text.contains("quoted text"));
        f.store.set_agent_setting(lagom_core::agent::policy::Setting::Confirm, false).unwrap();
        let key = result["result"]["id"].clone();
        assert_eq!(f.write("draft.edit",json!({"id":key,"revision":result["result"]["revision"],"to":"recipient@example.org"}))["ok"], true);
        assert_eq!(f.write("draft.send",json!({"id":key}))["ok"], true);
        let raw = String::from_utf8(f.store.pending(&d.id).unwrap().raw).unwrap();
        assert!(!raw.contains("text/html") && !raw.contains("secret HTML only"));
        assert!(f.store.undo_send(&d.id).unwrap());
        if kind == "reply" {
            assert_eq!(d.to, "sender@example.org");
            assert_eq!(d.reply.unwrap().0.as_deref(), Some("<source@example.org>"));
            assert!(d.attachments.is_empty());
        } else {
            assert!(d.reply.is_none());
            assert_eq!(d.attachments.len(), 1);
            assert_eq!(d.attachments[0].filename, "original.txt");
            assert_eq!(d.attachments[0].data, b"abc");
        }
    }
}

#[test]
fn waiting_attachments_reauthenticate_before_returning_bytes() {
    use lagom_core::agent::{
        id::MessageId,
        policy::{Access, Level, Resource},
        token::TokenScope,
    };

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("mail.db");
    let sock = dir.path().join("agent.sock");
    let store = Store::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    let (imap, smtp) = lagom_core::presets::servers("a@gmail.com").unwrap();
    let account = Account::new("a@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let inbox = store
        .save_folder(&account.id, "INBOX", None, FolderRole::Inbox)
        .unwrap();
    let row = store
        .save_message(&Message {
            summary: MessageSummary {
                account: account.id.clone(),
                folder: inbox,
                uid: 1,
                message_id: "header".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    db.execute(
        "INSERT INTO attachments VALUES(?,'1','a','text/plain',1,'7bit')",
        [row],
    )
    .unwrap();
    let id = ObjectId::Attachment(
        MessageId::Header(account.id.clone(), "header".into()),
        "1".into(),
    )
    .encode();
    store
        .set_agent_setting(lagom_core::agent::policy::Setting::Enabled, true)
        .unwrap();
    let (bridge, mut commands) = Bridge::detached();
    let _server = Server::start(&path, bridge);
    wait(|| sock.exists());
    for change in [
        "unrelated",
        "lower",
        "revoke",
        "revoke_at_reply",
        "expire",
        "source",
    ] {
        let agent = store.create_agent("test", "remote", "host", true).unwrap();
        let mut access = Access {
            resource: Resource::Mail,
            account: account.id.clone(),
            level: Level::Read,
        };
        store.set_agent_access("", &access).unwrap();
        let token = store
            .issue_token(
                &agent,
                TokenScope::Remote {
                    accounts: std::slice::from_ref(&account.id),
                    level: Level::Read,
                    ranges: &[],
                    expires: (change == "expire").then(|| chrono::Utc::now().timestamp() + 2),
                },
            )
            .unwrap();
        let req = format!(
            "{}\n",
            json!({"v":1,"cmd":"attachment","token":token.as_str(),"source_ip":"::1","args":{"id":id}})
        );
        thread::scope(|scope| {
            let call = scope.spawn(|| wire(&sock, req.as_bytes()));
            let mut queued = None;
            wait(|| {
                queued = commands.try_recv().ok();
                queued.is_some()
            });
            let Some(Cmd::AgentAttachment(request)) = queued else {
                panic!()
            };
            let expected = match change {
                "unrelated" => {
                    store.set_agent_access("other", &access).unwrap();
                    request.reply.send(Ok(vec![7])).unwrap();
                    assert!(matches!(
                        serde_json::from_slice(&call.join().unwrap()),
                        Ok(Response::Success(_))
                    ));
                    return;
                }
                "lower" => {
                    access.level = Level::None;
                    store.set_agent_access("", &access).unwrap();
                    Code::NotFound
                }
                "revoke" | "revoke_at_reply" => {
                    store.revoke_agent_tokens(&agent).unwrap();
                    Code::Unauthorized
                }
                "expire" => Code::Unauthorized,
                _ => {
                    db.execute(
                        "UPDATE agent_tokens SET cidr='[\"127.0.0.1/32\"]' WHERE agent=?",
                        [&agent],
                    )
                    .unwrap();
                    Code::Unauthorized
                }
            };
            if change == "revoke_at_reply" {
                let _ = request.reply.send(Ok(vec![7]));
                assert_eq!(call.join().unwrap(), fixed(expected));
            } else {
                // Hold the worker reply open. The service must finish without it.
                assert_eq!(call.join().unwrap(), fixed(expected));
                assert!(request.reply.is_closed());
                assert_eq!(
                    request
                        .resolve(&Store::agent_reader(&path, || false).unwrap())
                        .unwrap_err(),
                    expected
                );
            }
            assert_eq!(
                db.query_row(
                    "SELECT sum(refusals) FROM agent_audit WHERE agent=?",
                    [&agent],
                    |r| r.get::<_, usize>(0)
                )
                .unwrap(),
                1
            );
        });
    }
}

#[test]
fn protocol_limits_apply_before_confirmation() {
    let f = WriteFixture::new();
    let addresses = vec!["a@example.org"; 101].join(",");
    for (cmd, args) in [
        ("draft.create", json!({"to":addresses})),
        ("draft.create", json!({"body":"å".repeat(100_001)})),
        ("event.create", json!({"description":"å".repeat(8193)})),
        ("event.create", json!({"location":"å".repeat(1025)})),
        ("event.create", json!({"guests":addresses})),
        ("event.create", json!({"title":"a\nb"})),
        ("event.update", json!({"location":"a\rb"})),
        ("event.create", json!({"title":"å".repeat(999)})),
        ("draft.edit", json!({"subject":"a".repeat(999)})),
    ] {
        assert_eq!(response(f.start(cmd,args))["error"]["code"], "invalid_request");
    }
}

#[test]
fn draft_show_limits_unicode_body_and_audits_its_target() {
    let mut f = WriteFixture::new();
    let d = f.write("draft.create", json!({"body":"å".repeat(100_000),"to":"Joe <j@bücher.org>","cc":"J@xn--bcher-kva.org"}));
    let id = &d["result"]["id"];
    let result = response(f.start("draft.show", json!({"id":id})));
    assert_eq!(result["result"]["body"].as_str().unwrap().chars().count(),100_000);
    assert_eq!(result["result"]["truncated"],false);
    let target:String = Connection::open(f.store.path()).unwrap().query_row(
        "SELECT target FROM agent_audit WHERE command='draft.show'", [], |r|r.get(0)).unwrap();
    assert_eq!(&target, id.as_str().unwrap());
    let (_, sheet) = f.sending(id);
    assert_eq!(sheet.body.chars().count(), 100_000);
    assert_eq!(sheet.rows.iter().filter(|r| matches!(r.label,"To"|"Cc")).count(),1);
    let ObjectId::Draft(_, raw) = ObjectId::decode(id.as_str().unwrap()).unwrap() else { panic!() };
    let mut draft = f.store.draft(&raw).unwrap();
    draft.text.push('å');
    f.store.save_draft(&draft).unwrap();
    let shown = response(f.start("draft.show",json!({"id":id})));
    assert_eq!(shown["result"]["truncated"],true);
    assert_eq!(shown["result"]["body"].as_str().unwrap().chars().count(),100_000);
    assert_eq!(response(f.start("draft.send",json!({"id":id})))["error"]["code"],"invalid_request");
    f.store.set_agent_setting(lagom_core::agent::policy::Setting::Confirm,false).unwrap();
    for bad in [r#""quoted"@example.org"#, "å@example.org"] {
        let draft = f.write("draft.create",json!({"to":bad}));
        assert_eq!(response(f.start("draft.send",json!({"id":draft["result"]["id"]})))["error"]["code"],"invalid_request");
    }
}

#[test]
fn local_write_rolls_back_when_audit_cannot_finish() {
    let mut f = WriteFixture::new();
    let client = f.start("draft.create", json!({}));
    let cmd = f.recv();
    let db = Connection::open(f.store.path()).unwrap();
    assert_eq!(f.store.agent_audit(1).unwrap()[0].result, "pending");
    db.execute_batch("CREATE TRIGGER reject_ok BEFORE UPDATE OF result ON agent_audit WHEN NEW.result='ok'
        BEGIN SELECT RAISE(ABORT,'fixture'); END").unwrap();
    f.run(cmd);
    assert_eq!(response(client)["error"]["code"],"internal");
    assert!(f.store.drafts().unwrap().is_empty());
    assert_eq!(f.store.agent_audit(1).unwrap()[0].result, "internal");
}

#[test]
fn completed_remote_write_finalizes_after_busy_audit_and_disconnected_client() {
    let mut f = WriteFixture::new();
    let folder = f.store.save_folder(&f.account.id,"INBOX",None,FolderRole::Inbox).unwrap();
    let row = f.store.save_message(&Message { summary: MessageSummary { account:f.account.id.clone(),folder,uid:1,
        message_id:"target".into(),..Default::default() },..Default::default() }).unwrap();
    let id = f.store.agent_message_key(&f.account.id,row).unwrap().0.encode();
    for disconnect in [false,true] {
        let socket = f.start("action.star",json!({"id":id}));
        let Cmd::AgentWrite(job) = f.recv() else { panic!("missing job"); };
        assert_eq!(f.store.agent_audit(1).unwrap()[0].result,"pending");
        let lock = Connection::open(f.store.path()).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        let socket = (!disconnect).then_some(socket);
        job.reply.send(Ok((json!({}),None))).unwrap();
        if !disconnect { thread::sleep(Duration::from_millis(5200)); }
        lock.execute_batch("COMMIT").unwrap();
        if let Some(socket) = socket { assert_eq!(response(socket)["ok"],true); }
        wait(|| f.store.agent_audit(1).unwrap()[0].result == "ok");
    }
}

use super::*;
use lagom_core::account::Auth;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

fn fixture() -> (Worker, Account, UnboundedReceiver<Event>, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("lagom-create-{}-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let (me, _) = unbounded_channel();
    let (events, out) = unbounded_channel();
    let worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None,
        calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None,
        reauth_required: HashSet::new(), discovery: None, me, events };
    let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
    let mut account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    account.auth = Auth::OAuth(Provider::Google);
    worker.store.save_authorized_account(&account, &format!("https://mail.google.com/ {}", oauth::GOOGLE_CALENDAR_SCOPES)).unwrap();
    worker.store.replace_calendar_window(&account.id, 1, 2, &[(Calendar { remote_id: "primary".into(), name: "Fixture".into(), color: None, shown: true, writable: true }, vec![])]).unwrap();
    (worker, account, out, path)
}

fn draft() -> NewEvent {
    NewEvent { title: "Fixture".into(), start: "2026-11-09T09:00:00+01:00".into(), end: "2026-11-09T10:00:00+01:00".into(),
        all_day: false, time_zone: "Europe/Stockholm".into(), description: None, attendees: vec![], meet: true }
}

async fn server(method: &'static str, status: &'static str, body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") { headers.push(socket.read_u8().await.unwrap()); }
        let headers = String::from_utf8(headers).unwrap();
        assert!(headers.starts_with(&format!("{method} /calendars/primary/events")));
        let length = headers.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())).unwrap_or(0);
        socket.read_exact(&mut vec![0; length]).await.unwrap();
        socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    (url, server)
}

#[tokio::test]
async fn created_event_is_cached_before_completion_and_older_fetch_cannot_erase_it() {
    let (mut worker, account, mut events, path) = fixture();
    worker.start_calendar(account.clone(), 1, 3, std::future::ready(Ok(vec![])));
    let stale = worker.calendars.join_next_with_id().await.unwrap();
    let (task_tx, mut task_rx) = unbounded_channel();
    worker.task_sync = Some(task_tx);
    let (base, server) = server("POST", "200 OK", r#"{"id":"created","summary":"Fixture","start":{"dateTime":"2026-11-09T09:00:00+01:00"},"end":{"dateTime":"2026-11-09T10:00:00+01:00"},"hangoutLink":"https://meet.example.test/new"}"#).await;
    worker.start_create(41, account.clone(), "primary".into(), async move {
        lagom_core::google::create_with(&base, "primary", &draft(), |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await
    });
    // A pending create must not defer other commands.
    worker.quick(Cmd::MarkSeen(-1)).await;
    assert!(matches!(events.try_recv().unwrap(), Event::Seen(-1)));
    assert!(matches!(events.try_recv().unwrap(), Event::Changed));
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 41, error: None }));
    assert!(matches!(events.try_recv().unwrap(), Event::CalendarSync(_, SyncState::Synced(_))));
    assert!(matches!(events.try_recv().unwrap(), Event::Changed));
    let start = draft().event("fixture".into()).unwrap().start_utc;
    let events = worker.store.today_events("2026-11-09", start, start + 3600).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.join_url.as_deref(), Some("https://meet.example.test/new"));
    assert!(task_rx.try_recv().is_err(), "caching a created event must not pause Google Tasks");
    worker.calendar_finished(stale);
    assert_eq!(worker.store.today_events("2026-11-09", start, start + 3600).unwrap(), events);
    assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((1, 2)));
    assert!(worker.create_tasks.is_empty());
    server.await.unwrap();
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn create_scope_denial_reports_op_and_marks_calendar_access_without_losing_mail_scopes() {
    let (mut worker, account, mut events, path) = fixture();
    let (base, server) = server("POST", "403 Forbidden", r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#).await;
    worker.start_create(42, account.clone(), "primary".into(), async move {
        lagom_core::google::create_with(&base, "primary", &draft(), |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await
    });
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 42, error: Some(_) }));
    let scopes = worker.store.account_scopes(&account.id).unwrap();
    assert!(!oauth::calendar_allowed(&scopes));
    assert!(!oauth::calendar_writable(&scopes));
    assert!(oauth::has_scope(&scopes, "https://mail.google.com/"));
    assert!(oauth::has_scope(&scopes, "https://www.googleapis.com/auth/tasks"));
    assert!(worker.reauth_required.is_empty());
    server.await.unwrap();
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn create_rate_limit_preserves_calendar_scopes_and_followup_sync() {
    for body in [
        r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#,
        r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
        r#"{"error":{"errors":[{"reason":"quotaExceeded"}]}}"#,
        r#"{"error":{"errors":[{"reason":"forbidden"}]}}"#,
        "{}", "invalid JSON",
    ] {
        let (mut worker, account, mut events, path) = fixture();
        let scopes = worker.store.account_scopes(&account.id).unwrap();
        let (base, server) = server("POST", "403 Forbidden", body).await;
        worker.start_create(42, account.clone(), "primary".into(), async move {
            lagom_core::google::create_with(&base, "primary", &draft(), |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await
        });
        let result = worker.event_creates.join_next_with_id().await.unwrap();
        worker.event_created(result);
        assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 42, error: Some(_) }));
        assert_eq!(worker.store.account_scopes(&account.id).unwrap(), scopes, "{body}");
        assert!(worker.reauth_required.is_empty());
        worker.start_calendar(account.clone(), 10, 20, std::future::ready(Ok(vec![])));
        let result = worker.calendars.join_next_with_id().await.unwrap();
        worker.calendar_finished(result);
        assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((10, 20)));
        server.await.unwrap();
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn create_command_rechecks_calendar_and_scopes_before_network() {
    let (mut worker, account, mut events, path) = fixture();
    worker.quick(Cmd::CreateEvent { op: 1, account: account.id.clone(), calendar: "missing".into(), event: draft() }).await;
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 1, error: Some(_) }));
    worker.store.set_account_scopes(&account.id, "https://www.googleapis.com/auth/calendar.readonly").unwrap();
    worker.quick(Cmd::CreateEvent { op: 2, account: account.id.clone(), calendar: "primary".into(), event: draft() }).await;
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 2, error: Some(_) }));
    worker.store.set_account_scopes(&account.id, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    worker.store.replace_calendar_window(&account.id, 1, 2, &[(Calendar { remote_id: "primary".into(), name: "Read only".into(), color: None, shown: true, writable: false }, vec![])]).unwrap();
    worker.quick(Cmd::CreateEvent { op: 3, account: account.id.clone(), calendar: "primary".into(), event: draft() }).await;
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 3, error: Some(_) }));
    assert!(worker.event_creates.is_empty());
    assert!(worker.create_tasks.is_empty());
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn failed_create_reports_error_but_removed_account_preserves_remote_success() {
    let (mut worker, account, mut events, path) = fixture();
    worker.start_create(1, account.clone(), "primary".into(), async { panic!("fixture task failure") });
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 1, error: Some(_) }));
    while events.try_recv().is_ok() {}
    worker.start_create(2, account.clone(), "primary".into(), std::future::ready(draft().event("created".into()).map(|event| CreatedEvent { event, conference_pending: false })));
    worker.store.delete_account(&account.id).unwrap();
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 2, error: None }));
    assert!(worker.store.today_events("2026-11-09", 0, i64::MAX).unwrap().is_empty());
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn cache_failure_keeps_remote_success_and_retries_without_another_post() {
    let (mut worker, account, mut events, path) = fixture();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL, 'fixture disk full'); END;").unwrap();
    let (base, server) = server("POST", "200 OK", r#"{"id":"created","start":{"dateTime":"2026-11-09T09:00:00+01:00"},"end":{"dateTime":"2026-11-09T10:00:00+01:00"}}"#).await;
    worker.start_create(43, account.clone(), "primary".into(), async move {
        lagom_core::google::create_with(&base, "primary", &draft(), |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await
    });
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 43, error: None }), "a confirmed POST must close the draft even when caching fails");
    assert!(matches!(events.try_recv().unwrap(), Event::CalendarSync(_, SyncState::Failed(_))));
    assert!(worker.store.today_events("2026-11-09", 0, i64::MAX).unwrap().is_empty());
    server.await.unwrap();
    db.execute_batch("DROP TRIGGER reject_event").unwrap();
    worker.reauth_required.insert(account.id.clone());
    let (_tx, rx) = unbounded_channel();
    tokio::select! {
        _ = work(worker, rx) => panic!("worker stopped"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    events.recv().await.unwrap();
                    let count: i64 = db.query_row("SELECT COUNT(*) FROM events WHERE remote_id='created'", [], |r| r.get(0)).unwrap();
                    if count == 1 { break; }
                }
            }).await.expect("cache retry must not need another Save or POST");
        } => {}
    }
    drop(db);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn work_completes_creation_while_the_same_folder_is_stalled() {
    use lagom_core::account::{Security, ServerConfig};
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (mut worker, account, mut events, path) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
    let mail = Account::new("fixture@example.test".into(), config.clone(), config).unwrap();
    worker.store.save_account(&mail).unwrap();
    worker.reauth_required.insert(account.id.clone());
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
        let (mut started, mut resume) = (Some(started), Some(resume));
        loop {
            let mut line = String::new();
            match socket.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {},
                // Dropping the worker closes its cached session without LOGOUT.
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(e) => panic!("{e}"),
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let data = if command.starts_with("LOGIN ") { "" }
                else if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" }
                else if command.starts_with("LIST ") { "* LIST () NIL \"INBOX\"\r\n* LIST (\\Archive) NIL \"Archive\"\r\n" }
                else if command.starts_with("SELECT ") {
                    if command == "SELECT \"INBOX\"" {
                        started.take().unwrap().send(()).unwrap();
                        resume.take().unwrap().await.unwrap();
                    }
                    "* 0 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 1] next\r\n"
                } else if command == "UID SEARCH ALL" { "* SEARCH\r\n" }
                else { panic!("unexpected fixture command: {command}") };
            socket.get_mut().write_all(format!("{data}{tag} OK fixture\r\n").as_bytes()).await.unwrap();
        }
    });
    let session = ImapSync::login(&mail, "fixture").await.unwrap();
    worker.sessions.insert(mail.id.clone(), session);
    worker.start_create(44, account, "primary".into(), async move {
        ready.await.unwrap();
        draft().event("created".into()).map(|event| CreatedEvent { event, conference_pending: false })
    });
    worker.deferred.push_back(Cmd::Sync(Some(mail.id.clone())));
    let (_tx, rx) = unbounded_channel();
    tokio::select! {
        _ = work(worker, rx) => panic!("worker stopped"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match events.recv().await.unwrap() {
                        Event::EventCreated { op: 44, error: None } => break,
                        Event::Sync(id, SyncState::Synced(_)) if id == mail.id => panic!("mail finished before creation completion"),
                        _ => {}
                    }
                }
                assert_eq!(Store::open(&path).unwrap().today_events("2026-11-09", 0, i64::MAX).unwrap().len(), 1);
                release.send(()).unwrap();
                loop {
                    if matches!(events.recv().await.unwrap(), Event::Sync(id, SyncState::Synced(_)) if id == mail.id) { break; }
                }
            }).await.expect("creation must finish while the same SELECT remains blocked");
        } => {}
    }
    server.await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn pending_conference_retries_only_gets_and_caches_the_final_link() {
    let (mut worker, account, mut events, path) = fixture();
    let mut event = draft();
    event.start = "2027-05-09T09:00:00Z".into();
    event.end = "2027-05-09T10:00:00Z".into();
    let created = CreatedEvent { event: event.event("future".into()).unwrap(), conference_pending: true };
    let ready = CreatedEvent { event: CalendarEvent { join_url: Some("https://meet.example.test/future".into()), ..created.event.clone() }, conference_pending: false };
    worker.start_create(45, account.clone(), "primary".into(), std::future::ready(Ok(created.clone())));
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 45, error: None }));
    assert!(worker.pending_events[0].cached);
    assert_eq!(worker.store.today_events("2027-05-09", ready.event.start_utc, ready.event.end_utc).unwrap().len(), 1);
    worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("polls must wait for their retry deadline"));
    for response in [Err(Error::Network("fixture offline")), Ok(created), Ok(ready.clone())] {
        worker.pending_events[0].retry_at = tokio::time::Instant::now();
        let mut response = Some(response);
        worker.retry_events_with(|requested, calendar, id| {
            assert_eq!(requested, account);
            assert_eq!(calendar, "primary");
            assert_eq!(id, "future");
            std::future::ready(response.take().unwrap())
        });
        worker.pending_events[0].retry_at = tokio::time::Instant::now();
        worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("do not overlap requests for one event"));
        let result = worker.event_creates.join_next_with_id().await.unwrap();
        worker.event_created(result);
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, Event::EventCreated { .. }), "follow-up must never reopen or re-complete the draft");
        }
    }
    let cached = worker.store.today_events("2027-05-09", ready.event.start_utc, ready.event.end_utc).unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].event, ready.event);
    assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((1, 2)));
    assert!(worker.pending_events.is_empty());
    assert!(worker.create_tasks.is_empty());
    worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("completed conferences must stop polling"));
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn pending_conference_pauses_for_consent_and_discards_removed_accounts() {
    let (mut worker, account, _, path) = fixture();
    let created = CreatedEvent { event: draft().event("created".into()).unwrap(), conference_pending: true };
    worker.start_create(46, account.clone(), "primary".into(), std::future::ready(Ok(created)));
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    worker.pending_events[0].retry_at = tokio::time::Instant::now();
    worker.store.set_account_scopes(&account.id, "https://mail.google.com/").unwrap();
    worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("missing consent must not request credentials"));
    assert_eq!(worker.pending_events.len(), 1);
    worker.store.delete_account(&account.id).unwrap();
    worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("removed accounts must not request credentials"));
    assert!(worker.pending_events.is_empty());
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn deleted_remote_event_is_removed_outside_fetch_window_without_reopening_draft() {
    for (status, body) in [("404 Not Found", "{}"), ("410 Gone", "{}"), ("200 OK", r#"{"id":"created","status":"cancelled"}"#)] {
        let (mut worker, account, mut events, path) = fixture();
        let mut event = draft();
        event.start = "2027-05-09T09:00:00Z".into();
        event.end = "2027-05-09T10:00:00Z".into();
        let created = CreatedEvent { event: event.event("created".into()).unwrap(), conference_pending: true };
        worker.start_create(47, account.clone(), "primary".into(), std::future::ready(Ok(created)));
        let result = worker.event_creates.join_next_with_id().await.unwrap();
        worker.event_created(result);
        assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 47, error: None }));
        assert_eq!(worker.store.today_events("2027-05-09", 0, i64::MAX).unwrap().len(), 1);
        worker.pending_events[0].retry_at = tokio::time::Instant::now();
        let (base, server) = server("GET", status, body).await;
        worker.retry_events_with(|_, calendar, id| {
            let base = base.clone();
            async move { lagom_core::google::fetch_created_with(&base, &calendar, &id,
                |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await }
        });
        let result = worker.event_creates.join_next_with_id().await.unwrap();
        worker.event_created(result);
        assert!(worker.store.today_events("2027-05-09", 0, i64::MAX).unwrap().is_empty(), "confirmed deletion must remove the cached event");
        assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((1, 2)));
        assert!(worker.pending_events.is_empty());
        while let Ok(event) = events.try_recv() { assert!(!matches!(event, Event::EventCreated { .. })); }
        server.await.unwrap();
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn failed_cache_deletion_retries_locally_without_restoring_the_event() {
    let (mut worker, account, mut events, path) = fixture();
    let created = CreatedEvent { event: draft().event("created".into()).unwrap(), conference_pending: true };
    worker.start_create(48, account.clone(), "primary".into(), std::future::ready(Ok(created)));
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    while events.try_recv().is_ok() {}
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON events BEGIN SELECT RAISE(FAIL, 'fixture write failure'); END;").unwrap();
    worker.pending_events[0].retry_at = tokio::time::Instant::now();
    worker.retry_events_with(|_, _, _| std::future::ready(Err(Error::NotFound)));
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::CalendarSync(_, SyncState::Failed(_))));
    assert_eq!(worker.pending_events.len(), 1);
    assert_eq!(worker.store.today_events("2026-11-09", 0, i64::MAX).unwrap().len(), 1);
    db.execute_batch("DROP TRIGGER reject_delete; CREATE TRIGGER reject_insert BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL, 'deleted events must not be restored'); END;").unwrap();
    worker.retry_events_with::<_, std::future::Ready<_>>(|_, _, _| panic!("confirmed deletion needs only a cache retry"));
    assert!(worker.pending_events.is_empty());
    assert!(worker.store.today_events("2026-11-09", 0, i64::MAX).unwrap().is_empty());
    drop(db);
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

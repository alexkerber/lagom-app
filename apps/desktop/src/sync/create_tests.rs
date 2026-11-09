use super::*;
use lagom_core::account::Auth;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

fn fixture() -> (Worker, Account, UnboundedReceiver<Event>, PathBuf) {
    let path = std::env::temp_dir().join(format!("lagom-create-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let (me, _) = unbounded_channel();
    let (events, out) = unbounded_channel();
    let worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None,
        calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(),
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

async fn server(status: &'static str, body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") { headers.push(socket.read_u8().await.unwrap()); }
        let headers = String::from_utf8(headers).unwrap();
        assert!(headers.starts_with("POST /calendars/primary/events?"));
        let length = headers.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())).unwrap();
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
    let (base, server) = server("200 OK", r#"{"id":"created","summary":"Fixture","start":{"dateTime":"2026-11-09T09:00:00+01:00"},"end":{"dateTime":"2026-11-09T10:00:00+01:00"},"hangoutLink":"https://meet.example.test/new"}"#).await;
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
    worker.calendar_finished(stale);
    assert_eq!(worker.store.today_events("2026-11-09", start, start + 3600).unwrap(), events);
    assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((1, 2)));
    assert!(worker.create_tasks.is_empty());
    server.await.unwrap();
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn create_403_reports_op_and_marks_calendar_access_without_losing_mail_scopes() {
    let (mut worker, account, mut events, path) = fixture();
    let (base, server) = server("403 Forbidden", "{}").await;
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
async fn removed_account_and_failed_create_task_complete_with_errors() {
    let (mut worker, account, mut events, path) = fixture();
    worker.start_create(1, account.clone(), "primary".into(), async { panic!("fixture task failure") });
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 1, error: Some(_) }));
    while events.try_recv().is_ok() {}
    worker.start_create(2, account.clone(), "primary".into(), std::future::ready(draft().event("created".into())));
    worker.store.delete_account(&account.id).unwrap();
    let result = worker.event_creates.join_next_with_id().await.unwrap();
    worker.event_created(result);
    assert!(matches!(events.try_recv().unwrap(), Event::EventCreated { op: 2, error: Some(_) }));
    assert!(worker.store.today_events("2026-11-09", 0, i64::MAX).unwrap().is_empty());
    drop(worker);
    std::fs::remove_file(path).unwrap();
}

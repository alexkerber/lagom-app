use super::*;
use crate::store::Store;
use tokio::{net::TcpListener, io::{AsyncReadExt, AsyncWriteExt}};

const TRUNCATED_BODY: &str = "<truncated body>";

async fn server(replies: Vec<(&'static str, &str)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let replies: Vec<_> = replies.into_iter().map(|(status, body)| (status, body.to_owned())).collect();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") { request.push(stream.read_u8().await.unwrap()); }
            let headers = String::from_utf8(request.clone()).unwrap();
            let length = headers.lines().find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())).unwrap_or(0);
            let mut body_bytes = vec![0; length];
            stream.read_exact(&mut body_bytes).await.unwrap();
            request.extend(body_bytes);
            requests.push(String::from_utf8(request).unwrap());
            if status.is_empty() { continue; } // Drop the connection after reading the complete request.
            let length = body.len();
            let body = if body == TRUNCATED_BODY { "" } else { &body };
            stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}
const LIST: &str = include_str!("fixtures/calendar-list.json");
const EVENTS: &str = include_str!("fixtures/events.json");
const EMPTY: &str = r#"{"items":[]}"#;

#[tokio::test]
async fn fetch_pages_refresh_once_expand_recurrences_and_parse_join_links() {
    let (url, server) = server(vec![
        ("401 Unauthorized", "{}"), ("200 OK", LIST), ("200 OK", EMPTY),
        ("200 OK", EVENTS), ("200 OK", EMPTY),
    ]).await;
    let mut rejected_tokens = Vec::new();
    let result = fetch_with(&url, 1794000000, 1795000000, |rejected| {
        let token = if rejected.is_some() { "fresh" } else { "cached" };
        rejected_tokens.push(rejected);
        std::future::ready(Ok(Zeroizing::new(token.into())))
    }).await.unwrap();
    assert_eq!(rejected_tokens.iter().filter(|s| s.is_some()).count(), 1);
    assert_eq!(rejected_tokens[1].as_deref(), Some("cached"));
    assert_eq!(result.len(), 1);
    let (calendar, events) = &result[0];
    assert_eq!(calendar.remote_id, "fixture/calendar@example.test");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].start, "2026-11-09");
    assert!(events[0].all_day);
    assert_eq!(events[1].start_utc, mail_parser::DateTime::parse_rfc3339("2026-11-09T08:00:00Z").unwrap().to_timestamp());
    assert_eq!(events[1].join_url.as_deref(), Some("https://meet.example.test/fixture"));
    assert_eq!(events[1].html_link.as_deref(), Some("https://www.google.com/calendar/event?eid=fixture"));
    let requests = server.await.unwrap();
    assert!(requests[1].contains("Bearer fresh"));
    assert!(requests[2].contains("pageToken=calendar-page-2"));
    assert!(requests[3].contains("calendars/fixture%2Fcalendar@example.test/events?"));
    for key in ["singleEvents=true", "showDeleted=false", "timeZone=UTC", "timeMin=", "timeMax=", "maxResults=50", "fields=nextPageToken", "htmlLink"] { assert!(requests[3].contains(key), "{key}"); }
    assert!(requests[4].contains("pageToken=event-page-2"));
}

#[tokio::test]
async fn second_unauthorized_stops_and_scope_denial_differs_from_quota() {
    for (replies, kind, calls) in [
        (vec![("401 Unauthorized", "{}"), ("401 Unauthorized", "{}")], "reauth", 2),
        (vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#)], "scope", 1),
        (vec![("403 Forbidden", r#"{"error":{"details":[{"reason":"ACCESS_TOKEN_SCOPE_INSUFFICIENT"}]}}"#)], "scope", 1),
        (vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#)], "quota", 1),
    ] {
        let (url, server) = server(replies).await;
        let mut count = 0;
        let error = fetch_with(&url, 1, 2, |_| { count += 1; std::future::ready(Ok(Zeroizing::new("fixture".into()))) }).await.unwrap_err();
        assert!(match kind { "reauth" => matches!(error, Error::ReauthRequired), "scope" => matches!(error, Error::CalendarConsent), _ => matches!(error, Error::Network(_)) });
        assert_eq!(count, calls);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn failed_later_page_and_repeated_page_never_publish_partial_cache() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("cache.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let remote: Page<RemoteEvent> = serde_json::from_str(EVENTS).unwrap();
    let kept = event(remote.items.into_iter().next().unwrap()).unwrap();
    let calendar = Calendar { remote_id: "fixture".into(), name: "Fixture".into(), color: None, shown: true, writable: true };
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar, vec![kept.clone()])]).unwrap();
    for replies in [vec![("200 OK", LIST), ("500 Error", "{}")], vec![("200 OK", LIST), ("200 OK", LIST)],
        vec![("200 OK", LIST), ("200 OK", EMPTY), ("200 OK", EVENTS), ("500 Error", "{}")]] {
        let (url, server) = server(replies).await;
        let result = fetch_with(&url, 1, 2, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await;
        if let Ok(snapshot) = &result { store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, snapshot).unwrap(); }
        assert!(result.is_err());
        assert_eq!(store.today_events("2026-11-09", 0, 1).unwrap()[0].event, kept);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn calendar_missing_from_complete_list_hides_agenda_but_keeps_cached_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("cache.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let (url, server) = server(vec![
        ("200 OK", LIST), ("200 OK", EMPTY), ("200 OK", EVENTS), ("200 OK", EMPTY),
        ("200 OK", EMPTY),
    ]).await;
    let (start, end) = (1794000000, 1795000000);
    let snapshot = fetch_with(&url, start, end, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    store.replace_calendar_window(&account.id, start, end, &snapshot).unwrap();
    assert_eq!(store.today_events("2026-11-09", start, end).unwrap().len(), 2);
    let cached = || store.db.prepare("SELECT id,calendar,remote_id,title,start,end FROM events ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?,
            r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))).unwrap()
        .collect::<std::result::Result<Vec<_>, _>>().unwrap();
    let before = cached();
    let snapshot = fetch_with(&url, start, end, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(snapshot.is_empty());
    store.replace_calendar_window(&account.id, start, end, &snapshot).unwrap();
    assert!(store.today_events("2026-11-09", start, end).unwrap().is_empty());
    assert_eq!(cached(), before);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests[4].contains("users/me/calendarList?"));
}

#[tokio::test]
async fn calendar_sync_caches_the_window_only_with_access_and_a_refusal_drops_calendar_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("sync.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    let mail = "https://mail.google.com/ openid email profile";
    store.save_authorized_account(&account, mail).unwrap();
    let token = |_| std::future::ready(Ok(Zeroizing::new("fixture".into())));
    // Mail only: no request at all, so a closed port is enough.
    sync_calendar_with(&store, &account.id, 1794000000, "http://127.0.0.1:1/", token).await.unwrap();
    assert_eq!(store.calendar_window(&account.id).unwrap(), None);
    let granted = format!("{mail} {}", oauth::GOOGLE_CALENDAR_SCOPES);
    store.set_account_scopes(&account.id, &granted).unwrap();
    let (url, fetched) = server(vec![("200 OK", LIST), ("200 OK", EMPTY), ("200 OK", EVENTS), ("200 OK", EMPTY)]).await;
    let now = 1794000000;
    sync_calendar_with(&store, &account.id, now, &url, token).await.unwrap();
    assert_eq!(store.calendar_window(&account.id).unwrap(), Some(calendar_window(now)));
    assert_eq!(calendar_window(now), (now - 7 * 86400, now + 60 * 86400));
    assert_eq!(store.today_events("2026-11-09", 0, i64::MAX).unwrap().len(), 2);
    fetched.await.unwrap();
    let (url, refused) = server(vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#)]).await;
    assert!(matches!(sync_calendar_with(&store, &account.id, now, &url, token).await, Err(Error::CalendarConsent)));
    let scopes = store.account_scopes(&account.id).unwrap();
    assert!(!oauth::calendar_allowed(&scopes) && oauth::tasks_allowed(&scopes) && oauth::has_scope(&scopes, "https://mail.google.com/"), "{scopes}");
    assert_eq!(store.today_events("2026-11-09", 0, i64::MAX).unwrap().len(), 2, "a refusal keeps the cached events");
    refused.await.unwrap();
}

#[test]
fn invalid_dates_and_unsafe_meeting_urls_are_rejected() {
    for value in ["2026-02-30", "2026-13-01", "2026-11-09x", "2026-1-09"] {
        assert!(event_time(EventTime { date: Some(value.into()), date_time: None }).is_err());
    }
    for value in ["2026-11-09T10:00:00", "2026-11-09T25:00:00Z", "2026-11-09T10:00:00Zgarbage", "2026-11-09T10:00:00+99:00"] {
        assert!(event_time(EventTime { date: None, date_time: Some(value.into()) }).is_err());
    }
    assert!(event_time(EventTime { date: None, date_time: Some("2026-11-09T10:00:00.123-07:00".into()) }).is_ok());
    for url in ["javascript:alert(1)", "https://user:pass@example.test", "http://example.test"] { assert!(!safe_join_url(url)); assert!(join_host(url).is_none()); }
    assert!(safe_join_url("https://meet.example.test/fixture"));
    assert_eq!(join_host("https://meet.google.com/abc-defg-hij").as_deref(), Some("meet.google.com"));
    // A look-alike Unicode host shows in its ASCII form.
    assert!(join_host("https://g\u{43e}\u{43e}gle.com/x").is_some_and(|host| host.starts_with("xn--") && host.is_ascii()));
}

#[test]
fn calendar_links_allow_only_google_calendar_https_without_credentials() {
    for url in ["https://www.google.com/calendar/event?eid=fixture", "https://calendar.google.com/calendar/u/0/r/eventedit/fixture"] { assert!(safe_calendar_url(url)); }
    for url in ["http://www.google.com/calendar/event", "https://www.google.com.evil.test/calendar/", "https://user@calendar.google.com/calendar/", "javascript:alert(1)", "https://www.google.com/search"] { assert!(!safe_calendar_url(url)); }
}

fn draft() -> crate::calendar::NewEvent {
    crate::calendar::NewEvent {
        title: "Fixture planning".into(), start: "2026-03-29T01:30:00+01:00".into(), end: "2026-03-29T03:30:00+02:00".into(),
        time_zone: "Europe/Stockholm".into(), all_day: false, description: Some("Fixture notes".into()),
        attendees: vec!["guest@example.test".into()], meet: true,
    }
}
const CREATED: &str = r#"{"id":"created","summary":"Fixture planning","start":{"dateTime":"2026-03-29T01:30:00+01:00"},"end":{"dateTime":"2026-03-29T03:30:00+02:00"},"conferenceData":{"entryPoints":[{"entryPointType":"video","uri":"https://meet.example.test/new"}]}}"#;

fn request_body(request: &str) -> serde_json::Value { serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap() }

#[tokio::test]
async fn insert_meet_refreshes_once_preserves_dst_offsets_and_immediately_caches_link() {
    let (base, server) = server(vec![("401 Unauthorized", "{}"), ("200 OK", CREATED), ("200 OK", CREATED)]).await;
    let mut tokens = vec![];
    let event = create_with(&base, "fixture/calendar@example.test", &draft(), |rejected| {
        let token = if rejected.is_some() { "fresh" } else { "cached" };
        tokens.push(rejected);
        std::future::ready(Ok(Zeroizing::new(token.into())))
    }).await.unwrap();
    create_with(&base, "fixture/calendar@example.test", &draft(), |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert_eq!(tokens, [None, Some("cached".into())]);
    assert!(!event.conference_pending);
    let event = event.event;
    assert_eq!(event.end_utc - event.start_utc, 3600);
    assert_eq!(event.join_url.as_deref(), Some("https://meet.example.test/new"));
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("POST /calendars/fixture%2Fcalendar@example.test/events?"));
    assert!(requests[0].contains("conferenceDataVersion=1&sendUpdates=all"));
    assert!(requests[1].contains("Bearer fresh"));
    let body = request_body(&requests[0]);
    assert_eq!(body["start"], serde_json::json!({"dateTime": "2026-03-29T01:30:00+01:00", "timeZone": "Europe/Stockholm"}));
    assert_eq!(body["end"], serde_json::json!({"dateTime": "2026-03-29T03:30:00+02:00", "timeZone": "Europe/Stockholm"}));
    assert_eq!(body["summary"], "Fixture planning");
    assert_eq!(body["description"], "Fixture notes");
    assert_eq!(body["attendees"], serde_json::json!([{"email":"guest@example.test"}]));
    assert_eq!(body["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"], "hangoutsMeet");
    let request_id = body["conferenceData"]["createRequest"]["requestId"].as_str().unwrap();
    assert_eq!(uuid::Uuid::parse_str(request_id).unwrap().get_version_num(), 4);
    assert_eq!(body, request_body(&requests[1]), "401 retry must use the same conference request");
    assert_ne!(request_id, request_body(&requests[2])["conferenceData"]["createRequest"]["requestId"].as_str().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("cache.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    store.replace_calendar_window(&account.id, 1, 2, &[(Calendar {
        remote_id: "primary".into(), name: "Fixture".into(), color: None, shown: true, writable: true,
    }, vec![])]).unwrap();
    store.insert_calendar_event(&account.id, "primary", &event).unwrap();
    store.insert_calendar_event(&account.id, "primary", &event).unwrap();
    assert_eq!(store.today_events("2026-03-29", event.start_utc, event.end_utc).unwrap()[0].event, event);
    assert_eq!(store.today_events("2026-03-29", event.start_utc, event.end_utc).unwrap().len(), 1);
    assert_eq!(store.calendar_window(&account.id).unwrap(), Some((1, 2)), "creation must not claim fetch coverage");
    assert!(store.insert_calendar_event("another-account", "primary", &event).is_err());
}

#[tokio::test]
async fn insert_all_day_end_is_exclusive_and_no_guests_or_meet_are_omitted() {
    let response = r#"{"id":"day","start":{"date":"2026-03-29"},"end":{"date":"2026-03-30"}}"#;
    let (base, server) = server(vec![("200 OK", response)]).await;
    let mut draft = draft();
    draft.all_day = true;
    draft.start = "2026-03-29".into();
    draft.end = "2026-03-30".into();
    draft.meet = false;
    draft.attendees.clear();
    draft.description = None;
    let event = create_with(&base, "primary", &draft, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(!event.conference_pending);
    let event = event.event;
    assert!(event.all_day);
    assert_eq!(event.end, "2026-03-30");
    let requests = server.await.unwrap();
    assert!(requests[0].contains("conferenceDataVersion=1"));
    assert!(!requests[0].contains("sendUpdates"));
    let body = request_body(&requests[0]);
    assert_eq!(body["start"], serde_json::json!({"date":"2026-03-29"}));
    assert_eq!(body["end"], serde_json::json!({"date":"2026-03-30"}));
    for key in ["conferenceData", "attendees", "description"] { assert!(body.get(key).is_none(), "{key}"); }
}

#[tokio::test]
async fn insert_unauthorized_stops_after_one_refresh_and_scope_denial_needs_calendar_access() {
    for (replies, consent) in [
        (vec![("401 Unauthorized", "{}"), ("401 Unauthorized", "{}")], false),
        (vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#)], true),
        (vec![("403 Forbidden", r#"{"error":{"details":[{"reason":"ACCESS_TOKEN_SCOPE_INSUFFICIENT"}]}}"#)], true),
    ] {
        let (base, server) = server(replies).await;
        let mut tokens = vec![];
        let error = create_with(&base, "primary", &draft(), |rejected| {
            tokens.push(rejected);
            std::future::ready(Ok(Zeroizing::new("fixture".into())))
        }).await.unwrap_err();
        assert!(if consent { matches!(error, Error::CalendarConsent) } else { matches!(error, Error::ReauthRequired) });
        assert_eq!(tokens.len(), if consent { 1 } else { 2 });
        assert_eq!(server.await.unwrap().len(), tokens.len());
    }
}

#[tokio::test]
async fn successful_insert_missing_start_is_uncertain() {
    let (base, server) = server(vec![("200 OK", r#"{"id":"created","end":{"date":"2026-03-30"}}"#)]).await;
    let result = create_with(&base, "primary", &draft(), change_token).await;
    assert!(matches!(result, Err(Error::Uncertain)));
    assert!(server.await.unwrap()[0].starts_with("POST "));
}

#[tokio::test]
async fn invalid_insert_does_not_get_a_token_or_make_a_request() {
    for change in 0..7 {
        let mut draft = draft();
        match change {
            0 => draft.title = "  ".into(),
            1 => draft.end = draft.start.clone(),
            2 => draft.start = "2026-02-30T01:30:00+01:00".into(),
            3 => draft.attendees = vec!["bad address".into()],
            4 => draft.start = "2026-03-29T01:30:00".into(),
            5 => draft.time_zone.clear(),
            _ => draft.attendees = vec!["guest@example.test\r\nother@example.test".into()],
        }
        assert!(create_with("http://127.0.0.1:1/", "primary", &draft, |_| {
            panic!("invalid draft requested credentials");
            #[allow(unreachable_code)] std::future::ready(Ok(Zeroizing::new(String::new())))
        }).await.is_err());
    }
}

#[tokio::test]
async fn pending_meet_is_followed_by_id_outside_the_fetch_window() {
    const PENDING: &str = r#"{"id":"future/id","start":{"dateTime":"2027-05-09T09:00:00Z"},"end":{"dateTime":"2027-05-09T10:00:00Z"},"conferenceData":{"createRequest":{"status":{"statusCode":"pending"}}}}"#;
    const READY: &str = r#"{"id":"future/id","start":{"dateTime":"2027-05-09T09:00:00Z"},"end":{"dateTime":"2027-05-09T10:00:00Z"},"conferenceData":{"createRequest":{"status":{"statusCode":"success"}},"entryPoints":[{"entryPointType":"video","uri":"https://meet.example.test/future"}]}}"#;
    let (base, server) = server(vec![("200 OK", PENDING), ("200 OK", PENDING), ("200 OK", READY)]).await;
    let mut draft = draft();
    draft.start = "2027-05-09T09:00:00Z".into();
    draft.end = "2027-05-09T10:00:00Z".into();
    let event = create_with(&base, "fixture/calendar", &draft, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(event.conference_pending);
    assert!(event.event.join_url.is_none());
    let pending = fetch_created_with(&base, "fixture/calendar", &event.event.remote_id,
        |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(pending.conference_pending);
    let ready = fetch_created_with(&base, "fixture/calendar", &pending.event.remote_id,
        |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(!ready.conference_pending);
    assert_eq!(ready.event.join_url.as_deref(), Some("https://meet.example.test/future"));
    let requests = server.await.unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST ")).count(), 1);
    for request in &requests[1..] {
        assert!(request.starts_with("GET /calendars/fixture%2Fcalendar/events/future%2Fid "));
        assert!(!request.contains("timeMin") && !request.contains("timeMax"));
    }
}

#[tokio::test]
async fn conference_followup_refreshes_once_and_terminal_failure_stops_polling() {
    const FAILED: &str = r#"{"id":"created","start":{"date":"2027-05-09"},"end":{"date":"2027-05-10"},"conferenceData":{"createRequest":{"status":{"statusCode":"failure"}}}}"#;
    let (base, server) = server(vec![("401 Unauthorized", "{}"), ("200 OK", CREATED), ("200 OK", FAILED), ("503 Unavailable", "{}")]).await;
    let mut tokens = Vec::new();
    let ready = fetch_created_with(&base, "primary", "created", |rejected| {
        let access = if rejected.is_some() { "fresh" } else { "cached" };
        tokens.push(rejected);
        std::future::ready(Ok(Zeroizing::new(access.into())))
    }).await.unwrap();
    assert_eq!(tokens, [None, Some("cached".into())]);
    assert_eq!(ready.event.join_url.as_deref(), Some("https://meet.example.test/new"));
    assert!(!ready.conference_pending);
    let failed = fetch_created_with(&base, "primary", "created", |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.unwrap();
    assert!(!failed.conference_pending);
    assert!(failed.event.join_url.is_none());
    assert!(fetch_created_with(&base, "primary", "created", |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.is_err());
    let requests = server.await.unwrap();
    assert!(requests.iter().all(|r| r.starts_with("GET /calendars/primary/events/created ")));
    assert!(requests[1].contains("Bearer fresh"));
}

#[tokio::test]
async fn conference_followup_recognizes_deleted_events_and_rejects_other_ids() {
    let (base, server) = server(vec![("404 Not Found", "{}"), ("410 Gone", "{}"),
        ("200 OK", r#"{"id":"created","status":"cancelled"}"#), ("200 OK", CREATED)]).await;
    for _ in 0..3 {
        assert!(matches!(fetch_created_with(&base, "primary", "created",
            |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await, Err(Error::NotFound)));
    }
    assert!(fetch_created_with(&base, "primary", "other",
        |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await.is_err());
    server.await.unwrap();
}

const BEFORE: &str = r#"{"id":"created","etag":"\"old\"","recurringEventId":"series","summary":"Before","description":"Notes","attendees":[{"email":"old@example.test"}],"start":{"date":"2026-03-29"},"end":{"date":"2026-03-30"}}"#;
const OLD: &str = r#""old""#;

fn change_token(rejected: Option<String>) -> std::future::Ready<Result<Zeroizing<String>>> {
    std::future::ready(Ok(Zeroizing::new(if rejected.is_some() { "fresh".into() } else { "cached".into() })))
}

fn cached_calendar(calendar: &str) -> (tempfile::TempDir, Store, Account) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("cache.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    store.replace_calendar_window(&account.id, 1, 2, &[(Calendar {
        remote_id: calendar.into(), name: "Fixture".into(), color: None, shown: true, writable: true,
    }, vec![draft().event("created".into()).unwrap()])]).unwrap();
    (dir, store, account)
}

#[tokio::test]
async fn conditional_event_changes_cache_only_confirmed_writes() {
    let target = r#"{"id":"created","etag":"\"old\"","attendeesOmitted":true}"#;
    let new = target.replace("old", "new");
    let (base, server) = server(vec![("200 OK", BEFORE),
        ("200 OK", target), ("412 Precondition Failed", ""),
        ("200 OK", target), ("", ""),
        ("200 OK", target), ("401 Unauthorized", ""), ("200 OK", CREATED),
        ("200 OK", target), ("412 Precondition Failed", ""),
        ("200 OK", &new), ("", ""),
        ("200 OK", &new), ("204 No Content", "")]).await;
    let calendar = "fixture/calendar";
    let id = "created";
    let snapshot = change::fetch_event_with(&base, calendar, id, change_token).await.unwrap();
    assert_eq!(snapshot.attendees, ["old@example.test"]);
    assert_eq!(snapshot.description.as_deref(), Some("Notes"));
    assert_eq!(snapshot.etag, r#""old""#);

    let (_dir, store, account) = cached_calendar(calendar);
    store.insert_calendar_event(&account.id, calendar, &snapshot.event).unwrap();
    let change = |etag, draft| change::change_with(&store, &account.id, &base, calendar, id, etag, draft, change_token);
    let cached = || store.today_events("2026-03-29", 0, i64::MAX).unwrap()
        .into_iter().map(|row| row.event).collect::<Vec<_>>();
    let draft = draft();
    assert!(matches!(change("*", None).await, Err(Error::Invalid(_))));
    assert!(matches!(change(&snapshot.etag, Some(&draft)).await, Err(Error::Conflict)));
    assert!(matches!(change(&snapshot.etag, Some(&draft)).await, Err(Error::Uncertain)));
    assert_eq!(cached(), [snapshot.event]);
    change(&snapshot.etag, Some(&draft)).await.unwrap();
    let updated = cached();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].join_url.as_deref(), Some("https://meet.example.test/new"));
    assert!(matches!(change(&snapshot.etag, None).await, Err(Error::Conflict)));
    let new_tag = r#""new""#;
    assert!(matches!(change(new_tag, None).await, Err(Error::Uncertain)));
    assert_eq!(cached(), updated);
    change(new_tag, None).await.unwrap();
    assert!(cached().is_empty());
    assert_eq!(store.calendar_window(&account.id).unwrap(), Some((1, 2)));

    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("GET /calendars/fixture%2Fcalendar/events/created "));
    for check in requests[1..].iter().filter(|r| r.starts_with("GET ")) {
        assert!(check.starts_with("GET /calendars/fixture%2Fcalendar/events/created?fields=id%2Cstatus%2Crecurrence%2CrecurringEventId%2Cetag "));
    }
    let writes: Vec<_> = requests.iter().filter(|request| !request.starts_with("GET ")).collect();
    assert_eq!(writes.len(), 7);
    for (index, request) in writes.iter().enumerate() {
        let method = if index < 4 { "PATCH" } else { "DELETE" };
        assert!(request.starts_with(&format!("{method} /calendars/fixture%2Fcalendar/events/created?sendUpdates=all")));
        let tag = if index < 5 { &snapshot.etag } else { new_tag };
        assert!(request.to_ascii_lowercase().lines().any(|line| line == format!("if-match: {tag}")));
    }
    assert_eq!(request_body(writes[2]), request_body(writes[3]));
    let body = request_body(writes[3]);
    assert_eq!(body["attendees"][0]["email"], "guest@example.test");
    for key in ["start", "end"] {
        assert_eq!(body[key].get("date"), Some(&serde_json::Value::Null));
        assert!(body[key]["dateTime"].is_string() && body[key]["timeZone"].is_string());
    }
    assert!(writes[3].contains("Bearer fresh"));
}

#[tokio::test]
async fn invalid_event_paths_stop_before_requesting_credentials() {
    let base = "http://127.0.0.1:1/";
    let no_token = |_| -> std::future::Ready<Result<Zeroizing<String>>> { panic!("requested credentials") };
    let (_dir, store, account) = cached_calendar("primary");
    for invalid in [".\t.", "a\rb", "a\nb", "a\u{85}b", "", ".", ".."] {
        for (calendar, id) in [("primary", invalid), (invalid, "created")] {
            let result = change::fetch_event_with(base, calendar, id, no_token).await;
            assert!(matches!(result, Err(Error::Invalid(_))));
        }
        assert!(matches!(create_with(base, invalid, &draft(), no_token).await, Err(Error::Invalid(_))));
        for form in [None, Some(&draft())] {
            let result = change::change_with(&store, &account.id, base, "primary", invalid, OLD, form, no_token).await;
            assert!(matches!(result, Err(Error::Invalid(_))));
        }
    }
    let url = event_url(base, "a/b", "a/b%2F#?").unwrap();
    assert_eq!(url.path(), "/calendars/a%2Fb/events/a%2Fb%252F%23%3F");
}

#[tokio::test]
async fn event_read_rejects_incomplete_or_unusable_snapshots() {
    let original: serde_json::Value = serde_json::from_str(BEFORE).unwrap();
    for (field, value) in [
        ("status", serde_json::json!("cancelled")), ("attendeesOmitted", true.into()),
        ("etag", serde_json::Value::Null), ("etag", r#"W/"old""#.into()), ("id", "other".into()),
    ] {
        let mut reply = original.clone();
        if value.is_null() { reply.as_object_mut().unwrap().remove(field); } else { reply[field] = value; }
        let (base, server) = server(vec![("200 OK", &reply.to_string())]).await;
        let error = change::fetch_event_with(&base, "primary", "created", change_token).await.unwrap_err();
        if field == "status" { assert!(matches!(error, Error::NotFound)); }
        else { assert!(matches!(error, Error::Network(_)), "{field}"); }
        assert_eq!(server.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn changes_refuse_series_masters_and_unknown_calendars() {
    let (_dir, store, account) = cached_calendar("primary");
    let master = BEFORE.replace(r#""recurringEventId":"series""#, r#""recurrence":["RRULE:FREQ=DAILY"]"#);
    let (base, server) = server(vec![("200 OK", &master); 2]).await;
    for draft in [None, Some(&draft())] {
        let result = change::change_with(&store, &account.id, &base, "unknown", "created", OLD, draft,
            |_| -> std::future::Ready<Result<Zeroizing<String>>> { panic!("requested credentials") }).await;
        assert!(matches!(result, Err(Error::NotFound)));
        let result = change::change_with(&store, &account.id, &base, "primary", "created", OLD, draft, change_token).await;
        assert!(matches!(result, Err(Error::RecurringSeries)));
    }
    assert!(server.await.unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn pre_write_get_with_stale_etag_stops_before_writing() {
    let (_dir, store, account) = cached_calendar("primary");
    for form in [None, Some(&draft())] {
        let (base, server) = server(vec![("200 OK", BEFORE)]).await;
        let result = change::change_with(&store, &account.id, &base, "primary", "created", r#""stale""#, form, change_token).await;
        assert!(matches!(result, Err(Error::Conflict)));
        assert!(server.await.unwrap()[0].starts_with("GET "));
    }
}

#[tokio::test]
async fn missing_pre_write_target_evicts_cache_even_if_body_is_truncated() {
    for status in ["404 Not Found", "410 Gone"] {
        let (_dir, store, account) = cached_calendar("primary");
        let (base, server) = server(vec![(status, TRUNCATED_BODY)]).await;
        let result = change::change_with(&store, &account.id, &base, "primary", "created", OLD, None, change_token).await;
        assert!(matches!(result, Err(Error::NotFound)));
        assert!(store.today_events("2026-03-29", 0, i64::MAX).unwrap().is_empty());
        assert!(server.await.unwrap()[0].starts_with("GET "));
    }
}

#[tokio::test]
async fn gone_writes_remove_the_cached_event() {
    let (_dir, store, account) = cached_calendar("primary");
    for status in ["404 Not Found", "410 Gone"] {
        for form in [None, Some(&draft())] {
            store.insert_calendar_event(&account.id, "primary", &draft().event("created".into()).unwrap()).unwrap();
            let (base, server) = server(vec![("200 OK", BEFORE), (status, "")]).await;
            let result = change::change_with(&store, &account.id, &base, "primary", "created", OLD, form, change_token).await;
            assert!(matches!(result, Err(Error::NotFound)));
            assert!(store.today_events("2026-03-29", 0, i64::MAX).unwrap().is_empty());
            assert_eq!(server.await.unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn calendar_writes_classify_server_and_authorization_failures() {
    let (_dir, store, account) = cached_calendar("primary");
    let (base, server_task) = server(vec![("503 Unavailable", "")]).await;
    assert!(matches!(create_with(&base, "primary", &draft(), change_token).await, Err(Error::Uncertain)));
    assert_eq!(server_task.await.unwrap().len(), 1);
    for form in [None, Some(&draft())] {
        for status in ["503 Unavailable", "403 Forbidden", "401 Unauthorized"] {
            let mut replies = vec![("200 OK", BEFORE), (status, "")];
            if status.starts_with("401") { replies.push((status, "")); }
            let (base, server) = server(replies).await;
            let error = change::change_with(&store, &account.id, &base, "primary", "created", OLD, form, change_token).await.unwrap_err();
            match status {
                "503 Unavailable" => assert!(matches!(error, Error::Uncertain)),
                "403 Forbidden" => assert!(matches!(error, Error::Network("calendar access refused"))),
                _ => assert!(matches!(error, Error::ReauthRequired)),
            }
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), if status.starts_with("401") { 3 } else { 2 });
        }
    }
}

#[tokio::test]
async fn confirmed_patch_with_unusable_body_refreshes_or_invalidates_only_its_target() {
    let (_dir, store, account) = cached_calendar("primary");
    store.insert_calendar_event(&account.id, "primary", &draft().event("other".into()).unwrap()).unwrap();
    let all_day = crate::calendar::NewEvent { all_day: true, start: "2026-03-29".into(), end: "2026-03-30".into(), ..draft() };
    let wrong_id = CREATED.replace("created", "other");
    for (body, refresh) in [("{", ("200 OK", CREATED)), (wrong_id.as_str(), ("503 Unavailable", ""))] {
        store.insert_calendar_event(&account.id, "primary", &draft().event("created".into()).unwrap()).unwrap();
        let (base, server) = server(vec![("200 OK", BEFORE), ("200 OK", body), refresh]).await;
        change::change_with(&store, &account.id, &base, "primary", "created", OLD, Some(&all_day), change_token).await.unwrap();
        let events = store.today_events("2026-03-29", 0, i64::MAX).unwrap();
        let other = draft().event("other".into()).unwrap();
        assert!(events.iter().any(|row| row.event == other));
        if refresh.0 == "200 OK" {
            assert_eq!(events.len(), 2);
            assert!(events.iter().any(|row| row.event.remote_id == "created"
                && row.event.join_url.as_deref() == Some("https://meet.example.test/new")));
        } else { assert_eq!(events.len(), 1); }
        let patch = request_body(&server.await.unwrap()[1]);
        for (key, date) in [("start", &all_day.start), ("end", &all_day.end)] {
            assert_eq!(patch[key], serde_json::json!({"date":date, "dateTime":null, "timeZone":null}));
        }
    }
}

#[tokio::test]
async fn cache_errors_do_not_fail_confirmed_writes() {
    let (_dir, store, account) = cached_calendar("primary");
    store.db.execute_batch("PRAGMA query_only=ON").unwrap();
    for (form, response) in [(Some(&draft()), ("200 OK", CREATED)), (None, ("204 No Content", ""))] {
        let (base, server) = server(vec![("200 OK", BEFORE), response]).await;
        change::change_with(&store, &account.id, &base, "primary", "created", OLD, form, change_token).await.unwrap();
        assert_eq!(server.await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn stalled_tls_connection_is_a_network_failure_before_the_write() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}/", listener.local_addr().unwrap());
    let create = tokio::spawn(async move { create_with(&url, "primary", &draft(), change_token).await });
    let (mut socket, _) = listener.accept().await.unwrap();
    socket.read_u8().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    assert!(matches!(create.await.unwrap(), Err(Error::Network("calendar unavailable"))));
}

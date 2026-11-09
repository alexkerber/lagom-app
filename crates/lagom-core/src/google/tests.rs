use super::*;
use crate::store::Store;
use tokio::{net::TcpListener, io::{AsyncReadExt, AsyncWriteExt}};

async fn server(replies: Vec<(&'static str, &'static str)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") { request.push(stream.read_u8().await.unwrap()); }
            requests.push(String::from_utf8(request).unwrap());
            stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
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

#[test]
fn invalid_dates_and_unsafe_meeting_urls_are_rejected() {
    for value in ["2026-02-30", "2026-13-01", "2026-11-09x", "2026-1-09"] {
        assert!(event_time(EventTime { date: Some(value.into()), date_time: None }).is_err());
    }
    for value in ["2026-11-09T10:00:00", "2026-11-09T25:00:00Z", "2026-11-09T10:00:00Zgarbage", "2026-11-09T10:00:00+99:00"] {
        assert!(event_time(EventTime { date: None, date_time: Some(value.into()) }).is_err());
    }
    assert!(event_time(EventTime { date: None, date_time: Some("2026-11-09T10:00:00.123-07:00".into()) }).is_ok());
    for url in ["javascript:alert(1)", "https://user:pass@example.test", "http://example.test"] { assert!(!safe_join_url(url)); }
    assert!(safe_join_url("https://meet.example.test/fixture"));
}

#[test]
fn calendar_links_allow_only_google_calendar_https_without_credentials() {
    for url in ["https://www.google.com/calendar/event?eid=fixture", "https://calendar.google.com/calendar/u/0/r/eventedit/fixture"] { assert!(safe_calendar_url(url)); }
    for url in ["http://www.google.com/calendar/event", "https://www.google.com.evil.test/calendar/", "https://user@calendar.google.com/calendar/", "javascript:alert(1)", "https://www.google.com/search"] { assert!(!safe_calendar_url(url)); }
}

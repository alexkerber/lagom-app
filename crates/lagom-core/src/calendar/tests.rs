use super::*;

fn calendar() -> Calendar {
    Calendar { remote_id: "primary".into(), name: "Fixture calendar".into(), color: None, shown: true, writable: true }
}
fn event(id: &str, start: &str, end: &str, all_day: bool) -> Event {
    let stamp = |s: &str| mail_parser::DateTime::parse_rfc3339(&if all_day { format!("{s}T00:00:00Z") } else { s.into() }).unwrap().to_timestamp();
    Event { remote_id: id.into(), title: id.into(), start: start.into(), end: end.into(), start_utc: stamp(start), end_utc: stamp(end), all_day, location: None, join_url: None, html_link: None }
}
fn setup() -> (tempfile::TempDir, Store, Account) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("calendar.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    (dir, store, account)
}

#[test]
fn calendar_migrates_v9_without_changing_mail_or_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v9.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    for schema in [include_str!("../schema_v1.sql"), include_str!("../schema_v2.sql"), include_str!("../schema_v3.sql"),
        include_str!("../schema_v4.sql"), include_str!("../schema_v5.sql"), include_str!("../schema_v6.sql"),
        include_str!("../schema_v7.sql"), include_str!("../schema_v8.sql"), include_str!("../schema_v9.sql")] {
        db.execute_batch(schema).unwrap();
    }
    let store = Store { db };
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let list = store.default_task_list().unwrap();
    let task = store.add_task(list, "Keep local task", "", None).unwrap();
    drop(store);
    let store = Store::open(path).unwrap();
    assert_eq!(store.accounts().unwrap(), [account.clone()]);
    assert_eq!(store.tasks(list, false).unwrap(), [task]);
    assert_eq!(store.account_scopes(&account.id).unwrap(), "");
    store.set_account_scopes(&account.id, crate::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    store.save_account(&account).unwrap();
    assert!(crate::oauth::calendar_allowed(&store.account_scopes(&account.id).unwrap()));
    assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 11);
}

#[test]
fn today_overlaps_local_day_and_keeps_all_day_dates_across_offsets() {
    let (_dir, store, account) = setup();
    let events = vec![
        event("all-day", "2026-11-09", "2026-11-10", true),
        event("multi-day", "2026-11-08", "2026-11-11", true),
        event("ended", "2026-11-08", "2026-11-09", true),
        event("overnight", "2026-11-08T20:00:00Z", "2026-11-10T01:00:00Z", false),
        event("meeting", "2026-11-09T12:00:00Z", "2026-11-09T13:00:00Z", false),
    ];
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), events)]).unwrap();
    let midnight = mail_parser::DateTime::parse_rfc3339("2026-11-09T00:00:00Z").unwrap().to_timestamp();
    for offset in [-11 * 3600, 0, 14 * 3600] {
        let rows = store.today_events("2026-11-09", midnight - offset, midnight - offset + 86400).unwrap();
        let ids: Vec<_> = rows.iter().map(|e| e.event.remote_id.as_str()).collect();
        assert!(ids.contains(&"all-day") && ids.contains(&"multi-day") && ids.contains(&"overnight"));
        assert!(!ids.contains(&"ended"));
    }
}

#[test]
fn window_replacement_rolls_back_and_account_removal_keeps_local_tasks() {
    let (_dir, store, account) = setup();
    let task = store.add_task(store.default_task_list().unwrap(), "Local", "", None).unwrap();
    let event = event("kept", "2026-11-09", "2026-11-10", true);
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![event.clone()])]).unwrap();
    let mut invalid = event.clone();
    invalid.end_utc = invalid.start_utc;
    assert!(store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![invalid])]).is_err());
    assert_eq!(store.today_events("2026-11-09", 0, 1).unwrap()[0].event, event);
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![])]).unwrap();
    assert!(store.today_events("2026-11-09", 0, 1).unwrap().is_empty());
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![event])]).unwrap();
    store.delete_account(&account.id).unwrap();
    assert!(store.today_events("2026-11-09", 0, 1).unwrap().is_empty());
    assert_eq!(store.tasks(task.list, false).unwrap(), [task]);
}

#[test]
fn replacement_preserves_other_windows_calendars_and_accounts() {
    let (_dir, store, account) = setup();
    let old = event("old", "2026-11-01", "2026-11-02", true);
    let current = event("current", "2026-11-09", "2026-11-10", true);
    let future = event("future", "2026-12-01", "2026-12-02", true);
    let mut other = calendar();
    other.remote_id = "other".into();
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![old.clone(), current.clone(), future.clone()]), (other, vec![current.clone()])]).unwrap();
    let calendar_id: i64 = store.db.query_row("SELECT id FROM calendars WHERE account=?1 AND remote_id='primary'", [&account.id], |r| r.get(0)).unwrap();
    let mut other_account = account.clone();
    other_account.id = "other-account".into();
    store.save_account(&other_account).unwrap();
    store.replace_calendar_window(&other_account.id, i64::MIN, i64::MAX, &[(calendar(), vec![current.clone()])]).unwrap();
    store.replace_calendar_window(&account.id, current.start_utc, current.end_utc, &[(calendar(), vec![])]).unwrap();
    assert_eq!(store.today_events("2026-11-01", 0, 1).unwrap().len(), 1);
    assert_eq!(store.today_events("2026-12-01", 0, 1).unwrap().len(), 1);
    let today = store.today_events("2026-11-09", 0, 1).unwrap();
    assert_eq!(today.len(), 1);
    assert_eq!(today[0].account, other_account.id);
    assert_eq!(store.db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(), 4);
    assert_eq!(store.db.query_row("SELECT id FROM calendars WHERE account=?1 AND remote_id='primary'", [&account.id], |r| r.get::<_, i64>(0)).unwrap(), calendar_id);
}

#[test]
fn replacement_uses_overlap_bounds_and_upserts_moved_events() {
    let (_dir, store, account) = setup();
    let before = event("before", "2026-11-08T23:00:00Z", "2026-11-09T00:00:00Z", false);
    let after = event("after", "2026-11-10T00:00:00Z", "2026-11-10T01:00:00Z", false);
    let overlap = event("overlap", "2026-11-08T23:00:00Z", "2026-11-10T01:00:00Z", false);
    let mut moved = event("moved", "2026-11-01T12:00:00Z", "2026-11-01T13:00:00Z", false);
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![before.clone(), after.clone(), overlap, moved.clone()])]).unwrap();
    moved.start = "2026-11-09T12:00:00Z".into();
    moved.end = "2026-11-09T13:00:00Z".into();
    moved.start_utc = before.end_utc + 12 * 3600;
    moved.end_utc = moved.start_utc + 3600;
    let mut renamed = calendar();
    renamed.name = "Renamed".into();
    for _ in 0..2 {
        store.replace_calendar_window(&account.id, before.end_utc, after.start_utc, &[(renamed.clone(), vec![moved.clone()])]).unwrap();
        let rows = store.today_events("2026-11-09", before.end_utc, after.start_utc).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event, moved);
        assert_eq!(rows[0].calendar, "Renamed");
        assert_eq!(store.db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
    }
    assert!(store.replace_calendar_window(&account.id, 1, 1, &[(calendar(), vec![])]).is_err());
    assert_eq!(store.db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
}

#[test]
fn v10_upgrade_keeps_cached_events_and_roundtrips_optional_html_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    for schema in [include_str!("../schema_v1.sql"), include_str!("../schema_v2.sql"), include_str!("../schema_v3.sql"), include_str!("../schema_v4.sql"), include_str!("../schema_v5.sql"), include_str!("../schema_v6.sql"), include_str!("../schema_v7.sql"), include_str!("../schema_v8.sql"), include_str!("../schema_v9.sql"), include_str!("../schema_v10.sql")] { db.execute_batch(schema).unwrap(); }
    db.execute_batch("INSERT INTO accounts(id,display_name,email,imap_host,imap_port,imap_security,imap_username,smtp_host,smtp_port,smtp_security,smtp_username)
        VALUES('fixture','Fixture','fixture@example.test','localhost',993,'tls','fixture','localhost',465,'tls','fixture');
        INSERT INTO calendars(id,account,remote_id,name,writable) VALUES(1,'fixture','primary','Fixture',1);
        INSERT INTO events(calendar,remote_id,title,start,end,start_utc,end_utc,all_day) VALUES(1,'old','Kept','2026-11-09','2026-11-10',1,2,1);").unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let cached = store.today_events("2026-11-09", 0, 1).unwrap();
    assert_eq!(cached[0].event.title, "Kept");
    assert!(cached[0].event.html_link.is_none());
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let mut linked = event("linked", "2026-11-09", "2026-11-10", true);
    linked.html_link = Some("https://www.google.com/calendar/event?eid=fixture".into());
    store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(calendar(), vec![linked.clone()])]).unwrap();
    drop(store);
    let store = Store::open(path).unwrap();
    assert!(store.today_events("2026-11-09", 0, 1).unwrap().iter().any(|e| e.event == linked));
}

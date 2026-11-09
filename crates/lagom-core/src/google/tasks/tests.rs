use super::*;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

const LIST: &str = r#"{"items":[{"id":"work","title":"Work","etag":"l1","updated":"2026-11-09T08:00:00Z"}]}"#;
const SAVED_LIST: &str = r#"{"id":"work","title":"Work","etag":"l2","updated":"2026-11-09T09:00:00Z"}"#;
const TASK: &str = r#"{"id":"task","title":"Local title","etag":"t2","updated":"2026-11-09T09:00:00Z"}"#;
const EMPTY: &str = r#"{"items":[]}"#;

thread_local! {
    static RECOVERY_OWNER_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn after_recovery_owner_check() {
    if let Some(hook) = RECOVERY_OWNER_CHECK.with(|hook| hook.borrow_mut().take()) { hook(); }
}

async fn server(replies: Vec<(&str, &str)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    server_default(replies, SAVED_LIST).await
}

async fn server_default(replies: Vec<(&str, &str)>, default: &str) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let default = default.to_owned();
    let replies: Vec<_> = replies.into_iter().map(|(s, b)| (s.to_owned(), b.to_owned())).collect();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut replies = replies.into_iter().peekable();
        while replies.peek().is_some() {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap().unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await.unwrap()); }
            let head = String::from_utf8(head).unwrap();
            let length: usize = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(|s| s.parse().unwrap())).unwrap_or(0);
            let mut data = vec![0; length];
            socket.read_exact(&mut data).await.unwrap();
            if head.starts_with("GET /users/@me/lists/@default ") {
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{default}", default.len()).as_bytes()).await.unwrap();
                continue;
            }
            let (status, body) = replies.next().unwrap();
            requests.push(format!("{head}{}", String::from_utf8(data).unwrap()));
            socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (url, handle)
}

fn setup(linked: bool) -> (tempfile::TempDir, Store, Account, i64) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("fixture.db")).unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_authorized_account(&account, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    let list = store.default_task_list().unwrap();
    store.db.execute("DELETE FROM task_lists WHERE id<>?1", [list]).unwrap();
    if linked { store.db.execute("UPDATE task_lists SET account=?1,remote_id='work',dirty=0", [&account.id]).unwrap(); }
    (dir, store, account, list)
}
fn token(_: Option<String>) -> std::future::Ready<Result<Zeroizing<String>>> { std::future::ready(Ok(Zeroizing::new("fixture".into()))) }
fn linked_task(store: &Store, list: i64) -> Task {
    let task = store.add_task(list, "Local title", "Local notes", None).unwrap();
    store.db.execute("UPDATE tasks SET remote_id='task',remote_list_id='work',dirty=0 WHERE id=?1", [task.id]).unwrap();
    store.tasks(list, true).unwrap().remove(0)
}

#[tokio::test]
async fn paging_and_updated_min_include_completed_hidden_and_deleted() {
    let (_dir, mut store, account, list) = setup(true);
    let (url, handle) = server(vec![("200 OK", include_str!("../fixtures/task-lists.json")), ("200 OK", EMPTY),
        ("200 OK", include_str!("../fixtures/tasks.json")), ("200 OK", EMPTY), ("200 OK", LIST), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(store.tasks(list, true).unwrap()[0].title, "Remote title");
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let requests = handle.await.unwrap();
    assert!(requests[1].contains("pageToken=lists-2"));
    assert!(requests[3].contains("pageToken=tasks-2"));
    for flag in ["showCompleted=true", "showHidden=true", "showDeleted=true", "maxResults=100"] { assert!(requests[2].contains(flag)); }
    assert!(requests[5].contains("updatedMin=2026-11-09T09%3A00%3A00"));
}

#[tokio::test]
async fn pushes_before_pull_and_local_edit_wins_even_over_stale_pull() {
    let (_dir, mut store, account, list) = setup(true);
    let task = linked_task(&store, list);
    store.rename_task(task.id, "Local wins").unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", TASK), ("200 OK", include_str!("../fixtures/tasks.json")), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let saved = &store.tasks(list, true).unwrap()[0];
    assert_eq!(saved.title, "Local wins");
    assert!(!saved.dirty);
    let requests = handle.await.unwrap();
    assert!(requests[1].starts_with("PATCH /lists/work/tasks/task "));
    assert!(requests[1].contains("Local wins"));
    assert!(requests[2].starts_with("GET /lists/work/tasks?"));
}

#[tokio::test]
async fn acknowledged_push_survives_failed_pull_and_does_not_insert_again() {
    let (_dir, mut store, account, list) = setup(true);
    let task = store.add_task(list, "Local title", "", None).unwrap();
    store.db.execute("UPDATE tasks SET dirty=0 WHERE id=?1", [task.id]).unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", TASK), ("503 Offline", "{}"), ("200 OK", LIST), ("200 OK", EMPTY)]).await;
    assert!(sync_tasks_with(&mut store, &account.id, &url, token).await.is_err());
    let saved = &store.tasks(list, true).unwrap()[0];
    assert_eq!(saved.id, task.id);
    assert_eq!(saved.remote_id.as_deref(), Some("task"));
    assert!(!saved.dirty);
    assert!(store.google_lists(&account.id).unwrap()[0].cursor.is_none());
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(handle.await.unwrap().iter().filter(|r| r.starts_with("POST")).count(), 1);
}

#[tokio::test]
async fn failed_push_retains_dirty_and_never_pulls() {
    let (_dir, mut store, account, list) = setup(true);
    store.add_task(list, "Retry", "", None).unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("503 Offline", "{}")]).await;
    assert!(sync_tasks_with(&mut store, &account.id, &url, token).await.is_err());
    assert!(store.tasks(list, true).unwrap()[0].dirty);
    assert_eq!(handle.await.unwrap().len(), 2);
}

#[tokio::test]
async fn delete_tasks_in_both_directions_and_never_resurrect_a_local_tombstone() {
    for local in [true, false] {
        let (_dir, mut store, account, list) = setup(true);
        let task = linked_task(&store, list);
        let remote = if local { r#"{"items":[{"id":"task","title":"stale"}]}"# } else { r#"{"items":[{"id":"task","deleted":true,"updated":"2026-11-09T09:00:00Z"}]}"# };
        let mut replies = vec![("200 OK", LIST)];
        if local { store.delete_task(task.id).unwrap(); replies.push(("204 No Content", "")); }
        replies.push(("200 OK", remote));
        let (url, handle) = server(replies).await;
        sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
        assert!(store.tasks(list, true).unwrap().is_empty());
        assert_eq!(store.db.query_row("SELECT count(*) FROM tasks WHERE deleted=1 AND dirty=0", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let requests = handle.await.unwrap();
        assert_eq!(requests.iter().filter(|r| r.starts_with("DELETE")).count(), usize::from(local));
    }
}

#[tokio::test]
async fn first_sync_maps_names_creates_missing_lists_and_repeats_without_duplicates() {
    let (_dir, mut store, account, _) = setup(false);
    store.add_task_list("Local only").unwrap();
    let lists = r#"{"items":[{"id":"work","title":"Work"},{"id":"remote","title":"Remote only"}]}"#;
    let all = r#"{"items":[{"id":"work","title":"Work"},{"id":"remote","title":"Remote only"},{"id":"local","title":"Local only"}]}"#;
    let (url, handle) = server(vec![("200 OK", lists), ("200 OK", SAVED_LIST), ("200 OK", r#"{"id":"local","title":"Local only"}"#),
        ("200 OK", EMPTY), ("200 OK", EMPTY), ("200 OK", EMPTY), ("200 OK", all), ("200 OK", EMPTY), ("200 OK", EMPTY), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let before = store.task_lists().unwrap();
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(store.task_lists().unwrap(), before);
    assert_eq!(before.len(), 3);
    assert!(before.iter().all(|l| l.account.as_deref() == Some(&account.id)));
    assert_eq!(handle.await.unwrap().iter().filter(|r| r.starts_with("POST")).count(), 1);
}

#[tokio::test]
async fn mail_link_round_trips_in_plain_notes_with_escaping_and_preserves_notes() {
    let (_dir, mut store, account, list) = setup(true);
    let task = store.task_from_mail(list, &crate::store::MessageSummary { account: account.id.clone(), message_id: "<one+two@example.test>".into(), subject: "A & B\nsecond line".into(), ..Default::default() }).unwrap();
    store.update_task(task.id, "Mail", "Keep\nnotes\n", Some("2026-11-10".into())).unwrap();
    let task = store.tasks(list, true).unwrap().remove(0);
    let body = task_body(&task).unwrap();
    assert_eq!(body["due"], "2026-11-10T00:00:00Z");
    assert!(body.get("links").is_none());
    let notes = body["notes"].as_str().unwrap();
    assert!(notes.contains("\nlagom://mail?account="));
    let appended = format!("{notes}\nAdded in Google");
    let (text, link) = split_notes(&appended);
    assert_eq!(text, "Keep\nnotes\n\nAdded in Google");
    assert_eq!(link.unwrap().1, "<one+two@example.test>");
    let mut remote = body;
    remote["id"] = json!("task");
    let page = json!({"items":[remote]}).to_string();
    store.db.execute("DELETE FROM tasks", []).unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", &page)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let saved = store.tasks(list, true).unwrap().remove(0);
    assert_eq!((saved.notes, saved.mail_account, saved.mail_message_id, saved.mail_subject), (task.notes, task.mail_account, task.mail_message_id, task.mail_subject));
    handle.await.unwrap();
    assert_eq!(split_notes("ordinary lagom://mail?not-a-link").0, "ordinary lagom://mail?not-a-link");
}

#[tokio::test]
async fn unauthorized_refreshes_once_and_insufficient_scope_marks_only_tasks_access() {
    for (responses, kind, refreshes) in [
        (vec![("401 Unauthorized", "{}"), ("200 OK", LIST), ("200 OK", EMPTY)], "ok", 1),
        (vec![("401 Unauthorized", "{}"), ("401 Unauthorized", "{}")], "auth", 1),
        (vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#)], "scope", 0),
        (vec![("403 Forbidden", r#"{"error":{"details":[{"reason":"ACCESS_TOKEN_SCOPE_INSUFFICIENT"}]}}"#)], "scope", 0),
        (vec![("403 Forbidden", r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#)], "quota", 0),
    ] {
        let (_dir, mut store, account, _) = setup(true);
        let (url, handle) = server(responses).await;
        let mut refresh = 0;
        let result = sync_tasks_with(&mut store, &account.id, &url, |rejected| {
            if rejected.is_some() { refresh += 1; }
            std::future::ready(Ok(Zeroizing::new(if refresh == 0 { "old" } else { "fresh" }.into())))
        }).await;
        assert_eq!(refresh, refreshes);
        match kind { "ok" => result.unwrap(), "auth" => assert!(matches!(result, Err(Error::ReauthRequired))), "scope" => assert!(matches!(result, Err(Error::TasksConsent))), _ => assert!(matches!(result, Err(Error::Network(_)))) }
        let scopes = store.account_scopes(&account.id).unwrap();
        assert_eq!(oauth::tasks_allowed(&scopes), kind != "scope");
        assert!(oauth::calendar_allowed(&scopes));
        let requests = handle.await.unwrap();
        if refreshes > 0 { assert!(requests[1].contains("Bearer fresh")); }
    }
}

#[tokio::test]
async fn edits_during_push_and_pull_remain_dirty_even_with_same_second_timestamp() {
    for edit_at in [3, 4] {
        let (_dir, mut store, account, list) = setup(true);
        let task = linked_task(&store, list);
        store.rename_task(task.id, "First edit").unwrap();
        let editor = Store::open(store.path()).unwrap();
        let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", TASK), ("200 OK", include_str!("../fixtures/tasks.json")), ("200 OK", EMPTY)]).await;
        let mut requests = 0;
        sync_tasks_with(&mut store, &account.id, &url, |_| {
            requests += 1;
            if requests == edit_at { editor.rename_task(task.id, "Newer local edit").unwrap(); }
            token(None)
        }).await.unwrap();
        let saved = store.tasks(list, true).unwrap().remove(0);
        assert_eq!(saved.title, "Newer local edit");
        assert!(saved.dirty);
        handle.await.unwrap();
    }
}

#[test]
fn account_removal_keeps_linked_lists_tasks_and_mail_links_but_drops_google_metadata() {
    let (_dir, store, account, list) = setup(true);
    let task = linked_task(&store, list);
    store.db.execute("UPDATE tasks SET mail_account=?1,mail_message_id='<fixture@example.test>',etag='etag',remote_updated='time'", [&account.id]).unwrap();
    store.delete_account(&account.id).unwrap();
    let lists = store.task_lists().unwrap();
    assert_eq!(lists.len(), 1);
    assert!(lists[0].account.is_none() && lists[0].remote_id.is_none());
    let saved = store.tasks(list, true).unwrap().remove(0);
    assert_eq!(saved.id, task.id);
    assert_eq!(saved.mail_account.as_deref(), Some(account.id.as_str()));
    assert!(saved.remote_id.is_none() && saved.dirty);
    assert_eq!(store.db.query_row("SELECT etag,remote_updated,remote_list_id FROM tasks", [], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))).unwrap(), (None, None, None));
}

#[test]
fn v10_migration_keeps_local_data_and_marks_lists_for_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    for schema in [include_str!("../../schema_v1.sql"), include_str!("../../schema_v2.sql"), include_str!("../../schema_v3.sql"), include_str!("../../schema_v4.sql"), include_str!("../../schema_v5.sql"), include_str!("../../schema_v6.sql"), include_str!("../../schema_v7.sql"), include_str!("../../schema_v8.sql"), include_str!("../../schema_v9.sql"), include_str!("../../schema_v10.sql")] { db.execute_batch(schema).unwrap(); }
    db.execute_batch("INSERT INTO tasks(list,title,notes,updated,created,dirty) SELECT list,'Keep','Notes',42,41,1 FROM task_settings;").unwrap();
    drop(db);
    let store = Store::open(path).unwrap();
    let task = store.tasks(store.default_task_list().unwrap(), true).unwrap().remove(0);
    assert_eq!((task.title.as_str(), task.notes.as_str(), task.updated, task.created), ("Keep", "Notes", 42, 41));
    assert_eq!(store.task_lists().unwrap().len(), 2);
    assert_eq!(store.db.query_row("SELECT count(*) FROM task_lists WHERE dirty=1 AND deleted=0", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), 16);
    assert_eq!(store.db.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none(), true);
}

#[tokio::test]
async fn list_rename_and_delete_both_ways_preserve_tasks_in_default_list() {
    for local in [true, false] {
        let (_dir, mut store, account, default) = setup(true);
        let extra = store.add_task_list("Extra").unwrap();
        store.db.execute("UPDATE task_lists SET account=?2,remote_id='extra',dirty=0 WHERE id=?1", params![extra.id, account.id]).unwrap();
        let task = store.add_task(extra.id, "Keep on deletion", "Notes", None).unwrap();
        store.db.execute("UPDATE tasks SET remote_id='old-task',remote_list_id='extra',dirty=0 WHERE id=?1", [task.id]).unwrap();
        if local { store.delete_task_list(extra.id).unwrap(); }
        let mut replies = vec![("200 OK", LIST)];
        if local { replies.extend([("404 Not Found", "{}"), ("200 OK", TASK)]); }
        replies.push(("200 OK", EMPTY));
        let (url, handle) = server(replies).await;
        sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
        assert_eq!(store.task_lists().unwrap().len(), 1);
        let kept = store.tasks(default, true).unwrap();
        assert_eq!(kept[0].id, task.id);
        assert_eq!(kept[0].notes, "Notes");
        if !local { assert!(kept[0].dirty && kept[0].remote_id.is_none()); }
        handle.await.unwrap();
    }
    let (_dir, mut store, account, list) = setup(true);
    store.rename_task_list(list, "Local rename").unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", SAVED_LIST), ("200 OK", EMPTY),
        ("200 OK", r#"{"items":[{"id":"work","title":"Remote rename"}]}"#), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(store.task_lists().unwrap()[0].name, "Local rename");
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(store.task_lists().unwrap()[0].name, "Remote rename");
    assert!(handle.await.unwrap()[1].contains("Local rename"));
}

#[tokio::test]
async fn failed_or_repeated_pull_page_keeps_cache_and_cursor() {
    for second in [("503 Offline", "{}"), ("200 OK", include_str!("../fixtures/tasks.json"))] {
        let (_dir, mut store, account, list) = setup(true);
        let original = linked_task(&store, list);
        let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", include_str!("../fixtures/tasks.json")), second]).await;
        assert!(sync_tasks_with(&mut store, &account.id, &url, token).await.is_err());
        assert_eq!(store.tasks(list, true).unwrap(), [original]);
        assert!(store.google_lists(&account.id).unwrap()[0].cursor.is_none());
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn moving_during_insert_queues_cleanup_without_binding_the_old_list_id() {
    let (_dir, mut store, account, list) = setup(true);
    let extra = store.add_task_list("Extra").unwrap();
    store.db.execute("UPDATE task_lists SET account=?1,remote_id='extra',dirty=0 WHERE id=?2", params![account.id, extra.id]).unwrap();
    let task = store.add_task(list, "Move", "", None).unwrap();
    let editor = Store::open(store.path()).unwrap();
    let lists = r#"{"items":[{"id":"work","title":"Work"},{"id":"extra","title":"Extra"}]}"#;
    let (url, handle) = server(vec![("200 OK", lists), ("200 OK", TASK), ("200 OK", r#"{"id":"new-task","title":"Move"}"#), ("200 OK", EMPTY), ("200 OK", EMPTY)]).await;
    let mut requests = 0;
    sync_tasks_with(&mut store, &account.id, &url, |_| {
        requests += 1;
        if requests == 3 { editor.move_task(task.id, extra.id, 0).unwrap(); }
        token(None)
    }).await.unwrap();
    let moved = store.tasks(extra.id, true).unwrap();
    assert_eq!(moved[0].remote_id.as_deref(), Some("new-task"));
    let cleanup = store.pending_google_tasks(list).unwrap();
    assert_eq!(cleanup.len(), 1);
    assert!(cleanup[0].0.deleted);
    assert_eq!(cleanup[0].0.remote_id.as_deref(), Some("task"));
    handle.await.unwrap();
}

#[test]
fn cross_account_move_leaves_deletion_with_original_account() {
    let (_dir, store, account, list) = setup(true);
    let task = linked_task(&store, list);
    let (imap, smtp) = crate::presets::servers("other@gmail.com").unwrap();
    let other = Account::new("other@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&other).unwrap();
    let extra = store.add_task_list("Other").unwrap();
    store.db.execute("UPDATE task_lists SET account=?1,remote_id='other-list',dirty=0 WHERE id=?2", params![other.id, extra.id]).unwrap();
    store.move_task(task.id, extra.id, 0).unwrap();
    let moved = store.tasks(extra.id, true).unwrap();
    assert!(moved[0].dirty && moved[0].remote_id.is_none());
    let cleanup = store.pending_google_tasks(list).unwrap();
    assert_eq!(cleanup.len(), 1);
    assert!(cleanup[0].0.deleted);
    assert_eq!(store.google_lists(&account.id).unwrap()[0].id, list);
}

#[test]
fn reordering_keeps_remote_identity_and_deleted_lists_reject_new_tasks() {
    let (_dir, store, _, list) = setup(true);
    let task = linked_task(&store, list);
    store.move_task(task.id, list, 1).unwrap();
    assert_eq!(store.tasks(list, true).unwrap()[0].remote_id, task.remote_id);
    let deleted = store.add_task_list("Deleted").unwrap();
    store.delete_task_list(deleted.id).unwrap();
    assert!(store.add_task(deleted.id, "Hidden", "", None).is_err());
    assert!(store.move_task(task.id, deleted.id, 0).is_err());
    assert_eq!(store.tasks(list, true).unwrap()[0].id, task.id);
}

#[tokio::test]
async fn completion_due_and_notes_sync_both_ways_and_acknowledged_deletes_stay_deleted() {
    let (_dir, mut store, account, list) = setup(true);
    let task = linked_task(&store, list);
    store.update_task(task.id, "Done", "Notes", Some("2026-11-10".into())).unwrap();
    store.set_done(task.id, true).unwrap();
    let remote = r#"{"items":[{"id":"task","title":"Reopened","notes":"Changed remotely","status":"needsAction","due":"2026-11-11T00:00:00Z","updated":"2026-11-09T10:00:00Z"}]}"#;
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", TASK), ("200 OK", EMPTY),
        ("200 OK", LIST), ("200 OK", remote),
        ("200 OK", LIST), ("204 No Content", ""), ("200 OK", EMPTY),
        ("200 OK", LIST), ("200 OK", remote)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let saved = store.tasks(list, true).unwrap().remove(0);
    assert!(saved.done_at.is_none());
    assert_eq!(saved.notes, "Changed remotely");
    assert_eq!(saved.due.as_deref(), Some("2026-11-11T00:00:00Z"));
    store.delete_task(task.id).unwrap();
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert!(store.tasks(list, true).unwrap().is_empty());
    let requests = handle.await.unwrap();
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["due"], "2026-11-10T00:00:00Z");
    assert!(body["completed"].as_str().is_some());
}

#[tokio::test]
async fn review_detached_lists_are_not_uploaded_to_remaining_account() {
    let (_dir, mut store, work, list) = setup(true);
    let task = linked_task(&store, list);
    store.db.execute("UPDATE tasks SET mail_account=?1,mail_message_id='<private@example.test>' WHERE id=?2", params![work.id, task.id]).unwrap();
    let (imap, smtp) = crate::presets::servers("other@gmail.com").unwrap();
    let personal = Account::new("other@gmail.com".into(), imap, smtp).unwrap();
    store.save_authorized_account(&personal, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    store.delete_account(&work.id).unwrap();
    let fresh = store.add_task_list("Fresh").unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", r#"{"id":"fresh","title":"Fresh"}"#), ("200 OK", EMPTY), ("200 OK", EMPTY)]).await;
    let result = sync_tasks_with(&mut store, &personal.id, &url, token).await;
    let lists = store.task_lists().unwrap();
    assert!(lists.iter().find(|l| l.id == list).unwrap().account.is_none());
    result.unwrap();
    assert_eq!(lists.iter().find(|l| l.id == fresh.id).unwrap().account.as_deref(), Some(personal.id.as_str()));
    let saved = &store.tasks(list, true).unwrap()[0];
    assert_eq!(saved.id, task.id);
    assert_eq!(saved.mail_account.as_deref(), Some(work.id.as_str()));
    assert!(saved.remote_id.is_none());
    assert!(handle.await.unwrap().iter().all(|r| !r.contains("Local notes") && !r.contains("lagom://mail")));
}

#[tokio::test]
async fn review_full_task_page_with_maximum_notes_advances_cursor() {
    let (_dir, mut store, account, list) = setup(true);
    let notes = "📝".repeat(8192);
    let page = json!({"items": (0..100).map(|id| json!({"id": id.to_string(), "title": "Full notes", "notes": notes, "updated": "2026-11-09T09:00:00Z"})).collect::<Vec<_>>()}).to_string();
    assert!(page.len() > 256 * 1024);
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", &page)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    let tasks = store.tasks(list, true).unwrap();
    assert_eq!(tasks.len(), 100);
    assert!(tasks.iter().all(|t| t.notes == notes));
    assert!(store.google_lists(&account.id).unwrap()[0].cursor.is_some());
    handle.await.unwrap();
}

#[tokio::test]
async fn review_google_default_is_protected_and_old_tombstone_recovers() {
    for deleted in [false, true] {
        let (_dir, mut store, account, local_default) = setup(true);
        let other = store.add_task_list("Google default").unwrap();
        store.db.execute("UPDATE task_lists SET account=?2,remote_id='primary',dirty=0 WHERE id=?1", params![other.id, account.id]).unwrap();
        if deleted { store.delete_task_list(other.id).unwrap(); }
        let lists = r#"{"items":[{"id":"work","title":"Work"},{"id":"primary","title":"Google default"}]}"#;
        let (url, handle) = server_default(vec![("200 OK", lists), ("200 OK", EMPTY), ("200 OK", EMPTY)],
            r#"{"id":"primary","title":"Google default"}"#).await;
        sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
        assert_eq!(store.default_task_list().unwrap(), local_default);
        assert!(store.task_lists().unwrap().iter().any(|l| l.id == other.id));
        assert!(matches!(store.delete_task_list(other.id), Err(Error::Invalid(_))));
        assert!(handle.await.unwrap().iter().all(|r| !r.starts_with("DELETE")));
    }
}

#[tokio::test]
async fn review_rejected_list_deletion_restores_visibility_and_continues_sync() {
    let (_dir, mut store, account, list) = setup(true);
    let other = store.add_task_list("Cannot delete").unwrap();
    store.db.execute("UPDATE task_lists SET account=?2,remote_id='other',dirty=0 WHERE id=?1", params![other.id, account.id]).unwrap();
    store.delete_task_list(other.id).unwrap();
    store.add_task(list, "Local title", "", None).unwrap();
    let lists = r#"{"items":[{"id":"work","title":"Work"},{"id":"other","title":"Cannot delete"}]}"#;
    let (url, handle) = server(vec![("200 OK", lists), ("400 Bad Request", "{}"), ("200 OK", TASK), ("200 OK", EMPTY), ("200 OK", EMPTY),
        ("200 OK", lists), ("200 OK", EMPTY), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert!(store.task_lists().unwrap().iter().any(|l| l.id == other.id));
    assert!(!store.tasks(list, false).unwrap()[0].dirty);
    sync_tasks_with(&mut store, &account.id, &url, token).await.unwrap();
    assert_eq!(handle.await.unwrap().iter().filter(|r| r.starts_with("DELETE")).count(), 1);
}

#[test]
fn review_v13_migration_keeps_unowned_lists_local_and_new_lists_mappable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    for schema in [include_str!("../../schema_v1.sql"), include_str!("../../schema_v2.sql"), include_str!("../../schema_v3.sql"), include_str!("../../schema_v4.sql"), include_str!("../../schema_v5.sql"), include_str!("../../schema_v6.sql"), include_str!("../../schema_v7.sql"), include_str!("../../schema_v8.sql"), include_str!("../../schema_v9.sql"), include_str!("../../schema_v10.sql"), include_str!("../../schema_v11.sql"), include_str!("../../schema_v12.sql"), include_str!("../../schema_v13.sql")] { db.execute_batch(schema).unwrap(); }
    db.execute_batch("INSERT INTO tasks(list,title,notes,updated,created,dirty) SELECT list,'Keep','Private notes',42,41,1 FROM task_settings;").unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let list = store.default_task_list().unwrap();
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
    store.save_account(&account).unwrap();
    let fresh = store.add_task_list("Fresh").unwrap();
    store.link_google_lists(&account.id, &serde_json::from_str::<Page<RemoteList>>(LIST).unwrap().items).unwrap();
    let lists = store.task_lists().unwrap();
    assert!(lists.iter().find(|l| l.id == list).unwrap().account.is_none());
    assert_eq!(lists.iter().find(|l| l.id == fresh.id).unwrap().account.as_deref(), Some(account.id.as_str()));
    let task = store.tasks(list, true).unwrap().remove(0);
    assert_eq!((task.notes.as_str(), task.updated, task.created), ("Private notes", 42, 41));
    store.delete_account(&account.id).unwrap();
    assert!(store.task_lists().unwrap().iter().all(|l| l.account.is_none()));
    assert!(store.db.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none());
}

#[tokio::test]
async fn review_task_responses_remain_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await.unwrap()); }
        let size = 16 * 1024 * 1024 + 1;
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let _ = socket.write_all(&vec![b' '; size]).await;
    });
    let mut client = Client { http: oauth::http(Duration::from_secs(5)).unwrap(), base, token };
    let result = client.request(Method::GET, client.url(&["lists", "work", "tasks"]).unwrap(), None).await;
    assert!(matches!(result, Err(Error::Network("HTTP response too large"))));
    server.await.unwrap();
}

#[tokio::test]
async fn remote_list_deletion_never_uploads_work_tasks_to_personal_default() {
    let (_dir, mut store, personal, default) = setup(true);
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let work = Account::new("work@example.test".into(), imap, smtp).unwrap();
    store.save_authorized_account(&work, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    let list = store.add_task_list("Extra").unwrap().id;
    store.db.execute("UPDATE task_lists SET account=?2,remote_id='extra',dirty=0 WHERE id=?1", params![list, work.id]).unwrap();
    let task = store.add_task(list, "Work task", "Private work notes", None).unwrap();
    store.db.execute("UPDATE tasks SET remote_id='old-task',remote_list_id='extra',dirty=0,mail_account=?2,mail_message_id='<work@example.test>',mail_subject='Work mail' WHERE id=?1", params![task.id, work.id]).unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &work.id, &url, token).await.unwrap();
    assert!(handle.await.unwrap().iter().all(|r| r.starts_with("GET ")));
    assert!(store.tasks(default, true).unwrap().is_empty(), "work tasks must never enter the personal default list");
    assert_eq!(store.task_lists().unwrap().iter().find(|l| l.id == list).unwrap().account.as_deref(), Some(work.id.as_str()));
    let kept = store.tasks(list, true).unwrap().remove(0);
    assert_eq!((kept.id, kept.notes.as_str(), kept.mail_account.as_deref(), kept.mail_message_id.as_deref(), kept.mail_subject.as_deref()),
        (task.id, "Private work notes", Some(work.id.as_str()), Some("<work@example.test>"), Some("Work mail")));
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &personal.id, &url, token).await.unwrap();
    assert!(handle.await.unwrap().iter().all(|r| r.starts_with("GET ") && !r.contains("Private work notes")));
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", r#"{"id":"recovered","title":"Extra"}"#),
        ("200 OK", TASK), ("200 OK", EMPTY), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &work.id, &url, token).await.unwrap();
    let requests = handle.await.unwrap();
    assert!(requests[1].starts_with("POST /users/@me/lists "));
    assert!(requests[2].starts_with("POST /lists/recovered/tasks "));
    assert!(requests[2].contains("Private work notes") && requests[2].contains(&work.id));
    assert_eq!(store.tasks(list, true).unwrap()[0].id, task.id);
    assert_eq!(store.default_task_list().unwrap(), default);
}

#[tokio::test]
async fn default_change_between_recovery_check_and_move_cannot_leak_tasks() {
    let (dir, mut store, work, default) = setup(true);
    let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
    let personal = Account::new("personal@example.test".into(), imap, smtp).unwrap();
    store.save_authorized_account(&personal, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
    let personal_list = store.add_task_list("Personal").unwrap().id;
    store.db.execute("UPDATE task_lists SET account=?2,remote_id='work',dirty=0 WHERE id=?1", params![personal_list, personal.id]).unwrap();
    let extra = store.add_task_list("Extra").unwrap().id;
    store.db.execute("UPDATE task_lists SET account=?2,remote_id='extra',dirty=0 WHERE id=?1", params![extra, work.id]).unwrap();
    let task = store.add_task(extra, "Work task", "Private work notes", None).unwrap();
    store.db.execute("UPDATE tasks SET remote_id='old-task',remote_list_id='extra',dirty=0,mail_account=?2,mail_message_id='<work@example.test>',mail_subject='Work mail' WHERE id=?1", params![task.id, work.id]).unwrap();
    let settings = Store::open(dir.path().join("fixture.db")).unwrap();
    settings.db.busy_timeout(Duration::ZERO).unwrap();
    let change = std::rc::Rc::new(std::cell::RefCell::new(None));
    let observed = change.clone();
    RECOVERY_OWNER_CHECK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
        assert_eq!(settings.default_task_list().unwrap(), default);
        *observed.borrow_mut() = Some(settings.set_default_task_list(personal_list));
    })));
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &work.id, &url, token).await.unwrap();
    assert!(handle.await.unwrap().iter().all(|r| r.starts_with("GET ")));
    assert!(store.tasks(personal_list, true).unwrap().is_empty(), "a default switch during recovery must not expose work tasks");
    assert!(matches!(change.borrow_mut().take().unwrap(), Err(Error::Store(rusqlite::Error::SqliteFailure(error, _))) if error.code == rusqlite::ErrorCode::DatabaseBusy));
    let kept = store.tasks(default, true).unwrap().remove(0);
    assert_eq!((kept.id, kept.notes.as_str(), kept.mail_account.as_deref(), kept.mail_message_id.as_deref(), kept.mail_subject.as_deref()),
        (task.id, "Private work notes", Some(work.id.as_str()), Some("<work@example.test>"), Some("Work mail")));
    assert!(kept.dirty && kept.remote_id.is_none());
    assert!(!store.task_lists().unwrap().iter().any(|list| list.id == extra));
    let settings = Store::open(dir.path().join("fixture.db")).unwrap();
    settings.set_default_task_list(personal_list).unwrap();
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &personal.id, &url, token).await.unwrap();
    assert!(handle.await.unwrap().iter().all(|r| r.starts_with("GET ") && !r.contains("Private work notes")));
    let (url, handle) = server(vec![("200 OK", LIST), ("200 OK", TASK), ("200 OK", EMPTY)]).await;
    sync_tasks_with(&mut store, &work.id, &url, token).await.unwrap();
    let requests = handle.await.unwrap();
    assert!(requests[1].starts_with("POST /lists/work/tasks "));
    assert!(requests[1].contains("Private work notes") && requests[1].contains(&work.id));
    assert_eq!(store.default_task_list().unwrap(), personal_list);
}

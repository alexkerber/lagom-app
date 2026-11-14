use super::*;
use crate::store::MessageSummary;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn fixture(steps: Vec<(String, String)>) -> (ImapSync, Store, tempfile::TempDir, tokio::task::JoinHandle<()>) {
    let (client, wire) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move {
        let mut wire = BufReader::new(wire);
        for (expected, reply) in steps {
            let mut line = String::new();
            wire.read_line(&mut line).await.unwrap();
            let (tag, command) = line.trim_end().split_once(' ').expect("command");
            assert_eq!(command, expected);
            wire.get_mut().write_all(reply.replace("{tag}", tag).as_bytes()).await.unwrap();
        }
    });
    let config = ServerConfig { host: "localhost".into(), port: 3143, security: Security::Plain, username: "fixture".into() };
    let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
    let mut session = Client::new(Bounded::new(Box::new(client) as Wire)).login("fixture", "fixture").await.unwrap();
    let capabilities = session.capabilities().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path().join("mail.db")).unwrap();
    store.save_account(&account).unwrap();
    (ImapSync { account, session, capabilities, refresh_error: None, password: Default::default() }, store, temp, server)
}

fn step(command: &str, data: &str) -> (String, String) {
    (command.into(), format!("{data}{{tag}} OK done\r\n"))
}
fn login(capabilities: &str) -> Vec<(String, String)> {
    vec![step("LOGIN \"fixture\" \"fixture\"", ""), step("CAPABILITY", &format!("* CAPABILITY IMAP4rev1 {capabilities}\r\n"))]
}
fn selected() -> &'static str { "* 0 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 1] next\r\n" }
fn empty_sync(name: &str) -> Vec<(String, String)> {
    vec![step(&format!("SELECT \"{name}\""), selected()), step("UID SEARCH ALL", "* SEARCH\r\n"), step("UID SEARCH ALL", "* SEARCH\r\n")]
}
fn message(store: &Store, account: &str, folder: i64, uid: u32) -> i64 {
    store.save_synced_message(&Message { summary: MessageSummary {
        account: account.into(), folder, uid, message_id: format!("{uid}@fixture.test"), subject: "Fixture".into(), ..Default::default()
    }, body_text: "Cached body".into(), ..Default::default() }, &[]).unwrap();
    store.find_message(folder, &format!("{uid}@fixture.test")).unwrap().unwrap()
}

#[tokio::test]
async fn review_upgrade_preserves_encoded_folders_and_local_mail() {
    let names = [("R&--D", "R&-D"), ("R&-D", "R&D"), ("&U,BTFw-/&ZeVnLIqe-", "台北/日本語")];
    let mut steps = login("");
    let rows: String = names.iter().map(|(wire, _)| format!("* LIST () \"/\" \"{wire}\"\r\n")).collect();
    steps.push(step("LIST \"\" \"*\"", &rows));
    let (mut imap, store, temp, server) = fixture(steps).await;
    drop(store);
    let path = temp.path().join("upgrade.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    for schema in [include_str!("../schema_v1.sql"), include_str!("../schema_v2.sql"), include_str!("../schema_v3.sql"), include_str!("../schema_v4.sql"), include_str!("../schema_v5.sql"), include_str!("../schema_v6.sql"), include_str!("../schema_v7.sql"), include_str!("../schema_v8.sql"), include_str!("../schema_v9.sql"), include_str!("../schema_v10.sql"), include_str!("../schema_v11.sql"), include_str!("../schema_v12.sql"), include_str!("../schema_v13.sql"), include_str!("../schema_v14.sql")] {
        db.execute_batch(schema).unwrap();
    }
    let mut old = Store { db };
    old.save_account(&imap.account).unwrap();
    let mut saved = Vec::new();
    for (wire, decoded) in names {
        let folder = old.save_folder(&imap.account.id, wire, Some("/"), FolderRole::Other).unwrap();
        old.reset_uidvalidity(folder, 7).unwrap();
        let id = message(&old, &imap.account.id, folder, 1);
        old.snooze(id, Some(123456789)).unwrap();
        old.finish_sync(folder, &HashSet::from([1]), 1, Some(2), Some(3)).unwrap();
        let mut metadata = old.folder(folder).unwrap();
        metadata.name = decoded.into();
        saved.push((metadata, old.message(id).unwrap()));
    }
    old.db.execute_batch("CREATE TRIGGER refuse_rename BEFORE UPDATE ON folders WHEN NEW.name='R&D' BEGIN SELECT RAISE(ABORT, 'fixture refused'); END;").unwrap();
    drop(old);
    assert!(Store::open(&path).is_err());
    let old = Store { db: rusqlite::Connection::open(&path).unwrap() };
    assert_eq!(old.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
    for ((folder, message), (wire, _)) in saved.iter().zip(names) {
        assert_eq!(old.folder(folder.id).unwrap().name, wire);
        assert_eq!(old.message(message.summary.id).unwrap(), *message);
    }
    old.db.execute_batch("DROP TRIGGER refuse_rename").unwrap();
    drop(old);
    let store = Store::open(&path).unwrap();
    let folders = imap.list_folders(&store).await.unwrap();
    assert_eq!(folders.len(), saved.len());
    for (folder, message) in &saved {
        assert_eq!(store.folder(folder.id).unwrap(), *folder);
        assert_eq!(store.message(message.summary.id).unwrap(), *message);
    }
    drop(store);
    let reopened = Store::open(path).unwrap();
    for (folder, message) in saved {
        assert_eq!(reopened.folder(folder.id).unwrap(), folder);
        assert_eq!(reopened.message(message.summary.id).unwrap(), message);
    }
    assert!(reopened.db.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn list_create_refresh_and_refusal_preserve_account_folders() {
    for special in [false, true] {
        let list = if special { "LIST \"\" \"*\" RETURN (SPECIAL-USE)" } else { "LIST \"\" \"*\"" };
        let rows = "* LIST () \"/\" \"INBOX\"\r\n* LIST (\\All) \"/\" \"[Gmail]/All Mail\"\r\n* LIST (\\Drafts) \"/\" \"[Gmail]/Drafts\"\r\n* LIST (\\Flagged) \"/\" \"Favoriter\"\r\n* LIST (\\Important) \"/\" \"Viktigt\"\r\n* LIST () \"/\" \"[Gmail]/Important\"\r\n* LIST () \"/\" \"[Gmail]/Starred\"\r\n* LIST (\\Noselect) \"/\" \"Parent\"\r\n* LIST () \"/\" \"Parent/Child\"\r\n";
        let mut steps = login(if special { "SPECIAL-USE" } else { "" });
        steps.push(step(list, rows));
        steps.push(step("CREATE \"R&AOQ-ksm&APY-rg&AOU-s\"", ""));
        steps.push(step(list, &format!("{rows}* LIST () \"/\" \"R&AOQ-ksm&APY-rg&AOU-s\"\r\n")));
        steps.push((list.into(), "* LIST () NIL \"Partial\"\r\n{tag} NO refused\r\n".into()));
        steps.push(step(list, "* LIST () NIL \"INBOX\"\r\n"));
        let (mut imap, store, _temp, server) = fixture(steps).await;
        let other = Account::new("other@example.com".into(), imap.account.imap.clone(), imap.account.smtp.clone()).unwrap();
        store.save_account(&other).unwrap();
        let foreign = store.save_folder(&other.id, "Other", None, FolderRole::Other).unwrap();
        let folders = imap.list_folders(&store).await.unwrap();
        assert_eq!(folders.len(), 4);
        assert!(folders.iter().any(|f| f.name == "Parent/Child"));
        assert!(folders.iter().any(|f| f.role == FolderRole::Archive));
        let created = imap.create_folder(&store, "Räksmörgås").await.unwrap();
        assert_eq!(store.folder(created).unwrap().name, "Räksmörgås");
        let before = store.folders(&imap.account.id, 0).unwrap();
        assert!(imap.list_folders(&store).await.is_err());
        assert_eq!(store.folders(&imap.account.id, 0).unwrap(), before);
        assert_eq!(imap.list_folders(&store).await.unwrap().len(), 1);
        assert!(store.folder(created).is_err());
        assert_eq!(store.folder(foreign).unwrap().account, other.id);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn move_and_copy_fallback_check_every_tagged_reply_and_rollback() {
    for capability in ["MOVE UIDPLUS", "UIDPLUS", ""] {
        for failure in [None, Some("copy"), Some("store"), Some("expunge")] {
            if capability.contains("MOVE") && matches!(failure, Some("store" | "expunge")) { continue; }
            let mut steps = login(capability);
            steps.push(step("SELECT \"INBOX\"", selected()));
            let commands = if capability.contains("MOVE") {
                vec!["UID MOVE 1 \"Projects\"".to_string()]
            } else {
                vec!["UID COPY 1 \"Projects\"".into(), "UID STORE 1 +FLAGS.SILENT (\\Deleted)".into(),
                    if capability.contains("UIDPLUS") { "UID EXPUNGE 1".into() } else { "EXPUNGE".into() }]
            };
            for (i, command) in commands.iter().enumerate() {
                if failure == Some(["copy", "store", "expunge"][i]) {
                    steps.push((command.clone(), "{tag} NO refused\r\n".into()));
                    if failure == Some("expunge") { steps.push(step("UID STORE 1 -FLAGS.SILENT (\\Deleted)", "")); }
                    break;
                }
                steps.push(step(command, ""));
            }
            if failure.is_none() { steps.extend(empty_sync("Projects")); steps.extend(empty_sync("INBOX")); }
            // Keep the peer alive so a later EOF cannot mask an ignored refusal.
            else { steps.push(step("NOOP", "")); }
            let (mut imap, mut store, _temp, server) = fixture(steps).await;
            let inbox = store.save_folder(&imap.account.id, "INBOX", None, FolderRole::Inbox).unwrap();
            let target = store.save_folder(&imap.account.id, "Projects", None, FolderRole::Other).unwrap();
            store.reset_uidvalidity(inbox, 7).unwrap();
            let id = message(&store, &imap.account.id, inbox, 1);
            store.begin_move(id, target).unwrap();
            assert_eq!(store.list_messages(target, 0, 10, 0).unwrap()[0].id, id);
            let result = imap.move_message(&mut store, id, target).await;
            assert_eq!(result.is_err(), failure.is_some());
            if failure.is_some() {
                imap.session.noop().await.unwrap();
                assert_eq!(store.list_messages(inbox, 0, 10, 0).unwrap()[0].id, id);
                assert!(store.list_messages(target, 0, 10, 0).unwrap().is_empty());
                assert_eq!(store.message(id).unwrap().body_text, "Cached body");
            } else { assert!(store.message(id).is_err()); }
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn move_and_copy_return_the_destination_uid_from_copyuid() {
    for (capability, reply, expected) in [
        // RFC 6851 sends COPYUID untagged before the expunge; some servers tag it.
        ("MOVE UIDPLUS", "* OK [COPYUID 7 1 42] moved\r\n* 1 EXPUNGE\r\n{tag} OK done\r\n", Some((7, 42))),
        ("MOVE UIDPLUS", "* 1 EXPUNGE\r\n{tag} OK [COPYUID 7 1:1 42] done\r\n", Some((7, 42))),
        ("UIDPLUS", "{tag} OK [COPYUID 7 1 43] done\r\n", Some((7, 43))),
        // Any set other than exactly the moved UID is not this message.
        ("MOVE UIDPLUS", "* OK [COPYUID 7 2 42] moved\r\n{tag} OK done\r\n", None),
        ("MOVE UIDPLUS", "* OK [COPYUID 7 1 42:43] moved\r\n{tag} OK done\r\n", None),
        ("MOVE", "{tag} OK done\r\n", None),
    ] {
        let mut steps = login(capability);
        steps.push(step("SELECT \"INBOX\"", selected()));
        if capability.contains("MOVE") {
            steps.push(("UID MOVE 1 \"Projects\"".into(), reply.into()));
        } else {
            steps.push(("UID COPY 1 \"Projects\"".into(), reply.into()));
            steps.push(step("UID STORE 1 +FLAGS.SILENT (\\Deleted)", ""));
            steps.push(step("UID EXPUNGE 1", ""));
        }
        steps.extend(empty_sync("Projects"));
        steps.extend(empty_sync("INBOX"));
        let (mut imap, mut store, _temp, server) = fixture(steps).await;
        let inbox = store.save_folder(&imap.account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let target = store.save_folder(&imap.account.id, "Projects", None, FolderRole::Other).unwrap();
        store.reset_uidvalidity(inbox, 7).unwrap();
        let id = message(&store, &imap.account.id, inbox, 1);
        assert_eq!(imap.move_message(&mut store, id, target).await.unwrap(), Moved { folder: target, uid: expected }, "{reply}");
        assert!(imap.take_refresh_error().is_none());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn confirmed_move_is_not_rolled_back_when_refresh_is_refused() {
    let mut steps = login("MOVE");
    steps.push(step("SELECT \"INBOX\"", selected()));
    steps.push(step("UID MOVE 1 \"Projects\"", ""));
    steps.push(("SELECT \"Projects\"".into(), "{tag} NO temporarily unavailable\r\n".into()));
    let (mut imap, mut store, _temp, server) = fixture(steps).await;
    let inbox = store.save_folder(&imap.account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    let target = store.save_folder(&imap.account.id, "Projects", None, FolderRole::Other).unwrap();
    store.reset_uidvalidity(inbox, 7).unwrap();
    let id = message(&store, &imap.account.id, inbox, 1);
    imap.move_message(&mut store, id, target).await.unwrap();
    assert!(store.message(id).is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn custom_folder_fetches_only_the_initial_window_then_new_mail() {
    let mut steps = login("");
    for (all, fetched, flags) in [("1 2 3 4", vec![4, 3], "4,3"), ("1 2 3 4 5", vec![5], "5,4")] {
        steps.push(step("SELECT \"Projects\"", selected()));
        steps.push(step("UID SEARCH ALL", &format!("* SEARCH {all}\r\n")));
        for uid in fetched {
            let header = format!("From: fixture@example.com\r\nSubject: Project {uid}\r\nMessage-ID: <{uid}@fixture.test>\r\n\r\n");
            steps.push(step(&format!("UID FETCH {uid} (UID ENVELOPE FLAGS BODYSTRUCTURE RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER]<0.262144>)"),
                &format!("* 1 FETCH (UID {uid} FLAGS () RFC822.SIZE 200 BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len())));
            steps.push(step(&format!("UID FETCH {uid} (UID BODY.PEEK[TEXT])"), &format!("* 1 FETCH (UID {uid} BODY[TEXT] {{4}}\r\nbody)\r\n")));
        }
        steps.push(step(&format!("UID FETCH {flags} (UID FLAGS)"), ""));
        steps.push(step("UID SEARCH ALL", &format!("* SEARCH {all}\r\n")));
    }
    let (mut imap, mut store, _temp, server) = fixture(steps).await;
    let folder = store.save_folder(&imap.account.id, "Projects", None, FolderRole::Other).unwrap();
    imap.sync_folder(&mut store, folder, 2).await.unwrap();
    assert_eq!(store.known_uids(folder).unwrap(), [3, 4]);
    imap.sync_folder(&mut store, folder, 2).await.unwrap();
    assert_eq!(store.known_uids(folder).unwrap(), [3, 4, 5]);
    assert_eq!(store.list_messages(folder, 0, 10, 0).unwrap().len(), 3);
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_create_and_interrupted_list_do_not_change_the_cache() {
    let mut steps = login("");
    steps.push(("CREATE \"Rejected\"".into(), "{tag} NO permission denied\r\n".into()));
    steps.push(("LIST \"\" \"*\"".into(), "* LIST () NIL \"Partial\"\r\n".into()));
    let (mut imap, store, _temp, server) = fixture(steps).await;
    store.save_folder(&imap.account.id, "Existing", None, FolderRole::Other).unwrap();
    let before = store.folders(&imap.account.id, 0).unwrap();
    assert!(imap.create_folder(&store, "Rejected").await.is_err());
    assert_eq!(store.folders(&imap.account.id, 0).unwrap(), before);
    assert!(imap.list_folders(&store).await.is_err());
    assert_eq!(store.folders(&imap.account.id, 0).unwrap(), before);
    server.await.unwrap();
}

#[tokio::test]
async fn confirmed_move_keeps_optimistic_destination_if_cache_delete_fails() {
    let mut steps = login("MOVE");
    steps.push(step("SELECT \"INBOX\"", selected()));
    steps.push(step("UID MOVE 1 \"Projects\"", ""));
    let (mut imap, mut store, _temp, server) = fixture(steps).await;
    let inbox = store.save_folder(&imap.account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    let target = store.save_folder(&imap.account.id, "Projects", None, FolderRole::Other).unwrap();
    store.reset_uidvalidity(inbox, 7).unwrap();
    let id = message(&store, &imap.account.id, inbox, 1);
    store.db.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON messages BEGIN SELECT RAISE(ABORT,'fixture refused'); END;").unwrap();
    imap.move_message(&mut store, id, target).await.unwrap();
    assert_eq!(store.message_summary(id).unwrap().folder, target);
    assert_eq!(store.message(id).unwrap().summary.folder, inbox);
    assert!(imap.take_refresh_error().is_some());
    server.await.unwrap();
}

use crate::{store::{MessageSummary, Store, SUMMARY_COLUMNS, summary_row}, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeSet;

fn nodes(m: &MessageSummary) -> Vec<String> {
    let mut ids: BTreeSet<String> = m.thread_refs.split_whitespace()
        .chain(std::iter::once(m.message_id.as_str()))
        .filter(|id| !id.is_empty()).map(|id| format!("id:{id}")).collect();
    if ids.is_empty() {
        let mut subject = m.subject.trim().to_lowercase();
        while let Some(rest) = subject.strip_prefix("re:").or_else(|| subject.strip_prefix("fw:")).or_else(|| subject.strip_prefix("fwd:")) {
            subject = rest.trim().to_string();
        }
        let participants: BTreeSet<_> = [&m.from_address, &m.to, &m.cc].into_iter()
            .flat_map(|s| s.split([',', ';'])).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        if !subject.is_empty() && !participants.is_empty() {
            ids.insert(format!("subject:{}", serde_json::to_string(&(subject, participants)).unwrap()));
        } else {
            ids.insert(format!("local:{}:{}", m.folder, m.uid));
        }
    }
    ids.into_iter().collect()
}

impl Store {
    pub(crate) fn resolve_thread(&self, m: &MessageSummary) -> Result<String> {
        let nodes = nodes(m);
        let mut threads = BTreeSet::new();
        for node in &nodes {
            let known: Option<String> = self.db.query_row("SELECT thread_id FROM thread_links WHERE account=?1 AND node=?2",
                params![m.account, node], |r| r.get(0)).optional()?;
            threads.insert(known.unwrap_or_else(|| node.clone()));
        }
        let thread = threads.first().unwrap().clone();
        for old in threads.iter().filter(|old| **old != thread) {
            self.db.execute("UPDATE messages SET thread_id=?3 WHERE account=?1 AND thread_id=?2", params![m.account, old, thread])?;
            self.db.execute("UPDATE thread_links SET thread_id=?3 WHERE account=?1 AND thread_id=?2", params![m.account, old, thread])?;
        }
        for node in nodes {
            self.db.execute("INSERT INTO thread_links VALUES (?1,?2,?3) ON CONFLICT(account,node) DO UPDATE SET thread_id=excluded.thread_id",
                params![m.account, node, thread])?;
        }
        Ok(thread)
    }

    pub(crate) fn backfill_threads(&self) -> Result<()> {
        let rows = self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m"))?
            .query_map([], summary_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        for mut m in rows {
            if m.thread_key.contains('@') {
                m.thread_refs = m.thread_key.clone();
            }
            let thread = self.resolve_thread(&m)?;
            self.db.execute("UPDATE messages SET thread_id=?2,thread_refs=?3 WHERE id=?1", params![m.id, thread, m.thread_refs])?;
        }
        self.db.pragma_update(None, "user_version", 8)?;
        Ok(())
    }

    pub fn thread_messages(&self, account: &str, thread: &str) -> Result<Vec<MessageSummary>> {
        let mut stmt = self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE account=?1 AND thread_id=?2 ORDER BY date,id"))?;
        let rows = stmt.query_map(params![account, thread], summary_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Pages conversations, then includes every member in the selected folder/view.
    pub fn list_conversations(&self, account: &str, folder: Option<i64>, now: i64, limit: u32) -> Result<Vec<MessageSummary>> {
        let mut stmt = self.db.prepare(&conversation_sql(folder))?;
        let rows = stmt.query_map(params![account, folder, now, limit], summary_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn conversation_sql(folder: Option<i64>) -> String {
    let (index, scope, newer_scope) = if folder.is_some() {
        ("messages_folder_date", "m.folder=?2 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?3)",
            "n.folder=?2 AND (n.snoozed_until IS NULL OR n.snoozed_until<=?3)")
    } else {
        ("messages_later_date", "m.snoozed_until>?3", "n.snoozed_until>?3")
    };
    // Start with the page, then use the thread index; a folder-first join scans the whole mailbox.
    format!("WITH page AS (
        SELECT m.thread_id FROM messages m INDEXED BY {index} WHERE m.account=?1 AND {scope}
        AND m.id=(SELECT n.id FROM messages n INDEXED BY messages_thread WHERE n.account=m.account AND n.thread_id=m.thread_id AND {newer_scope}
            ORDER BY n.date DESC,n.id DESC LIMIT 1)
        ORDER BY m.date DESC,m.id DESC LIMIT ?4)
        SELECT {SUMMARY_COLUMNS} FROM page CROSS JOIN messages m INDEXED BY messages_thread ON m.account=?1 AND m.thread_id=page.thread_id
        WHERE {scope} ORDER BY m.date DESC,m.id DESC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mime, account::Account, store::{FolderRole, Message}};

    fn fixture() -> (tempfile::TempDir, Store, String, i64) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path().join("threads.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("fixture@fastmail.com").unwrap();
        let a = Account::new("fixture@fastmail.com".into(), imap, smtp).unwrap();
        s.save_account(&a).unwrap();
        let f = s.save_folder(&a.id, "INBOX", None, FolderRole::Inbox).unwrap();
        (dir, s, a.id, f)
    }

    #[test]
    fn v8_upgrade_preserves_threads_and_indexes_later() {
        let (dir, s, account, folder) = fixture();
        let id = s.save_message(&Message { summary: MessageSummary {
            account: account.clone(), folder, uid: 1, message_id: "root@test".into(), ..Default::default()
        }, body_text: "cached body".into(), ..Default::default() }).unwrap();
        s.snooze(id, Some(1000)).unwrap();
        let before = s.message(id).unwrap();
        s.db.execute_batch("DROP INDEX messages_later_date; PRAGMA user_version=8;").unwrap();
        drop(s);
        let s = Store::open(dir.path().join("threads.db")).unwrap();
        assert_eq!(s.message(id).unwrap(), before);
        assert_eq!(s.list_conversations(&account, None, 0, 1).unwrap(), [before.summary]);
        assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 9);
    }

    #[test]
    fn conversation_pages_use_the_latest_date_and_id_together() {
        let (_dir, s, account, folder) = fixture();
        let mut ids = Vec::new();
        for (uid, date, thread) in [(1, 100, "a@test"), (2, 100, "b@test"), (3, 90, "a@test")] {
            ids.push(s.save_message(&Message { summary: MessageSummary {
                account: account.clone(), folder, uid, date,
                message_id: format!("{uid}@test"), thread_refs: thread.into(), ..Default::default()
            }, ..Default::default() }).unwrap());
        }
        for view in [Some(folder), None] {
            if view.is_none() { for &id in &ids { s.snooze(id, Some(1000)).unwrap(); } }
            let page = s.list_conversations(&account, view, 0, 1).unwrap();
            let larger = s.list_conversations(&account, view, 0, 2).unwrap();
            assert_eq!(page[0].id, ids[1]);
            assert_eq!(page[0].id, larger[0].id);
            assert_eq!(larger.iter().map(|m| m.id).collect::<Vec<_>>(), [ids[1], ids[0], ids[2]]);
        }
    }

    #[test]
    fn conversation_page_does_not_scan_the_mailbox() {
        let (_dir, s, account, folder) = fixture();
        let id = s.save_message(&Message { summary: MessageSummary {
            account: account.clone(), folder, uid: 1, ..Default::default()
        }, ..Default::default() }).unwrap();
        s.db.execute("WITH RECURSIVE seq(n) AS (VALUES(2) UNION ALL SELECT n+1 FROM seq WHERE n<100000)
            INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,thread_id)
            SELECT account,folder,n,'','','','','','','',n,'',0,0,0,0,'',printf('thread-%d',n/2) FROM messages,seq WHERE id=?1", [id]).unwrap();
        for view in [Some(folder), None] {
            if view.is_none() { s.db.execute("UPDATE messages SET snoozed_until=1000", []).unwrap(); }
            let mut stmt = s.db.prepare(&conversation_sql(view)).unwrap();
            let start = std::time::Instant::now();
            let rows = stmt.query_map(params![account, view, 0, 200], summary_row).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
            eprintln!("100000 messages, 200 conversations, folder={view:?}: {:?}, {steps} VM steps", start.elapsed());
            assert_eq!(rows.len(), 399);
            assert!(steps < 100_000, "a page must not process all 100000 messages: {steps} VM steps");
        }
    }

    #[test]
    fn reply_chains_missing_parents_broken_references_and_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path().join("threads.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("fixture@fastmail.com").unwrap();
        let a = Account::new("fixture@fastmail.com".into(), imap, smtp).unwrap();
        s.save_account(&a).unwrap();
        let f = s.save_folder(&a.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let save = |uid, headers: &str| {
            let mut m = mime::parse(format!("From: sender@example.com\r\nTo: fixture@example.com\r\nSubject: Topic\r\n{headers}\r\nbody").as_bytes()).unwrap();
            m.summary.account = a.id.clone(); m.summary.folder = f; m.summary.uid = uid; m.summary.date = uid as i64;
            s.save_message(&m).unwrap()
        };
        let child = save(1, "Message-ID: <child@test>\r\nReferences: <missing@test>\r\nIn-Reply-To: <parent@test>\r\n");
        let root = save(2, "Message-ID: <root@test>\r\n");
        let parent = save(3, "Message-ID: <parent@test>\r\nIn-Reply-To: <root@test>\r\n");
        let key = s.message(root).unwrap().summary.thread_id;
        for id in [child, parent] { assert_eq!(s.message(id).unwrap().summary.thread_id, key); }
        let other = save(4, "Message-ID: <other@test>\r\n");
        assert_ne!(s.message(other).unwrap().summary.thread_id, key);
        assert_eq!(s.list_conversations(&a.id, Some(f), 0, 1).unwrap().len(), 1);
        assert_eq!(s.list_conversations(&a.id, Some(f), 0, 2).unwrap().len(), 4);
        assert_eq!(s.thread_messages(&a.id, &key).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [child, root, parent]);
        assert!(s.thread_messages("another-account", &key).unwrap().is_empty());
        let mut fallback = Message { summary: MessageSummary { account: a.id.clone(), folder: f, uid: 5, subject: "Re: Topic".into(), from_address: "sender@example.com".into(), to: "fixture@example.com".into(), ..Default::default() }, ..Default::default() };
        let one = s.save_message(&fallback).unwrap();
        fallback.summary.uid = 6; fallback.summary.subject = "Topic".into();
        std::mem::swap(&mut fallback.summary.from_address, &mut fallback.summary.to);
        let two = s.save_message(&fallback).unwrap();
        assert_eq!(s.message(one).unwrap().summary.thread_id, s.message(two).unwrap().summary.thread_id);
        fallback.summary.uid = 7; fallback.summary.from_address = "different@example.com".into();
        let three = s.save_message(&fallback).unwrap();
        assert_ne!(s.message(one).unwrap().summary.thread_id, s.message(three).unwrap().summary.thread_id);
    }
}

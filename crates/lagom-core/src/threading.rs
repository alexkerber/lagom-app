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
            .flat_map(|s| crate::contacts::split_recipients(s))
            .map(|s| s.parse::<lettre::message::Mailbox>().map_or(s.to_lowercase(), |m| m.email.to_string().to_lowercase())).collect();
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
    let (index, thread_index, scope) = if folder.is_some() {
        ("messages_folder_date", "messages_thread_folder", "m.folder=?2 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?3)")
    } else {
        ("messages_later_date", "messages_thread", "m.snoozed_until>?3")
    };
    // The date index visits the latest visible member first; DISTINCT skips the rest without rescanning the thread.
    format!("WITH page AS (
        SELECT DISTINCT m.thread_id FROM messages m INDEXED BY {index} WHERE m.account=?1 AND {scope}
        ORDER BY m.date DESC,m.id DESC LIMIT ?4)
        SELECT {SUMMARY_COLUMNS} FROM page CROSS JOIN messages m INDEXED BY {thread_index} ON m.account=?1 AND m.thread_id=page.thread_id
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
    fn conversation_pages_match_visible_members_across_scopes_and_deadlines() {
        let (_dir, s, account, folder) = fixture();
        let (imap, smtp) = crate::presets::servers("other@fastmail.com").unwrap();
        let other = Account::new("other@fastmail.com".into(), imap, smtp).unwrap();
        s.save_account(&other).unwrap();
        let other_folder = s.save_folder(&other.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let mut ids = Vec::new();
        let mut views = Vec::new();
        for (account, folder) in [(account, folder), (other.id, other_folder)] {
            let archive = s.save_folder(&account, "Archive", None, FolderRole::Archive).unwrap();
            for uid in 1..=48 {
                let id = s.save_message(&Message { summary: MessageSummary {
                    account: account.clone(), folder: if uid % 3 == 0 { archive } else { folder }, uid,
                    date: (uid * 7 % 13) as i64, message_id: format!("{uid}@test"),
                    thread_refs: format!("thread-{}@test", uid % 8), ..Default::default()
                }, ..Default::default() }).unwrap();
                s.snooze(id, [None, Some(50), Some(100), Some(150)][uid as usize / 3 % 4]).unwrap();
                ids.push(id);
            }
            views.push((account, [Some(folder), Some(archive), None]));
        }
        let mut messages: Vec<_> = ids.into_iter().map(|id| s.message(id).unwrap().summary).collect();
        messages.sort_by_key(|m| std::cmp::Reverse((m.date, m.id)));
        for (account, views) in views {
            for view in views {
                for now in [0, 50, 100, 150] {
                    let visible: Vec<_> = messages.iter().filter(|m| m.account == account && match view {
                        Some(folder) => m.folder == folder && m.snoozed_until.is_none_or(|t| t <= now),
                        None => m.snoozed_until.is_some_and(|t| t > now),
                    }).cloned().collect();
                    for limit in [0, 1, 3, 20] {
                        let mut threads = BTreeSet::new();
                        for m in &visible {
                            if threads.len() == limit { break; }
                            threads.insert(&m.thread_id);
                        }
                        let expected: Vec<_> = visible.iter().filter(|m| threads.contains(&m.thread_id)).cloned().collect();
                        assert_eq!(s.list_conversations(&account, view, now, limit as u32).unwrap(), expected,
                            "folder={view:?}, now={now}, limit={limit}");
                    }
                }
            }
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
    fn conversation_page_does_not_rescan_newer_messages_in_other_folders() {
        let (_dir, s, account, folder) = fixture();
        let archive = s.save_folder(&account, "Archive", None, FolderRole::Archive).unwrap();
        let id = s.save_message(&Message { summary: MessageSummary {
            account: account.clone(), folder, uid: 1, date: 1, message_id: "root@test".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
        s.db.execute("WITH RECURSIVE seq(n) AS (VALUES(2) UNION ALL SELECT n+1 FROM seq WHERE n<4000)
            INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,thread_id)
            SELECT account,CASE WHEN n<=2000 THEN folder ELSE ?2 END,n,'','','','','','','',n,'',0,0,0,0,'',thread_id FROM messages,seq WHERE id=?1", params![id, archive]).unwrap();
        let mut stmt = s.db.prepare(&conversation_sql(Some(folder))).unwrap();
        let start = std::time::Instant::now();
        let rows = stmt.query_map(params![account, folder, 0, 200], summary_row).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
        eprintln!("2000 Inbox + 2000 newer Archive messages in one thread: {:?}, {steps} VM steps", start.elapsed());
        assert_eq!(rows.len(), 2000);
        assert!(rows.iter().all(|m| m.folder == folder));
        assert_eq!(rows.first().unwrap().date, 2000);
        assert_eq!(rows.last().unwrap().id, id);
        assert!(steps < 500_000, "newer messages in another folder must not be scanned for every candidate: {steps} VM steps");
    }

    #[test]
    fn later_page_does_not_rescan_newer_unsnoozed_messages() {
        mixed_snooze_page(false);
    }

    #[test]
    fn inbox_page_does_not_rescan_newer_snoozed_messages() {
        mixed_snooze_page(true);
    }

    fn mixed_snooze_page(inbox: bool) {
        let (_dir, s, account, folder) = fixture();
        let archive = s.save_folder(&account, "Archive", None, FolderRole::Archive).unwrap();
        let view = inbox.then_some(folder);
        let id = s.save_message(&Message { summary: MessageSummary {
            account: account.clone(), folder, uid: 1, date: 1, message_id: "root@test".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
        s.db.execute("WITH RECURSIVE seq(n) AS (VALUES(2) UNION ALL SELECT n+1 FROM seq WHERE n<4000)
            INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,thread_id)
            SELECT account,CASE WHEN n<=2000 THEN folder ELSE ?2 END,n,'','','','','','','',n,'',0,0,0,0,'',thread_id FROM messages,seq WHERE id=?1", params![id, view.unwrap_or(archive)]).unwrap();
        s.db.execute("UPDATE messages SET snoozed_until=CASE WHEN (date<=2000)=?1 THEN 1000 END", [view.is_none()]).unwrap();
        let mut stmt = s.db.prepare(&conversation_sql(view)).unwrap();
        let start = std::time::Instant::now();
        let rows = stmt.query_map(params![account, view, 0, 200], summary_row).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
        eprintln!("2000 visible + 2000 newer hidden messages, folder={view:?}: {:?}, {steps} VM steps", start.elapsed());
        assert_eq!(rows.len(), 2000);
        assert!(rows.iter().all(|m| m.folder == folder));
        assert_eq!(rows.first().unwrap().date, 2000);
        assert_eq!(rows.last().unwrap().id, id);
        assert!(steps < 500_000, "hidden messages must not be scanned for every candidate: {steps} VM steps");
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
        fallback.summary.uid = 8;
        fallback.summary.to = "\"Team, West\" <sender@example.com>".into();
        let named = s.save_message(&fallback).unwrap();
        assert_eq!(s.message(named).unwrap().summary.thread_id, s.message(one).unwrap().summary.thread_id);
        fallback.summary.uid = 7; fallback.summary.from_address = "different@example.com".into();
        let three = s.save_message(&fallback).unwrap();
        assert_ne!(s.message(one).unwrap().summary.thread_id, s.message(three).unwrap().summary.thread_id);
    }
}

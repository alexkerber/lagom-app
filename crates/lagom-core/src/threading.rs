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
        let mut stmt = self.db.prepare(&format!("WITH visible AS (
            SELECT {SUMMARY_COLUMNS} FROM messages m WHERE account=?1 AND
            ((?2 IS NULL AND snoozed_until>?3) OR (folder=?2 AND (snoozed_until IS NULL OR snoozed_until<=?3)))
        ), page AS (SELECT thread_id FROM visible GROUP BY thread_id ORDER BY MAX(date) DESC,MAX(id) DESC LIMIT ?4)
        SELECT {SUMMARY_COLUMNS} FROM visible m JOIN page USING(thread_id) ORDER BY m.date DESC,m.id DESC"))?;
        let rows = stmt.query_map(params![account, folder, now, limit], summary_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mime, account::Account, store::{FolderRole, Message}};

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

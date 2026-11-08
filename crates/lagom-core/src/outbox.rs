//! Local drafts and durable sends. Network work starts only after the queue transaction commits.

use crate::{account::Account, smtp::{self, Outgoing}, store::Store, Error, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub account: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub text: String,
    pub reply: Option<(Option<String>, Vec<String>)>,
}

pub struct Pending {
    pub id: String,
    pub account: String,
    pub mail: Outgoing,
    pub raw: Vec<u8>,
    pub submitted: bool,
    pub error: Option<String>,
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| Error::Invalid("could not encode local mail"))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| Error::Invalid("could not read local mail"))
}

impl Store {
    pub fn save_draft(&self, draft: &Draft) -> Result<()> {
        self.db.execute("INSERT INTO drafts(id,account,content) VALUES (?1,?2,?3)
            ON CONFLICT(id) DO UPDATE SET content=excluded.content WHERE account=excluded.account AND content<>excluded.content",
            params![draft.id, draft.account, encode(draft)?])?;
        Ok(())
    }

    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let mut stmt = self.db.prepare("SELECT content FROM drafts ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| decode(&r?)).collect()
    }

    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM drafts WHERE id=?1", [id])?;
        Ok(())
    }

    /// `ready_at` is Unix milliseconds: the Undo window survives a restart too.
    pub fn enqueue(&self, account: &Account, mail: &Outgoing, ready_at: i64) -> Result<String> {
        let mut mail = mail.clone();
        let id = mail.message_id.get_or_insert_with(|| smtp::message_id(&account.email)).clone();
        let raw = smtp::compose(account, &mail)?.formatted();
        let tx = self.db.unchecked_transaction()?;
        tx.execute("INSERT INTO outbox(id,account,content,raw,draft,ready_at)
            VALUES (?1,?2,?3,?4,(SELECT content FROM drafts WHERE id=?1 AND account=?2),?5)",
            params![id, account.id, encode(&mail)?, raw, ready_at])?;
        tx.execute("DELETE FROM drafts WHERE id=?1 AND account=?2", params![id, account.id])?;
        tx.commit()?;
        Ok(id)
    }

    /// Undo cannot race an SMTP submission: claiming a row and cancelling it are exclusive.
    pub fn undo_send(&self, id: &str) -> Result<bool> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("INSERT INTO drafts(id,account,content)
            SELECT id,account,draft FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
        let removed = tx.execute("DELETE FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
        tx.commit()?;
        Ok(removed == 1)
    }

    pub fn pending(&self, id: &str) -> Result<Pending> {
        let (account, content, raw, submitted, error) = self.db.query_row(
            "SELECT account,content,raw,submitted,error FROM outbox WHERE id=?1", [id],
            |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).optional()?.ok_or(Error::NotFound)?;
        Ok(Pending { id: id.into(), account, mail: decode(&content)?, raw, submitted, error })
    }

    pub fn due_outbox(&self, account: &str, now: i64) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare("SELECT id FROM outbox WHERE account=?1 AND started=0 AND ready_at<=?2 ORDER BY ready_at,rowid")?;
        let rows = stmt.query_map(params![account, now], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn claim_send(&self, id: &str, now: i64) -> Result<bool> {
        Ok(self.db.execute("UPDATE outbox SET started=1 WHERE id=?1 AND started=0 AND ready_at<=?2", params![id, now])? == 1)
    }

    // shortcut: one mail worker per store; add a process lease before supporting concurrent app instances.
    /// Only the worker starting up calls this; in-flight sends belong to its previous process.
    pub fn recover_outbox(&self) -> Result<()> {
        self.db.execute("UPDATE outbox SET started=0 WHERE started=1", [])?;
        Ok(())
    }

    pub(crate) fn submitted(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE outbox SET submitted=1 WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn send_failed(&self, id: &str, error: &str) -> Result<()> {
        self.db.execute("UPDATE outbox SET started=0,error=?2 WHERE id=?1", params![id, error])?;
        Ok(())
    }

    pub(crate) fn sent(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM outbox WHERE id=?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_draft_queue_undo_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql")] {
            db.execute_batch(schema).unwrap();
        }
        drop(db);
        let store = Store::open(&path).unwrap();
        let (imap, smtp) = crate::presets::servers("sender@gmail.com").unwrap();
        let account = Account::new("sender@example.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let mut draft = Draft { id: smtp::message_id(&account.email), account: account.id.clone(), to: "unfinished, ".into(), text: "åäö\n**draft**".into(), reply: Some((Some("<parent@example.com>".into()), vec!["<root@example.com>".into()])), ..Default::default() };
        store.save_draft(&draft).unwrap();
        let writes = store.db.total_changes();
        store.save_draft(&draft).unwrap();
        assert_eq!(store.db.total_changes(), writes);
        draft.cc = "another unfinished address".into();
        store.save_draft(&draft).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.drafts().unwrap(), [draft.clone()]);
        let mail = Outgoing { to: vec!["recipient@example.com".into()], bcc: vec!["hidden@example.com".into()], text: draft.text.clone(), message_id: Some(draft.id.clone()), ..Default::default() };
        let id = store.enqueue(&account, &mail, 5000).unwrap();
        assert!(store.drafts().unwrap().is_empty());
        assert!(store.due_outbox(&account.id, 4999).unwrap().is_empty());
        assert_eq!(store.pending(&id).unwrap().mail, mail);
        assert!(!String::from_utf8_lossy(&store.pending(&id).unwrap().raw).contains("hidden@example.com"));
        assert!(store.undo_send(&id).unwrap());
        assert_eq!(store.drafts().unwrap(), [draft]);
        store.enqueue(&account, &mail, 5000).unwrap();
        assert!(!store.claim_send(&id, 4999).unwrap());
        assert!(store.claim_send(&id, 5000).unwrap());
        assert!(!store.claim_send(&id, 5000).unwrap());
        assert!(!store.undo_send(&id).unwrap());
        store.submitted(&id).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        store.recover_outbox().unwrap();
        assert!(store.pending(&id).unwrap().submitted);
        assert!(!store.undo_send(&id).unwrap());
        assert_eq!(store.due_outbox(&account.id, 5000).unwrap(), [id]);
        store.delete_account(&account.id).unwrap();
        assert!(store.due_outbox(&account.id, 5000).unwrap().is_empty());
    }

    #[test]
    fn invalid_send_keeps_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("mail.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("sender@gmail.com").unwrap();
        let account = Account::new("sender@example.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let draft = Draft { id: smtp::message_id(&account.email), account: account.id.clone(), ..Default::default() };
        store.save_draft(&draft).unwrap();
        assert!(store.enqueue(&account, &Outgoing { message_id: Some(draft.id.clone()), ..Default::default() }, 0).is_err());
        assert_eq!(store.drafts().unwrap(), [draft.clone()]);
        store.delete_draft(&draft.id).unwrap();
        assert!(store.drafts().unwrap().is_empty());
    }
}

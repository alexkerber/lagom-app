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
    pub copied: bool,
    pub state: String,
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
        self.db.execute("INSERT INTO drafts(id,account,content,revision) VALUES (?1,?2,?3,lower(hex(randomblob(16))))
            ON CONFLICT(id) DO UPDATE SET content=excluded.content,revision=excluded.revision WHERE account=excluded.account",
            params![draft.id, draft.account, encode(draft)?])?;
        Ok(())
    }

    pub fn draft_revision(&self, id: &str) -> Result<String> {
        Ok(self.db.query_row("SELECT revision FROM drafts WHERE id=?1", [id], |r| r.get(0))?)
    }

    /// A late autosave cannot overwrite a newer save, Undo, or a deleted draft.
    pub fn autosave_draft(&self, draft: &Draft, revision: &str) -> Result<()> {
        self.db.execute("UPDATE drafts SET content=?3,revision=lower(hex(randomblob(16)))
            WHERE id=?1 AND account=?2 AND revision=?4", params![draft.id, draft.account, encode(draft)?, revision])?;
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
        tx.execute("INSERT INTO drafts(id,account,content,revision)
            SELECT id,account,draft,lower(hex(randomblob(16))) FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
        let removed = tx.execute("DELETE FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
        tx.commit()?;
        Ok(removed == 1)
    }

    pub fn pending(&self, id: &str) -> Result<Pending> {
        let (account, content, raw, submitted, copied, state, error) = self.db.query_row(
            "SELECT account,content,raw,submitted,copied,state,error FROM outbox WHERE id=?1", [id],
            |r| Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        ).optional()?.ok_or(Error::NotFound)?;
        Ok(Pending { id: id.into(), account, mail: decode(&content)?, raw, submitted, copied, state, error })
    }

    pub fn due_outbox(&self, account: &str, now: i64) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare("SELECT id FROM outbox WHERE account=?1 AND started=0 AND state='ready' AND ready_at<=?2 ORDER BY ready_at,rowid")?;
        let rows = stmt.query_map(params![account, now], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn claim_send(&self, id: &str, now: i64) -> Result<bool> {
        Ok(self.db.execute("UPDATE outbox SET started=1 WHERE id=?1 AND started=0 AND state='ready' AND ready_at<=?2", params![id, now])? == 1)
    }

    // shortcut: one mail worker per store; add a process lease before supporting concurrent app instances.
    /// Only the worker starting up calls this; in-flight sends belong to its previous process.
    pub fn recover_outbox(&self) -> Result<()> {
        self.db.execute("UPDATE outbox SET started=0,state=CASE WHEN submitted=0 THEN 'uncertain' ELSE 'ready' END,error=CASE WHEN submitted=0 THEN 'Delivery uncertain after interruption' ELSE error END WHERE started=1", [])?;
        Ok(())
    }

    pub(crate) fn submitted(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE outbox SET submitted=1 WHERE id=?1", [id])?;
        Ok(())
    }

    pub(crate) fn copied(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE outbox SET copied=1 WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn send_failed(&self, id: &str, error: &Error, now: i64) -> Result<()> {
        let state = match error {
            Error::MaybeSent => "uncertain",
            Error::Rejected | Error::Authentication | Error::ReauthRequired | Error::Invalid(_) => "failed",
            _ => "ready",
        };
        self.db.execute("UPDATE outbox SET started=0,error=?2,state=?3,
            ready_at=?4+min(60000*(1 << min(attempts,6)),3600000),attempts=attempts+1
            WHERE id=?1 AND started=1", params![id, error.to_string(), state, now])?;
        Ok(())
    }

    /// Only a deliberate user action re-enables an uncertain or permanently refused send.
    pub fn retry_send(&self, id: &str, now: i64) -> Result<bool> {
        Ok(self.db.execute("UPDATE outbox SET state='ready',ready_at=?2,error=NULL
            WHERE id=?1 AND started=0 AND submitted=0 AND error IS NOT NULL", params![id, now])? == 1)
    }

    pub fn failed_outbox(&self) -> Result<Vec<Pending>> {
        let mut stmt = self.db.prepare("SELECT id FROM outbox WHERE started=0 AND submitted=0 AND (error IS NOT NULL OR state<>'ready') ORDER BY rowid")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().map(|id| self.pending(id)).collect()
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
        let revision = store.draft_revision(&draft.id).unwrap();
        store.save_draft(&draft).unwrap();
        assert_ne!(store.draft_revision(&draft.id).unwrap(), revision);
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

    fn queued() -> (tempfile::TempDir, Store, Account, String) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("mail.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("sender@gmail.com").unwrap();
        let account = Account::new("sender@example.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let draft = Draft { id: smtp::message_id(&account.email), account: account.id.clone(), to: "recipient@example.com".into(), text: "keep me".into(), ..Default::default() };
        store.save_draft(&draft).unwrap();
        let id = store.enqueue(&account, &Outgoing { to: vec![draft.to], text: draft.text, message_id: Some(draft.id), ..Default::default() }, 0).unwrap();
        (dir, store, account, id)
    }

    #[test]
    fn interrupted_submission_never_resends_on_recovery() {
        let (dir, store, account, id) = queued();
        assert!(store.claim_send(&id, 0).unwrap());
        drop(store);
        let store = Store::open(dir.path().join("mail.db")).unwrap();
        store.recover_outbox().unwrap();
        assert!(store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
        assert!(!store.claim_send(&id, i64::MAX).unwrap());
    }

    #[test]
    fn failed_send_waits_before_retrying() {
        let (_dir, store, account, id) = queued();
        assert!(store.claim_send(&id, 0).unwrap());
        store.send_failed(&id, &Error::Network("temporary refusal"), 0).unwrap();
        assert!(store.due_outbox(&account.id, 0).unwrap().is_empty());
    }

    #[test]
    fn retry_backoff_doubles_and_caps_at_one_hour() {
        let (_dir, store, account, id) = queued();
        let mut now = 0;
        for delay in [60000, 120000, 240000, 480000, 960000, 1920000, 3600000, 3600000] {
            assert!(store.claim_send(&id, now).unwrap());
            store.send_failed(&id, &Error::Network("temporary refusal"), now).unwrap();
            // The worker also records errors; a second recording must not advance attempts.
            store.send_failed(&id, &Error::Network("temporary refusal"), now).unwrap();
            assert!(store.due_outbox(&account.id, now + delay - 1).unwrap().is_empty());
            now += delay;
            assert_eq!(store.due_outbox(&account.id, now).unwrap(), [id.clone()]);
        }
    }

    #[test]
    fn failed_or_uncertain_send_returns_atomically_to_editable_draft() {
        for error in [Error::Rejected, Error::MaybeSent] {
            let (_dir, store, account, id) = queued();
            assert!(store.claim_send(&id, 0).unwrap());
            assert!(!store.undo_send(&id).unwrap());
            store.send_failed(&id, &error, 0).unwrap();
            assert_eq!(store.failed_outbox().unwrap().len(), 1);
            assert!(store.undo_send(&id).unwrap());
            assert!(matches!(store.pending(&id), Err(Error::NotFound)));
            assert_eq!(store.drafts().unwrap()[0].text, "keep me");
            assert!(store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
            assert!(!store.retry_send(&id, 0).unwrap());
        }
    }

    #[test]
    fn late_autosave_cannot_overwrite_save_send_undo_or_discard() {
        let (_dir, store, account, id) = queued();
        assert!(store.undo_send(&id).unwrap());
        let mut draft = store.drafts().unwrap().remove(0);
        let revision = store.draft_revision(&id).unwrap();
        let old = draft.clone();
        draft.text = "latest text".into();
        store.save_draft(&draft).unwrap();
        store.autosave_draft(&old, &revision).unwrap();
        assert_eq!(store.drafts().unwrap(), [draft.clone()]);
        let revision = store.draft_revision(&id).unwrap();
        let mail = Outgoing { to: vec![draft.to.clone()], text: draft.text.clone(), message_id: Some(id.clone()), ..Default::default() };
        store.enqueue(&account, &mail, 0).unwrap();
        store.autosave_draft(&old, &revision).unwrap();
        assert!(store.drafts().unwrap().is_empty());
        assert!(store.undo_send(&id).unwrap());
        store.autosave_draft(&old, &revision).unwrap();
        assert_eq!(store.drafts().unwrap(), [draft.clone()]);
        let revision = store.draft_revision(&id).unwrap();
        store.delete_draft(&id).unwrap();
        store.autosave_draft(&old, &revision).unwrap();
        assert!(store.drafts().unwrap().is_empty());
    }

    #[test]
    fn v5_migration_preserves_mail_and_stops_ambiguous_retries() {
        let dir = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(dir.path().join("mail.db")).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql")] {
            db.execute_batch(schema).unwrap();
        }
        let store = Store { db };
        let (imap, smtp) = crate::presets::servers("sender@gmail.com").unwrap();
        let account = Account::new("sender@example.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let draft = Draft { id: "interrupted".into(), account: account.id.clone(), text: "keep me".into(), ..Default::default() };
        let mail = Outgoing { to: vec!["recipient@example.com".into()], ..Default::default() };
        let interrupted = "interrupted";
        let failed = "failed";
        let submitted = "submitted";
        for id in [interrupted, failed, submitted] {
            store.db.execute("INSERT INTO outbox(id,account,content,raw,draft,ready_at,started,submitted,error)
                VALUES (?1,?2,?3,?4,?5,0,?6,?7,?8)",
                params![id, account.id, encode(&mail).unwrap(), b"fixture MIME".as_slice(), encode(&draft).unwrap(),
                    id == interrupted, id == submitted, (id == failed).then_some("may have been sent")]).unwrap();
        }
        drop(store);
        let store = Store::open(dir.path().join("mail.db")).unwrap();
        store.recover_outbox().unwrap();
        assert_eq!(store.pending(&interrupted).unwrap().state, "uncertain");
        assert_eq!(store.pending(&failed).unwrap().state, "uncertain");
        assert_eq!(store.due_outbox(&account.id, 0).unwrap(), [submitted]);
        assert!(store.undo_send(&interrupted).unwrap());
        assert_eq!(store.drafts().unwrap()[0].text, "keep me");
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

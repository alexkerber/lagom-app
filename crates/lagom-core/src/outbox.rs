//! Local drafts and durable sends. Network work starts only after the queue transaction commits.

use crate::{account::Account, smtp::{self, Outgoing}, store::Store, Error, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub account: String,
    pub to: String,
    pub cc: String,
    #[serde(default)]
    pub bcc: String,
    // Keep separately entered invalid tokens from combining into a valid mailbox on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_tokens: Option<[Vec<String>; 2]>,
    pub subject: String,
    pub text: String,
    pub reply: Option<(Option<String>, Vec<String>)>,
    // shortcut: attachment bytes share draft JSON; use blob references when large drafts make autosave costly.
    #[serde(default)]
    pub attachments: Vec<crate::smtp::OutgoingAttachment>,
    #[serde(default)]
    pub forward_source: Option<i64>,
    /// An HTML original, kept out of `text`. Older drafts have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted: Option<crate::compose::Quoted>,
}

pub struct DraftSummary {
    pub agent: Option<String>,
    pub id: String,
    pub account: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
}

pub struct OwnedDraft {
    pub id: String,
    pub account: String,
    pub to: String,
    pub subject: String,
    pub updated: i64,
    pub in_reply_to: Option<String>,
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

const REVOKED: Error = Error::Invalid("Agent access revoked. Send cancelled.");

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
            WHERE id=?1 AND account=?2 AND revision=?4 AND content<>?3", params![draft.id, draft.account, encode(draft)?, revision])?;
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
        self.enqueue_with_origin(account, mail, ready_at, None)
    }

    pub fn enqueue_agent(&self, who: &crate::agent::token::Identity, account: &Account, mail: &Outgoing, ready_at: i64) -> Result<String> {
        self.enqueue_with_origin(account, mail, ready_at, Some(who))
    }

    fn enqueue_with_origin(&self, account: &Account, mail: &Outgoing, ready_at: i64, origin: Option<&crate::agent::token::Identity>) -> Result<String> {
        let mut mail = mail.clone();
        let id = mail.message_id.get_or_insert_with(|| smtp::message_id(&account.email)).clone();
        let raw = smtp::compose_message(account, &mail, None, origin.is_some())?.formatted();
        let tx = self.db.is_autocommit().then(|| self.write_transaction()).transpose()?;
        if origin.is_none() {
            let owner: Option<String> = self.db.query_row("DELETE FROM outbox WHERE id=?1 AND account=?2
                AND draft_agent IS NOT NULL AND started=0 AND submitted=0 AND state<>'uncertain'
                RETURNING draft_agent", params![id, account.id], |row| row.get(0)).optional()?;
            if let Some(agent) = owner { self.audit_queue_change(&id, &agent, &account.id, "superseded")?; }
        }
        self.db.execute("INSERT INTO outbox(id,account,content,raw,draft,ready_at,draft_agent,agent_token,agent_policy_generation)
            VALUES (?1,?2,?3,?4,(SELECT content FROM drafts WHERE id=?1 AND account=?2),?5,?6,?7,
                CASE WHEN ?6 IS NOT NULL THEN (SELECT CAST(value AS INTEGER) FROM agent_settings WHERE key='policy_generation') END)",
            params![id, account.id, encode(&mail)?, raw, ready_at, origin.map(|i| i.agent_id()), origin.map(|i| i.token_id())])?;
        self.db.execute("DELETE FROM drafts WHERE id=?1 AND account=?2", params![id, account.id])?;
        if let Some(tx) = tx { tx.commit()?; }
        Ok(id)
    }

    /// Undo cannot race an SMTP submission: claiming a row and cancelling it are exclusive.
    pub fn undo_send(&self, id: &str) -> Result<bool> {
        let tx = self.db.unchecked_transaction()?;
        let removed = self.restore_queued_draft(id)?;
        tx.commit()?;
        Ok(removed)
    }

    fn restore_queued_draft(&self, id: &str) -> Result<bool> {
        if self.db.execute("DELETE FROM outbox WHERE id=?1 AND started=0 AND submitted=0
            AND EXISTS(SELECT 1 FROM drafts WHERE id=?1)", [id])? == 1 {
            return Ok(true);
        }
        self.db.execute("INSERT INTO drafts(id,account,content,revision)
            SELECT id,account,draft,lower(hex(randomblob(16))) FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
        self.db.execute("INSERT INTO agent_drafts(draft,agent) SELECT id,draft_agent FROM outbox
            WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL AND draft_agent IS NOT NULL", [id])?;
        let removed = self.db.execute("DELETE FROM outbox WHERE id=?1 AND started=0 AND submitted=0 AND draft IS NOT NULL", [id])?;
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
        let mut tx = self.write_transaction()?;
        let agent: Option<bool> = tx.query_row("UPDATE outbox SET started=1
            WHERE id=?1 AND started=0 AND state='ready' AND ready_at<=?2
            RETURNING draft_agent IS NOT NULL AND submitted=0", params![id, now], |r| r.get(0)).optional()?;
        let revoked = agent == Some(true) && self.restore_revoked_send(&mut tx, id)?;
        tx.commit()?;
        if revoked { return Err(REVOKED); }
        Ok(agent.is_some())
    }

    pub(crate) fn authorize_queued_send(&self, id: &str) -> Result<()> {
        let mut tx = self.write_transaction()?;
        let revoked = self.restore_revoked_send(&mut tx, id)?;
        tx.commit()?;
        if revoked { return Err(REVOKED); }
        Ok(())
    }

    fn restore_revoked_send(&self, tx: &mut Transaction<'_>, id: &str) -> Result<bool> {
        let (agent, account, token, current): (Option<String>, String, Option<String>, bool) = self.db.query_row(
            "SELECT draft_agent,account,agent_token,coalesce(agent_policy_generation =
                (SELECT CAST(value AS INTEGER) FROM agent_settings WHERE key='policy_generation'),0) FROM outbox WHERE id=? AND submitted=0", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        if let Some(agent) = agent {
            if !current || !self.agent_may_send(&agent, token.as_deref(), &account)? {
                self.db.execute("UPDATE outbox SET started=0 WHERE id=?", [id])?;
                self.return_revoked_draft(tx, id, &agent, &account)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn return_revoked_draft(&self, tx: &mut Transaction<'_>, id: &str, agent: &str, account: &str) -> Result<()> {
        let mut row = tx.savepoint()?;
        let restored = self.restore_queued_draft(id).and_then(|removed| {
            if !removed { return Err(Error::Invalid("missing queued draft")); }
            self.audit_queue_change(id, agent, account, "revoked")
        });
        if let Err(error) = restored {
            row.rollback()?;
            row.execute("DELETE FROM outbox WHERE id=?1", [id])?;
            eprintln!("Could not restore revoked agent send {id}: {error}");
        }
        row.commit()?;
        Ok(())
    }

    fn audit_queue_change(&self, id: &str, agent: &str, account: &str, result: &str) -> Result<()> {
        let target = crate::agent::id::ObjectId::Draft(account.to_owned(), id.into()).encode();
        self.db.execute("INSERT INTO agent_audit(at,agent,account,command,target,result,dispatched_write)
            VALUES(unixepoch(),?,?,'draft.send',?,?,1)", params![agent, account, target, result])?;
        Ok(())
    }

    pub(crate) fn cleanup_revoked_queue(&self) {
        // Authority is already committed. SMTP checks the generation independently.
        let cleanup = || -> Result<()> {
            let mut tx = self.write_transaction()?;
            self.return_agent_queue(&mut tx)?;
            Ok(tx.commit()?)
        };
        if let Err(error) = cleanup() {
            eprintln!("Could not restore revoked agent sends: {error}");
        }
    }

    fn return_agent_queue(&self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut stmt = self.db.prepare("SELECT id,draft_agent,account FROM outbox
            WHERE started=0 AND submitted=0 AND state<>'uncertain' AND draft_agent IS NOT NULL
              AND (agent_token IS NULL OR NOT EXISTS (SELECT 1 FROM agent_settings WHERE key='policy_generation' AND CAST(value AS INTEGER)=agent_policy_generation))")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, owner, account) in rows {
            self.return_revoked_draft(tx, &id, &owner, &account)?;
        }
        Ok(())
    }

    // shortcut: one mail worker per store; add a process lease before supporting concurrent app instances.
    /// Only the worker starting up calls this; in-flight sends belong to its previous process.
    pub fn recover_outbox(&self) -> Result<()> {
        let mut tx = self.write_transaction()?;
        self.db.execute("UPDATE outbox SET started=0,state=CASE WHEN submitted=0 THEN 'uncertain' ELSE 'ready' END,error=CASE WHEN submitted=0 THEN 'Delivery uncertain after interruption' ELSE error END WHERE started=1", [])?;
        self.return_agent_queue(&mut tx)?;
        tx.commit()?;
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
        let mut tx = self.write_transaction()?;
        let agent: Option<bool> = tx.query_row("UPDATE outbox SET started=0,error=?2,state=?3,
            ready_at=?4+min(60000*(1 << min(attempts,6)),3600000),attempts=attempts+1
            WHERE id=?1 AND started=1 RETURNING submitted=0 AND state<>'uncertain' AND draft_agent IS NOT NULL",
            params![id, error.to_string(), state, now], |r| r.get(0)).optional()?;
        // Revocation may have skipped this row while the worker held its claim.
        if agent == Some(true) { self.restore_revoked_send(&mut tx, id)?; }
        tx.commit()?;
        Ok(())
    }

    /// Reauthorization resumes only sends stopped by a revoked grant, never uncertain delivery.
    pub fn resume_outbox(&self, account: &str, now: i64) -> Result<()> {
        // shortcut: outbox failures have text reasons; migrate them before changing the reauthorization message.
        self.db.execute("UPDATE outbox SET state='ready',ready_at=?2,error=NULL
            WHERE account=?1 AND started=0 AND state='failed' AND error=?3",
            params![account, now, Error::ReauthRequired.to_string()])?;
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
        let agent = store.create_agent("test", "owner", "host", false).unwrap();
        store.db.execute("INSERT INTO agent_drafts VALUES(?,?)", params![draft.id, agent]).unwrap();
        let id = store.enqueue(&account, &mail, 5000).unwrap();
        assert!(store.drafts().unwrap().is_empty());
        assert!(store.due_outbox(&account.id, 4999).unwrap().is_empty());
        assert_eq!(store.pending(&id).unwrap().mail, mail);
        assert!(!String::from_utf8_lossy(&store.pending(&id).unwrap().raw).contains("hidden@example.com"));
        assert!(store.undo_send(&id).unwrap());
        assert_eq!(store.drafts().unwrap(), [draft]);
        assert!(store.agent_draft_meta(&account.id,&id).unwrap().1.is_none());
        store.db.execute("INSERT INTO agent_drafts VALUES(?,?)", params![id, agent]).unwrap();
        store.set_agent_access(&agent, &crate::agent::policy::Access { account:account.id.clone(),
            resource:crate::agent::policy::Resource::Mail,level:crate::agent::policy::Level::Triage }).unwrap();
        store.enqueue(&account, &mail, 5000).unwrap();
        assert!(!store.claim_send(&id, 4999).unwrap());
        assert!(store.claim_send(&id, 5000).unwrap());
        store.authorize_queued_send(&id).unwrap();
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

    use crate::agent::{policy::{Access, Level, Resource, Setting}, token::TokenScope};

    fn queued_agent(remote: bool) -> (tempfile::TempDir, Store, Account, String, String, zeroize::Zeroizing<String>, Access) {
        let (_dir, store, account, id) = queued();
        let agent = store.create_agent("test", "sender", "host", remote).unwrap();
        let token = store.issue_token(&agent, if remote {
            TokenScope::Remote { accounts:std::slice::from_ref(&account.id), level:Level::Send, ranges:&[], expires:None }
        } else { TokenScope::Local }).unwrap();
        store.set_agent_setting(Setting::Enabled,true).unwrap();
        let grant = Access { resource:Resource::Mail, account:account.id.clone(), level:Level::Send };
        for owner in ["", &agent] { store.set_agent_access(owner, &grant).unwrap(); }
        assert!(store.undo_send(&id).unwrap());
        store.set_draft_agent(&id,&store.authenticate(Some(&token),remote.then_some("127.0.0.1")).unwrap()).unwrap();
        let mail = Outgoing { to:vec!["a@example.org".into()], message_id:Some(id.clone()), ..Default::default() };
        store.enqueue_agent(&store.authenticate(Some(&token),remote.then_some("127.0.0.1")).unwrap(),&account,&mail,0).unwrap();
        (_dir, store, account, id, agent, token, grant)
    }

    #[test]
    fn undo_preserves_a_compose_resave() {
        let (_dir, store, account, id, ..) = queued_agent(false);
        let personal = Draft { id:id.clone(), account:account.id.clone(), text:"My edits".into(), ..Default::default() };
        store.save_draft(&personal).unwrap();
        assert!(store.undo_send(&id).unwrap());
        assert_eq!(store.draft(&id).unwrap(), personal);
        assert_eq!(store.agent_draft_meta(&account.id, &id).unwrap().1, None);
        assert!(!store.claim_send(&id, i64::MAX).unwrap());
    }

    #[test]
    fn compose_send_supersedes_queued_agent_mail() {
        let (_dir, store, account, id, agent, ..) = queued_agent(false);
        let personal = Draft { id:id.clone(), account:account.id.clone(), text:"My version".into(), ..Default::default() };
        store.save_draft(&personal).unwrap();
        let mail = Outgoing { message_id:Some(id.clone()), text:personal.text.clone(), to:vec!["me@example.org".into()], ..Default::default() };
        store.enqueue(&account, &mail, 0).unwrap();
        store.revoke_agent_tokens(&agent).unwrap();
        assert_eq!(store.due_outbox(&account.id, 0).unwrap(), [id.clone()]);
        assert_eq!(store.pending(&id).unwrap().mail, mail);
        assert_eq!(store.agent_audit(1).unwrap()[0].result, "superseded");
        assert!(store.undo_send(&id).unwrap());
        assert_eq!(store.draft(&id).unwrap(), personal);
    }

    #[test]
    fn compose_resave_survives_agent_revocation() {
        let (_dir, store, account, id, agent, token, _) = queued_agent(false);
        let personal = Draft { id:id.clone(), account:account.id.clone(), text:"My new draft".into(), ..Default::default() };
        store.save_draft(&personal).unwrap();
        store.revoke_agent_tokens(&agent).unwrap();
        assert!(store.authenticate(Some(&token),None).is_err());
        assert_eq!(store.draft(&id).unwrap(), personal);
        assert!(store.agent_draft_meta(&account.id,&id).unwrap().1.is_none());
        assert!(!store.claim_send(&id,i64::MAX).unwrap());
        assert!(store.pending(&id).is_err());
        assert_eq!(store.agent_audit(1).unwrap()[0].result,"revoked");
    }

    #[test]
    fn cleanup_failure_cannot_reverse_an_authority_change() {
        for action in ["revoke","disable","all","reissue"] {
            let (_dir, store, account, id, agent, token, _) = queued_agent(false);
            let mut other = store.pending(&id).unwrap().mail;
            other.message_id = Some("other".into());
            store.save_draft(&Draft { id:"other".into(), account:account.id.clone(), ..Default::default() }).unwrap();
            store.enqueue_agent(&store.authenticate(Some(&token), None).unwrap(), &account, &other, 0).unwrap();
            store.db.execute_batch("CREATE TRIGGER refuse_restore BEFORE INSERT ON agent_drafts WHEN NEW.draft<>'other' BEGIN
                SELECT RAISE(ABORT,'injected restore failure'); END").unwrap();
            match action {
                "disable" => store.disable_agents().unwrap(),
                "all" => store.revoke_all_agents().unwrap(),
                "reissue" => { store.issue_token(&agent,TokenScope::Local).unwrap(); }
                _ => store.revoke_agent_tokens(&agent).unwrap(),
            }
            if action == "disable" { assert!(!store.agent_enabled().unwrap()); }
            else { assert!(store.authenticate(Some(&token),None).is_err()); }
            assert!(!store.claim_send(&id,0).unwrap());
            assert!(store.draft(&id).is_err());
            assert!(store.pending("other").is_err());
            assert_eq!(store.agent_draft_meta(&account.id, "other").unwrap().1, Some(agent));
        }
    }

    #[test]
    fn failed_send_is_restored_on_policy_change_or_startup() {
        for change in ["lower", "issue", "startup"] {
            let (_dir, store, account, id, agent, _, grant) = queued_agent(change == "issue");
            assert!(store.claim_send(&id, 0).unwrap());
            store.send_failed(&id, &Error::Rejected, 0).unwrap();
            match change {
                "lower" => store.set_agent_access("", &Access { level:Level::Read, ..grant }).unwrap(),
                "issue" => {
                    store.issue_token(&agent, TokenScope::Remote { accounts:std::slice::from_ref(&account.id),
                        level:Level::Read, ranges:&[], expires:None }).unwrap();
                }
                _ => {
                    store.db.execute("UPDATE agent_access SET level=1 WHERE agent=''", []).unwrap();
                    Store::open(store.path()).unwrap().recover_outbox().unwrap();
                }
            }
            assert!(store.failed_outbox().unwrap().is_empty());
            assert_eq!(store.agent_draft_meta(&account.id,&id).unwrap().1,Some(agent));
        }
    }

    #[test]
    fn claimed_send_cannot_survive_a_policy_cycle() {
        for disable in [true, false] {
            let (_dir, store, _, id, _, _, grant) = queued_agent(false);
            assert!(store.claim_send(&id,0).unwrap());
            if disable {
                store.disable_agents().unwrap();
                store.set_agent_setting(Setting::Enabled,true).unwrap();
            } else {
                store.set_agent_access("", &Access { level:Level::Read, ..grant.clone() }).unwrap();
                store.set_agent_access("", &grant).unwrap();
            }
            assert!(store.authorize_queued_send(&id).is_err());
            assert!(store.pending(&id).is_err());
            assert_eq!(store.agent_audit(1).unwrap()[0].result,"revoked");
        }
    }

    #[test]
    fn claim_release_rolls_back_if_cleanup_cannot_remove_the_row() {
        let (_dir, store, _, id, agent, ..) = queued_agent(false);
        assert!(store.claim_send(&id, 0).unwrap());
        store.revoke_agent_tokens(&agent).unwrap();
        store.db.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON outbox BEGIN
            SELECT RAISE(ABORT,'injected delete failure'); END").unwrap();
        assert!(store.send_failed(&id, &Error::Rejected, 0).is_err());
        assert!(store.db.query_row("SELECT started FROM outbox WHERE id=?", [&id], |r| r.get::<_,bool>(0)).unwrap());
        store.db.execute_batch("DROP TRIGGER refuse_delete").unwrap();
        store.send_failed(&id, &Error::Rejected, 0).unwrap();
        assert!(store.pending(&id).is_err());
        assert!(store.draft(&id).is_ok());
    }

    #[test]
    fn revoke_restores_failed_and_retrying_sends() {
        for error in [Error::ReauthRequired,Error::Rejected,Error::Network("offline")] {
            let (_dir, store, account, id, agent, _, _) = queued_agent(false);
            assert!(store.claim_send(&id,0).unwrap());
            if matches!(error, Error::ReauthRequired) {
                store.revoke_agent_tokens(&agent).unwrap();
                store.send_failed(&id,&error,0).unwrap();
            } else {
                store.send_failed(&id,&error,0).unwrap();
                store.revoke_agent_tokens(&agent).unwrap();
            }
            assert!(store.pending(&id).is_err());
            assert_eq!(store.agent_draft_meta(&account.id,&id).unwrap().1,Some(agent));
        }
    }

    #[test]
    fn agent_send_rechecks_access_and_restores_owned_draft() {
        for change in ["disable", "revoke", "lower", "expire", "scope", "rotate_claimed", "rotate", "undo_off", "undo_revoke", "undo_all"] {
            let (_dir, store, account, id, agent, token, grant) = queued_agent(matches!(change,"scope" | "expire"));
            if matches!(change,"rotate" | "undo_off" | "undo_revoke" | "undo_all") {
                match change {
                    "rotate" => { store.issue_token(&agent, TokenScope::Local).unwrap(); }
                    "undo_off" => store.disable_agents().unwrap(),
                    "undo_all" => store.revoke_all_agents().unwrap(),
                    _ => store.revoke_agent_tokens(&agent).unwrap(),
                }
                let reopened = Store::open(store.path()).unwrap();
                assert!(reopened.pending(&id).is_err());
                assert_eq!(reopened.agent_draft_meta(&account.id,&id).unwrap().1.as_ref(),Some(&agent));
                assert_eq!(reopened.agent_audit(1).unwrap()[0].result,"revoked");
                continue;
            }
            assert!(store.claim_send(&id,0).unwrap());
            store.authorize_queued_send(&id).unwrap();
            match change {
                "rotate_claimed" => {
                    let original = store.authenticate(Some(&token),None).unwrap();
                    store.issue_token(&agent, TokenScope::Local).unwrap();
                    // Even a later token with the same public ID cannot authorize the claimed send.
                    store.db.execute("UPDATE agent_tokens SET id=? WHERE agent=?", params![original.token_id(),agent]).unwrap();
                }
                "disable" => store.disable_agents().unwrap(),
                "lower" => store.set_agent_access("", &Access { level:Level::Read, ..grant }).unwrap(),
                "scope" => { store.db.execute("UPDATE agent_tokens SET accounts='[]'",[]).unwrap(); }
                "expire" => { store.db.execute("UPDATE agent_tokens SET expires=1",[]).unwrap(); }
                _ => store.revoke_agent_tokens(&agent).unwrap(),
            }
            assert!(store.authorize_queued_send(&id).is_err());
            assert!(store.pending(&id).is_err());
            assert_eq!(store.agent_draft_meta(&account.id,&id).unwrap().1.as_ref(),Some(&agent));
            assert_eq!(store.agent_audit(1).unwrap()[0].result,"revoked");
        }
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
    fn reauthorization_resumes_only_revoked_grants_for_that_account() {
        for error in [Error::ReauthRequired, Error::Rejected, Error::Authentication, Error::MaybeSent, Error::Network("offline")] {
            for submitted in [false, true] {
                let (_dir, store, account, id) = queued();
                assert!(store.claim_send(&id, 0).unwrap());
                if submitted { store.submitted(&id).unwrap(); }
                store.send_failed(&id, &error, 0).unwrap();
                store.resume_outbox("another-account", 1).unwrap();
                assert!(store.due_outbox(&account.id, 1).unwrap().is_empty());
                store.resume_outbox(&account.id, 1).unwrap();
                assert_eq!(!store.due_outbox(&account.id, 1).unwrap().is_empty(), matches!(error, Error::ReauthRequired));
                assert_eq!(store.pending(&id).unwrap().submitted, submitted);
            }
        }
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
            assert_eq!(store.due_outbox(&account.id, now).unwrap(), std::slice::from_ref(&id));
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
        assert_eq!(store.pending(interrupted).unwrap().state, "uncertain");
        assert_eq!(store.pending(failed).unwrap().state, "uncertain");
        assert_eq!(store.due_outbox(&account.id, 0).unwrap(), [submitted]);
        assert!(store.undo_send(interrupted).unwrap());
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
        assert_eq!(store.drafts().unwrap(), std::slice::from_ref(&draft));
        store.delete_draft(&draft.id).unwrap();
        assert!(store.drafts().unwrap().is_empty());
    }
}

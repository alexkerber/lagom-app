//! Authorization and wire types shared by the desktop agent service and its client.
pub mod audit;
pub mod filter;
pub mod id;
pub mod policy;
pub mod protocol;
pub mod token;

#[cfg(test)]
mod tests;

use crate::store::Store;
use protocol::Code;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::Path;

impl Store {
    pub fn agent_change<T>(&self, change: impl FnOnce() -> Result<T, audit::Failure>) -> Result<T, audit::Failure> {
        let tx = rusqlite::Transaction::new_unchecked(&self.db, rusqlite::TransactionBehavior::Immediate)?;
        let value = change()?;
        tx.commit()?;
        Ok(value)
    }

    pub fn agent_draft_meta(&self, account: &str, id: &str) -> Result<(String, Option<String>), Code> {
        self.db.query_row("SELECT d.revision,a.agent FROM drafts d LEFT JOIN agent_drafts a ON a.draft=d.id
            WHERE d.account=? AND d.id=?", [account,id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(Code::NotFound)
    }

    pub fn agent_draft_ids(&self, account: &str) -> Result<Vec<String>, Code> {
        let mut stmt = self.db.prepare("SELECT id FROM drafts WHERE account=? ORDER BY created DESC,id")?;
        let ids = stmt.query_map([account], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_draft_agent(&self, draft: &str, who: &token::Identity) -> Result<(), Code> {
        self.db.execute("INSERT INTO agent_drafts VALUES(?,?)", [draft,who.agent_id()])?;
        Ok(())
    }

    /// A request sees one snapshot of both authorization and mail.
    pub fn agent_reader(
        path: &Path,
        cancelled: impl FnMut() -> bool + Send + 'static,
    ) -> Result<Self, Code> {
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.execute_batch("PRAGMA query_only=ON; BEGIN")?;
        filter::register_casefold(&db)?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        db.progress_handler(1024, Some(cancelled))?;
        Ok(Self { db })
    }

    /// The app has already migrated the store. Auditing must never run startup recovery.
    pub fn agent_writer(path: &Path) -> Result<Self, Code> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.execute_batch("PRAGMA foreign_keys=ON")?;
        Ok(Self { db })
    }

    pub fn agent_policy_generation(&self) -> Result<i64, Code> {
        Ok(self.db.query_row(
            "SELECT CAST(value AS INTEGER) FROM agent_settings WHERE key='policy_generation'",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn agent_enabled(&self) -> Result<bool, Code> {
        Ok(self.agent_generation()?.is_some())
    }

    /// A quick disable and re-enable still invalidates connections to the old listener.
    pub fn agent_generation(&self) -> Result<Option<i64>, Code> {
        Ok(self.db.query_row("SELECT CAST(coalesce((SELECT value FROM agent_settings WHERE key='generation'),'0') AS INTEGER)
            FROM agent_settings WHERE key='enabled' AND value='1'", [], |r| r.get(0)).optional()?)
    }

    pub fn disable_agents(&self) -> Result<(), Code> {
        let tx = self.db.unchecked_transaction()?;
        self.disable_agent_listener()?;
        tx.commit()?;
        self.cleanup_revoked_queue(None);
        Ok(())
    }

    fn disable_agent_listener(&self) -> Result<(), Code> {
        self.db.execute(
            "UPDATE agent_settings SET value='0' WHERE key='enabled'",
            [],
        )?;
        self.db.execute(
            "INSERT INTO agent_settings VALUES('generation',1)
            ON CONFLICT(key) DO UPDATE SET value=CAST(value AS INTEGER)+1",
            [],
        )?;
        self.db.execute("UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation'", [])?;
        Ok(())
    }

    pub fn agent_accounts(
        &self,
        who: &token::Identity,
        resources: &[policy::Resource],
        only: Option<&str>,
    ) -> Result<Vec<(String, String)>, Code> {
        let mut allowed = Vec::new();
        let mut check = |id: &str| -> Result<(), Code> {
            for resource in resources {
                if self.agent_level(who, *resource, id)? >= policy::Level::Read {
                    let email =
                        self.db
                            .query_row("SELECT email FROM accounts WHERE id=?", [id], |r| {
                                r.get(0)
                            })?;
                    allowed.push((id.to_owned(), email));
                    break;
                }
            }
            Ok(())
        };
        if let Some(id) = only {
            check(id)?;
        } else {
            let scope = who
                .accounts
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| Code::Internal)?;
            let mut ids = self.db.prepare("SELECT id FROM accounts WHERE ?1 IS NULL OR id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
            for id in ids.query_map([scope], |r| r.get::<_, String>(0))? {
                check(&id?)?;
            }
        }
        Ok(allowed)
    }

    pub fn agent_events(
        &self,
        account: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<crate::calendar::AgendaEvent>, Code> {
        let mut query = self.db.prepare("SELECT c.account,c.name,e.remote_id,e.title,e.start,e.end,e.start_utc,e.end_utc,e.all_day,e.location,e.join_url,e.html_link,c.remote_id
            FROM events e JOIN calendars c ON c.id=e.calendar WHERE c.account=?1 AND c.shown=1
            AND e.start_utc<?3 AND e.end_utc>?2 ORDER BY e.start_utc,e.remote_id")?;
        let rows = query.query_map(
            rusqlite::params![account, start, end],
            crate::calendar::agenda_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn agent_body(&self, account: &str, row: i64) -> Result<String, Code> {
        Ok(self.db.query_row(
            "SELECT body_text FROM messages WHERE account=? AND id=?",
            rusqlite::params![account, row],
            |r| r.get(0),
        )?)
    }

    pub fn agent_message_folders(&self, account: &str, header: &str) -> Result<Vec<String>, Code> {
        let mut stmt = self.db.prepare(&format!("SELECT f.name FROM messages m
            JOIN folders f ON f.id=coalesce(m.move_target,m.folder) AND f.account=m.account
            WHERE m.account=? AND m.message_id=? GROUP BY f.id ORDER BY {},min(m.id)",
            id::FOLDER_PRIORITY))?;
        let names = stmt.query_map([account, header], |r| r.get(0))?;
        Ok(names.collect::<rusqlite::Result<_>>()?)
    }

    pub fn agent_message_key(
        &self,
        account: &str,
        row: i64,
    ) -> Result<(id::MessageId, String), Code> {
        Ok(self.db.query_row("SELECT m.message_id,f.name,f.uidvalidity,m.uid,v.name FROM messages m
            JOIN folders f ON m.folder=f.id AND m.account=f.account JOIN folders v ON v.id=coalesce(m.move_target,m.folder) AND v.account=m.account WHERE m.id=? AND m.account=?",
            rusqlite::params![row,account], |r| {
                let header: String = r.get(0)?;
                let key = if header.is_empty() { id::MessageId::Uid(account.into(),r.get(1)?,r.get(2)?,r.get(3)?) }
                    else { id::MessageId::Header(account.into(),header) };
                Ok((key,r.get(4)?))
            })?)
    }
}

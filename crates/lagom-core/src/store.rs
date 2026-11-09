use crate::{
    account::{Account, Auth, Security, ServerConfig},
    Error, Result,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Store {
    pub(crate) db: Connection,
}

#[cfg(unix)]
fn owner_only(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
// shortcut: no ACL change on Windows; the per-user data directory is the only protection there.
#[cfg(not(unix))]
fn owner_only(_: &Path, _: u32) -> Result<()> {
    Ok(())
}
impl Store {
    pub fn open_default() -> Result<Self> {
        Self::open(Self::default_path()?)
    }
    /// `LAGOM_DATA_DIR` isolates test runs from the owner's real store.
    pub fn default_path() -> Result<std::path::PathBuf> {
        let dir = match std::env::var_os("LAGOM_DATA_DIR") {
            Some(dir) => std::path::PathBuf::from(dir),
            None => dirs::data_dir()
                .ok_or(Error::Invalid("platform data directory unavailable"))?
                .join("lagom"),
        };
        std::fs::create_dir_all(&dir)?;
        owner_only(&dir, 0o700)?;
        Ok(dir.join("lagom.sqlite3"))
    }
    /// Mail and account rows are private: the file and its WAL and SHM are owner-only (0600),
    /// also for stores created before this rule. SQLite gives WAL and SHM the database's mode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
        }
        owner_only(path, 0o600)?;
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        for side in ["-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(side);
            if Path::new(&p).exists() {
                owner_only(Path::new(&p), 0o600)?;
            }
        }
        db.pragma_update(None, "foreign_keys", true)?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 14 {
            return Err(Error::Invalid("store was created by a newer Lagom version"));
        }
        if version == 0 {
            db.execute_batch(include_str!("schema_v1.sql"))?;
        }
        if version < 2 {
            db.execute_batch(include_str!("schema_v2.sql"))?;
        }
        if version < 3 {
            db.execute_batch(include_str!("schema_v3.sql"))?;
        }
        if version < 4 {
            db.execute_batch(include_str!("schema_v4.sql"))?;
        }
        if version < 5 {
            db.execute_batch(include_str!("schema_v5.sql"))?;
        }
        if version < 6 {
            db.execute_batch(include_str!("schema_v6.sql"))?;
        }
        if version < 7 {
            db.execute_batch(include_str!("schema_v7.sql"))?;
        }
        let store = Self { db };
        if version < 8 {
            let tx = store.db.unchecked_transaction()?;
            store.db.execute_batch(include_str!("schema_v8.sql"))?;
            store.backfill_threads()?;
            tx.commit()?;
        }
        if version < 9 {
            store.db.execute_batch(include_str!("schema_v9.sql"))?;
        }
        if version < 10 {
            store.db.execute_batch(include_str!("schema_v10.sql"))?;
        }
        if version < 11 {
            store.db.execute_batch(include_str!("schema_v11.sql"))?;
        }
        if version < 12 {
            store.db.execute_batch(include_str!("schema_v12.sql"))?;
        }
        if version < 13 {
            store.db.execute_batch(include_str!("schema_v13.sql"))?;
        }
        if version < 14 {
            store.db.execute_batch(include_str!("schema_v14.sql"))?;
            if version < 13 {
                store.db.execute("UPDATE task_lists SET detached=0", [])?;
            }
        }
        Ok(store)
    }
    pub fn signature(&self, account: &str) -> Result<String> {
        Ok(self.db.query_row("SELECT text FROM signatures WHERE account=?1", [account], |r| r.get(0)).optional()?.unwrap_or_default())
    }

    pub fn set_signature(&self, account: &str, text: &str) -> Result<()> {
        self.db.execute("INSERT INTO signatures(account,text) VALUES (?1,?2) ON CONFLICT(account) DO UPDATE SET text=excluded.text", params![account, text])?;
        Ok(())
    }

    pub fn cached_attachment(&self, message: i64, section: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.db.query_row("SELECT data FROM attachment_cache WHERE message=?1 AND section=?2", params![message, section], |r| r.get(0)).optional()?)
    }

    pub(crate) fn cache_attachment(&self, message: i64, section: &str, data: &[u8]) -> Result<()> {
        self.db.execute("INSERT INTO attachment_cache VALUES (?1,?2,?3) ON CONFLICT(message,section) DO UPDATE SET data=excluded.data", params![message, section, data])?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        Path::new(self.db.path().expect("file-backed store"))
    }

    /// Runs `write` with at most ~250 ms of lock waiting, then restores the normal timeout.
    pub fn with_short_lock_wait<T>(&self, write: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        use std::time::{Duration, Instant};
        // A wall-clock deadline, not a count of sleeps: macOS can oversleep short sleeps severalfold,
        // so SQLite's own timeout and any fixed number of naps ran to about a second on CI.
        thread_local!(static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
        fn short_wait(_attempt: i32) -> bool {
            let left = DEADLINE.with(|d| d.get()).map_or(Duration::ZERO, |d| d.saturating_duration_since(Instant::now()));
            !left.is_zero() && { std::thread::sleep(left.min(Duration::from_millis(5))); true }
        }
        let previous: u64 = self.db.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
        DEADLINE.with(|d| d.set(Some(Instant::now() + Duration::from_millis(250))));
        self.db.busy_handler(Some(short_wait))?;
        let result = write(self);
        DEADLINE.with(|d| d.set(None));
        self.db.busy_timeout(std::time::Duration::from_millis(previous))?;
        result
    }

    pub fn save_account(&self, account: &Account) -> Result<()> {
        account.validate()?;
        self.db.execute("INSERT INTO accounts(id,display_name,email,imap_host,imap_port,imap_security,imap_username,smtp_host,smtp_port,smtp_security,smtp_username,auth) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
            ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,email=excluded.email,
            imap_host=excluded.imap_host,imap_port=excluded.imap_port,imap_security=excluded.imap_security,imap_username=excluded.imap_username,
            smtp_host=excluded.smtp_host,smtp_port=excluded.smtp_port,smtp_security=excluded.smtp_security,smtp_username=excluded.smtp_username,auth=excluded.auth",
            params![account.id,account.display_name,account.email,account.imap.host,account.imap.port,account.imap.security.as_str(),account.imap.username,account.smtp.host,account.smtp.port,account.smtp.security.as_str(),account.smtp.username,account.auth.as_str()])?;
        Ok(())
    }
    /// Removes the account and, by cascade, its folders, messages and search rows.
    pub fn delete_account(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM accounts WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self
            .db
            .prepare("SELECT * FROM accounts ORDER BY email,id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u16>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, u16>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
            ))
        })?;
        rows.map(|r| {
            let (id, display_name, email, ih, ip, is, iu, sh, sp, ss, su, auth) = r?;
            Ok(Account {
                auth: Auth::parse(&auth)?,
                id,
                display_name,
                email,
                imap: ServerConfig {
                    host: ih,
                    port: ip,
                    security: Security::parse(&is)?,
                    username: iu,
                },
                smtp: ServerConfig {
                    host: sh,
                    port: sp,
                    security: Security::parse(&ss)?,
                    username: su,
                },
            })
        })
        .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_task_write_restores_timeout_and_leaves_worker_timeout_unchanged() {
        use std::time::{Duration, Instant};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("busy.sqlite3");
        let store = Store::open(&path).unwrap();
        let worker = Store::open(&path).unwrap();
        let timeout = |s: &Store| s.db.pragma_query_value(None, "busy_timeout", |r| r.get::<_, u64>(0)).unwrap();
        let list = store.default_task_list().unwrap();
        worker.db.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let started = Instant::now();
        let result = store.with_short_lock_wait(|s| s.add_task(list, "Retry", "", None));
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(Error::Store(rusqlite::Error::SqliteFailure(e, _))) if e.code == rusqlite::ErrorCode::DatabaseBusy));
        assert!(elapsed < Duration::from_millis(750), "task write blocked for {elapsed:?}");
        assert_eq!(timeout(&store), 5000);
        assert_eq!(timeout(&worker), 5000);
        worker.db.execute_batch("ROLLBACK;").unwrap();
        assert!(store.tasks(list, true).unwrap().is_empty());
        store.db.busy_timeout(Duration::from_millis(1234)).unwrap();
        let task = store.with_short_lock_wait(|s| s.add_task(list, "Retry", "", None)).unwrap();
        assert_eq!(store.tasks(list, false).unwrap(), [task]);
        assert_eq!(timeout(&store), 1234);
        assert_eq!(timeout(&worker), 5000);
    }

    #[cfg(unix)]
    #[test]
    fn store_files_are_owner_only_also_when_they_existed() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.sqlite3");
        drop(Store::open(&path).unwrap());
        // A store from before the rule: world-readable.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = Store::open(&path).unwrap();
        store.save_account(&Account::new("a@gmail.com".into(), crate::presets::servers("a@gmail.com").unwrap().0, crate::presets::servers("a@gmail.com").unwrap().1).unwrap()).unwrap();
        let mode = |p: &str| std::fs::metadata(temp.path().join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("old.sqlite3"), 0o600);
        assert_eq!(mode("old.sqlite3-wal"), 0o600);
        assert_eq!(mode("old.sqlite3-shm"), 0o600);
        let dir = temp.path().join("data");
        std::env::set_var("LAGOM_DATA_DIR", &dir);
        let default = Store::default_path().unwrap();
        std::env::remove_var("LAGOM_DATA_DIR");
        assert_eq!(default, dir.join("lagom.sqlite3"));
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }
    #[test]
    fn auth_migrates_existing_rows_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("v2.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(include_str!("schema_v1.sql")).unwrap();
        db.execute_batch(include_str!("schema_v2.sql")).unwrap();
        db.execute("INSERT INTO accounts VALUES ('old','Alice','a@example.com','imap.example.com',993,'tls','a@example.com','smtp.example.com',587,'starttls','a@example.com')", []).unwrap();
        drop(db);
        let store = Store::open(&path).unwrap();
        let mut account = store.accounts().unwrap().remove(0);
        assert_eq!(account.auth, Auth::Password);
        for provider in [crate::oauth::Provider::Google, crate::oauth::Provider::Microsoft] {
            account.auth = Auth::OAuth(provider);
            store.save_account(&account).unwrap();
            assert_eq!(store.accounts().unwrap(), [account.clone()]);
        }
        assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
    }
    #[test]
    fn account_roundtrip_and_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.sqlite3");
        let (imap, smtp) = crate::presets::servers("a@gmail.com").unwrap();
        let mut account = Account::new("a@gmail.com".into(), imap, smtp).unwrap();
        {
            let store = Store::open(&path).unwrap();
            store.save_account(&account).unwrap();
            account.display_name = "Alice".into();
            store.save_account(&account).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.accounts().unwrap(), vec![account]);
        assert_eq!(
            store
                .db
                .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Archive,
    Junk,
    Other,
}
impl FolderRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::Trash => "trash",
            Self::Archive => "archive",
            Self::Junk => "junk",
            Self::Other => "other",
        }
    }
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "inbox" => Self::Inbox,
            "sent" | "sent mail" | "sent messages" => Self::Sent,
            "drafts" => Self::Drafts,
            "trash" | "deleted items" | "deleted messages" => Self::Trash,
            "archive" | "all mail" | "[gmail]/all mail" => Self::Archive,
            "junk" | "spam" => Self::Junk,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub account: String,
    pub name: String,
    pub delimiter: Option<String>,
    pub role: FolderRole,
    pub uidvalidity: Option<u32>,
    pub uidnext: Option<u32>,
    pub last_uid: u32,
    pub highest_modseq: Option<u64>,
    pub unread: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: i64,
    pub account: String,
    pub folder: i64,
    pub uid: u32,
    pub message_id: String,
    pub thread_key: String,
    pub thread_id: String,
    pub thread_refs: String,
    pub from_name: String,
    pub from_address: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: i64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub size: u32,
    pub snoozed_until: Option<i64>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub summary: MessageSummary,
    pub body_text: String,
    pub body_html: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub inbox: u64,
    pub later: u64,
}

pub(crate) const SUMMARY_COLUMNS: &str = "m.id,m.account,m.folder,m.uid,m.message_id,m.thread_key,m.from_name,m.from_address,m.to_addresses,m.cc,m.subject,m.date,m.snippet,m.seen,m.flagged,m.has_attachments,m.size,m.snoozed_until,m.thread_id,m.thread_refs";
pub(crate) fn summary_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageSummary> {
    Ok(MessageSummary {
        id: r.get(0)?,
        account: r.get(1)?,
        folder: r.get(2)?,
        uid: r.get(3)?,
        message_id: r.get(4)?,
        thread_key: r.get(5)?,
        from_name: r.get(6)?,
        from_address: r.get(7)?,
        to: r.get(8)?,
        cc: r.get(9)?,
        subject: r.get(10)?,
        date: r.get(11)?,
        snippet: r.get(12)?,
        seen: r.get(13)?,
        flagged: r.get(14)?,
        has_attachments: r.get(15)?,
        size: r.get(16)?,
        snoozed_until: r.get(17)?,
        thread_id: r.get(18)?,
        thread_refs: r.get(19)?,
    })
}
fn folder_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    let role: String = r.get(4)?;
    Ok(Folder {
        id: r.get(0)?,
        account: r.get(1)?,
        name: r.get(2)?,
        delimiter: r.get(3)?,
        role: FolderRole::from_name(&role),
        uidvalidity: r.get(5)?,
        uidnext: r.get(6)?,
        last_uid: r.get(7)?,
        highest_modseq: r.get(8)?,
        unread: r.get(9)?,
    })
}
impl Store {
    pub fn save_folder(
        &self,
        account: &str,
        name: &str,
        delimiter: Option<&str>,
        role: FolderRole,
    ) -> Result<i64> {
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(Error::Invalid("invalid folder name"));
        }
        Ok(self.db.query_row("INSERT INTO folders(account,name,delimiter,role) VALUES(?1,?2,?3,?4) ON CONFLICT(account,name) DO UPDATE SET delimiter=excluded.delimiter,role=excluded.role RETURNING id",params![account,name,delimiter,role.as_str()],|r|r.get(0))?)
    }
    pub(crate) fn retain_folders(&self, account: &str, present: &[i64]) -> Result<()> {
        let known = self.folders(account, 0)?;
        let tx = self.db.unchecked_transaction()?;
        for folder in known {
            if !present.contains(&folder.id) {
                tx.execute(
                    "DELETE FROM folders WHERE id=?1 AND account=?2",
                    params![folder.id, account],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn folders(&self, account: &str, now: i64) -> Result<Vec<Folder>> {
        let mut stmt = self.db.prepare("SELECT f.id,f.account,f.name,f.delimiter,f.role,f.uidvalidity,f.uidnext,f.last_uid,f.highest_modseq,(SELECT count(*) FROM messages m WHERE m.folder=f.id AND NOT m.seen AND (m.snoozed_until IS NULL OR m.snoozed_until<=?2)) FROM folders f WHERE f.account=?1 ORDER BY f.name")?;
        let rows = stmt.query_map(params![account, now], folder_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn folder(&self, id: i64) -> Result<Folder> {
        self.db.query_row("SELECT id,account,name,delimiter,role,uidvalidity,uidnext,last_uid,highest_modseq,0 FROM folders WHERE id=?1",[id],folder_row).optional()?.ok_or(Error::NotFound)
    }
    pub fn save_message(&self, message: &Message) -> Result<i64> {
        let tx = self.db.unchecked_transaction()?;
        let id = self.insert_message(message)?;
        tx.commit()?;
        Ok(id)
    }
    fn insert_message(&self, message: &Message) -> Result<i64> {
        let m = &message.summary;
        let thread = self.resolve_thread(m)?;
        let id = self.db.query_row("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,body_html,thread_id,thread_refs)
        SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20 FROM folders WHERE id=?2 AND account=?1
        ON CONFLICT(folder,uid) DO UPDATE SET message_id=excluded.message_id,thread_key=excluded.thread_key,from_name=excluded.from_name,from_address=excluded.from_address,to_addresses=excluded.to_addresses,cc=excluded.cc,subject=excluded.subject,date=excluded.date,snippet=excluded.snippet,seen=excluded.seen,flagged=excluded.flagged,has_attachments=excluded.has_attachments,size=excluded.size,body_text=excluded.body_text,body_html=excluded.body_html,thread_id=excluded.thread_id,thread_refs=excluded.thread_refs RETURNING id",
        params![m.account,m.folder,m.uid,m.message_id,m.thread_key,m.from_name,m.from_address,m.to,m.cc,m.subject,m.date,m.snippet,m.seen,m.flagged,m.has_attachments,m.size,message.body_text,message.body_html,thread,m.thread_refs],|r|r.get(0))?;
        Ok(id)
    }
    pub(crate) fn save_synced_message(&self, message: &Message, attachments: &[crate::attachment::Attachment]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let id = self.insert_message(message)?;
        self.db.execute("DELETE FROM attachments WHERE message=?1", [id])?;
        for part in attachments {
            self.db.execute("INSERT INTO attachments VALUES (?1,?2,?3,?4,?5,?6)",
                params![id, part.section, part.filename, part.mime_type, part.size, part.encoding])?;
        }
        self.db.execute("UPDATE messages SET attachments_loaded=1,threading_loaded=1 WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn attachment_metadata_loaded(&self, message: i64) -> Result<bool> {
        Ok(self.db.query_row("SELECT attachments_loaded FROM messages WHERE id=?1", [message], |r| r.get(0))?)
    }
    pub fn attachments(&self, message: i64) -> Result<Vec<crate::attachment::Attachment>> {
        let mut stmt = self.db.prepare("SELECT section,filename,mime_type,size,encoding FROM attachments WHERE message=?1 ORDER BY section")?;
        let rows = stmt.query_map([message], |r| Ok(crate::attachment::Attachment {
            section: r.get(0)?, filename: r.get(1)?, mime_type: r.get(2)?, size: r.get(3)?, encoding: r.get(4)?,
        }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub(crate) fn missing_metadata(&self, folder: i64, limit: usize) -> Result<Vec<u32>> {
        let mut stmt = self.db.prepare("SELECT uid FROM messages WHERE folder=?1 AND (NOT attachments_loaded OR NOT threading_loaded) ORDER BY uid DESC LIMIT ?2")?;
        let rows = stmt.query_map(params![folder, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn message_summary(&self, id: i64) -> Result<MessageSummary> {
        self.db.query_row(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE m.id=?1"), [id], summary_row).optional()?.ok_or(Error::NotFound)
    }
    pub fn message(&self, id: i64) -> Result<Message> {
        self.db.query_row(&format!("SELECT {SUMMARY_COLUMNS},m.body_text,m.body_html FROM messages m WHERE m.id=?1"),[id],|r| Ok(Message { summary:summary_row(r)?,body_text:r.get(20)?,body_html:r.get(21)? })).optional()?.ok_or(Error::NotFound)
    }
    /// Count unread Inbox mail across accounts, excluding snoozed mail and pending moves.
    pub fn unread_inbox_count(&self, now: i64, pending: &[i64]) -> Result<usize> {
        Ok(self.db.query_row("SELECT count(*) FROM messages m JOIN folders f ON f.id=m.folder
            WHERE f.role='inbox' AND m.seen=0 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?1)
            AND m.id NOT IN (SELECT value FROM json_each(?2))", params![now, serde_json::to_string(pending).expect("integer IDs serialize")], |r| r.get(0))?)
    }

    /// The newest unread Inbox mail across accounts, excluding mail still snoozed.
    pub fn unread_mail(&self, now: i64, limit: u32) -> Result<Vec<MessageSummary>> {
        let mut stmt = self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m JOIN folders f ON f.id=m.folder
            WHERE f.role='inbox' AND m.seen=0 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?1)
            ORDER BY m.date DESC,m.id DESC LIMIT ?2"))?;
        let rows = stmt.query_map(params![now, limit], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn list_messages(
        &self,
        folder: i64,
        now: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>> {
        let mut stmt=self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE m.folder=?1 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?2) ORDER BY m.date DESC,m.id DESC LIMIT ?3 OFFSET ?4"))?;
        let rows = stmt.query_map(params![folder, now, limit, offset], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn search(
        &self,
        account: &str,
        query: &str,
        now: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>> {
        if query.len() > 4096 {
            return Err(Error::Invalid("search query too long"));
        }
        // shortcut: index order avoids ranking every hit, add bounded relevance ranking when needed.
        let mut stmt=self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages_fts JOIN messages m ON m.id=messages_fts.rowid WHERE messages_fts MATCH ?1 AND m.account=?2 AND (m.snoozed_until IS NULL OR m.snoozed_until<=?3) ORDER BY messages_fts.rowid LIMIT ?4 OFFSET ?5"))?;
        let rows = stmt.query_map(params![query, account, now, limit, offset], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn snooze(&self, id: i64, until: Option<i64>) -> Result<()> {
        if self.db.execute(
            "UPDATE messages SET snoozed_until=?2 WHERE id=?1",
            params![id, until],
        )? == 0
        {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    pub fn later(
        &self,
        account: &str,
        now: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageSummary>> {
        let mut stmt=self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE m.account=?1 AND m.snoozed_until>?2 ORDER BY m.snoozed_until,m.id LIMIT ?3 OFFSET ?4"))?;
        let rows = stmt.query_map(params![account, now, limit, offset], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn counts(&self, account: &str, now: i64) -> Result<Counts> {
        Ok(self.db.query_row("SELECT count(CASE WHEN f.role='inbox' AND (m.snoozed_until IS NULL OR m.snoozed_until<=?2) THEN 1 END),count(CASE WHEN m.snoozed_until>?2 THEN 1 END) FROM messages m JOIN folders f ON f.id=m.folder WHERE m.account=?1",params![account,now],|r|Ok(Counts { inbox:r.get(0)?,later:r.get(1)? }))?)
    }
    pub fn set_flags(&self, folder: i64, uid: u32, seen: bool, flagged: bool) -> Result<()> {
        self.db.execute(
            "UPDATE messages SET seen=?3,flagged=?4 WHERE folder=?1 AND uid=?2",
            params![folder, uid, seen, flagged],
        )?;
        Ok(())
    }
    pub fn delete_message(&self, id: i64) -> Result<()> {
        self.db.execute("DELETE FROM messages WHERE id=?1", [id])?;
        Ok(())
    }
    /// The newest local row in `folder` with this Message-ID: where a moved message landed.
    pub fn find_message(&self, folder: i64, message_id: &str) -> Result<Option<i64>> {
        if message_id.is_empty() {
            return Ok(None);
        }
        Ok(self.db.query_row(
            "SELECT max(id) FROM messages WHERE folder=?1 AND message_id=?2",
            params![folder, message_id],
            |r| r.get(0),
        )?)
    }
    /// Metadata saved beyond a completed sync checkpoint, including snoozed rows.
    pub fn messages_since(&self, folder: i64, uid: u32) -> Result<Vec<MessageSummary>> {
        let mut stmt = self.db.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM messages m WHERE m.folder=?1 AND m.uid>?2 ORDER BY m.uid"))?;
        let rows = stmt.query_map(params![folder, uid], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn known_uids(&self, folder: i64) -> Result<Vec<u32>> {
        let mut stmt = self
            .db
            .prepare("SELECT uid FROM messages WHERE folder=?1 ORDER BY uid")?;
        let rows = stmt.query_map([folder], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn reset_uidvalidity(&mut self, folder: i64, validity: u32) -> Result<bool> {
        let old = self.folder(folder)?.uidvalidity;
        if old == Some(validity) {
            return Ok(false);
        }
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM messages WHERE folder=?1", [folder])?;
        tx.execute("UPDATE folders SET uidvalidity=?2,uidnext=NULL,last_uid=0,highest_modseq=NULL WHERE id=?1",params![folder,validity])?;
        tx.commit()?;
        Ok(true)
    }
    pub fn finish_sync(
        &mut self,
        folder: i64,
        present: &std::collections::HashSet<u32>,
        last_uid: u32,
        uidnext: Option<u32>,
        highest_modseq: Option<u64>,
    ) -> Result<()> {
        let known = self.known_uids(folder)?;
        let tx = self.db.transaction()?;
        for uid in known {
            if !present.contains(&uid) {
                tx.execute(
                    "DELETE FROM messages WHERE folder=?1 AND uid=?2",
                    params![folder, uid],
                )?;
            }
        }
        tx.execute(
            "UPDATE folders SET last_uid=?2,uidnext=?3,highest_modseq=?4 WHERE id=?1",
            params![folder, last_uid, uidnext, highest_modseq],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod mail_tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Store, Account, i64) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("mail.db")).unwrap();
        let (i, s) = crate::presets::servers("a@gmail.com").unwrap();
        let a = Account::new("a@gmail.com".into(), i, s).unwrap();
        store.save_account(&a).unwrap();
        let f = store
            .save_folder(&a.id, "INBOX", Some("/"), FolderRole::Inbox)
            .unwrap();
        (temp, store, a, f)
    }
    #[test]
    fn home_unread_is_bounded_newest_first_and_excludes_read_snoozed_and_non_inbox() {
        let (_temp, store, account, inbox) = setup();
        let mut second = account.clone();
        second.id = "second".into();
        second.email = "second@example.test".into();
        store.save_account(&second).unwrap();
        let other = store.save_folder(&second.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let trash = store.save_folder(&account.id, "Trash", None, FolderRole::Trash).unwrap();
        let mut expected = Vec::new();
        for uid in 1..=10 {
            let a = if uid % 2 == 0 && uid != 10 { &second } else { &account };
            let folder = if uid == 10 { trash } else if uid % 2 == 0 { other } else { inbox };
            let mut m = mail(a, folder, uid);
            m.summary.date = uid as i64;
            m.summary.seen = uid == 9;
            let id = store.save_message(&m).unwrap();
            if uid == 8 { store.snooze(id, Some(101)).unwrap(); }
            if uid <= 7 { expected.push(id); }
        }
        expected.reverse();
        assert_eq!(store.unread_mail(100, 5).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), expected[..5]);
        assert_eq!(store.unread_mail(101, 1).unwrap()[0].date, 8);
        assert!(store.unread_mail(100, 0).unwrap().is_empty());
        assert_eq!(store.unread_inbox_count(100, &[]).unwrap(), expected.len());
        assert_eq!(store.unread_inbox_count(100, &expected[..2]).unwrap(), expected.len() - 2);
        assert_eq!(store.unread_inbox_count(100, &expected).unwrap(), 0);
    }

    fn mail(a: &Account, f: i64, uid: u32) -> Message {
        Message {
            summary: MessageSummary {
                account: a.id.clone(),
                folder: f,
                uid,
                subject: format!("Hello {uid}"),
                from_name: "Alice".into(),
                from_address: "alice@example.com".into(),
                date: uid as i64,
                ..Default::default()
            },
            body_text: "rust sqlite search".into(),
            body_html: None,
        }
    }
    #[test]
    fn v12_upgrade_to_v14_preserves_signatures_attachments_tasks_and_lists() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("v12.db");
        let db = Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql"), include_str!("schema_v6.sql"), include_str!("schema_v7.sql"), include_str!("schema_v8.sql"), include_str!("schema_v9.sql"), include_str!("schema_v10.sql"), include_str!("schema_v11.sql"), include_str!("schema_v12.sql")] { db.execute_batch(schema).unwrap(); }
        let old = Store { db };
        let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
        old.save_authorized_account(&account, crate::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
        old.set_signature(&account.id, "Saved signature").unwrap();
        let folder = old.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let message = old.save_message(&mail(&account, folder, 1)).unwrap();
        let before = old.message(message).unwrap();
        old.db.execute("INSERT INTO attachments VALUES (?1,'1','data.bin','application/octet-stream',2,'base64')", [message]).unwrap();
        old.cache_attachment(message, "1", &[0, 255]).unwrap();
        let lists = old.db.prepare("SELECT id,account,remote_id,name,position FROM task_lists ORDER BY position,id").unwrap()
            .query_map([], |r| Ok(crate::tasks::TaskList { id: r.get(0)?, account: r.get(1)?, remote_id: r.get(2)?, name: r.get(3)?, position: r.get(4)?, google_default: false })).unwrap()
            .collect::<std::result::Result<Vec<_>, _>>().unwrap();
        old.db.execute("INSERT INTO calendar_windows VALUES (?1,100,200)", [&account.id]).unwrap();
        let list = old.default_task_list().unwrap();
        let task = old.db.query_row(&format!("INSERT INTO tasks(list,title,notes,updated,created,dirty) VALUES (?1,'Keep task','Private notes',42,41,1) RETURNING {}", crate::tasks::TASK_COLUMNS), [list], crate::tasks::task_row).unwrap();
        assert_eq!(old.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 12);
        drop(old);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
        assert_eq!(store.accounts().unwrap(), [account.clone()]);
        assert_eq!(store.account_scopes(&account.id).unwrap(), crate::oauth::GOOGLE_CALENDAR_SCOPES);
        assert_eq!(store.signature(&account.id).unwrap(), "Saved signature");
        assert_eq!(store.message(message).unwrap(), before);
        assert_eq!(store.cached_attachment(message, "1").unwrap(), Some(vec![0, 255]));
        assert_eq!(store.tasks(list, true).unwrap(), [task.clone()]);
        assert_eq!(store.default_task_list().unwrap(), list);
        assert_eq!(store.task_lists().unwrap(), lists);
        assert_eq!(store.calendar_window(&account.id).unwrap(), Some((100, 200)));
        assert_eq!(store.db.query_row("SELECT count(*) FROM task_lists WHERE dirty=1 AND detached=0", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert!(store.db.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none());
        drop(store);
        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.tasks(list, true).unwrap(), [task]);
        assert_eq!(reopened.task_lists().unwrap(), lists);
    }

    #[test]
    fn v10_upgrade_preserves_calendar_data_and_account_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("v10.db");
        let db = Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql"), include_str!("schema_v6.sql"), include_str!("schema_v7.sql"), include_str!("schema_v8.sql"), include_str!("schema_v9.sql"), include_str!("schema_v10.sql")] { db.execute_batch(schema).unwrap(); }
        let old = Store { db };
        let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
        old.save_account(&account).unwrap();
        old.db.execute("UPDATE accounts SET scopes='calendar.readonly' WHERE id=?1", [&account.id]).unwrap();
        old.db.execute("INSERT INTO calendars(id,account,remote_id,name,shown,writable) VALUES (1,?1,'primary','Calendar',1,0)", [&account.id]).unwrap();
        old.db.execute_batch("INSERT INTO events(calendar,remote_id,title,start,end,start_utc,end_utc,all_day) VALUES (1,'event','Keep event','2026-11-09','2026-11-10',1,2,1);").unwrap();
        drop(old);
        let store = Store::open(path).unwrap();
        store.set_signature(&account.id, "Team").unwrap();
        store.save_account(&account).unwrap();
        assert_eq!(store.db.query_row("SELECT scopes FROM accounts WHERE id=?1", [&account.id], |r| r.get::<_, String>(0)).unwrap(), "calendar.readonly");
        assert_eq!(store.db.query_row("SELECT title FROM events WHERE calendar=1", [], |r| r.get::<_, String>(0)).unwrap(), "Keep event");
        assert_eq!(store.signature(&account.id).unwrap(), "Team");
    }

    #[test]
    fn v9_upgrade_preserves_mail_drafts_outbox_and_tasks() {
        mail_upgrade(9);
    }

    #[test]
    fn v11_upgrade_preserves_signatures_cached_attachments_mail_drafts_outbox_and_tasks() {
        mail_upgrade(11);
    }

    fn mail_upgrade(version: usize) {
        use crate::{outbox::Draft, smtp::{Outgoing, OutgoingAttachment}};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(format!("v{version}.db"));
        let db = Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql"), include_str!("schema_v6.sql"), include_str!("schema_v7.sql"), include_str!("schema_v8.sql"), include_str!("schema_v9.sql"), include_str!("schema_v10.sql"), include_str!("schema_v11.sql")].iter().take(version) { db.execute_batch(schema).unwrap(); }
        let old = Store { db };
        let (imap, smtp) = crate::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
        old.save_account(&account).unwrap();
        let folder = old.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let id = old.save_message(&mail(&account, folder, 1)).unwrap();
        let before = old.message(id).unwrap();
        let attachment = crate::attachment::Attachment { section: "2".into(), filename: "data.bin".into(), mime_type: "application/octet-stream".into(), size: 2, encoding: "base64".into() };
        old.save_synced_message(&before, std::slice::from_ref(&attachment)).unwrap();
        if version == 11 {
            old.set_signature(&account.id, "Existing signature").unwrap();
            old.cache_attachment(id, &attachment.section, &[0, 255]).unwrap();
        }
        let task = old.db.query_row(&format!("INSERT INTO tasks(list,title,notes,updated,created,dirty) SELECT list,'Keep task','Notes',42,41,1 FROM task_settings RETURNING {}", crate::tasks::TASK_COLUMNS), [], crate::tasks::task_row).unwrap();
        let draft = Draft { id: "draft".into(), account: account.id.clone(), text: "Saved edits".into(), ..Default::default() };
        let mut legacy = serde_json::to_value(&draft).unwrap();
        legacy.as_object_mut().unwrap().remove("attachments");
        legacy.as_object_mut().unwrap().remove("forward_source");
        old.db.execute("INSERT INTO drafts(id,account,content,revision) VALUES (?1,?2,?3,'revision')", params![draft.id, account.id, legacy.to_string()]).unwrap();
        let outgoing = Outgoing { to: vec!["recipient@example.com".into()], text: "Queued".into(), ..Default::default() };
        let queued = old.enqueue(&account, &outgoing, 123).unwrap();
        let raw = old.pending(&queued).unwrap().raw;
        assert_eq!(old.db.pragma_query_value(None, "user_version", |r| r.get::<_, usize>(0)).unwrap(), version);
        drop(old);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
        assert_eq!(store.accounts().unwrap(), [account.clone()]);
        assert_eq!(store.attachments(id).unwrap(), [attachment]);
        assert_eq!(store.cached_attachment(id, "2").unwrap(), if version == 11 { Some(vec![0, 255]) } else { None });
        assert_eq!(store.message(id).unwrap(), before);
        assert_eq!(store.drafts().unwrap(), [draft.clone()]);
        assert_eq!(store.pending(&queued).unwrap().raw, raw);
        assert_eq!(store.tasks(task.list, false).unwrap(), [task]);
        assert_eq!(store.signature(&account.id).unwrap(), if version == 11 { "Existing signature" } else { "" });
        store.set_signature(&account.id, "**Team**\nContact").unwrap();
        store.save_account(&account).unwrap();
        let parts = vec![OutgoingAttachment { filename: "data.bin".into(), mime_type: "application/octet-stream".into(), data: vec![0, 255] }];
        let draft = Draft { attachments: parts.clone(), ..draft };
        store.save_draft(&draft).unwrap();
        let mail = Outgoing { attachments: parts.clone(), message_id: Some(draft.id.clone()), ..outgoing };
        let queued = store.enqueue(&account, &mail, 0).unwrap();
        assert_eq!(store.pending(&queued).unwrap().mail.attachments, parts);
        assert!(store.undo_send(&queued).unwrap());
        drop(store);
        let store = Store::open(path).unwrap();
        assert_eq!(store.signature(&account.id).unwrap(), "**Team**\nContact");
        assert_eq!(store.drafts().unwrap(), [draft]);
        store.delete_account(&account.id).unwrap();
        assert_eq!(store.signature(&account.id).unwrap(), "");
    }

    #[test]
    fn message_summary_revalidates_metadata_without_reading_bodies() {
        let (_temp, s, a, inbox) = setup();
        let id = s.save_message(&mail(&a, inbox, 1)).unwrap();
        let mut expected = s.message(id).unwrap().summary;
        // Invalid body types make accidental body decoding fail deterministically.
        s.db.execute("UPDATE messages SET body_text=x'ff',body_html=x'ff' WHERE id=?1", [id]).unwrap();
        assert!(s.message(id).is_err());
        assert_eq!(s.message_summary(id).unwrap(), expected);
        s.set_flags(inbox, 1, true, false).unwrap();
        s.snooze(id, Some(1000)).unwrap();
        expected.seen = true;
        expected.snoozed_until = Some(1000);
        assert_eq!(s.message_summary(id).unwrap(), expected);
        s.delete_message(id).unwrap();
        assert!(matches!(s.message_summary(id), Err(Error::NotFound)));
    }

    #[test]
    fn messages_since_uses_folder_and_uid_not_date_or_read_state() {
        let (_temp, s, a, inbox) = setup();
        let done = s.save_folder(&a.id, "Archive", None, FolderRole::Archive).unwrap();
        for uid in 1..=3 {
            s.save_message(&mail(&a, inbox, uid)).unwrap();
            s.save_message(&mail(&a, done, uid)).unwrap();
        }
        let mut m = mail(&a, inbox, 3);
        m.summary.date = 0;
        m.summary.seen = true;
        let id = s.save_message(&m).unwrap();
        s.snooze(id, Some(1000)).unwrap();
        let rows = s.messages_since(inbox, 1).unwrap();
        assert_eq!(rows.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2, 3]);
        assert!(rows[1].seen);
        assert_eq!(rows[1].snoozed_until, Some(1000));
        assert!(s.messages_since(inbox, 3).unwrap().is_empty());
    }

    #[test]
    fn insert_page_fts_and_snooze() {
        let (_temp, s, a, f) = setup();
        let mut m = mail(&a, f, 1);
        let id = s.save_message(&m).unwrap();
        s.save_message(&mail(&a, f, 2)).unwrap();
        assert_eq!(s.message(id).unwrap().body_text, m.body_text);
        assert_eq!(s.list_messages(f, 100, 1, 0).unwrap()[0].uid, 2);
        assert_eq!(s.list_messages(f, 100, 1, 1).unwrap()[0].uid, 1);
        for q in ["Hello", "Alice", "from_address:alice", "sqlite"] {
            assert_eq!(s.search(&a.id, q, 100, 10, 0).unwrap().len(), 2);
        }
        s.snooze(id, Some(200)).unwrap();
        assert_eq!(s.list_messages(f, 100, 10, 0).unwrap().len(), 1);
        assert_eq!(s.search(&a.id, "sqlite", 100, 10, 0).unwrap().len(), 1);
        assert_eq!(s.counts(&a.id, 100).unwrap(), Counts { inbox: 1, later: 1 });
        assert_eq!(s.later(&a.id, 100, 10, 0).unwrap().len(), 1);
        assert_eq!(s.folders(&a.id, 100).unwrap()[0].unread, 1);
        m.body_text = "replacement".into();
        assert_eq!(s.save_message(&m).unwrap(), id);
        assert_eq!(s.message(id).unwrap().summary.snoozed_until, Some(200));
        assert_eq!(s.list_messages(f, 200, 10, 0).unwrap().len(), 2);
        assert_eq!(s.search(&a.id, "replacement", 200, 10, 0).unwrap().len(), 1);
        s.delete_message(id).unwrap();
        assert!(s
            .search(&a.id, "replacement", 200, 10, 0)
            .unwrap()
            .is_empty());
        assert!(s.search(&a.id, "\"", 100, 10, 0).is_err());
    }
    fn save_legacy(old: &Store, message: Message) -> Message {
        let m = &message.summary;
        old.db.execute("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,body_html)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![m.account,m.folder,m.uid,m.message_id,m.thread_key,m.from_name,m.from_address,m.to,m.cc,m.subject,m.date,m.snippet,m.seen,m.flagged,m.has_attachments,m.size,message.body_text,message.body_html]).unwrap();
        let id = old.db.last_insert_rowid();
        let mut saved_message = message;
        saved_message.summary.id = id;
        saved_message
    }

    #[test]
    fn v7_upgrade_preserves_data_and_adds_thread_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v7.db");
        let db = Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql"), include_str!("schema_v6.sql"), include_str!("schema_v7.sql")] {
            db.execute_batch(schema).unwrap();
        }
        let old = Store { db };
        let (imap, smtp) = crate::presets::servers("fixture@fastmail.com").unwrap();
        let account = Account::new("fixture@fastmail.com".into(), imap, smtp).unwrap();
        old.save_account(&account).unwrap();
        let folder = old.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let mut root = mail(&account, folder, 1);
        root.summary.message_id = "root@test".into();
        root.summary.thread_key = "root@test".into();
        let mut child = mail(&account, folder, 2);
        child.summary.message_id = "child@test".into();
        child.summary.thread_key = "root@test".into();
        let mut root = save_legacy(&old, root);
        let mut child = save_legacy(&old, child);
        old.snooze(root.summary.id, Some(1000)).unwrap();
        root.summary.snoozed_until = Some(1000);
        old.db.execute("INSERT INTO attachments VALUES (?1,'2','report.pdf','application/pdf',128,'base64')", [root.summary.id]).unwrap();
        let attachments = old.attachments(root.summary.id).unwrap();
        let saved_folder = old.folder(folder).unwrap();
        old.db.execute("UPDATE messages SET attachments_loaded=1", []).unwrap();
        drop(old);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
        for (index, columns) in [
            ("messages_thread", vec!["account", "thread_id", "date", "id"]),
            ("messages_thread_folder", vec!["account", "thread_id", "folder", "date", "id"]),
            ("messages_later_date", vec!["account", "date", "id"]),
        ] {
            let actual = store.db.prepare(&format!("PRAGMA index_info({index})")).unwrap()
                .query_map([], |r| r.get::<_, String>(2)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            assert_eq!(actual, columns);
        }
        assert_eq!(store.accounts().unwrap(), std::slice::from_ref(&account));
        assert_eq!(store.folder(folder).unwrap(), saved_folder);
        assert_eq!(store.attachments(root.summary.id).unwrap(), attachments);
        let a = store.message(root.summary.id).unwrap();
        let b = store.message(child.summary.id).unwrap();
        root.summary.thread_id = a.summary.thread_id.clone();
        child.summary.thread_id = a.summary.thread_id.clone();
        root.summary.thread_refs = "root@test".into();
        child.summary.thread_refs = "root@test".into();
        assert_eq!(a, root);
        assert_eq!(b, child);
        assert_eq!(store.list_conversations(&account.id, Some(folder), 0, 1).unwrap(), std::slice::from_ref(&b.summary));
        assert_eq!(store.list_conversations(&account.id, None, 0, 1).unwrap(), std::slice::from_ref(&a.summary));
        assert_eq!(a.summary.thread_id, b.summary.thread_id);
        assert_eq!(b.summary.thread_refs, "root@test");
        assert_eq!(b.body_text, child.body_text);
        assert_eq!(store.missing_metadata(folder, 10).unwrap(), [2, 1]);
        store.save_synced_message(&b, &[]).unwrap();
        assert_eq!(store.missing_metadata(folder, 10).unwrap(), [1]);
        drop(store);
        assert_eq!(Store::open(path).unwrap().message(a.summary.id).unwrap(), a);
    }

    #[test]
    fn attachment_migration_roundtrip_rollback_and_cascade() {
        for version in [4, 5, 6, 7] {
            attachment_migration(version);
        }
    }
    fn attachment_migration(version: usize) {
        use crate::{attachment::Attachment, outbox::Draft, smtp::Outgoing};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(format!("v{version}.db"));
        let db = Connection::open(&path).unwrap();
        for schema in [include_str!("schema_v1.sql"), include_str!("schema_v2.sql"), include_str!("schema_v3.sql"), include_str!("schema_v4.sql"), include_str!("schema_v5.sql"), include_str!("schema_v6.sql"), include_str!("schema_v7.sql")].iter().take(version) {
            db.execute_batch(schema).unwrap();
        }
        let old = Store { db };
        let config = ServerConfig { host: "localhost".into(), port: 3143, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        old.save_account(&account).unwrap();
        let folder = old.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let mut message = mail(&account, folder, 1);
        message.summary.has_attachments = false;
        let saved_message = save_legacy(&old, message.clone());
        let id = saved_message.summary.id;
        let saved_folder = old.folder(folder).unwrap();
        let draft = Draft { id: "draft".into(), account: account.id.clone(), text: "unfinished draft".into(), ..Default::default() };
        let outgoing = Outgoing { to: vec!["recipient@example.com".into()], text: "queued mail".into(), ..Default::default() };
        if version >= 5 {
            old.db.execute("INSERT INTO drafts(id,account,content) VALUES (?1,?2,?3)",
                params![draft.id, account.id, serde_json::to_string(&draft).unwrap()]).unwrap();
            old.db.execute("INSERT INTO outbox(id,account,content,raw,ready_at,submitted) VALUES ('queued',?1,?2,?3,1234,1)",
                params![account.id, serde_json::to_string(&outgoing).unwrap(), b"queued MIME bytes".as_slice()]).unwrap();
        }
        if version >= 6 {
            old.db.execute_batch("UPDATE drafts SET revision='saved-revision';
                UPDATE outbox SET state='failed',attempts=3,copied=1;").unwrap();
        }
        assert_eq!(old.db.pragma_query_value(None, "user_version", |r| r.get::<_, usize>(0)).unwrap(), version);
        drop(old);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
        assert_eq!(store.accounts().unwrap(), std::slice::from_ref(&account));
        assert_eq!(store.folder(folder).unwrap(), saved_folder);
        let mut saved_message = saved_message;
        saved_message.summary.thread_id = store.message(id).unwrap().summary.thread_id;
        assert!(!saved_message.summary.thread_id.is_empty());
        assert_eq!(store.message(id).unwrap(), saved_message);
        assert_eq!(store.search(&account.id, "sqlite", 100, 10, 0).unwrap(), [saved_message.summary]);
        if version >= 5 {
            assert_eq!(store.drafts().unwrap(), std::slice::from_ref(&draft));
            let pending = store.pending("queued").unwrap();
            assert_eq!(pending.mail.text, outgoing.text);
            assert_eq!(pending.raw, b"queued MIME bytes");
            assert!(pending.submitted);
            assert_eq!(pending.copied, version >= 6);
            assert_eq!(pending.state, if version >= 6 { "failed" } else { "ready" });
            assert_eq!(store.db.query_row("SELECT ready_at,attempts FROM outbox", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?))).unwrap(), (1234, if version >= 6 { 3 } else { 0 }));
            let revision = store.draft_revision(&draft.id).unwrap();
            assert!(!revision.is_empty());
            if version >= 6 { assert_eq!(revision, "saved-revision"); }
        } else {
            store.save_draft(&draft).unwrap();
            assert_eq!(store.drafts().unwrap(), [draft]);
            assert!(store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
        }
        assert_eq!(store.missing_metadata(folder, 1).unwrap(), [1]);
        let part = Attachment { section: "2.1".into(), filename: "report.pdf".into(), mime_type: "application/pdf".into(), size: 128, encoding: "base64".into() };
        store.save_synced_message(&message, std::slice::from_ref(&part)).unwrap();
        assert_eq!(store.attachments(id).unwrap(), std::slice::from_ref(&part));
        assert!(store.missing_metadata(folder, 1).unwrap().is_empty());
        assert!(store.save_synced_message(&message, &[part.clone(), part.clone()]).is_err());
        assert_eq!(store.attachments(id).unwrap(), std::slice::from_ref(&part));
        drop(store);
        store = Store::open(&path).unwrap();
        assert_eq!(store.attachments(id).unwrap(), [part]);
        store.reset_uidvalidity(folder, 10).unwrap();
        assert!(store.attachments(id).unwrap().is_empty());
    }
    #[test]
    fn uid_reset_and_expunge() {
        let (_t, mut s, a, f) = setup();
        s.reset_uidvalidity(f, 10).unwrap();
        let id = s.save_message(&mail(&a, f, 1)).unwrap();
        s.save_message(&mail(&a, f, 2)).unwrap();
        assert!(!s.reset_uidvalidity(f, 10).unwrap());
        s.finish_sync(f, &[2].into_iter().collect(), 2, Some(3), None)
            .unwrap();
        assert!(s.message(id).is_err());
        assert_eq!(s.known_uids(f).unwrap(), vec![2]);
        assert!(s.reset_uidvalidity(f, 11).unwrap());
        assert!(s.known_uids(f).unwrap().is_empty());
        assert_eq!(s.folder(f).unwrap().last_uid, 0);
        assert!(s.search(&a.id, "sqlite", 100, 10, 0).unwrap().is_empty());
    }
    #[test]
    fn migration_from_v1_and_future_rejected() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("v1.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(include_str!("schema_v1.sql")).unwrap();
        let (i, s) = crate::presets::servers("a@gmail.com").unwrap();
        let a = Account::new("a@gmail.com".into(), i, s).unwrap();
        db.execute("INSERT INTO accounts VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![a.id,a.display_name,a.email,a.imap.host,a.imap.port,a.imap.security.as_str(),a.imap.username,a.smtp.host,a.smtp.port,a.smtp.security.as_str(),a.smtp.username]).unwrap();
        drop(db);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.accounts().unwrap(), vec![a]);
        assert_eq!(
            s.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            14
        );
        s.db.pragma_update(None, "user_version", 99).unwrap();
        drop(s);
        assert!(Store::open(path).is_err());
    }
    #[test]
    fn migration_from_v3_seeds_task_lists_and_preserves_mail() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("v3.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(include_str!("schema_v1.sql")).unwrap();
        db.execute_batch(include_str!("schema_v2.sql")).unwrap();
        db.execute_batch(include_str!("schema_v3.sql")).unwrap();
        let old = Store { db };
        let (i, o) = crate::presets::servers("a@gmail.com").unwrap();
        let a = Account::new("a@gmail.com".into(), i, o).unwrap();
        old.save_account(&a).unwrap();
        let f = old.save_folder(&a.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let mut message = save_legacy(&old, mail(&a, f, 1));
        let id = message.summary.id;
        let folder = old.folder(f).unwrap();
        drop(old);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.accounts().unwrap(), std::slice::from_ref(&a));
        assert_eq!(s.folder(f).unwrap(), folder);
        message.summary.thread_id = s.message(id).unwrap().summary.thread_id;
        assert_eq!(s.message(id).unwrap(), message);
        assert_eq!(s.search(&a.id, "sqlite", 100, 10, 0).unwrap(), [message.summary]);
        assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0)).unwrap(), 14);
        let lists = s.task_lists().unwrap();
        assert_eq!(lists.iter().map(|l| (l.name.as_str(), l.position, l.account.as_deref())).collect::<Vec<_>>(), [("Work", 0, None), ("Private", 1, None)]);
        assert!(lists.iter().all(|l| s.tasks(l.id, true).unwrap().is_empty()));
        drop(s);
        assert_eq!(Store::open(path).unwrap().task_lists().unwrap(), lists);
    }
    #[test]
    fn deleted_folders_clean_fts_and_ids_are_not_reused() {
        let (_t, s, a, f) = setup();
        let id = s.save_message(&mail(&a, f, 1)).unwrap();
        let kept = s
            .save_folder(&a.id, "Kept", None, FolderRole::Other)
            .unwrap();
        s.retain_folders(&a.id, &[kept]).unwrap();
        assert!(s.message(id).is_err());
        assert!(s.folder(f).is_err());
        assert!(s.search(&a.id, "sqlite", 100, 10, 0).unwrap().is_empty());
        let new_id = s.save_message(&mail(&a, kept, 1)).unwrap();
        assert!(new_id > id);
    }
    #[test]
    fn delete_account_cascades_and_find_message() {
        let (_t, s, a, f) = setup();
        let mut m = mail(&a, f, 1);
        m.summary.message_id = "x@test".into();
        let id = s.save_message(&m).unwrap();
        assert_eq!(s.find_message(f, "x@test").unwrap(), Some(id));
        assert_eq!(s.find_message(f, "").unwrap(), None);
        s.delete_account(&a.id).unwrap();
        assert!(s.accounts().unwrap().is_empty());
        assert!(s.message(id).is_err());
        assert!(s.folder(f).is_err());
        assert!(s.search(&a.id, "sqlite", 100, 10, 0).unwrap().is_empty());
    }
    #[test]
    fn rejects_cross_account_message() {
        let (_t, s, mut a, f) = setup();
        a.id = "other".into();
        s.save_account(&a).unwrap();
        assert!(s.save_message(&mail(&a, f, 1)).is_err());
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    #[test]
    #[ignore = "100k-message search budget; run explicitly with --release --ignored --nocapture"]
    fn search_100k_budget() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("budget.db")).unwrap();
        let (i, s) = crate::presets::servers("budget@gmail.com").unwrap();
        let a = Account::new("budget@gmail.com".into(), i, s).unwrap();
        store.save_account(&a).unwrap();
        let f = store
            .save_folder(&a.id, "INBOX", Some("/"), FolderRole::Inbox)
            .unwrap();
        let tx = store.db.unchecked_transaction().unwrap();
        {
            let mut insert=store.db.prepare("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text) VALUES(?1,?2,?3,'','','Alice','alice@example.com','budget@gmail.com','','Status update',?3,'',0,0,0,512,?4)").unwrap();
            for uid in 1..=100_000 {
                let body = format!(
                    "sqlite common body unique{uid} {}",
                    "message text ".repeat(30)
                );
                insert.execute(params![a.id, f, uid, body]).unwrap();
            }
        }
        tx.commit().unwrap();
        for query in ["unique50000", "sqlite", "Alice", "subject:Status"] {
            let start = std::time::Instant::now();
            let rows = store.search(&a.id, query, i64::MAX, 50, 0).unwrap();
            let elapsed = start.elapsed();
            println!("100k FTS {query}: {elapsed:?}, {} results", rows.len());
            assert!(!rows.is_empty());
            assert!(
                elapsed < std::time::Duration::from_millis(50),
                "{query}: {elapsed:?} exceeds 50 ms"
            );
        }
    }
}

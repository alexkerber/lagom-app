use crate::{
    account::{Account, Security, ServerConfig},
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
        if version > 2 {
            return Err(Error::Invalid("store was created by a newer Lagom version"));
        }
        if version == 0 {
            db.execute_batch(include_str!("schema_v1.sql"))?;
        }
        if version < 2 {
            db.execute_batch(include_str!("schema_v2.sql"))?;
        }
        Ok(Self { db })
    }
    pub fn save_account(&self, account: &Account) -> Result<()> {
        account.validate()?;
        self.db.execute("INSERT INTO accounts VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
            ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,email=excluded.email,
            imap_host=excluded.imap_host,imap_port=excluded.imap_port,imap_security=excluded.imap_security,imap_username=excluded.imap_username,
            smtp_host=excluded.smtp_host,smtp_port=excluded.smtp_port,smtp_security=excluded.smtp_security,smtp_username=excluded.smtp_username",
            params![account.id,account.display_name,account.email,account.imap.host,account.imap.port,account.imap.security.as_str(),account.imap.username,account.smtp.host,account.smtp.port,account.smtp.security.as_str(),account.smtp.username])?;
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
            ))
        })?;
        rows.map(|r| {
            let (id, display_name, email, ih, ip, is, iu, sh, sp, ss, su) = r?;
            Ok(Account {
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

const SUMMARY_COLUMNS: &str = "m.id,m.account,m.folder,m.uid,m.message_id,m.thread_key,m.from_name,m.from_address,m.to_addresses,m.cc,m.subject,m.date,m.snippet,m.seen,m.flagged,m.has_attachments,m.size,m.snoozed_until";
fn summary_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageSummary> {
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
        let m = &message.summary;
        Ok(self.db.query_row("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text,body_html)
        SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18 FROM folders WHERE id=?2 AND account=?1
        ON CONFLICT(folder,uid) DO UPDATE SET message_id=excluded.message_id,thread_key=excluded.thread_key,from_name=excluded.from_name,from_address=excluded.from_address,to_addresses=excluded.to_addresses,cc=excluded.cc,subject=excluded.subject,date=excluded.date,snippet=excluded.snippet,seen=excluded.seen,flagged=excluded.flagged,has_attachments=excluded.has_attachments,size=excluded.size,body_text=excluded.body_text,body_html=excluded.body_html RETURNING id",
        params![m.account,m.folder,m.uid,m.message_id,m.thread_key,m.from_name,m.from_address,m.to,m.cc,m.subject,m.date,m.snippet,m.seen,m.flagged,m.has_attachments,m.size,message.body_text,message.body_html],|r|r.get(0))?)
    }
    pub fn message(&self, id: i64) -> Result<Message> {
        self.db.query_row(&format!("SELECT {SUMMARY_COLUMNS},m.body_text,m.body_html FROM messages m WHERE m.id=?1"),[id],|r| Ok(Message { summary:summary_row(r)?,body_text:r.get(18)?,body_html:r.get(19)? })).optional()?.ok_or(Error::NotFound)
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
        let s = Store { db };
        s.save_account(&a).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.accounts().unwrap(), vec![a]);
        assert_eq!(
            s.db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            2
        );
        s.db.pragma_update(None, "user_version", 99).unwrap();
        drop(s);
        assert!(Store::open(path).is_err());
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

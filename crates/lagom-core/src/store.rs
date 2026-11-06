use crate::{
    account::{Account, Security, ServerConfig},
    Error, Result,
};
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    pub(crate) db: Connection,
}
impl Store {
    pub fn open_default() -> Result<Self> {
        let dir = dirs::data_dir()
            .ok_or(Error::Invalid("platform data directory unavailable"))?
            .join("lagom");
        std::fs::create_dir_all(&dir)?;
        Self::open(dir.join("lagom.sqlite3"))
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", true)?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(Error::Invalid("store was created by a newer Lagom version"));
        }
        if version == 0 {
            db.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE accounts (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, email TEXT NOT NULL,
                imap_host TEXT NOT NULL, imap_port INTEGER NOT NULL, imap_security TEXT NOT NULL, imap_username TEXT NOT NULL,
                smtp_host TEXT NOT NULL, smtp_port INTEGER NOT NULL, smtp_security TEXT NOT NULL, smtp_username TEXT NOT NULL);
                PRAGMA user_version = 1; COMMIT;")?;
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

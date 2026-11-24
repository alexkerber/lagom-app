use super::{protocol::Code, token::Identity};
use crate::store::Store;
use rusqlite::params;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(i64)]
pub enum Level {
    None,
    Read,
    Triage,
    Send,
}

impl Level {
    pub(super) fn from_db(value: i64) -> Option<Self> {
        [Self::None, Self::Read, Self::Triage, Self::Send]
            .get(usize::try_from(value).ok()?)
            .copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Mail,
    Calendar,
}

impl Store {
    /// Reauthenticate and read both policy matrices in one store snapshot.
    pub fn effective(
        &self,
        token: Option<&str>,
        source_ip: Option<&str>,
        resource: Resource,
        account: &str,
    ) -> Result<Level, Code> {
        let tx = self.db.unchecked_transaction()?;
        let identity = self.authenticate(token, source_ip)?;
        let level = self.agent_level(&identity, resource, account)?;
        tx.commit()?;
        Ok(level)
    }

    pub fn agent_level(
        &self,
        identity: &Identity,
        resource: Resource,
        account: &str,
    ) -> Result<Level, Code> {
        if account.is_empty()
            || identity
                .accounts
                .as_ref()
                .is_some_and(|ids| !ids.iter().any(|a| a == account))
        {
            return Ok(Level::None);
        }
        let active: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_settings WHERE key='enabled' AND value='1')",
            [],
            |r| r.get(0),
        )?;
        if !active {
            return Ok(Level::None);
        }
        let kind = match resource {
            Resource::Mail => "mail",
            Resource::Calendar => "calendar",
        };
        let matrix = |agent: &str| -> Result<Level, Code> {
            let n = self.db.query_row("SELECT coalesce((SELECT level FROM agent_access
                WHERE agent=?1 AND kind=?2 AND account IN ('',?3) ORDER BY account<>'' DESC LIMIT 1),0)",
                params![agent,kind,account], |r| r.get(0))?;
            Ok(Level::from_db(n).unwrap_or(Level::None))
        };
        let level = identity
            .level
            .min(matrix("")?)
            .min(matrix(&identity.agent)?);
        if level == Level::None
            || !self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
                [account],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Ok(Level::None);
        }
        Ok(
            if resource == Resource::Calendar && level == Level::Triage {
                Level::Read
            } else {
                level
            },
        )
    }
}

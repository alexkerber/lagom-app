use super::{protocol::Code, token::Identity};
use crate::store::Store;
use rusqlite::params;

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Mail,
    Calendar,
}

/// A switch in Settings > AI Agents. Confirmation is on unless explicitly disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Enabled,
    Confirm,
    Sidebar,
}

impl Setting {
    fn key(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Confirm => "confirm",
            Self::Sidebar => "sidebar",
        }
    }
}

/// One stored row of a policy matrix. Account "" is the group default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub resource: Resource,
    pub account: String,
    pub level: Level,
}

impl Store {
    pub fn agent_setting(&self, setting: Setting) -> Result<bool, Code> {
        let query = match setting {
            Setting::Confirm => "SELECT NOT EXISTS(SELECT 1 FROM agent_settings WHERE key=? AND value='0')",
            _ => "SELECT EXISTS(SELECT 1 FROM agent_settings WHERE key=? AND value='1')",
        };
        Ok(self.db.query_row(
            query,
            [setting.key()],
            |r| r.get(0),
        )?)
    }

    pub fn set_agent_setting(&self, setting: Setting, on: bool) -> Result<(), Code> {
        if matches!(setting, Setting::Enabled) && !on {
            return self.disable_agents();
        }
        self.db.execute(
            "INSERT OR REPLACE INTO agent_settings VALUES(?,?)",
            params![setting.key(), if on { "1" } else { "0" }],
        )?;
        Ok(())
    }

    /// The stored rows of one matrix. Agent "" is the global matrix.
    pub fn agent_access(&self, agent: &str) -> Result<Vec<Access>, Code> {
        let mut rows = self.db.prepare(
            "SELECT kind,account,level FROM agent_access WHERE agent=? ORDER BY kind,account",
        )?;
        let rows = rows.query_map([agent], |r| {
            Ok(Access {
                resource: if r.get::<_, String>(0)? == "mail" {
                    Resource::Mail
                } else {
                    Resource::Calendar
                },
                account: r.get(1)?,
                level: Level::from_db(r.get(2)?).unwrap_or(Level::None),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Calendars have no Triage. Evaluation takes the minimum of both matrices, so a stored agent
    /// row above the global one never widens access.
    pub fn set_agent_access(&self, agent: &str, access: &Access) -> Result<(), Code> {
        let kind = match (access.resource, access.level) {
            (Resource::Calendar, Level::Triage) => return Err(Code::InvalidRequest),
            (Resource::Mail, _) => "mail",
            (Resource::Calendar, _) => "calendar",
        };
        self.db.execute(
            "INSERT OR REPLACE INTO agent_access VALUES(?,?,?,?)",
            params![agent, kind, access.account, access.level as i64],
        )?;
        Ok(())
    }

    /// Disconnect all: every token goes and access switches off, in one transaction.
    pub fn revoke_all_agents(&self) -> Result<(), Code> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM agent_tokens", [])?;
        self.disable_agent_listener()?;
        tx.commit()?;
        Ok(())
    }

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

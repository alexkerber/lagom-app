use super::{
    audit::{Failure, TokenFailure},
    policy::{Access, Level, Resource},
    protocol::Code,
};
use crate::store::Store;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ipnet::IpNet;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const DAY: i64 = 86400;

fn random<const N: usize>() -> Result<[u8; N], Code> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| Code::Internal)?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub enum TokenScope<'a> {
    Local,
    Remote {
        accounts: &'a [String],
        level: Level,
        ranges: &'a [IpNet],
        expires: Option<i64>,
    },
}

pub struct Identity {
    pub expires: Option<i64>,
    pub(super) agent: String,
    pub(super) token: String,
    pub(super) level: Level,
    pub(super) accounts: Option<Vec<String>>,
}

impl Identity {
    pub fn agent_id(&self) -> &str {
        &self.agent
    }
    pub fn token_id(&self) -> &str {
        &self.token
    }
}

/// An agent as Settings lists it. `active`: it holds a token that has not expired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRow {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub host: String,
    pub remote: bool,
    pub active: bool,
    pub expires: Option<i64>,
}

// The outbox is local. Socket authentication also checks the caller's source address.
fn token_scope(remote: i64, level: i64, accounts: Option<String>, cidr: Option<String>, expires: Option<i64>,
    created: i64, source: Option<Option<&str>>) -> Result<(Level, Option<Vec<String>>), TokenFailure> {
    use TokenFailure::*;
    let now = chrono::Utc::now().timestamp();
    let accounts = match remote {
        0 if source.flatten().is_some() => return Err(Source),
        0 if expires.is_none() && accounts.is_none() && cidr.is_none() => None,
        1 => {
            let end = expires.ok_or(InvalidScope)?;
            if end <= now {
                return Err(Expired);
            }
            if end <= created || end > created.saturating_add(365 * DAY) {
                return Err(InvalidScope);
            }
            let nets: Vec<IpNet> = serde_json::from_str(&cidr.ok_or(InvalidScope)?).map_err(|_| InvalidScope)?;
            if nets.is_empty() {
                return Err(InvalidScope);
            }
            if let Some(source) = source {
                let ip = source.ok_or(Source)?.parse::<IpAddr>().map_err(|_| Source)?.to_canonical();
                if !nets.iter().any(|n| n.contains(&ip)) {
                    return Err(Source);
                }
            }
            Some(
                serde_json::from_str(&accounts.ok_or(InvalidScope)?)
                    .map_err(|_| InvalidScope)?,
            )
        }
        _ => return Err(InvalidScope),
    };
    let level = Level::from_db(level).ok_or(InvalidScope)?;
    Ok((level, accounts))
}

impl Store {
    pub fn agents(&self) -> Result<Vec<AgentRow>, Code> {
        let mut rows = self.db.prepare(
            "SELECT id,kind,name,host,remote,EXISTS(SELECT 1 FROM agent_tokens t WHERE t.agent=a.id
             AND (t.expires IS NULL OR t.expires>unixepoch())),
             (SELECT max(expires) FROM agent_tokens t WHERE t.agent=a.id)
             FROM agents a ORDER BY created,id",
        )?;
        let rows = rows.query_map([], |r| {
            Ok(AgentRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                host: r.get(3)?,
                remote: r.get(4)?,
                active: r.get(5)?,
                expires: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// What Connect wrote outside the store, so Disconnect can remove it. It holds no secret.
    pub fn agent_connection(&self, agent: &str) -> Result<Option<String>, Code> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM agent_settings WHERE key='connection:' || ?",
                [agent],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_agent_connection(&self, agent: &str, receipt: Option<&str>) -> Result<(), Code> {
        match receipt {
            Some(receipt) => self.db.execute(
                "INSERT OR REPLACE INTO agent_settings VALUES('connection:' || ?,?)",
                params![agent, receipt],
            )?,
            None => self.db.execute(
                "DELETE FROM agent_settings WHERE key='connection:' || ?",
                [agent],
            )?,
        };
        Ok(())
    }

    pub fn create_agent(
        &self,
        kind: &str,
        name: &str,
        host: &str,
        remote: bool,
    ) -> Result<String, Code> {
        self.change_authority(|| self.insert_agent(kind, name, host, remote))
    }

    fn insert_agent(&self, kind: &str, name: &str, host: &str, remote: bool) -> Result<String, Code> {
        let id = hex(&random::<6>()?);
        self.db.execute(
            "INSERT INTO agents(id,kind,name,host,remote,created) VALUES(?,?,?,?,?,unixepoch())",
            params![id, kind, name, host, remote],
        )?;
        Ok(id)
    }

    pub fn create_remote_agent(
        &self,
        host: &str,
        accounts: &[String],
        level: Level,
        ranges: &[IpNet],
        expires: i64,
    ) -> Result<(String, Zeroizing<String>), Code> {
        if host.trim().is_empty() || accounts.is_empty() || ranges.is_empty() {
            return Err(Code::InvalidRequest);
        }
        let tx = self.write_transaction()?;
        let id = self.insert_agent("remote", host, host, true)?;
        tx.execute("UPDATE agent_access SET level=0 WHERE agent=?", [&id])?;
        for account in accounts {
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
                [account],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Code::NotFound);
            }
            for resource in [Resource::Mail, Resource::Calendar] {
                let level = if resource == Resource::Calendar && level == Level::Triage {
                    Level::Read
                } else {
                    level
                };
                self.set_agent_access_inner(
                    &id,
                    &Access {
                        resource,
                        account: account.clone(),
                        level,
                    },
                )?;
            }
        }
        let token = self.issue_token_inner(
            &id,
            TokenScope::Remote {
                accounts,
                level,
                ranges,
                expires: Some(expires),
            },
        )?;
        tx.commit()?;
        self.cleanup_revoked_queue();
        Ok((id, token))
    }

    pub fn remove_remote_agent(&self, id: &str) -> Result<(), Code> {
        self.change_authority(|| {
            self.db.execute("DELETE FROM agents WHERE id=? AND remote=1", [id])?;
            Ok(())
        })
    }

    pub fn issue_token(
        &self,
        agent: &str,
        scope: TokenScope<'_>,
    ) -> Result<Zeroizing<String>, Code> {
        self.change_authority(|| self.issue_token_inner(agent, scope))
    }

    fn issue_token_inner(
        &self,
        agent: &str,
        scope: TokenScope<'_>,
    ) -> Result<Zeroizing<String>, Code> {
        let now = chrono::Utc::now().timestamp();
        let remote: i64 =
            self.db
                .query_row("SELECT remote FROM agents WHERE id=?", [agent], |r| {
                    r.get(0)
                })?;
        let (level, accounts, cidr, expires) = match (remote, scope) {
            (0, TokenScope::Local) => {
                self.db.execute("DELETE FROM agent_tokens WHERE agent=?", [agent])?;
                (Level::Send, None, None, None)
            }
            (
                1,
                TokenScope::Remote {
                    accounts,
                    level,
                    ranges,
                    expires,
                },
            ) => {
                let end = expires.unwrap_or(now + 30 * DAY);
                if ranges.is_empty() || end <= now || end > now + 365 * DAY || accounts.iter().any(String::is_empty) {
                    return Err(Code::InvalidRequest);
                }
                (
                    level,
                    Some(serde_json::to_string(accounts).map_err(|_| Code::Internal)?),
                    Some(serde_json::to_string(ranges).map_err(|_| Code::Internal)?),
                    Some(end),
                )
            }
            _ => return Err(Code::InvalidRequest),
        };
        let id = hex(&random::<4>()?);
        let secret = Zeroizing::new(random::<32>()?);
        let mut raw = Zeroizing::new(format!("lgm_{id}_"));
        URL_SAFE_NO_PAD.encode_string(&*secret, &mut raw);
        let hash = Sha256::digest(raw.as_bytes());
        self.db.execute("INSERT INTO agent_tokens(id,agent,hash,level,accounts,cidr,expires,created) VALUES(?,?,?,?,?,?,?,?)",
            params![id,agent,&hash[..],level as i64,accounts,cidr,expires,now])?;
        Ok(raw)
    }

    /// `save` gets the agent ID and the token before the commit. If it fails, nothing is committed:
    /// no agent, no token, and access that was off stays off.
    pub fn provision_read_agent(&self, account: &str, save: impl FnOnce(&str, &str) -> Result<(), Code>) -> Result<Zeroizing<String>, Code> {
        let tx = self.write_transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
            [account],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Code::NotFound);
        }
        let agent = self.insert_agent("developer", "Manual test", "local", false)?;
        tx.execute("UPDATE agent_access SET level=0 WHERE agent=?", [&agent])?;
        for kind in ["mail", "calendar"] {
            if self.matrix("", kind, account)? < Level::Read {
                return Err(Code::Forbidden);
            }
            tx.execute(
                "INSERT INTO agent_access VALUES(?,?,?,1)",
                params![agent, kind, account],
            )?;
        }
        let token = self.issue_token_inner(&agent, TokenScope::Local)?;
        tx.execute(
            "UPDATE agent_settings SET value='1' WHERE key='enabled'",
            [],
        )?;
        save(&agent, &token)?;
        tx.commit()?;
        self.cleanup_revoked_queue();
        Ok(token)
    }

    pub fn revoke_agent_tokens(&self, agent: &str) -> Result<(), Code> {
        self.change_authority(|| {
            self.db.execute("DELETE FROM agent_tokens WHERE agent=?", [agent])?;
            Ok(())
        })
    }

    pub(crate) fn agent_may_send(&self, agent: &str, token: Option<&str>, account: &str) -> crate::Result<bool> {
        let mut stmt = self.db.prepare("SELECT a.remote,t.id,t.level,t.accounts,t.cidr,t.expires,t.created
            FROM agents a JOIN agent_tokens t ON a.id=t.agent WHERE a.id=? AND t.id=? AND t.level=3")?;
        let mut rows = stmt.query(params![agent, token])?;
        while let Some(row) = rows.next()? {
            let expires = row.get(5)?;
            let Ok((level, accounts)) = token_scope(row.get(0)?,row.get(2)?,row.get(3)?,row.get(4)?,expires,row.get(6)?,None) else { continue };
            let who = Identity { agent: agent.to_owned(), token:row.get(1)?, level, accounts, expires };
            if self.agent_level(&who, super::policy::Resource::Mail, account)
                .map_err(|_| crate::Error::Invalid("agent policy unavailable"))? == Level::Send {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn authenticate(
        &self,
        raw: Option<&str>,
        source_ip: Option<&str>,
    ) -> Result<Identity, Failure> {
        use Failure::Unauthorized as Refused;
        use TokenFailure::*;

        let raw = raw.ok_or(Refused(Missing))?;
        let (id, secret) = raw
            .strip_prefix("lgm_")
            .and_then(|s| s.split_once('_'))
            .ok_or(Refused(Malformed))?;
        if id.len() != 8
            || !id.bytes().all(|b| b.is_ascii_hexdigit())
            || secret.len() != 43
            || URL_SAFE_NO_PAD.decode_slice(secret, &mut [0; 32]) != Ok(32)
        {
            return Err(Refused(Malformed));
        }
        let (agent, remote, hash, level, accounts, cidr, expires, created) = self
            .db
            .query_row(
                "SELECT a.id,a.remote,t.hash,t.level,t.accounts,t.cidr,t.expires,t.created
             FROM agent_tokens t JOIN agents a ON a.id=t.agent WHERE t.id=?",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                        r.get(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or(Refused(UnknownOrRevoked))?;
        if !bool::from(hash.ct_eq(Sha256::digest(raw.as_bytes()).as_slice())) {
            return Err(Refused(WrongSecret));
        }
        let refused = |reason| Failure::VerifiedRefusal {
            reason,
            agent: agent.clone(),
            token: id.into(),
        };
        let (level, accounts) = token_scope(remote, level, accounts, cidr, expires, created, Some(source_ip))
            .map_err(refused)?;
        Ok(Identity {
            expires,
            agent,
            token: id.into(),
            level,
            accounts,
        })
    }
}

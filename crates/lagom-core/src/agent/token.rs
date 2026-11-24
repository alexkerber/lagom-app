use super::{
    audit::{Failure, TokenFailure},
    policy::Level,
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
}

impl Store {
    pub fn agents(&self) -> Result<Vec<AgentRow>, Code> {
        let mut rows = self.db.prepare(
            "SELECT id,kind,name,host,remote,EXISTS(SELECT 1 FROM agent_tokens t WHERE t.agent=a.id
             AND (t.expires IS NULL OR t.expires>unixepoch()))
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
        let id = hex(&random::<6>()?);
        self.db.execute(
            "INSERT INTO agents(id,kind,name,host,remote,created) VALUES(?,?,?,?,?,unixepoch())",
            params![id, kind, name, host, remote],
        )?;
        Ok(id)
    }

    pub fn issue_token(
        &self,
        agent: &str,
        scope: TokenScope<'_>,
    ) -> Result<Zeroizing<String>, Code> {
        let tx = self.db.unchecked_transaction()?;
        let raw = self.issue_token_inner(agent, scope)?;
        tx.commit()?;
        Ok(raw)
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
                self.db
                    .execute("DELETE FROM agent_tokens WHERE agent=?", [agent])?;
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
                if end <= now || end > now + 365 * DAY || accounts.iter().any(String::is_empty) {
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

    pub fn provision_read_agent(&self, account: &str) -> Result<Zeroizing<String>, Code> {
        let tx = self.db.unchecked_transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
            [account],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Code::NotFound);
        }
        let agent = self.create_agent("developer", "Manual test", "local", false)?;
        tx.execute("UPDATE agent_access SET level=0 WHERE agent=?", [&agent])?;
        for kind in ["mail", "calendar"] {
            tx.execute("INSERT INTO agent_access SELECT '',?1,?2,1 WHERE coalesce((SELECT level FROM agent_access
                WHERE agent='' AND kind=?1 AND account IN ('',?2) ORDER BY account<>'' DESC LIMIT 1),0)=0
                ON CONFLICT DO UPDATE SET level=1", params![kind,account])?;
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
        tx.commit()?;
        Ok(token)
    }

    pub fn revoke_agent_tokens(&self, agent: &str) -> Result<(), Code> {
        self.db
            .execute("DELETE FROM agent_tokens WHERE agent=?", [agent])?;
        Ok(())
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
        let now = chrono::Utc::now().timestamp();
        let accounts = match remote {
            0 if source_ip.is_some() => return Err(refused(Source)),
            0 if expires.is_none() && accounts.is_none() && cidr.is_none() => None,
            1 => {
                let end = expires.ok_or_else(|| refused(InvalidScope))?;
                if end <= now {
                    return Err(refused(Expired));
                }
                if end <= created || end > created.saturating_add(365 * DAY) {
                    return Err(refused(InvalidScope));
                }
                let ip = source_ip
                    .ok_or_else(|| refused(Source))?
                    .parse::<IpAddr>()
                    .map_err(|_| refused(Source))?
                    .to_canonical();
                if let Some(cidr) = cidr {
                    let nets: Vec<IpNet> =
                        serde_json::from_str(&cidr).map_err(|_| refused(InvalidScope))?;
                    if !nets.is_empty() && !nets.iter().any(|n| n.contains(&ip)) {
                        return Err(refused(Source));
                    }
                }
                Some(
                    serde_json::from_str(&accounts.ok_or_else(|| refused(InvalidScope))?)
                        .map_err(|_| refused(InvalidScope))?,
                )
            }
            _ => return Err(refused(InvalidScope)),
        };
        let level = Level::from_db(level).ok_or_else(|| refused(InvalidScope))?;
        Ok(Identity {
            agent,
            token: id.into(),
            level,
            accounts,
        })
    }
}

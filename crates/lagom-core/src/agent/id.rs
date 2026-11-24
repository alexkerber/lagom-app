use super::{
    audit::Failure,
    policy::{Level, Resource},
    protocol::Code,
};
use crate::store::Store;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageId {
    Header(String, String),
    Uid(String, String, u32, u32),
}

fn encode(prefix: &str, parts: impl Serialize) -> String {
    format!(
        "{prefix}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parts).expect("ID parts serialize"))
    )
}

fn parts<T: for<'a> Deserialize<'a>>(data: &str) -> Result<T, Code> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(data).map_err(|_| Code::NotFound)?)
        .map_err(|_| Code::NotFound)
}

impl MessageId {
    pub fn encode(&self) -> String {
        match self {
            Self::Header(a, h) => encode("m1", (a, h)),
            Self::Uid(a, f, v, u) => encode("u1", (a, f, v, u)),
        }
    }
    pub fn decode(id: &str) -> Result<Self, Code> {
        if id.len() > 2048 {
            return Err(Code::NotFound);
        }
        match id.split_once('.') {
            Some(("m1", data)) => {
                let (a, h): (String, String) = parts(data)?;
                if a.is_empty() || h.is_empty() {
                    return Err(Code::NotFound);
                }
                Ok(Self::Header(a, h))
            }
            Some(("u1", data)) => {
                let (a, f, v, u): (String, String, u32, u32) = parts(data)?;
                if a.is_empty() || f.is_empty() || v == 0 || u == 0 {
                    return Err(Code::NotFound);
                }
                Ok(Self::Uid(a, f, v, u))
            }
            _ => Err(Code::NotFound),
        }
    }
    pub fn account(&self) -> &str {
        match self {
            Self::Header(a, _) | Self::Uid(a, _, _, _) => a,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedMessage {
    target: String,
    pub row: i64,
    pub ambiguous: bool,
}

impl ResolvedMessage {
    pub(super) fn audit_target(&self) -> &str {
        &self.target
    }
}

impl Store {
    pub fn resolve_agent_message(
        &self,
        token: Option<&str>,
        ip: Option<&str>,
        id: &str,
        folder: Option<&str>,
        required: Level,
    ) -> Result<ResolvedMessage, Failure> {
        let tx = self.db.unchecked_transaction()?;
        let identity = self.authenticate(token, ip)?;
        let key = MessageId::decode(id)?;
        let account = key.account();
        let level = self.agent_level(&identity, Resource::Mail, account)?;
        if level < Level::Read {
            return Err(Failure::OutsideScope);
        }
        let result = match &key {
            MessageId::Uid(_, name, validity, uid) => {
                if folder.is_some_and(|f| f != name) {
                    return Err(Code::NotFound.into());
                }
                self.db.query_row(
                    "SELECT m.id,m.account,f.name,f.uidvalidity,m.uid FROM folders f JOIN messages m ON m.folder=f.id
                    WHERE f.account=?1 AND m.account=?1 AND f.name=?2 AND f.uidvalidity=?3 AND m.uid=?4
                    AND m.message_id=''", params![account, name, validity, uid],
                    |r| Ok(ResolvedMessage {
                        row: r.get(0)?, ambiguous: false,
                        target: MessageId::Uid(r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?).encode(),
                    }),
                ).optional()?
            }
            MessageId::Header(_, header) => {
                self.db.query_row(
                    "WITH copies AS (
                    SELECT m.id,m.account,m.message_id,f.name,f.role,m.folder,m.date,m.size,m.from_address,m.from_name,m.subject
                    FROM messages m JOIN folders f ON f.id=m.folder AND f.account=m.account
                    WHERE m.account=?1 AND m.message_id=?2)
                    SELECT id, (SELECT count(*)<>count(DISTINCT folder) OR min(date)<>max(date)
                        OR min(size)<>max(size) OR min(from_address)<>max(from_address)
                        OR min(from_name)<>max(from_name) OR min(subject)<>max(subject) FROM copies), account,message_id
                    FROM copies WHERE ?3 IS NULL OR name=?3
                    ORDER BY CASE role WHEN 'inbox' THEN 0 WHEN 'sent' THEN 2 WHEN 'archive' THEN 3
                        WHEN 'junk' THEN 4 WHEN 'trash' THEN 5 ELSE 1 END, id LIMIT 1",
                    params![account, header, folder],
                    |r| Ok(ResolvedMessage {
                        row: r.get(0)?, ambiguous: r.get(1)?,
                        target: MessageId::Header(r.get(2)?, r.get(3)?).encode(),
                    }),
                ).optional()?
            }
        }.ok_or(Code::NotFound)?;
        if level < required {
            return Err(Code::Forbidden.into());
        }
        if required > Level::Read && result.ambiguous {
            return Err(Code::Conflict.into());
        }
        tx.commit()?;
        Ok(result)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectId {
    Message(MessageId),
    Thread(String, String),
    Draft(String, String),
    Event(String, String, String),
    Attachment(MessageId, String),
}

impl ObjectId {
    pub fn encode(&self) -> String {
        match self {
            Self::Message(m) => m.encode(),
            Self::Thread(a, t) => encode("t1", (a, t)),
            Self::Draft(a, d) => encode("d1", (a, d)),
            Self::Event(a, c, e) => encode("e1", (a, c, e)),
            Self::Attachment(m, s) => encode("a1", (m.encode(), s)),
        }
    }

    pub fn decode(id: &str) -> Result<Self, Code> {
        if id.len() > 2048 {
            return Err(Code::NotFound);
        }
        let (prefix, data) = id.split_once('.').ok_or(Code::NotFound)?;
        match prefix {
            "m1" | "u1" => Ok(Self::Message(MessageId::decode(id)?)),
            "e1" => {
                let (a, c, e): (String, String, String) = parts(data)?;
                if a.is_empty() || c.is_empty() || e.is_empty() {
                    return Err(Code::NotFound);
                }
                Ok(Self::Event(a, c, e))
            }
            "t1" | "d1" | "a1" => {
                let (a, b): (String, String) = parts(data)?;
                if a.is_empty() || b.is_empty() {
                    return Err(Code::NotFound);
                }
                match prefix {
                    "t1" => Ok(Self::Thread(a, b)),
                    "d1" => Ok(Self::Draft(a, b)),
                    _ => Ok(Self::Attachment(MessageId::decode(&a)?, b)),
                }
            }
            _ => Err(Code::NotFound),
        }
    }
}

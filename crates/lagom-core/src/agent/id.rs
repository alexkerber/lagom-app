use super::{
    audit::Failure,
    policy::{Level, Resource},
    protocol::Code,
};
use crate::store::Store;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageId {
    Header(String, String),
    HeaderHash(String, String),
    FolderHash(String, String, u32, u32),
    Uid(String, String, u32, u32),
}

fn encode(prefix: &str, parts: impl Serialize) -> String {
    format!(
        "{prefix}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parts).expect("ID parts serialize"))
    )
}

fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn bounded(id: String, limit: usize, compact: impl FnOnce() -> String) -> String {
    if id.len() <= limit {
        id
    } else {
        compact()
    }
}

fn parts<T: for<'a> Deserialize<'a>>(data: &str) -> Result<T, Code> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(data).map_err(|_| Code::NotFound)?)
        .map_err(|_| Code::NotFound)
}

impl MessageId {
    pub fn encode(&self) -> String {
        match self {
            Self::Header(a, h) => {
                bounded(encode("m1", (a, h)), 1400, || encode("mh", (a, digest(h))))
            }
            Self::HeaderHash(a, h) => encode("mh", (a, h)),
            Self::Uid(a, f, v, u) => bounded(encode("u1", (a, f, v, u)), 1400, || {
                encode("uh", (a, digest(f), v, u))
            }),
            Self::FolderHash(a, h, v, u) => encode("uh", (a, h, v, u)),
        }
    }
    pub fn decode(id: &str) -> Result<Self, Code> {
        if id.len() > 2048 {
            return Err(Code::NotFound);
        }
        match id.split_once('.') {
            Some((prefix @ ("m1" | "mh"), data)) => {
                let (a, h): (String, String) = parts(data)?;
                if a.is_empty() || h.is_empty() {
                    return Err(Code::NotFound);
                }
                Ok(if prefix == "mh" {
                    Self::HeaderHash(a, h)
                } else {
                    Self::Header(a, h)
                })
            }
            Some((prefix @ ("u1" | "uh"), data)) => {
                let (a, f, v, u): (String, String, u32, u32) = parts(data)?;
                if a.is_empty() || f.is_empty() || v == 0 || u == 0 {
                    return Err(Code::NotFound);
                }
                Ok(if prefix == "uh" {
                    Self::FolderHash(a, f, v, u)
                } else {
                    Self::Uid(a, f, v, u)
                })
            }
            _ => Err(Code::NotFound),
        }
    }
    pub fn account(&self) -> &str {
        match self {
            Self::Header(a, _)
            | Self::HeaderHash(a, _)
            | Self::FolderHash(a, _, _, _)
            | Self::Uid(a, _, _, _) => a,
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
        let tx = self
            .db
            .is_autocommit()
            .then(|| self.db.unchecked_transaction())
            .transpose()?;
        let identity = self.authenticate(token, ip)?;
        let key = MessageId::decode(id)?;
        let account = key.account();
        let level = self.agent_level(&identity, Resource::Mail, account)?;
        if level < Level::Read {
            return Err(Failure::OutsideScope);
        }
        let result = self.lookup_agent_key(&key, folder)?;
        if level < required {
            return Err(Code::Forbidden.into());
        }
        if required > Level::Read && result.ambiguous {
            return Err(Code::Conflict.into());
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(result)
    }

    pub fn agent_message_row(
        &self,
        account: &str,
        row: i64,
    ) -> Result<(MessageId, String, ResolvedMessage), Failure> {
        let (key, folder) = self.agent_message_key(account, row)?;
        let resolved = self.lookup_agent_key(&key, None)?;
        Ok((key, folder, resolved))
    }

    fn agent_hash_lookup(&self, account: &str, hash: &str, sql: &str) -> Result<String, Code> {
        let mut stmt = self.db.prepare(sql)?;
        for value in stmt.query_map([account], |r| r.get::<_, String>(0))? {
            let value = value?;
            if digest(&value) == hash {
                return Ok(value);
            }
        }
        Err(Code::NotFound)
    }

    pub fn agent_thread_key(&self, account: &str, hash: &str) -> Result<String, Code> {
        self.agent_hash_lookup(
            account,
            hash,
            "SELECT DISTINCT thread_id FROM messages WHERE account=?",
        )
    }

    fn lookup_agent_key(
        &self,
        key: &MessageId,
        folder: Option<&str>,
    ) -> Result<ResolvedMessage, Failure> {
        let key = match key {
            MessageId::HeaderHash(a, h) => MessageId::Header(
                a.clone(),
                self.agent_hash_lookup(
                    a,
                    h,
                    "SELECT DISTINCT message_id FROM messages WHERE account=?",
                )?,
            ),
            MessageId::FolderHash(a, h, v, u) => MessageId::Uid(
                a.clone(),
                self.agent_hash_lookup(a, h, "SELECT name FROM folders WHERE account=?")?,
                *v,
                *u,
            ),
            key => key.clone(),
        };
        let account = key.account();
        match &key {
            MessageId::HeaderHash(..) | MessageId::FolderHash(..) => unreachable!(),
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
        }.ok_or(Code::NotFound.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectId {
    Message(MessageId),
    Thread(String, String),
    ThreadHash(String, String),
    Draft(String, String),
    Event(String, String, String),
    Attachment(MessageId, String),
    SectionHash(MessageId, String),
}

impl ObjectId {
    pub fn encode(&self) -> String {
        match self {
            Self::Message(m) => m.encode(),
            Self::Thread(a, t) => {
                bounded(encode("t1", (a, t)), 2048, || encode("th", (a, digest(t))))
            }
            Self::ThreadHash(a, h) => encode("th", (a, h)),
            Self::Draft(a, d) => encode("d1", (a, d)),
            Self::Event(a, c, e) => encode("e1", (a, c, e)),
            Self::Attachment(m, s) => bounded(encode("a1", (m.encode(), s)), 2048, || {
                encode("ah", (m.encode(), digest(s)))
            }),
            Self::SectionHash(m, h) => encode("ah", (m.encode(), h)),
        }
    }

    pub fn decode(id: &str) -> Result<Self, Code> {
        if id.len() > 2048 {
            return Err(Code::NotFound);
        }
        let (prefix, data) = id.split_once('.').ok_or(Code::NotFound)?;
        match prefix {
            "m1" | "mh" | "u1" | "uh" => Ok(Self::Message(MessageId::decode(id)?)),
            "e1" => {
                let (a, c, e): (String, String, String) = parts(data)?;
                if a.is_empty() || c.is_empty() || e.is_empty() {
                    return Err(Code::NotFound);
                }
                Ok(Self::Event(a, c, e))
            }
            "t1" | "th" | "d1" | "a1" | "ah" => {
                let (a, b): (String, String) = parts(data)?;
                if a.is_empty() || b.is_empty() {
                    return Err(Code::NotFound);
                }
                match prefix {
                    "th" => Ok(Self::ThreadHash(a, b)),
                    "ah" => Ok(Self::SectionHash(MessageId::decode(&a)?, b)),
                    "t1" => Ok(Self::Thread(a, b)),
                    "d1" => Ok(Self::Draft(a, b)),
                    _ => Ok(Self::Attachment(MessageId::decode(&a)?, b)),
                }
            }
            _ => Err(Code::NotFound),
        }
    }
}

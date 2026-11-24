use super::{id::AuditTarget, protocol::Code, token::Identity};
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use std::net::IpAddr;

#[derive(Clone, Debug)]
pub enum Failure {
    Error(Code),
    Unauthorized(TokenFailure),
    #[non_exhaustive]
    VerifiedRefusal {
        reason: TokenFailure,
        agent: String,
        token: String,
    },
    VerifiedBusy {
        agent: String,
        token: String,
    },
    OutsideScope,
}

impl From<Failure> for Code {
    fn from(error: Failure) -> Self {
        match error {
            Failure::Unauthorized(_) | Failure::VerifiedRefusal { .. } => Code::Unauthorized,
            Failure::VerifiedBusy { .. } => Code::Busy,
            Failure::OutsideScope => Code::NotFound,
            Failure::Error(code) => code,
        }
    }
}

impl From<Code> for Failure {
    fn from(code: Code) -> Self {
        Self::Error(code)
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Error(error.into())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum TokenFailure {
    Missing,
    Malformed,
    UnknownOrRevoked,
    WrongSecret,
    Expired,
    Source,
    InvalidScope,
}

impl TokenFailure {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::UnknownOrRevoked => "unknown_or_revoked",
            Self::WrongSecret => "wrong_secret",
            Self::Expired => "expired",
            Self::Source => "source",
            Self::InvalidScope => "invalid_scope",
        }
    }
}

pub use lettre::Address;

pub struct AuditRow<'a> {
    pub identity: Option<&'a Identity>,
    pub account: Option<uuid::Uuid>,
    pub command: &'a str,
    pub target: Option<&'a AuditTarget>,
    pub outcome: Result<(), Failure>,
    pub source_ip: Option<IpAddr>,
    pub recipients: &'a [lettre::Address],
}

/// One audit row as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub at: i64,
    pub agent: Option<String>,
    pub account: Option<String>,
    pub command: String,
    pub result: String,
}

impl Store {
    /// Newest first.
    pub fn agent_audit(&self, limit: u32) -> Result<Vec<AuditEntry>, Code> {
        let mut rows = self.db.prepare(
            "SELECT at,agent,CASE WHEN refusals<=1 THEN account END,
               CASE WHEN refusals>1 THEN 'refusals' ELSE command END,
               CASE WHEN refusals>1 THEN refusals || ' refused' ELSE result END
             FROM (SELECT *,row_number() OVER (PARTITION BY agent ORDER BY at DESC,id DESC) n FROM agent_audit)
             WHERE n<=? ORDER BY at DESC,id DESC",
        )?;
        let rows = rows.query_map([limit], |r| {
            Ok(AuditEntry {
                at: r.get(0)?,
                agent: r.get(1)?,
                account: r.get(2)?,
                command: r.get(3)?,
                result: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count_anonymous_refusals(&self, minute: i64, count: i64) -> Result<(), Code> {
        self.db.execute(
            "INSERT INTO agent_audit(at,command,result) VALUES(?1,'refusals','unauthorized:' || ?2)
            ON CONFLICT(at) WHERE agent IS NULL AND command='refusals' DO UPDATE
            SET result='unauthorized:' || (CAST(substr(result,14) AS INTEGER)+?2)",
            params![minute, count],
        )?;
        Ok(())
    }

    pub fn prune_agent_audit(&self, now: i64) -> rusqlite::Result<()> {
        let keep_since = now.saturating_sub(90 * 86400);
        self.db
            .execute("DELETE FROM agent_audit WHERE at<?", [keep_since])?;
        Ok(())
    }

    pub fn record_agent_audit(&self, row: AuditRow<'_>) -> Result<(), Code> {
        self.record_agent_audit_at(row, chrono::Utc::now().timestamp())
    }

    pub(super) fn record_agent_audit_at(&self, row: AuditRow<'_>, now: i64) -> Result<(), Code> {
        if self.db.is_autocommit() {
            return self.agent_audit_transaction(|| self.record_agent_audit_at(row, now));
        }
        let tx = &self.db;
        let command = match row.command {
            "accounts" | "folders" | "emails" | "search" | "thread" | "attachment" | "contacts"
            | "events" | "availability" | "draft.list" | "draft.show" | "draft.create"
            | "draft.schedule" | "draft.edit" | "draft.delete" | "draft.send" | "action.archive" | "action.move"
            | "action.read" | "action.unread" | "action.star" | "action.unstar"
            | "action.snooze" | "action.unsnooze" | "action.trash" | "event.create"
            | "event.update" | "event.delete" => row.command,
            _ => "unknown",
        };
        let (agent, token) = match &row.outcome {
            Err(
                Failure::VerifiedRefusal { agent, token, .. }
                | Failure::VerifiedBusy { agent, token },
            ) => (Some(agent.as_str()), Some(token.as_str())),
            _ => (
                row.identity.map(Identity::agent_id),
                row.identity.map(Identity::token_id),
            ),
        };
        let result = match &row.outcome {
            Ok(()) => "ok".into(),
            Err(Failure::Error(code)) => serde_json::to_value(code)
                .map_err(|_| Code::Internal)?
                .as_str()
                .ok_or(Code::Internal)?
                .into(),
            Err(Failure::VerifiedBusy { .. }) => "busy".into(),
            Err(Failure::OutsideScope) => "not_found:outside_scope".into(),
            Err(Failure::Unauthorized(reason) | Failure::VerifiedRefusal { reason, .. }) => {
                format!("unauthorized:{}", reason.label())
            }
        };
        let minute = now - now.rem_euclid(60);
        let bucket = agent.is_some() && row.outcome.is_err();
        let mut aggregate = false;
        if bucket {
            aggregate = tx.execute(
                "UPDATE agent_audit SET refusals=refusals+1
                WHERE agent=? AND at=? AND refusals>0",
                params![agent, minute],
            )? > 0;
        } else if agent.is_none() {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM agent_audit WHERE at>=? AND agent IS NULL",
                [minute],
                |r| r.get(0),
            )?;
            aggregate = count >= 60;
            if aggregate {
                self.count_anonymous_refusals(minute, 1)?;
            }
        }
        if let Some(identity) = row.identity {
            tx.execute(
                "UPDATE agent_tokens SET last_used=? WHERE id=? AND agent=?",
                params![now, identity.token_id(), identity.agent_id()],
            )?;
        }
        if !aggregate {
            let detail = if matches!(
                command,
                "draft.send" | "event.create" | "event.update" | "event.delete"
            ) && !row.recipients.is_empty()
            {
                Some(
                    serde_json::to_string(
                        &row.recipients
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|_| Code::Internal)?,
                )
            } else {
                None
            };
            let target = match &row.outcome {
                Err(
                    Failure::OutsideScope
                    | Failure::Error(Code::NotFound | Code::Unauthorized)
                    | Failure::Unauthorized(_)
                    | Failure::VerifiedRefusal { .. },
                ) => None,
                _ if row.identity.is_some() => row.target.map(AuditTarget::audit_target),
                _ => None,
            };
            tx.execute("INSERT INTO agent_audit(at,agent,token,account,command,target,result,source_ip,detail,refusals) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![if bucket {minute} else {now},agent,token,row.account.map(|id| id.to_string()),command,
                    target,result,row.source_ip.map(|ip|ip.to_string()),detail,bucket])?;
        }
        let last: Option<i64> = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM agent_settings WHERE key='audit_pruned'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if last.is_none_or(|at| at <= now - 86400 || at > now) {
            self.prune_agent_audit(now)?;
            tx.execute(
                "INSERT OR REPLACE INTO agent_settings VALUES('audit_pruned',?)",
                [now],
            )?;
        }
        Ok(())
    }

    pub fn agent_audit_transaction<T>(
        &self,
        work: impl FnOnce() -> Result<T, Code>,
    ) -> Result<T, Code> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.db,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let result = work()?;
        tx.commit()?;
        Ok(result)
    }
}

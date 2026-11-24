use super::{id::ObjectId, protocol::Code, token::Identity};
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use std::net::IpAddr;

#[derive(Clone, Copy, Debug)]
pub enum Failure {
    Error(Code),
    Unauthorized(TokenFailure),
    OutsideScope,
}

impl From<Failure> for Code {
    fn from(error: Failure) -> Self {
        match error {
            Failure::Unauthorized(_) => Code::Unauthorized,
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

pub struct AuditRow<'a> {
    pub identity: Option<&'a Identity>,
    pub account: Option<uuid::Uuid>,
    pub command: &'a str,
    pub target: Option<&'a ObjectId>,
    pub outcome: Result<(), Failure>,
    pub source_ip: Option<IpAddr>,
    pub recipients: &'a [lettre::Address],
}

impl Store {
    pub fn prune_agent_audit(&self, now: i64) -> rusqlite::Result<()> {
        let keep_since = now.saturating_sub(90 * 86400);
        let excess: Option<i64> = self
            .db
            .query_row(
                "SELECT id FROM agent_audit ORDER BY at DESC,id DESC LIMIT 1 OFFSET 50000",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if excess.is_some()
            || self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_audit WHERE at<?)",
                [keep_since],
                |r| r.get::<_, bool>(0),
            )?
        {
            self.db.execute(
                "DELETE FROM agent_audit WHERE at<?1 OR (at,id) <=
                (SELECT at,id FROM agent_audit ORDER BY at DESC,id DESC LIMIT 1 OFFSET 50000)",
                [keep_since],
            )?;
        }
        Ok(())
    }

    pub fn record_agent_audit(&self, row: AuditRow<'_>) -> Result<(), Code> {
        self.record_agent_audit_at(row, chrono::Utc::now().timestamp())
    }

    pub(super) fn record_agent_audit_at(&self, row: AuditRow<'_>, now: i64) -> Result<(), Code> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.db,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let command = match row.command {
            "accounts" | "folders" | "emails" | "search" | "thread" | "attachment" | "contacts"
            | "events" | "availability" | "draft.list" | "draft.show" | "draft.create"
            | "draft.edit" | "draft.delete" | "draft.send" | "action.archive" | "action.move"
            | "action.read" | "action.unread" | "action.star" | "action.unstar"
            | "action.snooze" | "action.unsnooze" | "action.trash" | "event.create"
            | "event.update" | "event.delete" => row.command,
            _ => "unknown",
        };
        let result = match row.outcome {
            Ok(()) => "ok".into(),
            Err(Failure::Error(code)) => serde_json::to_value(code)
                .map_err(|_| Code::Internal)?
                .as_str()
                .ok_or(Code::Internal)?
                .into(),
            Err(Failure::OutsideScope) => "not_found:outside_scope".into(),
            Err(Failure::Unauthorized(reason)) => format!("unauthorized:{}", reason.label()),
        };
        let mut aggregate = false;
        if row.identity.is_none() {
            let minute = now - now.rem_euclid(60);
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM agent_audit WHERE at>=? AND agent IS NULL",
                [minute],
                |r| r.get(0),
            )?;
            aggregate = count >= 60;
            if aggregate && tx.execute("UPDATE agent_audit SET result='unauthorized:' || (CAST(substr(result,14) AS INTEGER)+1)
                WHERE at=? AND command='refusals'", [minute])? == 0 {
                tx.execute("INSERT INTO agent_audit(at,command,result) VALUES(?,'refusals','unauthorized:1')",[minute])?;
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
            tx.execute("INSERT INTO agent_audit(at,agent,token,account,command,target,result,source_ip,detail) VALUES(?,?,?,?,?,?,?,?,?)",
                params![now,row.identity.map(Identity::agent_id),row.identity.map(Identity::token_id),row.account.map(|id| id.to_string()),command,
                    row.target.map(ObjectId::encode),result,row.source_ip.map(|ip|ip.to_string()),detail])?;
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
        tx.commit()?;
        Ok(())
    }
}

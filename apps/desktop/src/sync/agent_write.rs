use super::*;
use crate::agent::{
    read::error,
    write::{calendar_allowed, calendar_error, draft_change, Job, Snapshot},
};
use lagom_core::agent::{
    audit::Failure,
    id::{AuditTarget, ObjectId},
    protocol::Code,
};
use serde_json::{json, Value};

impl Worker {
    pub(super) async fn agent_write(
        &mut self,
        job: &Job,
        target: &mut Option<AuditTarget>,
    ) -> Result<Value, Failure> {
        job.check(&self.store)?;
        let account = job.sender.clone();
        let r = &job.req;
        if r.cmd.starts_with("draft.") {
            let forward = if let Some(id) = r.args.get("forward").and_then(Value::as_str) {
                let resolved = self.store.resolve_agent_message(
                    r.token.as_deref().map(String::as_str),
                    r.source_ip.as_deref(),
                    id,
                    None,
                    lagom_core::agent::policy::Level::Read,
                )?;
                Some((resolved.row, self.forward_parts(resolved.row).await.map_err(error)?))
            } else {
                None
            };
            let value = self.store.agent_change(|| {
                let value = draft_change(&self.store, job, forward)?;
                if r.cmd == "draft.create" {
                    let key = ObjectId::decode(value["id"].as_str().ok_or(Code::Internal)?)?;
                    *target = Some(self.store.agent_object_target(&key)?);
                }
                Ok(value)
            })?;
            if let Snapshot::Mail(_, mail) = &job.snapshot {
                let id = mail.message_id.clone().ok_or(Code::Internal)?;
                self.emit(Event::AgentQueued(id.clone()));
                let me = self.me.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(crate::compose::UNDO_WINDOW).await;
                    let _ = me.send(Cmd::Send(id));
                });
            } else {
                let _ = self.me.send(Cmd::Drafts(account.id));
            }
            return Ok(value);
        }
        if let Snapshot::Event { calendar, old, new } = &job.snapshot {
            if !job.approved {
                return Err(Code::Denied.into());
            }
            calendar_allowed(&self.store, &account, calendar)?;
            self.cancel_calendar(&account.id);
            let store = &self.store;
            let refusal = std::cell::Cell::new(None);
            let check = || {
                job.check(store)
                    .and_then(|_| calendar_allowed(store, &account, calendar))
                    .map_err(|e| {
                        refusal.set(Some(e));
                        Error::Cancelled
                    })
            };
            let result = match (old, new) {
                (None, Some(draft)) => {
                    let created = agent_create(&account, calendar, draft, &check).await;
                    created.and_then(|created| {
                        let id = created.event.remote_id.clone();
                        let mut pending = PendingEvent {
                            account: account.clone(),
                            calendar: calendar.clone(),
                            created,
                            cached: false,
                            deleted: false,
                            retry_at: tokio::time::Instant::now() + Duration::from_secs(5),
                        };
                        let cached = self.cache_event(&mut pending);
                        if !pending.cached || pending.created.conference_pending {
                            self.pending_events.push(pending);
                        }
                        self.calendar_synced(&account, cached);
                        Ok(id)
                    })
                }
                (Some(old), draft) => {
                    let id = &old.event.remote_id;
                    self.pending_events.retain(|p| {
                        p.account.id != account.id || p.calendar != *calendar || p.created.event.remote_id != *id
                    });
                    self.create_tasks.retain(|_, (job, a, c)| {
                        !(a.id == account.id && c == calendar && matches!(job,CreateJob::Conference(e) if e==id))
                    });
                    agent_change(store, &account, calendar, id, &old.etag, draft.as_ref(), &check)
                        .await
                        .map(|_| id.clone())
                }
                _ => return Err(Code::Internal.into()),
            };
            if let Some(failure) = refusal.take() {
                return Err(failure);
            }
            if matches!(result, Err(Error::CalendarConsent)) {
                self.store.drop_calendar_scopes(&account.id).map_err(error)?;
            }
            let key = ObjectId::Event(account.id, calendar.clone(), result.map_err(calendar_error)?);
            if old.is_none() {
                *target = self.store.agent_object_target(&key).ok();
            }
            return Ok(json!({"id":key.encode()}));
        }
        let row = job.message(&self.store)?.row;
        let cmd = r.cmd.as_str();
        if matches!(cmd, "action.snooze" | "action.unsnooze") {
            let until = if cmd == "action.snooze" {
                let s = r
                    .args
                    .get("until")
                    .and_then(Value::as_str)
                    .ok_or(Code::InvalidRequest)?;
                Some(
                    s.parse::<i64>()
                        .ok()
                        .or_else(|| chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp()))
                        .filter(|t| *t > now())
                        .ok_or(Code::InvalidRequest)?,
                )
            } else {
                None
            };
            return self.store.agent_change(|| {
                job.check(&self.store)?;
                self.store.snooze(job.message(&self.store)?.row, until).map_err(error)?;
                Ok(json!({}))
            });
        }
        let folder = if cmd == "action.move" {
            let name = r
                .args
                .get("folder")
                .and_then(Value::as_str)
                .ok_or(Code::InvalidRequest)?;
            Some(
                self.store
                    .folders(&account.id, now())
                    .map_err(error)?
                    .iter()
                    .find(|f| f.name == name)
                    .ok_or(Code::NotFound)?
                    .id,
            )
        } else {
            None
        };
        let mut session = self.take(&account).await.map_err(error)?;
        job.check(&self.store)?;
        if job.message(&self.store)?.row != row {
            return Err(Code::Conflict.into());
        }
        if matches!(cmd, "action.move" | "action.archive" | "action.trash") && !session.targeted_move_supported() {
            return Err(Code::Unsupported.into());
        }
        let store = &mut self.store;
        use lagom_core::imap_sync::{Destination, MailFlag};
        let refusal = std::cell::Cell::new(None);
        let check = |store: &Store| {
            job.check(store)
                .and_then(|_| {
                    if job.message(store)?.row != row {
                        return Err(Code::Conflict.into());
                    }
                    Ok(())
                })
                .map_err(|e| {
                    refusal.set(Some(e));
                    Error::Cancelled
                })
        };
        let result = match cmd {
            "action.archive" | "action.trash" | "action.move" => {
                let dest = match cmd {
                    "action.archive" => Destination::Archive,
                    "action.trash" => Destination::Trash,
                    _ => Destination::Folder(folder.ok_or(Code::InvalidRequest)?),
                };
                session.move_checked(store, row, dest, check).await.map(|_| ())
            }
            "action.read" | "action.unread" => {
                session
                    .set_flag_checked(store, row, MailFlag::Seen, cmd == "action.read", check)
                    .await
            }
            "action.star" | "action.unstar" => {
                session
                    .set_flag_checked(store, row, MailFlag::Flagged, cmd == "action.star", check)
                    .await
            }
            _ => return Err(Code::InvalidRequest.into()),
        };
        self.give(&account.id, session, &result);
        if let Some(failure) = refusal.take() {
            return Err(failure);
        }
        result.map_err(error)?;
        Ok(json!({}))
    }
}

async fn agent_create(
    account: &Account,
    calendar: &str,
    draft: &NewEvent,
    check: impl Fn() -> lagom_core::Result<()>,
) -> lagom_core::Result<CreatedEvent> {
    #[cfg(test)]
    if let Some(base) = crate::agent::write::fixture_calendar(account) {
        return lagom_core::google::create_with(&base, calendar, draft, |_| {
            std::future::ready(check().map(|_| "fixture".to_owned().into()))
        })
        .await;
    }
    lagom_core::google::create_event_checked(account, calendar, draft, check).await
}

async fn agent_change(
    store: &Store,
    account: &Account,
    calendar: &str,
    id: &str,
    etag: &str,
    draft: Option<&NewEvent>,
    check: impl Fn() -> lagom_core::Result<()>,
) -> lagom_core::Result<()> {
    #[cfg(test)]
    if let Some(base) = crate::agent::write::fixture_calendar(account) {
        return lagom_core::google::change_with(store, &account.id, &base, calendar, id, etag, draft, |_| {
            std::future::ready(check().map(|_| "fixture".to_owned().into()))
        })
        .await;
    }
    lagom_core::google::change_event_checked(store, account, calendar, id, etag, draft, check).await
}

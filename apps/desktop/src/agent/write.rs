use super::{
    confirm::Sheet,
    read::{error, Result},
};
use crate::sync::{Bridge, Cmd};
use lagom_core::{
    account::{Account, Auth},
    agent::{
        audit::Failure,
        id::{AuditTarget, MessageId, ObjectId, ResolvedMessage},
        policy::{Level, Resource, Setting},
        protocol::{Code, Request},
        token::Identity,
    },
    calendar::NewEvent,
    compose::{self, ComposeKind, Original},
    contacts::split_recipients,
    google::{self, EventSnapshot},
    oauth,
    outbox::Draft,
    smtp::Outgoing,
    store::Store,
    Error,
};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};

pub(crate) struct Job {
    pub req: Request,
    pub sender: Account,
    pub generation: i64,
    pub snapshot: Snapshot,
    pub approved: bool,
    pub reply: oneshot::Sender<Result<(Value, Option<AuditTarget>)>>,
}

pub(crate) enum Snapshot {
    None,
    Mail(String, Outgoing),
    Event {
        calendar: String,
        old: Option<EventSnapshot>,
        new: Option<NewEvent>,
    },
}

fn field<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
    r.args.get(name)?.as_str()
}
fn required<'a>(r: &'a Request, name: &str) -> Result<&'a str> {
    Ok(field(r, name).ok_or(Code::InvalidRequest)?)
}

fn policy(cmd: &str) -> Result<(Resource, Level, &'static [&'static str])> {
    use {Level::*, Resource::*};
    Ok(match cmd {
        "draft.create" => (
            Mail,
            Triage,
            &["reply", "forward", "to", "cc", "bcc", "subject", "body"],
        ),
        "draft.edit" => (Mail, Triage, &["id", "revision", "to", "cc", "bcc", "subject", "body"]),
        "draft.delete" => (Mail, Triage, &["id"]),
        "draft.send" => (Mail, Send, &["id"]),
        "action.move" => (Mail, Triage, &["id", "folder"]),
        "action.snooze" => (Mail, Triage, &["id", "until", "folder"]),
        "action.archive" | "action.trash" | "action.read" | "action.unread" | "action.star" | "action.unstar"
        | "action.unsnooze" => (Mail, Triage, &["id", "folder"]),
        "event.create" => (
            Calendar,
            Send,
            &[
                "calendar",
                "title",
                "start",
                "end",
                "guests",
                "description",
                "location",
                "meet",
            ],
        ),
        "event.update" => (
            Calendar,
            Send,
            &[
                "id",
                "title",
                "start",
                "end",
                "guests",
                "description",
                "location",
                "meet",
            ],
        ),
        "event.delete" => (Calendar, Send, &["id"]),
        "draft.schedule" => return Err(Code::Unsupported.into()),
        _ => return Err(Code::InvalidRequest.into()),
    })
}

pub fn handles(cmd: &str) -> bool {
    cmd.starts_with("event.")
        || cmd.starts_with("action.")
        || cmd.starts_with("draft.") && !matches!(cmd, "draft.list" | "draft.show")
}

fn scope(store: &Store, who: &Identity, r: &Request) -> Result<String> {
    let (resource, _, keys) = policy(&r.cmd)?;
    if r.args
        .iter()
        .any(|(k, v)| !v.is_string() || k != "account" && !keys.contains(&k.as_str()))
    {
        return Err(Code::InvalidRequest.into());
    }
    let source = match (field(r, "reply"), field(r, "forward")) {
        (Some(_), Some(_)) => return Err(Code::InvalidRequest.into()),
        (a, b) => a.or(b).map(MessageId::decode).transpose()?,
    };
    let key = field(r, "id").map(ObjectId::decode).transpose()?;
    let scoped = match &key {
        Some(ObjectId::Draft(a, _)) if r.cmd.starts_with("draft.") => Some(a.as_str()),
        Some(ObjectId::Event(a, _, _)) if r.cmd.starts_with("event.") => Some(a.as_str()),
        Some(ObjectId::Message(m)) if r.cmd.starts_with("action.") => Some(m.account()),
        None if matches!(r.cmd.as_str(), "event.create" | "draft.create") => {
            source.as_ref().map(MessageId::account)
        }
        None => return Err(Code::InvalidRequest.into()),
        _ => return Err(Code::NotFound.into()),
    };
    let selector = field(r, "account");
    let mut accounts = store.agent_accounts(who, &[resource], scoped.or(selector.filter(|s| !s.contains('@'))))?;
    if let Some(sel) = selector {
        accounts.retain(|(id, email)| sel == id || email.eq_ignore_ascii_case(sel));
    }
    let account = match accounts.as_slice() {
        [(id, _)] => id,
        [] => return Err(Failure::OutsideScope),
        _ => return Err(Code::Conflict.into()),
    };

    Ok(account.clone())
}

fn require_level(store: &Store, who: &Identity, r: &Request, account: &str) -> Result<()> {
    let (resource, level, _) = policy(&r.cmd)?;
    let effective = store.agent_level(who, resource, account)?;
    if effective == Level::None {
        return Err(Failure::OutsideScope);
    }
    if effective < level {
        return Err(Code::Forbidden.into());
    }
    Ok(())
}

impl Job {
    pub fn check(&self, store: &Store) -> Result<Identity> {
        if !self.approved && self.reply.is_closed() {
            return Err(Code::Denied.into());
        }
        if store.agent_generation()? != Some(self.generation) {
            return Err(Code::Unauthorized.into());
        }
        let who = store.authenticate(
            self.req.token.as_deref().map(String::as_str),
            self.req.source_ip.as_deref(),
        )?;
        require_level(store, &who, &self.req, &self.sender.id)?;
        if store.account(&self.sender.id).map_err(error)? != self.sender {
            return Err(Code::Conflict.into());
        }
        Ok(who)
    }

    pub fn message(&self, store: &Store) -> Result<ResolvedMessage> {
        resolve(store, &self.req, required(&self.req, "id")?, Level::Triage)
    }
}

fn resolve(store: &Store, r: &Request, id: &str, level: Level) -> Result<ResolvedMessage> {
    let folder = if r.cmd == "action.move" {
        None
    } else {
        field(r, "folder")
    };
    store.resolve_agent_message(
        r.token.as_deref().map(String::as_str),
        r.source_ip.as_deref(),
        id,
        folder,
        level,
    )
}

fn draft_id(r: &Request) -> Result<String> {
    match ObjectId::decode(required(r, "id")?)? {
        ObjectId::Draft(_, id) => Ok(id),
        _ => Err(Code::NotFound.into()),
    }
}
fn owned(store: &Store, account: &str, id: &str, who: &Identity) -> Result<String> {
    let (revision, owner) = store.agent_draft_meta(account, id)?;
    if owner.as_deref() != Some(who.agent_id()) {
        return Err(Code::Forbidden.into());
    }
    Ok(revision)
}

fn scoped_draft(store: &Store, account: &str, id: &str) -> Result<Draft> {
    let draft = store.draft(id).map_err(error)?;
    if draft.id != id || draft.account != account {
        return Err(Code::Conflict.into());
    }
    Ok(draft)
}

fn outgoing(d: Draft) -> Result<Outgoing> {
    if d.forward_source.is_some() {
        return Err(Code::Unavailable.into());
    }
    let (in_reply_to, references) = d.reply.unwrap_or_default();
    let [to, cc] = d
        .recipient_tokens
        .unwrap_or_else(|| [split_recipients(&d.to), split_recipients(&d.cc)]);
    Ok(Outgoing {
        to,
        cc,
        bcc: split_recipients(&d.bcc),
        subject: d.subject,
        text: d.text,
        message_id: Some(d.id),
        attachments: d.attachments,
        quoted: d.quoted,
        in_reply_to,
        references,
    })
}

pub fn calendar_error(e: Error) -> Code {
    match e {
        Error::Conflict => Code::Conflict,
        Error::Uncertain => Code::Uncertain,
        Error::CalendarConsent => Code::Forbidden,
        Error::RecurringSeries => Code::Unsupported,
        Error::Invalid(_) => Code::InvalidRequest,
        e => error(e),
    }
}

pub fn calendar_allowed(store: &Store, account: &Account, calendar: &str) -> Result<()> {
    let c = store
        .calendars(&account.id)
        .map_err(error)?
        .into_iter()
        .find(|c| c.remote_id == calendar)
        .ok_or(Code::NotFound)?;
    if account.auth != Auth::OAuth(oauth::Provider::Google) {
        return Err(Code::Unsupported.into());
    }
    if !c.writable || !oauth::calendar_writable(&store.account_scopes(&account.id).map_err(error)?) {
        return Err(Code::Forbidden.into());
    }
    Ok(())
}

fn event_snapshot(store: &Store, r: &Request, account: &Account) -> Result<Snapshot> {
    let (calendar, id) = if r.cmd == "event.create" {
        (required(r, "calendar")?.to_owned(), None)
    } else {
        let ObjectId::Event(_, c, id) = ObjectId::decode(required(r, "id")?)? else {
            return Err(Code::NotFound.into());
        };
        (c, Some(id))
    };
    calendar_allowed(store, account, &calendar)?;
    let old = if let Some(id) = id {
        let fetched = tokio::runtime::Handle::current().block_on(fetch_event(account, &calendar, &id));
        if matches!(fetched, Err(Error::CalendarConsent)) {
            Store::agent_writer(store.path())?
                .drop_calendar_scopes(&account.id)
                .map_err(error)?;
        }
        Some(fetched.map_err(calendar_error)?)
    } else {
        None
    };
    let new = if r.cmd == "event.delete" {
        None
    } else {
        let e = old.as_ref().map(|s| &s.event);
        let start = field(r, "start")
            .or(e.map(|e| e.start.as_str()))
            .ok_or(Code::InvalidRequest)?;
        let draft = NewEvent {
            title: field(r, "title")
                .or(e.map(|e| e.title.as_str()))
                .ok_or(Code::InvalidRequest)?
                .into(),
            start: start.into(),
            end: field(r, "end")
                .or(e.map(|e| e.end.as_str()))
                .ok_or(Code::InvalidRequest)?
                .into(),
            location: field(r, "location").map(str::to_owned),
            all_day: start.len() == 10,
            time_zone: "UTC".into(),
            description: field(r, "description").map(str::to_owned),
            attendees: field(r, "guests").map(split_recipients).unwrap_or_default(),
            meet: field(r, "meet")
                .map(str::parse)
                .transpose()
                .map_err(|_| Code::InvalidRequest)?
                .unwrap_or(false),
        };
        draft.event("preview".into()).map_err(calendar_error)?;
        Some(draft)
    };
    Ok(Snapshot::Event { calendar, old, new })
}

pub(super) fn dispatch(
    store: &Store,
    who: &Identity,
    req: &Request,
    bridge: &Bridge,
    generation: i64,
    audit: &mut super::Audit,
    mut off: watch::Receiver<()>,
    peer: Option<&std::sync::Arc<std::os::unix::net::UnixStream>>,
) -> Result<Value> {
    let id = scope(store, who, req)?;
    audit.account = Some(id.clone());
    if req.cmd.starts_with("action.") {
        audit.target = Some(resolve(store, req, required(req, "id")?, Level::Read)?.target);
    } else if let Some(key) = field(req, "id") {
        audit.target = Some(store.agent_object_target(&ObjectId::decode(key)?)?);
    }
    require_level(store, who, req, &id)?;
    if req.cmd.starts_with("action.") {
        resolve(store, req, required(req, "id")?, Level::Triage)?;
    }
    if matches!(req.cmd.as_str(), "draft.edit" | "draft.delete") {
        owned(store, &id, &draft_id(req)?, who)?;
    }
    let account = store.account(&id).map_err(error)?;
    let snapshot = if req.cmd == "draft.send" {
        let d = draft_id(req)?;
        let revision = owned(store, &id, &d, who)?;
        let mail = outgoing(scoped_draft(store, &id, &d)?)?;
        lagom_core::smtp::compose(&account, &mail).map_err(|_| Code::InvalidRequest)?;
        Snapshot::Mail(revision, mail)
    } else if req.cmd.starts_with("event.") {
        event_snapshot(store, req, &account)?
    } else {
        Snapshot::None
    };
    let addresses: Vec<_> = match &snapshot {
        Snapshot::Mail(_, m) => m.to.iter().chain(&m.cc).chain(&m.bcc).collect(),
        Snapshot::Event { new, old, .. } => old
            .iter()
            .flat_map(|s| &s.attendees)
            .chain(new.iter().flat_map(|e| &e.attendees))
            .collect(),
        _ => vec![],
    };
    for a in addresses {
        if let Some(address) = lagom_core::contacts::recipient_address(a).and_then(|a| a.parse().ok()) {
            if !audit.recipients.contains(&address) {
                audit.recipients.push(address);
            }
        }
    }
    let confirm = matches!(snapshot, Snapshot::Event { .. })
        || matches!(snapshot, Snapshot::Mail(..)) && store.agent_setting(Setting::Confirm)?;
    let rt = tokio::runtime::Handle::current();
    if confirm {
        let agent = store
            .agents()?
            .into_iter()
            .find(|a| a.id == who.agent_id())
            .ok_or(Code::Unauthorized)?;
        let (_closed, done) = oneshot::channel();
        let (sheet, answer) = Sheet::new(
            agent,
            &account,
            req,
            &snapshot,
            done,
            std::sync::Arc::downgrade(peer.ok_or(Code::Denied)?),
        );
        let expires = sheet.expires;
        if !bridge.send(Cmd::AgentConfirm(sheet)) {
            return Err(Code::Unavailable.into());
        }
        rt.block_on(async {
            tokio::select! {
                biased;
                answer=tokio::time::timeout_at(expires,answer) =>
                    answer.map_err(|_|Code::Denied)?.map_err(|_|Code::Denied)?,
                _=off.changed()=>Err(Code::Denied),
            }
        })?;
    }
    let (reply, rx) = oneshot::channel();
    if !bridge.send(Cmd::AgentWrite(Box::new(Job {
        req: req.clone(),
        sender: account,
        generation,
        snapshot,
        approved: confirm,
        reply,
    }))) {
        return Err(Code::Unavailable.into());
    }
    let (value, created) = rt.block_on(async {
        tokio::select! {
            biased;
            r=rx=>r.map_err(|_|Code::Unavailable)?,
            _=off.changed(), if !confirm=>Err(Code::Denied.into()),
        }
    })?;
    if created.is_some() {
        audit.target = created;
    }
    Ok(value)
}

pub fn draft_change(
    store: &Store,
    job: &Job,
    forward: Option<(i64, Vec<lagom_core::smtp::OutgoingAttachment>)>,
) -> Result<Value> {
    let who = job.check(store)?;
    let account = &job.sender;
    let r = &job.req;
    let creating = r.cmd == "draft.create";
    let id = if creating {
        lagom_core::smtp::message_id(&account.email)
    } else {
        draft_id(r)?
    };
    if !creating {
        let meta = owned(store, &job.sender.id, &id, &who);
        let revision = match (&job.snapshot, meta) {
            (Snapshot::Mail(..), Err(Failure::Error(Code::NotFound))) => return Err(Code::Conflict.into()),
            (_, result) => result?,
        };
        if let Snapshot::Mail(expected, mail) = &job.snapshot {
            if expected != &revision {
                return Err(Code::Conflict.into());
            }
            if !job.approved && store.agent_setting(Setting::Confirm)? {
                return Err(Code::Denied.into());
            }
            store
                .enqueue(
                    account,
                    mail,
                    chrono::Utc::now().timestamp_millis() + crate::compose::UNDO_WINDOW.as_millis() as i64,
                )
                .map_err(error)?;
            return Ok(json!({"id":required(r,"id")?}));
        }
        if r.cmd == "draft.delete" {
            store.delete_draft(&id).map_err(error)?;
            return Ok(json!({}));
        }
        if required(r, "revision")? != revision {
            return Err(Code::Conflict.into());
        }
    }
    let mut d = if creating {
        Draft {
            id: id.clone(),
            account: account.id.clone(),
            ..Default::default()
        }
    } else {
        scoped_draft(store, &account.id, &id)?
    };
    if let Some(src) = field(r, "forward").or(field(r, "reply")) {
        let forwarding = field(r, "forward").is_some();
        let source = resolve(store, r, src, Level::Read)?;
        if source.ambiguous {
            return Err(Code::Conflict.into());
        }
        let m = store.message(source.row).map_err(error)?;
        let s = &m.summary;
        let original = Original {
            from: &s.from_name,
            address: &s.from_address,
            to: &s.to,
            subject: &s.subject,
            date: &chrono::DateTime::from_timestamp(s.date, 0)
                .ok_or(Code::Conflict)?
                .to_rfc3339(),
            text: &m.body_text,
        };
        let kind = if forwarding {
            ComposeKind::Forward
        } else {
            ComposeKind::Reply
        };
        d.subject = if forwarding {
            compose::forward_subject(&s.subject)
        } else {
            compose::reply_subject(&s.subject)
        };
        if forwarding {
            let (source, parts) = forward.ok_or(Code::Unavailable)?;
            if source != s.id {
                return Err(Code::Conflict.into());
            }
            d.attachments = parts;
        } else {
            d.to = s.from_address.clone();
            d.reply = Some(crate::mail::reply_headers(&s.message_id, &s.thread_key));
        }
        if let Some(html) = m.body_html {
            d.quoted = Some(compose::quote(
                original,
                &html,
                &store.inline_parts(s.id).map_err(error)?,
                kind,
            ));
        } else {
            d.text = compose::compose_body("", Some(original), kind);
        }
    }
    for (key, dest) in [
        ("to", &mut d.to),
        ("cc", &mut d.cc),
        ("bcc", &mut d.bcc),
        ("subject", &mut d.subject),
    ] {
        if let Some(v) = field(r, key) {
            *dest = v.into();
        }
    }
    if field(r, "to").is_some() || field(r, "cc").is_some() {
        d.recipient_tokens = None;
    }
    if let Some(body) = field(r, "body") {
        if creating {
            d.text.insert_str(0, body);
        } else {
            d.text = body.into();
        }
    }
    store.save_draft(&d).map_err(error)?;
    if creating {
        store.set_draft_agent(&id, &who)?;
    }
    Ok(
        json!({"id":ObjectId::Draft(account.id.clone(),id.clone()).encode(),"revision":store.draft_revision(&id).map_err(error)?}),
    )
}

// Each loopback fixture has its own account, including when socket tests run in parallel.
#[cfg(test)]
pub(crate) static CALENDAR_FIXTURES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::LazyLock::new(Default::default);

#[cfg(test)]
pub(crate) fn fixture_calendar(account: &Account) -> Option<String> {
    CALENDAR_FIXTURES.lock().unwrap().get(&account.id).cloned()
}

async fn fetch_event(account: &Account, calendar: &str, id: &str) -> lagom_core::Result<EventSnapshot> {
    #[cfg(test)]
    if let Some(base) = fixture_calendar(account) {
        return google::fetch_event_with(&base, calendar, id, |_| {
            std::future::ready(Ok("fixture".to_owned().into()))
        })
        .await;
    }
    google::fetch_event(account, calendar, id).await
}

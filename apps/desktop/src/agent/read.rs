use base64::{engine::general_purpose::STANDARD, Engine};
use lagom_core::{
    agent::{
        audit::Failure,
        filter::Filter,
        id::{MessageId, ObjectId, ResolvedMessage},
        policy::{Level, Resource},
        protocol::{Code, Request},
        token::Identity,
    },
    store::{MessageSummary, Store},
    Error,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

pub(super) type Result<T> = std::result::Result<T, Failure>;
pub(crate) fn error(e: Error) -> Code {
    match e {
        Error::Store(e) => e.into(),
        Error::NotFound => Code::NotFound,
        Error::Invalid("invalid_request" | "invalid_filter") => Code::InvalidRequest,
        Error::Invalid(
            "attachment exceeds 64 MiB" | "attachment is too large" | "agent response too large",
        ) => Code::TooLarge,
        Error::Invalid("unavailable")
        | Error::Network(_)
        | Error::Authentication
        | Error::ReauthRequired
        | Error::Secret => Code::Unavailable,
        _ => Code::Internal,
    }
}

#[derive(Clone)]
pub(super) struct ReadPolicy {
    token: Zeroizing<String>,
    ip: Option<String>,
    generation: i64,
    expires: Option<i64>,
    levels: Vec<(String, Resource, Level)>,
}

impl ReadPolicy {
    pub(super) fn check(&self, store: &Store) -> Result<Option<i64>> {
        let identity = store.authenticate(Some(&self.token), self.ip.as_deref())?;
        if store.agent_generation()? != Some(self.generation) {
            return Err(Code::Unauthorized.into());
        }
        for (account, resource, level) in &self.levels {
            if store.agent_level(&identity, *resource, account)? < *level {
                return Err(Failure::OutsideScope);
            }
        }
        Ok(identity.expires)
    }

    pub(super) fn expired(&self) -> bool {
        self.expires
            .is_some_and(|end| chrono::Utc::now().timestamp() >= end)
    }
}

pub(crate) const ATTACHMENT_LIMIT: usize = 20 * 1024 * 1024;

pub(crate) struct Attachment {
    access: ReadPolicy,
    message: MessageId,
    folder: Option<String>,
    pub section: String,
    pub reply: oneshot::Sender<std::result::Result<Vec<u8>, Code>>,
}

impl Attachment {
    pub(crate) fn resolve(&self, store: &Store) -> std::result::Result<ResolvedMessage, Code> {
        self.access.check(store)?;
        store
            .resolve_agent_message(
                Some(&self.access.token),
                self.access.ip.as_deref(),
                &self.message.encode(),
                self.folder.as_deref(),
                Level::Read,
            )
            .map_err(Code::from)
    }
}

fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            out.extend(c.escape_default());
        } else if matches!(c,'\u{200e}'|'\u{200f}'|'\u{061c}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
        {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

fn message(store: &Store, mut m: MessageSummary, body: bool) -> Result<Value> {
    let (key, mut folder, resolved) = store.agent_message_row(&m.account, m.id)?;
    let id = key.encode();
    if body && resolved.row != m.id {
        let preferred = store.message_summary(resolved.row).map_err(error)?;
        if preferred.thread_id == m.thread_id {
            m = preferred;
            folder = store.agent_message_key(&m.account, m.id)?.1;
        }
    }
    let date = chrono::DateTime::from_timestamp(m.date, 0)
        .ok_or(Code::Internal)?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut result = json!({"id":id,"account":m.account,"thread":ObjectId::Thread(m.account.clone(),m.thread_id).encode(),
        "folder":clean(&folder),"from":clean(&m.from_address),"subject":clean(&m.subject),"date":date,
        "unread":!m.seen,"starred":m.flagged,"ambiguous":resolved.ambiguous});
    if body {
        let text = store.agent_body(&m.account, m.id)?;
        let available = text != lagom_core::mime::BODY_UNAVAILABLE;
        result["body_available"] = json!(available);
        if available {
            let end = text.char_indices().nth(100_000).map(|(at, _)| at);
            result["body"] = json!(clean(&text[..end.unwrap_or(text.len())]));
            result["truncated"] = json!(end.is_some());
        }
        result["attachments"] = json!(store.attachments(m.id).map_err(error)?.into_iter().map(|p|
            json!({"id":ObjectId::Attachment(key.clone(),p.section).encode(),"filename":clean(&p.filename),
                   "mime_type":clean(&p.mime_type),"size":p.size})).collect::<Vec<_>>());
    }
    Ok(result)
}

pub(super) fn dispatch(
    store: &Store,
    who: &Identity,
    req: &Request,
    fetch: impl FnOnce(Attachment) -> bool,
    touched: &mut Option<String>,
    target: &mut Option<ResolvedMessage>,
    access: &std::sync::OnceLock<ReadPolicy>,
    mut off: tokio::sync::watch::Receiver<()>,
    mut policy: tokio::sync::watch::Receiver<Option<i64>>,
) -> Result<Value> {
    let cmd = req.cmd.as_str();
    let keys: &[&str] = match cmd {
        "accounts" | "folders" | "contacts" => &[],
        "emails" => &["filter", "page", "limit"],
        "search" => &["query", "in", "page", "limit"],
        "thread" => &["id"],
        "attachment" => &["id", "folder"],
        "events" | "availability" => &["start", "end"],
        _ => return Err(Code::InvalidRequest.into()),
    };
    if req
        .args
        .keys()
        .any(|k| k != "account" && !keys.contains(&k.as_str()))
    {
        return Err(Code::InvalidRequest.into());
    }
    let string = |key| {
        req.args
            .get(key)
            .map(|v| v.as_str().ok_or(Code::InvalidRequest))
            .transpose()
    };
    let number = |key| {
        req.args
            .get(key)
            .map(|v| v.as_i64().ok_or(Code::InvalidRequest))
            .transpose()
    };
    let selector = string("account")?;
    let resources: &[Resource] = match cmd {
        "accounts" => &[Resource::Mail, Resource::Calendar],
        "events" | "availability" => &[Resource::Calendar],
        _ => &[Resource::Mail],
    };
    let key = if matches!(cmd, "thread" | "attachment") {
        let key = ObjectId::decode(string("id")?.ok_or(Code::InvalidRequest)?)?;
        let account = match (&key, cmd) {
            (ObjectId::Thread(a, _) | ObjectId::ThreadHash(a, _), "thread") => a,
            (ObjectId::Attachment(m, _) | ObjectId::SectionHash(m, _), "attachment") => m.account(),
            _ => return Err(Code::NotFound.into()),
        };

        if store.agent_level(who, Resource::Mail, account)? < Level::Read {
            return Err(Failure::OutsideScope);
        }
        Some((account.to_owned(), key))
    } else {
        None
    };
    let account_id = key
        .as_ref()
        .map(|(a, _)| a.as_str())
        .or(selector.filter(|s| !s.contains('@')));
    let mut accounts = store.agent_accounts(who, resources, account_id)?;
    if let Some(sel) = selector {
        accounts.retain(|(id, email)| id == sel || email.eq_ignore_ascii_case(sel));
        if accounts.is_empty() {
            return Err(Failure::OutsideScope);
        }
        if accounts.len() != 1 {
            return Err(Code::Conflict.into());
        }
        if key.as_ref().is_some_and(|(a, _)| a != &accounts[0].0) {
            return Err(Failure::OutsideScope);
        }
    }
    if key.is_none() && accounts.len() == 1 {
        *touched = Some(accounts[0].0.clone());
    }
    let generation = store.agent_generation()?.ok_or(Code::Unauthorized)?;
    let mut levels = Vec::new();
    for (account, _) in &accounts {
        for resource in resources {
            let level = store.agent_level(who, *resource, account)?;
            if level >= Level::Read {
                levels.push((account.clone(), *resource, level));
            }
        }
    }
    let token = req.token.clone().ok_or(Code::Unauthorized)?;
    let policy_at_start = access.get_or_init(|| ReadPolicy {
        token,
        ip: req.source_ip.clone(),
        generation,
        levels,
        expires: who.expires,
    });
    let now = chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
    let mut size = 0;
    let mut push = |v: Value| -> Result<()> {
        size += serde_json::to_vec(&v).map_err(|_| Code::Internal)?.len() + 1;
        if size > super::MAX_RESPONSE {
            return Err(Code::TooLarge.into());
        }
        rows.push(v);
        Ok(())
    };
    match cmd {
        "accounts" => {
            for (id, email) in accounts {
                push(json!({"id":id,"email":clean(&email),
                    "mail":store.agent_level(who,Resource::Mail,&id)?,
                    "calendar":store.agent_level(who,Resource::Calendar,&id)?}))?;
            }
        }
        "folders" => {
            for (id, _) in accounts {
                for f in store.folders(&id, now).map_err(error)? {
                    push(
                        json!({"account":id,"name":clean(&f.name),"role":f.role.as_str(),"unread":f.unread}),
                    )?;
                }
            }
        }
        "contacts" => {
            for (id, _) in accounts {
                store
                    .agent_contacts(&id, |name, address| {
                        push(json!({"account":id,"name":clean(name),"address":clean(address)}))
                            .map_err(|_| Error::Invalid("agent response too large"))
                    })
                    .map_err(error)?;
            }
        }
        "emails" | "search" => {
            let query = if cmd == "search" {
                string("query")?.ok_or(Code::InvalidRequest)?
            } else {
                string("filter")?.unwrap_or("")
            };
            let filter = Filter::parse(query).map_err(error)?;
            let positive = |key| -> std::result::Result<Option<u32>, Code> {
                number(key)?
                    .map(|n| {
                        u32::try_from(n)
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or(Code::InvalidRequest)
                    })
                    .transpose()
            };
            let ids = accounts.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
            let (messages, next) = store
                .filtered_messages(
                    &ids,
                    &filter,
                    string("in")?,
                    now,
                    positive("page")?.unwrap_or(1),
                    positive("limit")?,
                )
                .map_err(error)?;
            for m in messages {
                push(message(store, m, false)?)?;
            }
            return Ok(json!({"messages":rows,"next_page":next}));
        }
        "thread" => {
            let (account, key) = key.as_ref().ok_or(Code::NotFound)?;
            let thread = match key {
                ObjectId::Thread(_, thread) => thread.clone(),
                ObjectId::ThreadHash(_, hash) => store.agent_thread_key(account, hash)?,
                _ => return Err(Code::NotFound.into()),
            };
            let mut headers = std::collections::HashSet::new();
            for m in store.thread_messages(account, &thread).map_err(error)? {
                if !m.message_id.is_empty() && !headers.insert(m.message_id.clone()) {
                    continue;
                }
                push(message(store, m, true)?)?;
            }
            if rows.is_empty() {
                return Err(Code::NotFound.into());
            }
            *touched = Some(account.clone());
            return Ok(json!({"messages":rows}));
        }
        "attachment" => {
            let (account, key) = key.as_ref().ok_or(Code::NotFound)?;
            let message = match key {
                ObjectId::Attachment(m, _) | ObjectId::SectionHash(m, _) => m,
                _ => return Err(Code::NotFound.into()),
            };
            let (reply, mut rx) = oneshot::channel();
            let mut request = Attachment {
                access: policy_at_start.clone(),
                message: message.clone(),
                folder: string("folder")?.map(str::to_owned),
                section: String::new(),
                reply,
            };
            let resolved = request.resolve(store)?;
            let part = store
                .attachments(resolved.row)
                .map_err(error)?
                .into_iter()
                .find(|p| match key {
                    ObjectId::Attachment(_, section) => &p.section == section,
                    _ => {
                        ObjectId::Attachment(message.clone(), p.section.clone()).encode()
                            == key.encode()
                    }
                })
                .ok_or(Code::NotFound)?;
            request.section = part.section.clone();
            *target = Some(resolved);
            *touched = Some(account.clone());
            if !fetch(request) {
                return Err(Code::Unavailable.into());
            }
            let bytes = tokio::runtime::Handle::current().block_on(async {
                let mut expires = who.expires;
                loop {
                    let expiry = async {
                        if let Some(end) = expires {
                            let millis = end.saturating_mul(1000).saturating_sub(chrono::Utc::now().timestamp_millis());
                            tokio::time::sleep(std::time::Duration::from_millis(millis.max(0) as u64)).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    };
                    tokio::select! {
                        biased;
                        _ = off.changed() => break Err(Code::Unavailable),
                        _ = expiry => break Err(Code::Unauthorized),
                        change = policy.changed() => {
                            if change.is_err() { break Err(Code::Unavailable); }
                            let fresh = Store::agent_reader(store.path(),|| false)?;
                            expires = policy_at_start.check(&fresh).map_err(Code::from)?;
                        },
                        r = &mut rx => break r.unwrap_or(Err(Code::Unavailable)),
                    }
                }
            })?;
            if bytes.len() > ATTACHMENT_LIMIT {
                return Err(Code::TooLarge.into());
            }
            return Ok(
                json!({"filename":clean(&part.filename),"mime_type":clean(&part.mime_type),"data":STANDARD.encode(bytes)}),
            );
        }
        "events" | "availability" => {
            let start = number("start")?.unwrap_or(now);
            let end = number("end")?.unwrap_or(start.saturating_add(86400));
            if start >= end {
                return Err(Code::InvalidRequest.into());
            }
            let (earliest, latest) = lagom_core::google::calendar_window(now);
            if cmd == "availability" && (start < earliest || end > latest) {
                return Err(Code::Unsupported.into());
            }
            if cmd == "availability" && accounts.is_empty() {
                return Ok(json!({"free":[],"busy":[]}));
            }
            let mut busy = Vec::new();
            for (id, _) in accounts {
                if cmd == "availability"
                    && !store
                        .calendar_window(&id)
                        .map_err(error)?
                        .is_some_and(|(a, b)| a <= start && b >= end)
                {
                    return Err(Code::Unsupported.into());
                }
                for row in store.agent_events(&id, start, end)? {
                    let e = row.event;
                    if cmd == "availability" {
                        busy.push((start.max(e.start_utc), end.min(e.end_utc)));
                    } else {
                        push(
                            json!({"account":id,"id":ObjectId::Event(id.clone(),row.calendar_id,e.remote_id).encode(),
                        "calendar":clean(&row.calendar),"title":clean(&e.title),"start":clean(&e.start),"end":clean(&e.end),"all_day":e.all_day}),
                        )?;
                    }
                }
            }
            if cmd == "availability" {
                busy.sort_unstable();
                let mut merged: Vec<(i64, i64)> = Vec::new();
                for (a, b) in busy {
                    if let Some(last) = merged.last_mut().filter(|last| last.1 >= a) {
                        last.1 = last.1.max(b);
                    } else {
                        merged.push((a, b));
                    }
                }
                let mut free = Vec::new();
                let mut cursor = start;
                for &(a, b) in &merged {
                    if cursor < a {
                        free.push((cursor, a));
                    }
                    cursor = b;
                }
                if cursor < end {
                    free.push((cursor, end));
                }
                return Ok(json!({"busy":merged,"free":free}));
            }
        }
        _ => return Err(Code::InvalidRequest.into()),
    }
    Ok(json!({(cmd):rows}))
}

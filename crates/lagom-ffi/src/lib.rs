//! The mobile boundary separates local access from serial network work; records hide core storage types.
use lagom_core::{account::{Account, Security, ServerConfig}, imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp::{self, Outgoing}, store::{FolderRole, MessageSummary, Store}};
use std::{future::Future, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};
use zeroize::Zeroizing;

uniffi::setup_scaffolding!();

type Result<T> = std::result::Result<T, MailError>;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MailError {
    #[error("{message}")]
    Failed { message: String },
}
impl From<lagom_core::Error> for MailError {
    fn from(error: lagom_core::Error) -> Self { failure(error.to_string()) }
}
fn failure(message: impl Into<String>) -> MailError { MailError::Failed { message: message.into() } }
fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64 }
fn milliseconds() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64 }
fn network<T>(future: impl Future<Output = lagom_core::Result<T>>) -> lagom_core::Result<T> {
    tokio::runtime::Handle::current().block_on(future)
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct MailAccount { pub id: String, pub email: String, pub display_name: String }
impl From<Account> for MailAccount {
    fn from(a: Account) -> Self { Self { id: a.id, email: a.email, display_name: a.display_name } }
}
#[derive(Clone, Debug, uniffi::Enum)]
pub enum TransportSecurity { Tls, StartTls, Plain }
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailServer { pub host: String, pub port: u16, pub username: String, pub security: TransportSecurity }
impl From<MailServer> for ServerConfig {
    fn from(s: MailServer) -> Self {
        Self { host: s.host, port: s.port, username: s.username, security: match s.security {
            TransportSecurity::Tls => Security::Tls, TransportSecurity::StartTls => Security::StartTls, TransportSecurity::Plain => Security::Plain,
        } }
    }
}
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum Mailbox { Inbox, Later, Done, Sent }
impl Mailbox {
    fn role(self) -> Option<FolderRole> { match self {
        Self::Inbox => Some(FolderRole::Inbox), Self::Later => None,
        Self::Done => Some(FolderRole::Archive), Self::Sent => Some(FolderRole::Sent),
    } }
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailRow {
    pub id: i64, pub account: String, pub sender_name: String, pub sender_address: String,
    pub subject: String, pub preview: String, pub date_unix: i64, pub unread: bool, pub has_attachments: bool,
}
impl From<MessageSummary> for MailRow {
    fn from(m: MessageSummary) -> Self { Self { id: m.id, account: m.account, sender_name: m.from_name,
        sender_address: m.from_address, subject: m.subject, preview: m.snippet, date_unix: m.date,
        unread: !m.seen, has_attachments: m.has_attachments } }
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailAttachment { pub section: String, pub filename: String, pub mime_type: String, pub size: u32 }
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailMessage { pub row: MailRow, pub text_body: String, pub html_body: Option<String>, pub attachments: Vec<MailAttachment> }
#[derive(Clone, Debug, uniffi::Record)]
pub struct ReplyDraft { pub account: String, pub to: String, pub subject: String, pub body: String, pub reply_to: i64 }
#[derive(Clone, Debug, uniffi::Record)]
pub struct SendFailure { pub id: String, pub subject: String, pub message: String }

#[derive(uniffi::Object)]
pub struct LagomCore { stores: Arc<Stores> }
struct Stores { local: Mutex<Store>, network: Mutex<Store> }

impl LagomCore {
    async fn run<T: Send + 'static>(&self, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_on(false, action).await
    }
    async fn run_network<T: Send + 'static>(&self, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_on(true, action).await
    }
    async fn run_on<T: Send + 'static>(&self, network: bool, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        let stores = self.stores.clone();
        // Network futures borrow SQLite across awaits; a separate WAL connection keeps local work available.
        tokio::task::spawn_blocking(move || {
            let mut store = (if network { &stores.network } else { &stores.local }).lock().map_err(|_| failure("mail worker stopped; reopen the app"))?;
            action(&mut store).map_err(MailError::from)
        }).await.map_err(|_| failure("mail worker stopped unexpectedly"))?
    }
}
fn account(store: &Store, id: &str) -> lagom_core::Result<Account> {
    store.accounts()?.into_iter().find(|a| a.id == id).ok_or(lagom_core::Error::NotFound)
}
async fn add(store: &Store, account: Account, password: Zeroizing<String>) -> lagom_core::Result<MailAccount> {
    let session = ImapSync::login(&account, &password).await?;
    smtp::verify(&account, &password).await?;
    account.set_password(&password)?;
    if let Err(error) = store.save_account(&account) {
        account.delete_password()?;
        return Err(error);
    }
    let _ = session.logout().await;
    Ok(account.into())
}
async fn sync_account(store: &mut Store, a: &Account) -> lagom_core::Result<()> {
    let password = a.password()?;
    let mut error = None;
    for id in store.due_outbox(&a.id, milliseconds())? {
        let pending = store.pending(&id)?;
        if store.claim_send(&id, milliseconds())? {
            if let Err(e) = smtp::send_queued(a, &password, &pending, store).await { error = Some(e); }
        }
    }
    let mut session = ImapSync::login(a, &password).await?;
    let result = session.sync(store, DEFAULT_LIMIT).await;
    if result.is_ok() { let _ = session.logout().await; }
    result?;
    match error { Some(error) => Err(error), None => Ok(()) }
}

#[uniffi::export(async_runtime = "tokio")]
impl LagomCore {
    #[uniffi::constructor]
    pub fn open(data_dir: String) -> Result<Arc<Self>> {
        // Reopening the same worker must not recover a send still owned by this process.
        static OPEN: std::sync::OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Weak<Stores>>>> = std::sync::OnceLock::new();
        std::fs::create_dir_all(&data_dir).map_err(lagom_core::Error::from)?;
        let dir = std::fs::canonicalize(data_dir).map_err(lagom_core::Error::from)?;
        let mut open = OPEN.get_or_init(Default::default).lock().map_err(|_| failure("mail registry unavailable"))?;
        if let Some(stores) = open.get(&dir).and_then(std::sync::Weak::upgrade) { return Ok(Arc::new(Self { stores })); }
        let store = Store::open(dir.join("lagom.sqlite3"))?;
        store.recover_outbox()?;
        let local = Store::open(dir.join("lagom.sqlite3"))?;
        let core = Arc::new(Self { stores: Arc::new(Stores { local: Mutex::new(local), network: Mutex::new(store) }) });
        open.insert(dir, Arc::downgrade(&core.stores));
        Ok(core)
    }
    pub async fn accounts(&self) -> Result<Vec<MailAccount>> {
        self.run(|s| Ok(s.accounts()?.into_iter().map(Into::into).collect())).await
    }
    pub async fn add_account(&self, email: String, password: String) -> Result<MailAccount> {
        let password = Zeroizing::new(password);
        self.run_network(move |s| network(async {
            let servers = match lagom_core::discover(&email).await {
                lagom_core::Discovery::Password { preset, .. } => (preset.imap, preset.smtp),
                lagom_core::Discovery::OAuth(provider) => {
                    let domain = match provider { lagom_core::oauth::Provider::Google => "gmail.com", lagom_core::oauth::Provider::Microsoft => "outlook.com" };
                    let (mut imap, mut smtp) = lagom_core::presets::servers(&format!("account@{domain}"))
                        .ok_or(lagom_core::Error::Invalid("provider has no password preset"))?;
                    imap.username = email.clone(); smtp.username = email.clone(); (imap, smtp)
                }
                lagom_core::Discovery::Manual => return Err(lagom_core::Error::Invalid("server settings not found; use Advanced")),
            };
            add(s, Account::new(email, servers.0, servers.1)?, password).await
        })).await
    }
    pub async fn add_account_manual(&self, email: String, password: String, imap: MailServer, smtp: MailServer) -> Result<MailAccount> {
        let password = Zeroizing::new(password);
        self.run_network(move |s| network(add(s, Account::new(email, imap.into(), smtp.into())?, password))).await
    }
    pub async fn remove_account(&self, id: String) -> Result<()> {
        self.run_network(move |s| { account(s, &id)?.delete_password()?; s.delete_account(&id) }).await
    }
    pub async fn sync(&self, account_id: String) -> Result<()> {
        self.run_network(move |s| { let a = account(s, &account_id)?; network(sync_account(s, &a)) }).await
    }
    pub async fn sync_all(&self) -> Result<()> {
        self.run_network(|s| network(async {
            let mut first_error = None;
            for a in s.accounts()? {
                if let Err(error) = sync_account(s, &a).await {
                    first_error.get_or_insert(lagom_core::Error::Folder { folder: a.email, source: Box::new(error) });
                }
            }
            match first_error { Some(error) => Err(error), None => Ok(()) }
        })).await
    }
    pub async fn next_snooze(&self) -> Result<Option<i64>> {
        self.run(|s| {
            let now = now();
            let mut next = None;
            for a in s.accounts()? {
                if let Some(due) = s.next_snooze(&a.id, now)? {
                    next = Some(next.map_or(due, |old: i64| old.min(due)));
                }
            }
            Ok(next)
        }).await
    }
    pub async fn inbox(&self, limit: u32, offset: u32) -> Result<Vec<MailRow>> {
        self.mailbox(None, Mailbox::Inbox, limit, offset).await
    }
    pub async fn mailbox(&self, account_id: Option<String>, mailbox: Mailbox, limit: u32, offset: u32) -> Result<Vec<MailRow>> {
        self.run(move |s| {
            if limit > 500 { return Err(lagom_core::Error::Invalid("page limit must be at most 500")); }
            if let Some(id) = &account_id { account(s, id)?; }
            Ok(s.mailbox_messages(account_id.as_deref(), mailbox.role(), now(), limit, offset)?.into_iter().map(Into::into).collect())
        }).await
    }
    pub async fn message(&self, id: i64) -> Result<MailMessage> {
        self.run(move |s| {
            let m = s.message(id)?;
            Ok(MailMessage { row: m.summary.into(), text_body: m.body_text, html_body: m.body_html,
                attachments: s.attachments(id)?.into_iter().map(|a| MailAttachment { section: a.section, filename: a.filename, mime_type: a.mime_type, size: a.size }).collect() })
        }).await
    }
    pub async fn later(&self, ids: Vec<i64>, until_unix: i64) -> Result<()> {
        self.run(move |s| {
            if until_unix <= now() { return Err(lagom_core::Error::Invalid("Later must be in the future")); }
            for id in &ids { s.message_summary(*id)?; }
            for id in ids { s.snooze(id, Some(until_unix))?; }
            Ok(())
        }).await
    }
    pub async fn done(&self, ids: Vec<i64>) -> Result<()> { self.move_messages(ids, false).await }
    pub async fn delete(&self, ids: Vec<i64>) -> Result<()> { self.move_messages(ids, true).await }
    pub async fn mark_read(&self, id: i64, read: bool) -> Result<()> {
        self.run_network(move |s| network(async {
            let a = account(s, &s.message_summary(id)?.account)?;
            let mut session = ImapSync::login(&a, &a.password()?).await?;
            session.mark_seen(s, id, read).await?;
            let _ = session.logout().await;
            Ok(())
        })).await
    }
    pub async fn reply_draft(&self, id: i64) -> Result<ReplyDraft> {
        self.run(move |s| {
            let m = s.message(id)?;
            let subject = if m.summary.subject.to_ascii_lowercase().starts_with("re:") { m.summary.subject } else { format!("Re: {}", m.summary.subject) };
            Ok(ReplyDraft { account: m.summary.account, to: m.summary.from_address, subject,
                body: format!("\n\n{}", m.body_text.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")), reply_to: id })
        }).await
    }
    /// Returns after durable enqueue. Sync delivers it; a transport error never invites a duplicate enqueue.
    pub async fn send(&self, account_id: String, to: Vec<String>, cc: Vec<String>, subject: String, body_markdown: String, reply_to: Option<i64>) -> Result<String> {
        self.run(move |s| {
            let a = account(s, &account_id)?;
            let mut outgoing = Outgoing { to, cc, subject, text: body_markdown, ..Default::default() };
            if let Some(id) = reply_to {
                let m = s.message_summary(id)?;
                if m.account != a.id { return Err(lagom_core::Error::Invalid("reply belongs to another account")); }
                if !m.message_id.is_empty() {
                    outgoing.in_reply_to = Some(format!("<{}>", m.message_id.trim_matches(['<', '>'])));
                    outgoing.references = m.thread_refs.split_whitespace().map(|r| format!("<{}>", r.trim_matches(['<', '>']))).collect();
                    outgoing.references.push(outgoing.in_reply_to.clone().unwrap_or_default());
                }
            }
            s.enqueue(&a, &outgoing, milliseconds())
        }).await
    }
    pub async fn send_failures(&self) -> Result<Vec<SendFailure>> {
        self.run(|s| Ok(s.failed_outbox()?.into_iter().map(|p| SendFailure { id: p.id, subject: p.mail.subject, message: p.error.unwrap_or(p.state) }).collect())).await
    }
    pub async fn retry_send(&self, id: String) -> Result<()> {
        self.run(move |s| {
            if s.retry_send(&id, milliseconds())? { Ok(()) } else { Err(lagom_core::Error::Invalid("message cannot be retried")) }
        }).await
    }
}
impl LagomCore {
    async fn move_messages(&self, ids: Vec<i64>, trash: bool) -> Result<()> {
        self.run_network(move |s| network(async {
            for id in ids {
                let a = account(s, &s.message_summary(id)?.account)?;
                let mut session = ImapSync::login(&a, &a.password()?).await?;
                if trash { session.trash(s, id).await?; } else {
                    session.archive(s, id).await?;
                    // A real move replaces the row; an already archived message keeps its snooze.
                    match s.snooze(id, None) {
                        Ok(()) | Err(lagom_core::Error::NotFound) => {},
                        Err(error) => return Err(error),
                    }
                }
                let _ = session.logout().await;
            }
            Ok(())
        })).await
    }
}

#[uniffi::export]
pub fn tomorrow_at_eight() -> Result<i64> {
    use chrono::{Local, TimeZone};
    let tomorrow = Local::now().date_naive().succ_opt().and_then(|date| date.and_hms_opt(8, 0, 0))
        .ok_or_else(|| failure("tomorrow is outside the calendar range"))?;
    Local.from_local_datetime(&tomorrow).earliest().map(|t| t.timestamp()).ok_or_else(|| failure("08:00 does not exist in the local time zone"))
}

#[cfg(test)]
mod tests;

#[uniffi::export]
pub fn split_recipients(value: String) -> Vec<String> {
    lagom_core::contacts::split_recipients(&value)
}

#[uniffi::export]
pub fn mail_document(html: String, dark: bool) -> String {
    use lagom_core::html::{csp, inert_links, styled};
    let (background, ink, accent, scheme) = if dark { ("#2C2C2C", "#F2F2EE", "#8AB0F5", "dark") }
        else { ("#FFFFFF", "#111110", "#2A5BB8", "light") };
    let look = if styled(&html) { "body{background:#fff;color:#000;padding:16px}".to_owned() }
        else { format!("body{{color:{ink}}}a{{color:{accent}}}") };
    format!(r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{}"><meta http-equiv="x-dns-prefetch-control" content="off"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="{scheme}"><style>html{{background:{background}}}body{{margin:0;font:14px/1.4 -apple-system;overflow-wrap:break-word}}img{{max-width:100%;height:auto}}{look}</style></head>{}"#, csp(false), inert_links(&html))
}

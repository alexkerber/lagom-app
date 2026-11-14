//! The mobile boundary separates local access, folder-state network steps and one-message fetches; records hide core storage types.
use lagom_core::{account::{Account, Security, ServerConfig}, imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp::{self, Outgoing}, store::{FolderRole, MessageSummary, Store}, unsubscribe::Unsubscribe};
use std::{collections::HashMap, future::Future, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};
use zeroize::Zeroizing;

uniffi::setup_scaffolding!();

mod secret_store;
mod drafts;
pub use drafts::{DraftRow, LocalDraft};

type Result<T> = std::result::Result<T, MailError>;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MailError {
    #[error("it is no longer here")]
    NotFound,
    // Kotlin exceptions already own message; the payload needs a distinct name.
    #[error("{reason}")]
    Failed { reason: String },
}
impl From<lagom_core::Error> for MailError {
    fn from(error: lagom_core::Error) -> Self {
        // The person reads Display; the log keeps the technical detail.
        match error { lagom_core::Error::NotFound => Self::NotFound, error => { eprintln!("lagom: {error:?}"); failure(error.to_string()) } }
    }
}
fn failure(message: impl Into<String>) -> MailError { MailError::Failed { reason: message.into() } }
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
pub enum SignInProvider { Google, Microsoft }
impl From<SignInProvider> for lagom_core::oauth::Provider {
    fn from(p: SignInProvider) -> Self { match p { SignInProvider::Google => Self::Google, SignInProvider::Microsoft => Self::Microsoft } }
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct SignInStart { pub url: String, pub callback_scheme: String }
/// Call once at app start, before a core syncs: token refresh needs the client IDs too.
/// `app_id` is the bundle or application ID; an empty client ID turns that provider off.
#[uniffi::export]
pub fn configure_sign_in(app_id: String, google_client_id: String, microsoft_client_id: String) {
    lagom_core::oauth::configure_mobile(app_id, google_client_id, microsoft_client_id);
}
#[uniffi::export]
pub fn sign_in_available(provider: SignInProvider) -> bool { lagom_core::oauth::mobile_available(provider.into()) }
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum Mailbox { Inbox, Later, Done, Sent, Trash }
impl Mailbox {
    fn role(self) -> Option<FolderRole> { match self {
        Self::Inbox => Some(FolderRole::Inbox), Self::Later => None,
        Self::Done => Some(FolderRole::Archive), Self::Sent => Some(FolderRole::Sent), Self::Trash => Some(FolderRole::Trash),
    } }
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailRow {
    pub id: i64, pub account: String, pub sender_name: String, pub sender_address: String,
    pub subject: String, pub preview: String, pub date_unix: i64, pub unread: bool, pub has_attachments: bool,
    // List actions use these visible members; reading may include other folders.
    pub thread_id: String, pub message_count: u32, pub message_ids: Vec<i64>,
}
impl From<MessageSummary> for MailRow {
    fn from(m: MessageSummary) -> Self { Self { thread_id: m.thread_id, message_count: 1, message_ids: vec![m.id], id: m.id, account: m.account, sender_name: m.from_name,
        sender_address: m.from_address, subject: m.subject, preview: m.snippet, date_unix: m.date,
        unread: !m.seen, has_attachments: m.has_attachments } }
}
fn conversation_rows(messages: Vec<MessageSummary>) -> Vec<MailRow> {
    let mut rows: Vec<MailRow> = Vec::new();
    let mut threads = std::collections::HashMap::new();
    for m in messages {
        let key = (m.account.clone(), m.folder, m.thread_id.clone());
        if let Some(&index) = threads.get(&key) {
            let row: &mut MailRow = &mut rows[index];
            row.message_count += 1;
            row.message_ids.push(m.id);
            row.unread |= !m.seen;
            row.has_attachments |= m.has_attachments;
        } else {
            threads.insert(key, rows.len());
            rows.push(m.into());
        }
    }
    rows
}

fn mail_message(s: &Store, id: i64) -> lagom_core::Result<MailMessage> {
    let m = s.message(id)?;
    let unsubscribe = s.unsubscribe(id)?.map(|choice| UnsubscribeOffer {
        // shortcut: the sender's name stands for the list; read List-Id when one sender runs several lists.
        list: if m.summary.from_name.is_empty() { m.summary.from_address.clone() } else { m.summary.from_name.clone() },
        explain: choice.explain().into(),
        link: match choice { Unsubscribe::Link(url) => Some(url.into()), _ => None },
    });
    Ok(MailMessage { unsubscribe, row: m.summary.into(), text_body: m.body_text, html_body: m.body_html,
        attachments: s.attachments(id)?.into_iter().map(|a| MailAttachment { section: a.section, filename: a.filename, mime_type: a.mime_type, size: a.size }).collect() })
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct MailAttachment { pub section: String, pub filename: String, pub mime_type: String, pub size: u32 }
#[derive(Clone, Debug, uniffi::Record)]
pub struct MailMessage { pub row: MailRow, pub text_body: String, pub html_body: Option<String>, pub attachments: Vec<MailAttachment>,
    #[uniffi(default = None)] pub unsubscribe: Option<UnsubscribeOffer> }
/// The quiet "Unsubscribe from <list>" line. Nothing runs until the person confirms.
#[derive(Clone, Debug, uniffi::Record)]
pub struct UnsubscribeOffer {
    pub list: String,
    /// What confirming does, for the confirm step.
    pub explain: String,
    /// Set only for a plain web page: the app opens it in the browser after the confirm, and never fetches it.
    pub link: Option<String>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct InlineResource { pub mime_type: String, pub data: Vec<u8> }
#[derive(Clone, Debug, uniffi::Record)]
pub struct ReplyDraft {
    pub account: String, pub to: String, pub cc: String, pub subject: String, pub body: String,
    pub reply_to: Option<i64>, pub attachments: Vec<SendAttachment>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ComposeKind { Reply, ReplyAll, Forward }
#[derive(Clone, Debug, uniffi::Record)]
pub struct SendAttachment { pub filename: String, pub mime_type: String, pub data: Vec<u8> }
#[derive(Clone, Debug, uniffi::Record)]
pub struct SendFailure { pub id: String, pub subject: String, pub message: String }
/// One message Done, Later or Delete changed. The app keeps it for Undo and passes it back unchanged.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MailChange {
    pub id: i64, pub account: String,
    /// The Message-ID header: a move gives the message a new store id, and this finds it.
    pub header_id: String,
    pub from_folder: i64, pub to_folder: i64, pub snoozed_until: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ContactSuggestion { pub name: String, pub address: String, pub recipient: String }

#[derive(uniffi::Object)]
pub struct LagomCore { stores: Arc<Stores>, background_cancel: Arc<std::sync::atomic::AtomicBool> }
struct Stores { path: std::path::PathBuf, local: Mutex<Store>,
    // One step that writes folder state (a folder sync, a move, a send). Two syncs of one folder race on its
    // checkpoint and can lose a message. Tokio's lock is FIFO: a waiting move runs between a sync's folders.
    network: tokio::sync::Mutex<Store>,
    // A whole sync, so two syncs never overlap. Order: sync, removal, network, local.
    sync: Mutex<()>,
    // One-message actions share it; removal takes it alone, so no token refresh writes a removed account's secret back.
    removal: std::sync::RwLock<()>,
    // Read flags the server confirmed. A folder step's older FETCH may write over them, so each step
    // writes them again before it lets go of `network`.
    read_flags: Mutex<HashMap<i64, bool>>,
    // One read flag at a time, so `read_flags` records them in the server's order.
    marking: Mutex<()>,
    contacts: Mutex<HashMap<String, lagom_core::contacts::Contacts>>,
    // shortcut: one browser sign-in at a time, lost if the app is killed in the browser; the user starts again.
    sign_in: Mutex<Option<lagom_core::oauth::Pending>> }

impl LagomCore {
    async fn attachment_bytes(&self, id: i64, section: String) -> Result<Vec<u8>> {
        let key = section.clone();
        let cached = self.run(move |s| {
            if !s.attachments(id)?.iter().any(|part| part.section == key) {
                return Err(lagom_core::Error::NotFound);
            }
            s.cached_attachment(id, &key)
        }).await?;
        if let Some(data) = cached { return Ok(data); }
        self.run_own(move |s| network(async {
            if !s.attachments(id)?.iter().any(|part| part.section == section) {
                return Err(lagom_core::Error::NotFound);
            }
            // A concurrent download may have populated the cache meanwhile.
            if let Some(data) = s.cached_attachment(id, &section)? { return Ok(data); }
            let a = account(s, &s.message_summary(id)?.account)?;
            let mut session = ImapSync::login(&a, &a.password()?).await?;
            let result = session.fetch_attachment(s, id, &section).await;
            let _ = session.logout().await;
            result
        })).await
    }
    async fn run<T: Send + 'static>(&self, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_on(Lane::Local, action).await
    }
    async fn run_network<T: Send + 'static>(&self, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_on(Lane::Network, action).await
    }
    /// One message's fetch or flag on its own IMAP session and WAL connection. It runs no folder sync, so it
    /// waits for no sync or download.
    async fn run_own<T: Send + 'static>(&self, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_on(Lane::Own, action).await
    }
    async fn run_on<T: Send + 'static>(&self, lane: Lane, action: impl FnOnce(&mut Store) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        let stores = self.stores.clone();
        // Network futures borrow SQLite across awaits; a separate WAL connection keeps local work available.
        tokio::task::spawn_blocking(move || {
            // A panic in one action must not disable the app: SQLite transactions keep the store consistent.
            match lane {
                Lane::Local => action(&mut stores.local.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
                Lane::Network => {
                    let mut store = stores.network.blocking_lock();
                    let result = action(&mut store);
                    stores.rewrite_read_flags(&store);
                    result
                }
                Lane::Own => {
                    let _removal = stores.removal.read().unwrap_or_else(std::sync::PoisonError::into_inner);
                    Store::open(&stores.path).and_then(|mut store| action(&mut store))
                }
            }.map_err(MailError::from)
        }).await.map_err(|_| failure("something went wrong; try again"))?
    }
    async fn run_sync<T: Send + 'static>(&self, action: impl FnOnce(&Stores) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        let stores = self.stores.clone();
        tokio::task::spawn_blocking(move || {
            let _sync = stores.sync.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            action(&stores).map_err(MailError::from)
        }).await.map_err(|_| failure("something went wrong; try again"))?
    }
}
#[derive(Clone, Copy)]
enum Lane { Local, Network, Own }
impl Stores {
    fn rewrite_read_flags(&self, store: &Store) {
        let flags = std::mem::take(&mut *self.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (id, seen) in flags {
            let written = store.message(id).and_then(|m| store.set_flags(m.summary.folder, m.summary.uid, seen, m.summary.flagged));
            // A moved or deleted row has nothing to keep; a failed write tries again, unless a newer flag came meanwhile.
            if !matches!(written, Ok(()) | Err(lagom_core::Error::NotFound)) {
                self.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(id).or_insert(seen);
            }
        }
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
/// Runs on a blocking thread under `Stores::sync` and takes `network` for one step at a time.
fn sync_account(stores: &Stores, a: &Account) -> lagom_core::Result<()> {
    let password = a.password()?;
    let mut error = None;
    {
        let store = &mut *stores.network.blocking_lock();
        let outbox = (|| -> lagom_core::Result<()> {
            for id in store.due_outbox(&a.id, milliseconds())? {
                // Undo send may remove the row after due_outbox listed it.
                let pending = match store.pending(&id) { Err(lagom_core::Error::NotFound) => continue, other => other? };
                if store.claim_send(&id, milliseconds())? {
                    // The Sent copy syncs the Sent folder.
                    if let Err(e) = network(smtp::send_queued(a, &password, &pending, store)) { error = Some(e); }
                }
            }
            Ok(())
        })();
        stores.rewrite_read_flags(store);
        outbox?;
    }
    let mut session = network(ImapSync::login(a, &password))?;
    let mut folders = network(session.list_folders(&stores.network.blocking_lock()))?;
    folders.sort_by_key(|f| f.role != FolderRole::Inbox);
    let mut session = Some(session);
    let mut first = None;
    for folder in folders {
        let mut store = stores.network.blocking_lock();
        let result = network(async {
            if session.is_none() { session = Some(ImapSync::login(a, &password).await?); }
            session.as_mut().unwrap().sync_folder(&mut store, folder.id, DEFAULT_LIMIT).await
        });
        stores.rewrite_read_flags(&store);
        drop(store);
        if let Err(e) = result {
            // A failed session may hold a half-read response; reconnect for the next folder.
            session = None;
            if e.is_account_wide() { return Err(e); }
            first.get_or_insert(lagom_core::Error::Folder { folder: folder.name, source: Box::new(e) });
        }
    }
    if let Some(session) = session { let _ = network(session.logout()); }
    if let Some(error) = first { return Err(error); }
    match error { Some(error) => Err(error), None => Ok(()) }
}

#[uniffi::export(async_runtime = "tokio")]
impl LagomCore {
    #[uniffi::constructor]
    pub fn open(data_dir: String) -> Result<Arc<Self>> {
        #[cfg(target_os = "android")]
        secret_store::require_installed()?;
        // Reopening the same worker must not recover a send still owned by this process.
        static OPEN: std::sync::OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Weak<Stores>>>> = std::sync::OnceLock::new();
        std::fs::create_dir_all(&data_dir).map_err(lagom_core::Error::from)?;
        let dir = std::fs::canonicalize(data_dir).map_err(lagom_core::Error::from)?;
        let mut open = OPEN.get_or_init(Default::default).lock().map_err(|_| failure("something went wrong; try again"))?;
        if let Some(stores) = open.get(&dir).and_then(std::sync::Weak::upgrade) { return Ok(Arc::new(Self { stores, background_cancel: Arc::default() })); }
        let path = dir.join("lagom.sqlite3");
        let store = Store::open(&path)?;
        store.recover_outbox()?;
        let local = Store::open(&path)?;
        let core = Arc::new(Self { stores: Arc::new(Stores { path, local: Mutex::new(local), network: tokio::sync::Mutex::new(store), sync: Mutex::new(()), removal: Default::default(), read_flags: Default::default(), marking: Mutex::new(()), contacts: Mutex::new(HashMap::new()), sign_in: Mutex::new(None) }), background_cancel: Arc::default() });
        open.insert(dir, Arc::downgrade(&core.stores));
        Ok(core)
    }
    pub async fn accounts(&self) -> Result<Vec<MailAccount>> {
        self.run(|s| Ok(s.accounts()?.into_iter().map(Into::into).collect())).await
    }
    pub async fn refresh_contacts(&self, account_id: String) -> Result<()> {
        let stores = self.stores.clone();
        self.run(move |s| {
            account(s, &account_id)?;
            let contacts = s.contacts(&account_id, now())?;
            stores.contacts.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(account_id, contacts);
            Ok(())
        }).await
    }
    pub fn suggest_contacts(&self, account_id: String, query: String) -> Vec<ContactSuggestion> {
        self.stores.contacts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&account_id).map(|contacts| contacts.suggest(&query)).unwrap_or_default()
            .into_iter().map(|c| ContactSuggestion { recipient: c.recipient(), name: c.name, address: c.address }).collect()
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
    /// Starts a system-browser sign-in. A new start replaces an unfinished one.
    pub fn begin_sign_in(&self, provider: SignInProvider) -> Result<SignInStart> {
        let (url, pending) = lagom_core::oauth::begin(provider.into())?;
        let start = SignInStart { url, callback_scheme: pending.scheme() };
        *self.stores.sign_in.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
        Ok(start)
    }
    /// `callback_url` is the redirect the browser handed back. Only the pending sign-in's own redirect
    /// (its URI and state) is taken; anything else is refused and leaves the sign-in pending.
    // shortcut: adds new accounts only. When a refresh token expires the user removes the account and
    // signs in again; use core's `reauthenticate` checks here once that happens in practice.
    pub async fn finish_sign_in(&self, callback_url: String) -> Result<MailAccount> {
        let callback_url = Zeroizing::new(callback_url);
        let pending = {
            let mut slot = self.stores.sign_in.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match slot.take() {
                Some(pending) if pending.accepts(&callback_url) => pending,
                other => { *slot = other; return Err(failure("Sign-in was interrupted. Try again.")); }
            }
        };
        self.run_network(move |s| network(async {
            let signed = lagom_core::oauth::finish(pending, &callback_url).await?;
            let account = signed.account;
            let saved = s.accounts().and_then(|all| {
                if all.iter().any(|a| a.email.eq_ignore_ascii_case(&account.email)) {
                    return Err(lagom_core::Error::Invalid("This account is already in lagom."));
                }
                s.save_authorized_account(&account, &signed.scopes)
            });
            // The sign-in stored its refresh token under the new account's id; an unsaved account must not keep it.
            if let Err(error) = saved { let _ = account.delete_password(); return Err(error); }
            Ok(account.into())
        })).await
    }
    pub async fn remove_account(&self, id: String) -> Result<()> {
        // A running sync finishes its account, and running one-message actions their token refresh, before the rows go.
        self.run_sync(move |stores| {
            let _removal = stores.removal.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let s = &mut *stores.network.blocking_lock();
            account(s, &id)?.delete_password()?;
            // Serialize with refresh, and evict even if the caller cancels this future.
            let _local = stores.local.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            s.delete_account(&id)?;
            stores.contacts.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
            Ok(())
        }).await
    }
    pub async fn sync(&self, account_id: String) -> Result<()> {
        self.run_sync(move |stores| { let a = account(&stores.network.blocking_lock(), &account_id)?; sync_account(stores, &a) }).await
    }
    pub async fn sync_all(&self) -> Result<()> {
        self.run_sync(|stores| {
            let mut first_error = None;
            let accounts = stores.network.blocking_lock().accounts()?;
            for a in accounts {
                if let Err(error) = sync_account(stores, &a) {
                    first_error.get_or_insert(lagom_core::Error::Folder { folder: a.email, source: Box::new(error) });
                }
            }
            match first_error { Some(error) => Err(error), None => Ok(()) }
        }).await
    }
    // UniFFI's Swift async bridge does not forward Task cancellation; the flag stops before the next account.
    pub fn cancel_background_sync(&self) { self.background_cancel.store(true, std::sync::atomic::Ordering::Relaxed); }
    /// Syncs only each Inbox and never sends: a cancel can never strand a claimed outbox row.
    // shortcut: a process kill after an Inbox sync drops its notifications; persist a notified-UID marker if that matters.
    pub async fn background_sync(&self) -> Result<Vec<MailRow>> {
        // A cancel may arrive before this call starts, so the flag is never reset: open a new handle per run.
        let cancel = self.background_cancel.clone();
        self.run_sync(move |stores| {
            let mut rows = Vec::new();
            let accounts = stores.network.blocking_lock().accounts()?;
            for a in accounts {
                let s = &mut *stores.network.blocking_lock();
                // After the lock: a cancel may arrive while a move holds it.
                if cancel.load(std::sync::atomic::Ordering::Relaxed) { break; }
                // No Inbox yet means no first sync: new_mail would stay silent anyway.
                let Some(before) = s.folders(&a.id, now())?.into_iter().find(|f| f.role == FolderRole::Inbox) else { continue };
                let synced = network(async {
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    let result = session.sync_folder(s, before.id, DEFAULT_LIMIT).await;
                    let _ = session.logout().await;
                    result
                });
                stores.rewrite_read_flags(s);
                // One account's failure (locked secret, network) must not stop the others.
                if synced.is_err() { continue; }
                let (after, messages) = (s.folder(before.id)?, s.messages_since(before.id, before.last_uid)?);
                rows.extend(lagom_core::new_mail::new_mail(&before, &after, messages).into_iter().map(MailRow::from));
            }
            Ok(rows)
        }).await
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
            Ok(conversation_rows(s.mailbox_conversations(account_id.as_deref(), mailbox.role(), now(), limit.saturating_add(offset))?).into_iter().skip(offset as usize).take(limit as usize).collect())
        }).await
    }
    pub async fn message(&self, id: i64) -> Result<MailMessage> {
        self.run(move |s| mail_message(s, id)).await
    }
    pub async fn conversation(&self, message_id: i64) -> Result<Vec<MailMessage>> {
        self.run(move |s| s.conversation(message_id)?.into_iter().map(|m| mail_message(s, m.id)).collect()).await
    }
    pub async fn attachment(&self, message_id: i64, section: String) -> Result<Vec<u8>> {
        self.attachment_bytes(message_id, section).await
    }
    pub async fn load_inline(&self, id: i64) -> Result<()> {
        let needed = self.run(move |s| {
            let message = s.message(id)?;
            // No HTML part and not loaded: sync left the body out, and fetch_inline fetches it.
            Ok(!s.inline_loaded(id)? && message.body_html.as_deref().is_none_or(|html| !lagom_core::inline::referenced_ids(html).is_empty()))
        }).await?;
        if !needed { return Ok(()); }
        self.run_own(move |s| network(async {
            if s.inline_loaded(id)? { return Ok(()); }
            let a = account(s, &s.message_summary(id)?.account)?;
            let mut session = ImapSync::login(&a, &a.password()?).await?;
            let result = session.fetch_inline(s, id).await;
            let _ = session.logout().await;
            result
        })).await
    }
    pub async fn inline_resource(&self, id: i64, request: String) -> Result<InlineResource> {
        self.run(move |s| {
            let part = s.inline_resource(id, &request)?;
            Ok(InlineResource { mime_type: part.mime_type, data: part.data })
        }).await
    }
    pub async fn search(&self, account_id: Option<String>, query: String, limit: u32, offset: u32) -> Result<Vec<MailRow>> {
        self.run(move |s| {
            if limit > 500 { return Err(lagom_core::Error::Invalid("page limit must be at most 500")); }
            if query.len() > 4096 { return Err(lagom_core::Error::Invalid("search query too long")); }
            if let Some(id) = &account_id { account(s, id)?; }
            let Some(query) = lagom_core::store::fts_query(&query) else { return Ok(Vec::new()); };
            Ok(conversation_rows(s.search_conversations(account_id.as_deref(), &query, now(), limit, offset)?))
        }).await
    }
    /// Returns what Undo needs; Later only changes the snooze, so Undo stays local.
    pub async fn later(&self, ids: Vec<i64>, until_unix: i64) -> Result<Vec<MailChange>> {
        self.run(move |s| {
            if until_unix <= now() { return Err(lagom_core::Error::Invalid("Later must be in the future")); }
            let before = ids.iter().map(|id| s.message_summary(*id)).collect::<lagom_core::Result<Vec<_>>>()?;
            for id in ids { s.snooze(id, Some(until_unix))?; }
            Ok(before.into_iter().map(|m| MailChange { id: m.id, account: m.account, header_id: m.message_id,
                from_folder: m.folder, to_folder: m.folder, snoozed_until: m.snoozed_until }).collect())
        }).await
    }
    pub async fn done(&self, ids: Vec<i64>) -> Result<Vec<MailChange>> { self.move_messages(ids, false).await }
    pub async fn delete(&self, ids: Vec<i64>) -> Result<Vec<MailChange>> { self.move_messages(ids, true).await }
    /// Puts Done, Later or Delete back: each message returns to the folder it left, with its old snooze.
    pub async fn undo(&self, changes: Vec<MailChange>) -> Result<()> {
        self.run_network(move |s| network(async {
            for c in changes {
                let id = if c.from_folder == c.to_folder { c.id } else {
                    let a = account(s, &c.account)?;
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    // The move dropped the old row; the target folder's sync made a new one.
                    // shortcut: a second copy with the same Message-ID in that folder may go back instead; track UIDs if that shows up.
                    let current = match s.find_message(c.to_folder, &c.header_id)? {
                        Some(id) => id,
                        None => {
                            session.sync_folder(s, c.to_folder, DEFAULT_LIMIT).await?;
                            s.find_message(c.to_folder, &c.header_id)?.ok_or(lagom_core::Error::NotFound)?
                        }
                    };
                    session.move_message(s, current, c.from_folder).await?;
                    let _ = session.logout().await;
                    // shortcut: a failed refresh after the move back leaves no row, so an old snooze is lost; the mail is back either way.
                    match s.find_message(c.from_folder, &c.header_id)? { Some(id) => id, None => continue }
                };
                s.snooze(id, c.snoozed_until)?;
            }
            Ok(())
        })).await
    }
    pub async fn mark_read(&self, id: i64, read: bool) -> Result<()> {
        let stores = self.stores.clone();
        self.run_own(move |s| {
            let _marking = stores.marking.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            network(async {
                let a = account(s, &s.message_summary(id)?.account)?;
                let mut session = ImapSync::login(&a, &a.password()?).await?;
                session.mark_seen(s, id, read).await?;
                stores.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, read);
                let _ = session.logout().await;
                Ok(())
            })
        }).await
    }
    pub async fn reply_draft(&self, id: i64) -> Result<ReplyDraft> {
        self.compose_draft(id, ComposeKind::Reply).await
    }
    pub async fn compose_draft(&self, id: i64, kind: ComposeKind) -> Result<ReplyDraft> {
        let mut draft = self.run(move |s| {
            use lagom_core::compose::{self, Original};
            use chrono::{Local, TimeZone};
            let message = s.message(id)?;
            let m = &message.summary;
            let a = account(s, &m.account)?;
            let (to, cc) = match kind {
                ComposeKind::Reply => (m.from_address.clone(), String::new()),
                ComposeKind::ReplyAll => {
                    // shortcut: identities are the account email and logins; include aliases when accounts store them.
                    compose::reply_all(&m.from_address, &m.to, &m.cc, &[&a.email, &a.imap.username, &a.smtp.username])
                }
                ComposeKind::Forward => (String::new(), String::new()),
            };
            let forward = kind == ComposeKind::Forward;
            let subject = if forward { compose::forward_subject(&m.subject) } else { compose::reply_subject(&m.subject) };
            let date = Local.timestamp_opt(m.date, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            let body = compose::compose_body(&s.signature(&a.id)?, Some(Original {
                from: &m.from_name, address: &m.from_address, date: &date, subject: &m.subject, to: &m.to, text: &message.body_text,
            }), if forward { compose::ComposeKind::Forward } else { compose::ComposeKind::Reply });
            Ok(ReplyDraft { account: a.id, to, cc, subject, body, reply_to: (!forward).then_some(id), attachments: vec![] })
        }).await?;
        if kind == ComposeKind::Forward {
            let cached = self.run(move |s| s.cached_forward_attachments(id)).await?;
            draft.attachments = match cached {
                Some(parts) => parts.into_iter().map(|a| SendAttachment { filename: a.filename, mime_type: a.mime_type, data: a.data }).collect(),
                // One login covers every uncached part; `attachment` opens one part at a time.
                None => self.run_own(move |s| network(async {
                    let a = account(s, &s.message_summary(id)?.account)?;
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    let mut parts = Vec::new();
                    for part in s.attachments(id)? {
                        let data = match s.cached_attachment(id, &part.section)? {
                            Some(data) => data,
                            None => session.fetch_attachment(s, id, &part.section).await?,
                        };
                        parts.push(SendAttachment { filename: part.filename, mime_type: part.mime_type, data });
                    }
                    let _ = session.logout().await;
                    Ok(parts)
                })).await?,
            };
        }
        Ok(draft)
    }
    /// Returns after durable enqueue. Sync delivers it; a transport error never invites a duplicate enqueue.
    pub async fn send(&self, account_id: String, to: Vec<String>, cc: Vec<String>, subject: String, body_markdown: String, reply_to: Option<i64>, attachments: Vec<SendAttachment>) -> Result<String> {
        self.run(move |s| {
            let a = account(s, &account_id)?;
            let mut outgoing = Outgoing { to, cc, subject, text: body_markdown,
                attachments: attachments.into_iter().map(|a| smtp::OutgoingAttachment { filename: a.filename, mime_type: a.mime_type, data: a.data }).collect(),
                ..Default::default() };
            if let Some(id) = reply_to {
                let m = s.message_summary(id)?;
                if m.account != a.id { return Err(lagom_core::Error::Invalid("reply belongs to another account")); }
                if !m.message_id.is_empty() {
                    outgoing.in_reply_to = Some(format!("<{}>", m.message_id.trim_matches(['<', '>'])));
                    outgoing.references = m.thread_refs.split_whitespace().map(|r| format!("<{}>", r.trim_matches(['<', '>']))).collect();
                    outgoing.references.push(outgoing.in_reply_to.clone().unwrap_or_default());
                }
            }
            s.enqueue(&a, &outgoing, milliseconds() + 5000)
        }).await
    }
    /// After the person confirms: the one-click POST, or the unsubscribe mail sent now. A web page is the app's to open.
    pub async fn unsubscribe(&self, message_id: i64) -> Result<()> {
        match self.run(move |s| s.unsubscribe(message_id)?.ok_or(lagom_core::Error::NotFound)).await? {
            Unsubscribe::OneClick(url) => Ok(lagom_core::unsubscribe::one_click(&url).await?),
            Unsubscribe::Mail(_) => self.run_network(move |s| network(async {
                // The password first: a locked keychain must not leave a queued row behind.
                let a = account(s, &s.message_summary(message_id)?.account)?;
                let password = a.password()?;
                let id = s.enqueue_unsubscribe(message_id)?;
                let pending = s.pending(&id)?;
                // A failed send stays in the outbox and shows in send_failures, like any other.
                if s.claim_send(&id, milliseconds())? {
                    match smtp::send_queued(&a, &password, &pending, s).await {
                        // Submitted; only the Sent copy failed.
                        Ok(()) | Err(lagom_core::Error::SentCopy) => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })).await,
            Unsubscribe::Link(_) => Err(failure("open the unsubscribe page in the browser")),
        }
    }
    /// The account's signature, as Settings shows it.
    pub async fn signature(&self, account_id: String) -> Result<String> {
        self.run(move |s| { account(s, &account_id)?; s.signature(&account_id) }).await
    }
    pub async fn set_signature(&self, account_id: String, text: String) -> Result<()> {
        self.run(move |s| { account(s, &account_id)?; s.set_signature(&account_id, &text) }).await
    }
    /// A new message's body: the signature under two empty lines, as on desktop, or nothing.
    pub async fn new_mail_body(&self, account_id: String) -> Result<String> {
        self.run(move |s| Ok(lagom_core::compose::compose_body(&s.signature(&account_id)?, None, lagom_core::compose::ComposeKind::Reply))).await
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
    async fn move_messages(&self, ids: Vec<i64>, trash: bool) -> Result<Vec<MailChange>> {
        self.run_network(move |s| network(async {
            let mut changes = Vec::new();
            // shortcut: a failure mid-conversation drops Undo for the members already moved; they stay in Done or Trash.
            for id in ids {
                let m = s.message_summary(id)?;
                let a = account(s, &m.account)?;
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
                let role = if trash { FolderRole::Trash } else { FolderRole::Archive };
                // The folder that now holds it: an account can have more than one folder with this role.
                let folders: Vec<i64> = s.folder_metadata(&a.id)?.into_iter().filter(|f| f.role == role).map(|f| f.id).collect();
                let mut to = folders.iter().copied().find(|f| *f == m.folder);
                for f in &folders {
                    if to.is_none() && s.find_message(*f, &m.message_id)?.is_some() { to = Some(*f); }
                }
                // shortcut: a message without Message-ID cannot be found after a move, so Undo leaves it there, as on desktop.
                if let Some(to) = to {
                    changes.push(MailChange { id, account: m.account, header_id: m.message_id, from_folder: m.folder, to_folder: to, snoozed_until: m.snoozed_until });
                }
            }
            Ok(changes)
        })).await
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LaterChoice {
    pub label: String,
    pub until_unix: i64,
}

#[uniffi::export]
impl LagomCore {
    /// Local times, ordered evening, tomorrow (the swipe default), next Monday.
    pub fn later_choices(&self) -> Vec<LaterChoice> {
        use lagom_core::later::LaterWhen;
        let now = chrono::Local::now();
        [LaterWhen::Evening, LaterWhen::Tomorrow, LaterWhen::NextWeek].into_iter()
            .map(|when| LaterChoice { label: when.label().into(), until_unix: when.at(now).timestamp() }).collect()
    }
}

mod planner;

#[cfg(test)]
mod tests;

#[uniffi::export]
pub fn split_recipients(value: String) -> Vec<String> {
    lagom_core::contacts::split_recipients(&value)
}

#[uniffi::export]
pub fn mail_document(html: String, dark: bool, message_id: i64, remote: bool) -> String {
    use lagom_core::html::{csp, inert_tags, styled};
    let (background, ink, accent, scheme) = if dark { ("#2C2C2C", "#F2F2EE", "#8AB0F5", "dark") }
        else { ("#FFFFFF", "#111110", "#2A5BB8", "light") };
    let look = if styled(&html) { "body{background:#fff;color:#000;padding:16px}".to_owned() }
        else { format!("body{{color:{ink}}}a{{color:{accent}}}") };
    let html = lagom_core::inline::rewrite(&html, message_id, false);
    format!(r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{}"><meta http-equiv="x-dns-prefetch-control" content="off"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="{scheme}"><style>html{{background:{background}}}body{{margin:0;font:14px/1.4 system-ui,-apple-system,sans-serif;overflow-wrap:break-word}}img{{max-width:100%;height:auto}}{look}</style></head>{}"#, csp(remote, "lagom-cid:"), inert_tags(&html))
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Mailto { pub to: Vec<String>, pub cc: Vec<String>, pub bcc: Vec<String>, pub subject: String, pub body: String }

/// `None`: not a mailto link, or too long. See `lagom_core::compose::parse_mailto`.
#[uniffi::export]
pub fn parse_mailto(url: String) -> Option<Mailto> {
    lagom_core::compose::parse_mailto(&url).map(|m| Mailto { to: m.to, cc: m.cc, bcc: m.bcc, subject: m.subject, body: m.body })
}

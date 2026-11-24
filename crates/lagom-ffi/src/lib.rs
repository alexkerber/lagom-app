//! The mobile boundary separates local access, folder-state network steps and one-message fetches; records hide core storage types.
use lagom_core::{account::{Account, Security, ServerConfig}, imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp::{self, Outgoing}, store::{FolderRole, MessageSummary, Store}, unsubscribe::Unsubscribe};
use std::{collections::HashMap, future::Future, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};
use zeroize::Zeroizing;

uniffi::setup_scaffolding!();

mod secret_store;
mod drafts;
mod sync_status;
pub use sync_status::{sync_status, SyncState, SyncStatus};
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
pub struct MailAccount { pub id: String, pub email: String, pub display_name: String,
    /// How it signs in, in Settings' words ("Email and password", "Google account").
    #[uniffi(default = "")] pub kind: String, #[uniffi(default = false)] pub google_hosted_password: bool }
impl From<Account> for MailAccount {
    fn from(a: Account) -> Self { Self { google_hosted_password: a.google_hosted_password(), kind: a.auth.kind().into(), id: a.id, email: a.email, display_name: a.display_name } }
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
#[derive(Clone, Debug, uniffi::Record)]
pub struct SignInResult { pub account: MailAccount, pub calendar_grant: bool }
/// Call once at app start, before a core syncs: token refresh needs the client IDs too.
/// `app_id` is the bundle or application ID; an empty client ID turns that provider off.
#[uniffi::export]
pub fn configure_sign_in(app_id: String, google_client_id: String, microsoft_client_id: String) {
    lagom_core::oauth::configure_mobile(app_id, google_client_id, microsoft_client_id);
}
#[uniffi::export]
pub fn sign_in_available(provider: SignInProvider) -> bool { lagom_core::oauth::mobile_available(provider.into()) }
#[uniffi::export]
pub fn is_outlook_address(email: String) -> bool { lagom_core::discovery::is_outlook_address(&email) }
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
    /// An HTML original: shown under the editor with `mail_document`, saved and sent with the draft.
    #[uniffi(default = None)] pub quoted: Option<QuotedMail>,
}
/// See `lagom_core::compose::Quoted`. `html` holds no `cid:` images, so it needs no message.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QuotedMail { pub text: String, pub html: String }
impl From<lagom_core::compose::Quoted> for QuotedMail {
    fn from(q: lagom_core::compose::Quoted) -> Self { Self { text: q.text, html: q.html } }
}
impl From<QuotedMail> for lagom_core::compose::Quoted {
    fn from(q: QuotedMail) -> Self { Self { text: q.text, html: q.html } }
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
/// A server folder without a fixed mailbox of its own: desktop's folder group and its "Move to" targets.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MailFolder {
    pub id: i64, pub account: String,
    /// The last part of the server name, "Trains"; `path` is the whole name, "Travel / Trains".
    pub name: String, pub path: String, pub depth: u32, pub unread: u32,
}
fn mail_folders(s: &Store, account_id: Option<&str>) -> lagom_core::Result<Vec<MailFolder>> {
    let mut folders = Vec::new();
    for a in s.accounts()?.into_iter().filter(|a| account_id.is_none_or(|id| a.id == id)) {
        let mut parts: Vec<(Vec<String>, MailFolder)> = s.folders(&a.id, now())?.into_iter()
            // Inbox, Sent, Drafts, Done and Trash have their own places, as in desktop's `folders::merge`.
            .filter(|f| matches!(f.role, FolderRole::Junk | FolderRole::Other))
            .map(|f| {
                let names: Vec<String> = match f.delimiter.as_deref().filter(|d| !d.is_empty()) {
                    Some(d) => f.name.split(d).map(str::to_string).collect(),
                    None => vec![f.name.clone()],
                };
                let folder = MailFolder { id: f.id, account: f.account, name: names.last().cloned().unwrap_or_default(),
                    path: names.join(" / "), depth: names.len().saturating_sub(1) as u32, unread: f.unread.min(u32::MAX as u64) as u32 };
                (names, folder)
            }).collect();
        // By path parts, so a folder's children follow it before a sibling such as "Travel 2".
        parts.sort_by(|a, b| a.0.cmp(&b.0));
        folders.extend(parts.into_iter().map(|(_, f)| f));
    }
    Ok(folders)
}
#[derive(Clone, Copy)]
enum MoveTarget { Done, Trash, Folder(i64) }
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
    // Credential replacement and removal wait for one-message actions to finish refreshing tokens.
    removal: std::sync::RwLock<()>,
    // Read flags the server confirmed during the current folder step (see `FolderLock`).
    read_flags: Mutex<HashMap<i64, bool>>,
    // One read flag at a time, in the server's order. A move and the replay hold it too, so no
    // read flag runs halfway through either.
    marking: Mutex<()>,
    // The latest read flag asked for per message, with its call number, until it runs. A move
    // stores it on the server first and takes it, so a read flag asked for just before a move is not lost.
    pending_reads: Mutex<HashMap<i64, (bool, u64)>>,
    calls: std::sync::atomic::AtomicU64,
    contacts: Mutex<HashMap<String, lagom_core::contacts::Contacts>>,
    // shortcut: one browser sign-in at a time, lost if the app is killed in the browser; the user starts again.
    sign_in: Mutex<Option<lagom_core::oauth::Pending>>,
    // A calendar fetch, or a new event and its cache write: a fetch that began before Google stored the
    // event must not replace the window without it. After removal in the lock order.
    calendar: Mutex<()>,
    // New events Google confirmed but the cache refused: (account, calendar, event), written again at the next sync.
    // After calendar in the lock order.
    // shortcut: kept in memory only, so a restart before that sync loses an event past the fetch window; store it if that happens.
    unsaved_events: Mutex<Vec<(String, String, lagom_core::calendar::Event)>> }

impl LagomCore {
    async fn install_sign_in(&self, signed: lagom_core::oauth::SignedIn, grant: Option<String>) -> Result<SignInResult> {
        let calendar_grant = grant.is_some();
        let account = if let Some(target) = grant {
            self.run_stores(move |stores| {
                let _removal = stores.removal.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                let _calendar = stores.calendar.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                Store::open(&stores.path)?.save_browser_sign_in(signed, Some(&target))
            }).await?
        } else {
            self.run_sync(move |stores| {
                let _removal = stores.removal.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                stores.folder_lock().save_browser_sign_in(signed, None)
            }).await?
        };
        Ok(SignInResult { account: account.into(), calendar_grant })
    }
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
        // Network futures borrow SQLite across awaits; a separate WAL connection keeps local work available.
        self.run_stores(move |stores| {
            // A panic in one action must not disable the app: SQLite transactions keep the store consistent.
            match lane {
                Lane::Local => action(&mut stores.local.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
                Lane::Network => action(&mut stores.folder_lock()),
                Lane::Own => {
                    let _removal = stores.removal.read().unwrap_or_else(std::sync::PoisonError::into_inner);
                    Store::open(&stores.path).and_then(|mut store| action(&mut store))
                }
            }
        }).await
    }
    async fn run_sync<T: Send + 'static>(&self, action: impl FnOnce(&Stores) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        self.run_stores(move |stores| {
            let _sync = stores.sync.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            action(stores)
        }).await
    }
    async fn run_stores<T: Send + 'static>(&self, action: impl FnOnce(&Stores) -> lagom_core::Result<T> + Send + 'static) -> Result<T> {
        let stores = self.stores.clone();
        tokio::task::spawn_blocking(move || {
            // A mail request can finish rotating its old token after a calendar grant replaces it.
            lagom_core::oauth::configure_refresh_store(&stores.path);
            action(&stores).map_err(MailError::from)
        }).await.map_err(|_| failure("something went wrong; try again"))?
    }
}
#[derive(Clone, Copy)]
enum Lane { Local, Network, Own }
/// `network` for one folder-state step. A read flag confirmed before the step is already right, and the
/// step fetches it fresh. One confirmed during the step may be newer than the step's FETCH, so the
/// step's end writes those again.
struct FolderLock<'a> { stores: &'a Stores, store: tokio::sync::MutexGuard<'a, Store> }
impl Stores {
    fn folder_lock(&self) -> FolderLock<'_> {
        let store = self.network.blocking_lock();
        self.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        FolderLock { stores: self, store }
    }
}
impl Stores {
    /// Stores a read flag that was asked for but has not run yet, before `id` moves.
    /// The caller holds `marking`.
    async fn carry_read_flag(&self, session: &mut ImapSync, s: &mut Store, id: i64) -> lagom_core::Result<()> {
        let Some((seen, call)) = self.pending_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&id).copied() else { return Ok(()) };
        // On failure the entry stays, so the waiting mark_read runs it and reports its own result.
        session.mark_seen(s, id, seen).await?;
        self.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, seen);
        let mut pending = self.pending_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.get(&id).map(|p| p.1) == Some(call) { pending.remove(&id); }
        Ok(())
    }
}
impl std::ops::Deref for FolderLock<'_> { type Target = Store; fn deref(&self) -> &Store { &self.store } }
impl std::ops::DerefMut for FolderLock<'_> { fn deref_mut(&mut self) -> &mut Store { &mut self.store } }
impl Drop for FolderLock<'_> { fn drop(&mut self) { self.replay(); } }
impl FolderLock<'_> {
    /// Writes again the read flags confirmed during this step. Drop calls it; call it sooner to read flags now.
    fn replay(&mut self) {
        // Under `marking`: a read flag that finished after the drain must not be overwritten by an older one.
        let _marking = self.stores.marking.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let flags = std::mem::take(&mut *self.stores.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (id, seen) in flags {
            // shortcut: a failed write leaves the older flag until the next sync; retry here if that shows up.
            if let Ok(m) = self.store.message(id) { let _ = self.store.set_seen(m.summary.folder, m.summary.uid, seen); }
        }
    }
}
fn account(store: &Store, id: &str) -> lagom_core::Result<Account> {
    store.accounts()?.into_iter().find(|a| a.id == id).ok_or(lagom_core::Error::NotFound)
}
async fn add(store: &Store, account: Account, password: Zeroizing<String>) -> lagom_core::Result<MailAccount> {
    let session = ImapSync::login(&account, &password).await?;
    smtp::verify(&account, &password).await?;
    store.save_password_account(&account, &password)?;
    let _ = session.logout().await;
    Ok(account.into())
}
/// Runs on a blocking thread under `Stores::sync` and takes `network` for one step at a time.
fn sync_account(stores: &Stores, a: &Account) -> lagom_core::Result<()> {
    let password = a.password()?;
    let mut error = None;
    {
        let store = &mut *stores.folder_lock();
        let outbox = (|| -> lagom_core::Result<()> {
            for id in store.due_outbox(&a.id, milliseconds())? {
                if store.claim_send(&id, milliseconds())? {
                    let pending = match store.pending(&id) {
                        Ok(pending) => pending,
                        Err(e) => {
                            store.send_failed(&id, &e, milliseconds())?;
                            return Err(e);
                        }
                    };
                    // The Sent copy syncs the Sent folder.
                    if let Err(e) = network(smtp::send_queued(a, &password, &pending, store)) { error = Some(e); }
                }
            }
            Ok(())
        })();
        outbox?;
    }
    let mut session = network(ImapSync::login(a, &password))?;
    let mut folders = network(session.list_folders(&stores.folder_lock()))?;
    folders.sort_by_key(|f| f.role != FolderRole::Inbox);
    let mut session = Some(session);
    let mut first = None;
    for folder in folders {
        let mut store = stores.folder_lock();
        let result = network(async {
            if session.is_none() { session = Some(ImapSync::login(a, &password).await?); }
            session.as_mut().unwrap().sync_folder(&mut store, folder.id, DEFAULT_LIMIT).await
        });
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
/// Mail, then Calendar and Google Tasks, which run even when mail failed.
fn sync_with_google(stores: &Stores, a: &Account) -> lagom_core::Result<()> {
    let mail = sync_account(stores, a);
    if a.auth == lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google) {
        let _ = sync_google(stores, &a.id);
    }
    mail
}
/// Both granted services get a turn, even if the first fetch fails; either failure is reported.
fn sync_google(stores: &Stores, id: &str) -> lagom_core::Result<()> {
    let _calendar = stores.calendar.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut store = Store::open(&stores.path)?;
    // Mail may have started before the grant replaced the credential.
    let a = account(&store, id)?;
    if a.auth != lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google) { return Ok(()); }
    let scopes = store.account_scopes(id)?;
    planner::cache_unsaved(stores, &store, id);
    let calendar = lagom_core::oauth::calendar_allowed(&scopes).then(||
        network(lagom_core::google::sync_calendar(&store, &a, now()))
            .inspect_err(|e| eprintln!("lagom: Google Calendar sync failed: {e:?}")));
    let tasks = lagom_core::oauth::tasks_allowed(&scopes).then(||
        network(lagom_core::google::sync_tasks(&mut store, &a))
            .inspect_err(|e| eprintln!("lagom: Google Tasks sync failed: {e:?}")));
    match (calendar, tasks) {
        (Some(calendar), Some(tasks)) => calendar.and(tasks),
        (Some(result), None) | (None, Some(result)) => result,
        (None, None) => Ok(()),
    }
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
        let core = Arc::new(Self { stores: Arc::new(Stores { path, local: Mutex::new(local), network: tokio::sync::Mutex::new(store), sync: Mutex::new(()), removal: Default::default(), read_flags: Default::default(), marking: Mutex::new(()), pending_reads: Default::default(), calls: Default::default(), contacts: Mutex::new(HashMap::new()), sign_in: Mutex::new(None), calendar: Mutex::new(()), unsaved_events: Default::default() }), background_cancel: Arc::default() });
        open.insert(dir, Arc::downgrade(&core.stores));
        Ok(core)
    }
    pub async fn accounts(&self) -> Result<Vec<MailAccount>> {
        self.run(|s| Ok(s.accounts()?.into_iter().map(Into::into).collect())).await
    }
    /// "Your name" on desktop: what others see as the sender. An empty name falls back to the address.
    pub async fn set_account_name(&self, account_id: String, name: String) -> Result<MailAccount> {
        self.run(move |s| {
            let mut a = account(s, &account_id)?;
            let name = name.trim();
            a.display_name = if name.is_empty() { a.email.clone() } else { name.to_string() };
            s.set_account_name(&a)?;
            Ok(a.into())
        }).await
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
    /// Starts the browser sign-in that adds Google Calendar and Tasks to a Google account. `finish_sign_in` ends it.
    pub fn begin_calendar_access(&self, account_id: String) -> Result<SignInStart> {
        let a = account(&self.stores.local.lock().unwrap_or_else(std::sync::PoisonError::into_inner), &account_id)?;
        let (url, pending) = lagom_core::oauth::begin_calendar_access(&a)?;
        let start = SignInStart { url, callback_scheme: pending.scheme() };
        *self.stores.sign_in.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
        Ok(start)
    }
    /// `callback_url` is the redirect the browser handed back. Only the pending sign-in's own redirect
    /// (its URI and state) is taken; anything else is refused and leaves the sign-in pending.
    pub async fn finish_sign_in(&self, callback_url: String) -> Result<SignInResult> {
        let callback_url = Zeroizing::new(callback_url);
        let pending = {
            let mut slot = self.stores.sign_in.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match slot.take() {
                Some(pending) if pending.accepts(&callback_url) => pending,
                other => { *slot = other; return Err(failure("Sign-in was interrupted. Try again.")); }
            }
        };
        let grant = pending.account().map(|a| a.id.clone());
        let signed = lagom_core::oauth::finish(pending, &callback_url).await?;
        self.install_sign_in(signed, grant).await
    }
    pub async fn remove_account(&self, id: String) -> Result<()> {
        // A running sync finishes its account, and running one-message actions their token refresh, before the rows go.
        self.run_sync(move |stores| {
            let _removal = stores.removal.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let s = &mut *stores.folder_lock();
            // Serialize with refresh, and evict even if the caller cancels this future.
            let _local = stores.local.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            s.remove_credentials_and_account(&id, Account::delete_password)?;
            stores.contacts.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&id);
            stores.unsaved_events.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(account, _, _)| *account != id);
            Ok(())
        }).await
    }
    /// The removal confirmation's words: what removing the account deletes that the provider does not have.
    pub async fn removal_note(&self, account_id: String) -> Result<String> {
        self.run(move |s| s.removal_note(&account_id)).await
    }
    pub async fn sync(&self, account_id: String) -> Result<()> {
        self.run_sync(move |stores| { let a = account(&stores.folder_lock(), &account_id)?; sync_with_google(stores, &a) }).await
    }
    /// Calendar and Google Tasks for one account. It waits for no mail sync: its own lock serializes it.
    pub async fn sync_google(&self, account_id: String) -> Result<()> {
        self.run_stores(move |stores| {
            let _removal = stores.removal.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            sync_google(stores, &account_id)
        }).await
    }
    pub async fn sync_all(&self) -> Result<()> {
        self.run_sync(|stores| {
            let mut first_error = None;
            let accounts = stores.folder_lock().accounts()?;
            for a in accounts {
                // Re-read: a Calendar grant may have replaced this account's credential since the list was read.
                let Ok(a) = account(&stores.folder_lock(), &a.id) else { continue };
                if let Err(error) = sync_with_google(stores, &a) {
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
            let accounts = stores.folder_lock().accounts()?;
            for a in accounts {
                let mut lock = stores.folder_lock();
                let s = &mut *lock;
                // After the lock: a cancel may arrive while a move holds it.
                if cancel.load(std::sync::atomic::Ordering::Relaxed) { break; }
                // Re-read: a Calendar grant may have replaced this account's credential since the list was read.
                let Ok(a) = account(s, &a.id) else { continue };
                // No Inbox yet means no first sync: new_mail would stay silent anyway.
                let Some(before) = s.folders(&a.id, now())?.into_iter().find(|f| f.role == FolderRole::Inbox) else { continue };
                let synced = network(async {
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    let result = session.sync_folder(s, before.id, DEFAULT_LIMIT).await;
                    let _ = session.logout().await;
                    result
                });
                // One account's failure (locked secret, network) must not stop the others.
                if synced.is_err() { continue; }
                // A read flag confirmed during the sync must not come back as new mail.
                lock.replay();
                let (after, messages) = (lock.folder(before.id)?, lock.messages_since(before.id, before.last_uid)?);
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
    /// The account's own folders, or every account's when `account_id` is None, in tree order.
    pub async fn folders(&self, account_id: Option<String>) -> Result<Vec<MailFolder>> {
        self.run(move |s| {
            if let Some(id) = &account_id { account(s, id)?; }
            mail_folders(s, account_id.as_deref())
        }).await
    }
    pub async fn folder_mail(&self, folder_id: i64, limit: u32, offset: u32) -> Result<Vec<MailRow>> {
        self.run(move |s| {
            if limit > 500 { return Err(lagom_core::Error::Invalid("page limit must be at most 500")); }
            let folder = s.folder(folder_id)?;
            Ok(conversation_rows(s.list_conversations(&folder.account, Some(folder_id), now(), limit.saturating_add(offset))?).into_iter().skip(offset as usize).take(limit as usize).collect())
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
    pub async fn done(&self, ids: Vec<i64>) -> Result<Vec<MailChange>> { self.move_messages(ids, MoveTarget::Done).await }
    pub async fn delete(&self, ids: Vec<i64>) -> Result<Vec<MailChange>> { self.move_messages(ids, MoveTarget::Trash).await }
    /// "Move to" a folder of the message's own account. Undo takes the result like Done's.
    pub async fn move_to_folder(&self, ids: Vec<i64>, folder_id: i64) -> Result<Vec<MailChange>> {
        self.move_messages(ids, MoveTarget::Folder(folder_id)).await
    }
    /// Desktop's "New folder": a top-level folder on the server, checked as desktop checks it.
    pub async fn create_folder(&self, account_id: String, name: String) -> Result<MailFolder> {
        let name = name.trim().to_string();
        self.run_network(move |s| network(async {
            let a = account(s, &account_id)?;
            let server = s.folders(&a.id, now())?;
            if server.is_empty() { return Err(lagom_core::Error::Invalid("Sync folders before creating a folder")); }
            lagom_core::imap_sync::validate_folder_name(&name, server.iter().filter_map(|f| f.delimiter.as_deref()), server.iter().map(|f| f.name.as_str()))
                .map_err(lagom_core::Error::Invalid)?;
            let mut session = ImapSync::login(&a, &a.password()?).await?;
            let created = session.create_folder(s, &name).await;
            let _ = session.logout().await;
            // No delimiter in the name, so it is its own path at the top.
            Ok(MailFolder { id: created?, account: a.id, path: name.clone(), name, depth: 0, unread: 0 })
        })).await
    }
    /// Deletes everything in Trash for good: the account's, or every account's when None. The app asks first.
    pub async fn empty_trash(&self, account_id: Option<String>) -> Result<()> {
        self.run_network(move |s| network(async {
            let mut first = None;
            for a in s.accounts()?.into_iter().filter(|a| account_id.as_ref().is_none_or(|id| &a.id == id)) {
                let trash: Vec<i64> = s.folder_metadata(&a.id)?.into_iter().filter(|f| f.role == FolderRole::Trash).map(|f| f.id).collect();
                if trash.is_empty() { continue; }
                let result = async {
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    for folder in trash { session.empty_trash(s, folder).await?; }
                    let _ = session.logout().await;
                    Ok(())
                }.await;
                // One account's failure must not keep the others' Trash full.
                if let Err(error) = result { first.get_or_insert(error); }
            }
            match first { Some(error) => Err(error), None => Ok(()) }
        })).await
    }
    /// Puts Done, Later or Delete back: each message returns to the folder it left, with its old snooze.
    pub async fn undo(&self, changes: Vec<MailChange>) -> Result<()> {
        let stores = self.stores.clone();
        self.run_network(move |s| {
            // No read flag runs halfway through a move; one asked for and not yet run moves with it.
            let _marking = stores.marking.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            network(async {
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
                        stores.carry_read_flag(&mut session, s, current).await?;
                        session.move_message(s, current, c.from_folder).await?;
                        let _ = session.logout().await;
                        // shortcut: a failed refresh after the move back leaves no row, so an old snooze is lost; the mail is back either way.
                        match s.find_message(c.from_folder, &c.header_id)? { Some(id) => id, None => continue }
                    };
                    s.snooze(id, c.snoozed_until)?;
                }
                Ok(())
            })
        }).await
    }
    pub async fn mark_read(&self, id: i64, read: bool) -> Result<()> {
        let stores = self.stores.clone();
        // At the call, before any wait: a move that starts later still sees it. Numbered under the
        // same lock, so a later call always holds the higher number.
        let call = {
            let mut pending = stores.pending_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let call = stores.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            pending.insert(id, (read, call));
            call
        };
        self.run_own(move |s| {
            let _marking = stores.marking.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mine = |p: &HashMap<i64, (bool, u64)>| p.get(&id).map(|p| p.1) == Some(call);
            // A later read flag runs instead, or a move already stored this one.
            if !mine(&stores.pending_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) { return Ok(()); }
            let result = network(async {
                let a = account(s, &s.message_summary(id)?.account)?;
                let mut session = ImapSync::login(&a, &a.password()?).await?;
                session.mark_seen(s, id, read).await?;
                // Recorded, then written again: a folder step that ended between the two writes
                // cannot leave its older FETCH behind.
                stores.read_flags.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id, read);
                if let Ok(m) = s.message(id) { s.set_seen(m.summary.folder, m.summary.uid, read)?; }
                let _ = session.logout().await;
                Ok(())
            });
            let mut pending = stores.pending_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if mine(&pending) { pending.remove(&id); }
            result
        }).await
    }
    pub async fn reply_draft(&self, id: i64) -> Result<ReplyDraft> {
        self.compose_draft(id, ComposeKind::Reply).await
    }
    pub async fn compose_draft(&self, id: i64, kind: ComposeKind) -> Result<ReplyDraft> {
        // The quote is a snapshot: fetch what the reader fetches first, the referenced images or a body
        // sync left out. A failed fetch still opens the draft, without them.
        // shortcut: a fetch the reader already runs is not shared; a second session may run. Share it if that shows up.
        let _ = self.load_inline(id).await;
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
            let original = Original { from: &m.from_name, address: &m.from_address, date: &date, subject: &m.subject, to: &m.to, text: &message.body_text };
            let core_kind = if forward { compose::ComposeKind::Forward } else { compose::ComposeKind::Reply };
            let signature = s.signature(&a.id)?;
            // An HTML original stays HTML, out of the editable body; plain text quotes into it as before.
            let (body, quoted) = match message.body_html.as_deref().filter(|html| !html.trim().is_empty()) {
                Some(html) => {
                    let inline = if s.inline_loaded(id)? { s.inline_parts(id)? } else { Vec::new() };
                    (compose::compose_body(&signature, None, core_kind), Some(compose::quote(original, html, &inline, core_kind).into()))
                }
                None => (compose::compose_body(&signature, Some(original), core_kind), None),
            };
            Ok(ReplyDraft { account: a.id, to, cc, subject, body, reply_to: (!forward).then_some(id), attachments: vec![], quoted })
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
    async fn move_messages(&self, ids: Vec<i64>, target: MoveTarget) -> Result<Vec<MailChange>> {
        let stores = self.stores.clone();
        self.run_network(move |s| {
            // No read flag runs halfway through a move; one asked for and not yet run moves with it.
            let _marking = stores.marking.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            network(async {
                let mut changes = Vec::new();
                // shortcut: a failure mid-conversation drops Undo for the members already moved; they stay in Done or Trash.
                for id in ids {
                    let m = s.message_summary(id)?;
                    let a = account(s, &m.account)?;
                    let mut session = ImapSync::login(&a, &a.password()?).await?;
                    stores.carry_read_flag(&mut session, s, id).await?;
                    let role = match target {
                        MoveTarget::Trash => { session.trash(s, id).await?; Some(FolderRole::Trash) }
                        MoveTarget::Done => {
                            session.archive(s, id).await?;
                            // A real move replaces the row; an already archived message keeps its snooze.
                            match s.snooze(id, None) {
                                Ok(()) | Err(lagom_core::Error::NotFound) => {},
                                Err(error) => return Err(error),
                            }
                            Some(FolderRole::Archive)
                        }
                        // move_message refuses a folder of another account.
                        MoveTarget::Folder(folder) => {
                            session.move_message(s, id, folder).await?;
                            // Moved out of Later into its own folder: it shows there again. A real move replaced the row.
                            match s.snooze(id, None) {
                                Ok(()) | Err(lagom_core::Error::NotFound) => {},
                                Err(error) => return Err(error),
                            }
                            None
                        }
                    };
                    let _ = session.logout().await;
                    // The folder that now holds it: an account can have more than one folder with this role.
                    let folders: Vec<i64> = match (target, role) {
                        (MoveTarget::Folder(folder), _) => vec![folder],
                        (_, role) => s.folder_metadata(&a.id)?.into_iter().filter(|f| Some(f.role) == role).map(|f| f.id).collect(),
                    };
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
            })
        }).await
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

//! The mail worker. One background thread runs a Tokio runtime (the core's IMAP and SMTP need
//! it), owns the network and sync writes, and works through `Cmd`s in order. Compose commits
//! locally; the UI reads the store and hears about changes through `Event`s, without network waits.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    future::Future,
    pin::Pin,
    time::Duration,
};

use lagom_core::{
    account::Account,
    oauth::{self, Provider, SignedIn},
    discovery::{self, Discovery},
    imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp,
    store::{FolderRole, Store},
    Error,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zeroize::Zeroizing;

/// Incremental sync of every account, besides IDLE on INBOX.
pub const SYNC_EVERY: Duration = Duration::from_secs(120);

/// What an optimistic UI action does on the server (or, for Later, in the store).
#[derive(Debug, Clone)]
pub enum Act {
    Archive,
    Trash,
    Move(i64),
    NewFolder(String),
    /// Local only: until this Unix time, `None` to bring it back.
    Snooze(Option<i64>),
}

pub enum Cmd {
    /// Onboarding API; the UI consumes OpenUrl on its own thread.
    #[allow(dead_code)]
    SignIn(Provider),
    #[allow(dead_code)]
    CancelSignIn,
    #[allow(dead_code)]
    Discover(String),
    AddAccount(Box<Account>, Zeroizing<String>),
    RemoveAccount(String),
    /// One account, or all of them.
    Sync(Option<String>),
    /// IDLE saw a change: INBOX only.
    SyncInbox(String),
    /// `id` is the store row the UI saw. The worker follows it through earlier moves.
    Op { op: u64, id: i64, act: Act },
    CreateFolder { op: u64, account: String, name: String },
    MarkSeen(i64),
    Download { id: i64, section: String },
    /// The message is already durable, including its Undo deadline.
    Send(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    Syncing,
    /// Unix time of the last finished sync.
    Synced(i64),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delivery {
    Sent,
    /// Nothing left: the server refused it or was never reached.
    NotSent,
    /// The connection broke after the hand-over may have happened.
    Maybe,
}

pub enum Event {
    /// Forward to GPUI cx.open_url; never log this URL.
    OpenUrl(String),
    SignedIn(Result<String, String>),
    SignInCancelled,
    /// The email identifies the request, so the UI can ignore a stale result.
    #[allow(dead_code)]
    Discovered { email: String, discovery: Discovery },
    ReauthRequired(String),
    /// The store changed: read it again.
    Changed,
    Sync(String, SyncState),
    Op { op: u64, error: Option<String> },
    /// The new account's id, or why it could not connect.
    Added(Result<String, String>),
    /// A MarkSeen finished, done or not.
    Seen(i64),
    Downloaded(Result<PathBuf, String>),
    /// `error` says why, also when the message went out but its Sent copy failed.
    Sent { delivery: Delivery, error: Option<String> },
}

#[derive(Clone)]
pub struct Bridge(UnboundedSender<Cmd>);

impl Bridge {
    /// A bridge without a worker: the receiver stands in for it, dropping it stops it.
    #[cfg(test)]
    pub fn detached() -> (Self, UnboundedReceiver<Cmd>) {
        let (tx, rx) = unbounded_channel();
        (Self(tx), rx)
    }

    /// False when the worker has stopped (it could not open the store, or it panicked).
    #[must_use]
    pub fn send(&self, cmd: Cmd) -> bool {
        self.0.send(cmd).is_ok()
    }
}

pub fn start(path: PathBuf) -> (Bridge, UnboundedReceiver<Event>) {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = unbounded_channel();
    let me = cmd_tx.clone();
    std::thread::Builder::new()
        .name("lagom-mail".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("mail runtime");
            rt.block_on(run(path, cmd_rx, me, ev_tx));
        })
        .expect("mail thread");
    (Bridge(cmd_tx), ev_rx)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

async fn run(path: PathBuf, mut rx: UnboundedReceiver<Cmd>, me: UnboundedSender<Cmd>, events: UnboundedSender<Event>) {
    let store = match Store::open(&path) {
        Ok(store) => store,
        Err(e) => {
            let _ = events.send(Event::Sync(String::new(), SyncState::Failed(e.to_string())));
            return;
        }
    };
    if let Err(error) = store.recover_outbox() {
        let _ = events.send(Event::Sync(String::new(), SyncState::Failed(error.to_string())));
        return;
    }
    let _ = events.send(Event::Changed);
    let mut w = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, discovery: None, me, events };
    for account in w.store.accounts().unwrap_or_default() {
        w.watch(&account);
    }
    // The first tick is immediate: the launch sync.
    let mut tick = tokio::time::interval(SYNC_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if let Some(cmd) = w.deferred.pop_front() {
            w.handle(cmd, &mut rx).await;
            continue;
        }
        tokio::select! {
            result = async {
                match &mut w.sign_in { Some(flow) => flow.await, None => std::future::pending().await }
            } => {
                w.sign_in = None;
                w.signed_in(result);
            },
            (email, discovery) = async {
                match &mut w.discovery { Some(flow) => flow.await, None => std::future::pending().await }
            } => {
                w.discovery = None;
                w.emit(Event::Discovered { email, discovery });
            },
            cmd = rx.recv() => match cmd {
                Some(cmd) => w.handle(cmd, &mut rx).await,
                None => break,
            },
            _ = tick.tick() => w.sync(None, false, &mut rx).await,
        }
    }
}

type Flow<T> = Pin<Box<dyn Future<Output = T> + Send>>;

struct Worker {
    store: Store,
    /// One logged-in session per account, reused between commands. Dropped after any error.
    sessions: HashMap<String, ImapSync>,
    idle: HashMap<String, tokio::task::JoinHandle<()>>,
    /// Every store id a moved message had, to (its folder now, Message-ID), so undo and later
    /// actions that carry an old id still find it.
    moved: HashMap<i64, (i64, String)>,
    /// Commands that arrived during a sync and wait for it to finish.
    deferred: VecDeque<Cmd>,
    sign_in: Option<Flow<lagom_core::Result<SignedIn>>>,
    discovery: Option<Flow<(String, Discovery)>>,
    me: UnboundedSender<Cmd>,
    events: UnboundedSender<Event>,
}

impl Worker {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    async fn handle(&mut self, cmd: Cmd, rx: &mut UnboundedReceiver<Cmd>) {
        match cmd {
            Cmd::AddAccount(account, password) => self.add(*account, password, rx).await,
            Cmd::RemoveAccount(id) => self.remove(&id),
            Cmd::Sync(account) => self.sync(account, false, rx).await,
            Cmd::SyncInbox(account) => self.sync(Some(account), true, rx).await,
            quick => self.quick(quick).await,
        }
    }

    /// Commands that a running sync handles between folders, so a triage key never waits long.
    async fn quick(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SignIn(provider) => {
                let events = self.events.clone();
                self.sign_in = Some(Box::pin(oauth::sign_in(provider, move |url| {
                    let _ = events.send(Event::OpenUrl(url.into()));
                })));
            }
            Cmd::CancelSignIn => {
                self.sign_in = None;
                self.emit(Event::SignInCancelled);
            }
            Cmd::Discover(email) => {
                self.discovery = Some(Box::pin(async move {
                    let result = discovery::discover(&email).await;
                    (email, result)
                }));
            }
            Cmd::Op { op, id, act } => {
                let error = self.op(id, act).await.err().map(|e| e.to_string());
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::CreateFolder { op, account, name } => {
                let error = self.create_folder(&account, &name).await.err().map(|e| e.to_string());
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::Download { id, section } => {
                let result = self.download(id, &section).await.map_err(|e| e.to_string());
                self.emit(Event::Downloaded(result));
            }
            Cmd::MarkSeen(id) => {
                // Either way the UI drops its override: the store says what the server has.
                let _ = self.mark_seen(id).await;
                self.emit(Event::Seen(id));
                self.emit(Event::Changed);
            }
            Cmd::Send(id) => { self.send(&id).await; }
            other => self.deferred.push_back(other),
        }
    }

    fn signed_in(&mut self, result: lagom_core::Result<SignedIn>) {
        let signed = match result {
            Ok(signed) => signed,
            Err(Error::Cancelled) => { self.emit(Event::SignInCancelled); return; }
            Err(error) => { self.emit(Event::SignedIn(Err(error.to_string()))); return; }
        };
        let account = signed.account;
        if let Err(error) = self.store.save_account(&account) {
            let _ = account.delete_password();
            self.emit(Event::SignedIn(Err(error.to_string())));
            return;
        }
        self.emit(Event::SignedIn(Ok(account.id.clone())));
        self.emit(Event::Changed);
        self.watch(&account);
        self.deferred.push_back(Cmd::Sync(Some(account.id)));
    }

    fn account(&self, id: &str) -> lagom_core::Result<Account> {
        self.store.accounts()?.into_iter().find(|a| a.id == id).ok_or(Error::NotFound)
    }

    /// The account's session, logged in if needed. Give it back with `give`.
    async fn take(&mut self, account: &Account) -> lagom_core::Result<ImapSync> {
        if let Some(session) = self.sessions.remove(&account.id) {
            return Ok(session);
        }
        let password = account.password()?;
        ImapSync::login(account, &password).await
    }

    /// README: a session that saw an error may hold a half-read response; never reuse it.
    fn give<T>(&mut self, account: &str, session: ImapSync, result: &lagom_core::Result<T>) {
        if result.is_ok() {
            self.sessions.insert(account.into(), session);
        }
    }

    async fn add(&mut self, account: Account, password: Zeroizing<String>, rx: &mut UnboundedReceiver<Cmd>) {
        let result = async {
            let session = ImapSync::login(&account, &password).await?;
            smtp::verify(&account, &password).await?;
            account.set_password(&password)?;
            if let Err(e) = self.store.save_account(&account) {
                let _ = account.delete_password();
                return Err(e);
            }
            Ok(session)
        }
        .await;
        match result {
            Ok(session) => {
                self.sessions.insert(account.id.clone(), session);
                self.emit(Event::Added(Ok(account.id.clone())));
                self.emit(Event::Changed);
                self.watch(&account);
                self.sync(Some(account.id), false, rx).await;
            }
            Err(e) => self.emit(Event::Added(Err(e.to_string()))),
        }
    }

    fn remove(&mut self, id: &str) {
        if let Some(task) = self.idle.remove(id) {
            task.abort();
        }
        self.sessions.remove(id);
        if let Ok(account) = self.account(id) {
            // Keep the row while the secret stays: without it nobody could find the secret again.
            if account.delete_password().is_err() {
                self.emit(Event::Sync(id.to_string(), SyncState::Failed("could not remove the saved password; try Remove again".into())));
                self.emit(Event::Changed);
                return;
            }
        }
        let _ = self.store.delete_account(id);
        self.emit(Event::Changed);
    }

    async fn sync(&mut self, only: Option<String>, inbox_only: bool, rx: &mut UnboundedReceiver<Cmd>) {
        let accounts = self.store.accounts().unwrap_or_default();
        for account in accounts.into_iter().filter(|a| only.as_ref().is_none_or(|id| *id == a.id)) {
            self.emit(Event::Sync(account.id.clone(), SyncState::Syncing));
            match self.store.due_outbox(&account.id, chrono::Local::now().timestamp_millis()) {
                Ok(ids) => for id in ids { self.send(&id).await; },
                Err(e) => self.emit(Event::Sync(account.id.clone(), SyncState::Failed(e.to_string()))),
            }
            let state = match self.sync_account(&account, inbox_only, rx).await {
                Ok(()) => SyncState::Synced(now()),
                Err(Error::ReauthRequired) => {
                    self.emit(Event::ReauthRequired(account.id.clone()));
                    SyncState::Failed(Error::ReauthRequired.to_string())
                }
                Err(e) => SyncState::Failed(e.to_string()),
            };
            self.emit(Event::Sync(account.id.clone(), state));
            self.emit(Event::Changed);
        }
    }

    async fn sync_account(&mut self, account: &Account, inbox_only: bool, rx: &mut UnboundedReceiver<Cmd>) -> lagom_core::Result<()> {
        let mut session = self.take(account).await?;
        let listed = session.list_folders(&self.store).await;
        self.give(&account.id, session, &listed);
        let mut folders = listed?;
        // INBOX first, so the list fills where the user looks.
        folders.sort_by_key(|f| f.role != FolderRole::Inbox);
        if inbox_only {
            folders.truncate(1);
        }
        // One bad folder must not keep the others from syncing.
        let mut first = None;
        for folder in folders {
            // Triage keys pressed meanwhile run between folders.
            while let Ok(cmd) = rx.try_recv() {
                self.quick(cmd).await;
            }
            let mut session = self.take(account).await?;
            let result = session.sync_folder(&mut self.store, folder.id, DEFAULT_LIMIT).await;
            self.give(&account.id, session, &result);
            match result {
                Ok(()) => self.emit(Event::Changed),
                Err(e) if e.is_account_wide() => return Err(e),
                Err(e) => {
                    first.get_or_insert(lagom_core::Error::Folder { folder: folder.name, source: Box::new(e) });
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    async fn op(&mut self, id: i64, act: Act) -> lagom_core::Result<()> {
        let id = self.resolve(id).await?;
        if let Act::Snooze(until) = act {
            return self.store.snooze(id, until);
        }
        let message = self.store.message(id)?.summary;
        let account = self.account(&message.account)?;
        let mut session = self.take(&account).await?;
        let store = &mut self.store;
        let result = async {
            let target = match act {
                Act::Archive => {
                    session.archive(store, id).await?;
                    role(store, &account.id, FolderRole::Archive)
                }
                Act::Trash => {
                    session.trash(store, id).await?;
                    role(store, &account.id, FolderRole::Trash)
                }
                Act::Move(folder) => {
                    session.move_message(store, id, folder).await?;
                    Some(folder)
                }
                Act::NewFolder(name) => {
                    let folder = ensure_folder(&mut session, store, &account.id, &name).await?;
                    session.move_message(store, id, folder).await?;
                    Some(folder)
                }
                Act::Snooze(_) => unreachable!("handled above"),
            };
            Ok(target)
        }
        .await;
        self.give(&account.id, session, &result);
        if let Some(target) = result?.filter(|_| !message.message_id.is_empty()) {
            follow(&mut self.moved, id, message.folder, &message.message_id, target);
        }
        Ok(())
    }

    async fn create_folder(&mut self, account: &str, name: &str) -> lagom_core::Result<()> {
        let folders = self.store.folders(account, now())?;
        crate::folders::validate_name(name, folders.iter().filter_map(|f| f.delimiter.as_deref()), folders.iter().map(|f| f.name.as_str())).map_err(Error::Invalid)?;
        let account = self.account(account)?;
        let mut session = self.take(&account).await?;
        let result = ensure_folder(&mut session, &self.store, &account.id, name).await;
        self.give(&account.id, session, &result);
        result.map(|_| ())
    }

    /// The current store id of a message the UI knew as `id`. The core drops a moved row; the
    /// target folder's next sync brings it back, so undo syncs that one folder first.
    // shortcut: a message without Message-ID cannot be found after a move, so its undo fails with a toast; match on headers when that shows up.
    async fn resolve(&mut self, id: i64) -> lagom_core::Result<i64> {
        let Some((folder, message_id)) = self.moved.get(&id).cloned() else { return Ok(id) };
        if self.store.find_message(folder, &message_id)?.is_none() {
            let account = self.account(&self.store.folder(folder)?.account)?;
            let mut session = self.take(&account).await?;
            let result = session.sync_folder(&mut self.store, folder, DEFAULT_LIMIT).await;
            self.give(&account.id, session, &result);
            result?;
        }
        self.store.find_message(folder, &message_id)?.ok_or(Error::NotFound)
    }

    async fn download(&mut self, id: i64, section: &str) -> lagom_core::Result<PathBuf> {
        let id = self.resolve(id).await?;
        let account = self.account(&self.store.message(id)?.summary.account)?;
        let part = self.store.attachments(id)?.into_iter().find(|p| p.section == section).ok_or(Error::NotFound)?;
        let mut session = self.take(&account).await?;
        let result = session.fetch_attachment(&mut self.store, id, section).await;
        self.give(&account.id, session, &result);
        lagom_core::attachment::save_download(&part.filename, &result?)
    }

    async fn mark_seen(&mut self, id: i64) -> lagom_core::Result<()> {
        let account = self.account(&self.store.message(id)?.summary.account)?;
        let mut session = self.take(&account).await?;
        let result = session.mark_seen(&mut self.store, id, true).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn send(&mut self, id: &str) {
        let result = async {
            if !self.store.claim_send(id, chrono::Local::now().timestamp_millis())? {
                return Ok(false);
            }
            let prepared = (|| {
                let pending = self.store.pending(id)?;
                let account = self.account(&pending.account)?;
                let password = account.password()?;
                Ok((pending, account, password))
            })();
            let (pending, account, password) = match prepared {
                Ok(prepared) => prepared,
                Err(e) => {
                    self.store.send_failed(id, &e, chrono::Local::now().timestamp_millis())?;
                    return Err(e);
                }
            };
            smtp::send_queued(&account, &password, &pending, &mut self.store).await?;
            Ok(true)
        }.await;
        let (delivery, error) = match result {
            Ok(false) => return,
            Ok(true) => (Delivery::Sent, None),
            Err(e) => {
                if matches!(e, Error::ReauthRequired) {
                    if let Ok(pending) = self.store.pending(id) {
                        self.emit(Event::ReauthRequired(pending.account));
                    }
                }
                let delivery = match e {
                    Error::SentCopy => Delivery::Sent,
                    Error::MaybeSent => Delivery::Maybe,
                    _ => Delivery::NotSent,
                };
                (delivery, Some(e.to_string()))
            }
        };
        self.emit(Event::Sent { delivery, error });
        self.emit(Event::Changed);
    }

    /// IDLE on INBOX in its own task with its own connection; a change queues an INBOX sync.
    fn watch(&mut self, account: &Account) {
        let id = account.id.clone();
        let (account, me) = (account.clone(), self.me.clone());
        let task = tokio::spawn(async move {
            loop {
                let result = async {
                    let password = account.password()?;
                    ImapSync::login(&account, &password).await?.idle_inbox().await
                }
                .await;
                match result {
                    Ok(_) => {
                        if me.send(Cmd::SyncInbox(account.id.clone())).is_err() {
                            return;
                        }
                    }
                    // No IDLE on this server: the timer sync is all there is.
                    Err(Error::Network("server does not support IDLE")) => return,
                    // Offline or dropped: try again after one sync period, never in a tight loop.
                    Err(_) => tokio::time::sleep(SYNC_EVERY).await,
                }
            }
        });
        if let Some(old) = self.idle.insert(id, task) {
            old.abort();
        }
    }
}

async fn ensure_folder(session: &mut ImapSync, store: &Store, account: &str, name: &str) -> lagom_core::Result<i64> {
    // Moves and undo can reach a folder before the UI has received its store id.
    match store.folders(account, now())?.into_iter().find(|f| f.name == name) {
        Some(folder) => Ok(folder.id),
        None => session.create_folder(store, name).await,
    }
}

fn role(store: &Store, account: &str, role: FolderRole) -> Option<i64> {
    store.folders(account, 0).ok()?.into_iter().find(|f| f.role == role).map(|f| f.id)
}

/// The worker against GreenMail: add account, send, sync, archive, trash, new folder, move and
/// undo, each checked on the server by a fresh sync into a second store. Runs only with
/// `LAGOM_GREENMAIL=1` and the container from crates/lagom-core/README.md.
/// Store row `id` (in folder `from`) moved to `target`. Older ids of the same message point
/// at `from`; the same Message-ID elsewhere (another folder, another account) is another copy.
fn follow(moved: &mut HashMap<i64, (i64, String)>, id: i64, from: i64, message_id: &str, target: i64) {
    for place in moved.values_mut().filter(|p| p.0 == from && p.1 == message_id) {
        place.0 = target;
    }
    moved.insert(id, (target, message_id.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn folder_command_reports_validation_errors_without_connecting() {
        use lagom_core::account::{Security, ServerConfig};
        let path = std::env::temp_dir().join(format!("lagom-folder-worker-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, discovery: None, me, events };
        let server = ServerConfig { host: "127.0.0.1".into(), port: 3143, security: Security::Plain, username: "test@lagom.test".into() };
        let account = Account::new("test@lagom.test".into(), server.clone(), server).unwrap();
        worker.store.save_account(&account).unwrap();
        worker.store.save_folder(&account.id, "INBOX", Some("."), FolderRole::Inbox).unwrap();
        for name in ["", "  ", "inbox", "Parent.Child"] {
            worker.quick(Cmd::CreateFolder { op: 42, account: account.id.clone(), name: name.into() }).await;
            assert!(matches!(out.recv().await, Some(Event::Op { op: 42, error: Some(_) })));
            assert!(matches!(out.recv().await, Some(Event::Changed)));
        }
        assert_eq!(worker.store.folders(&account.id, now()).unwrap().len(), 1);
        assert!(worker.sessions.is_empty());
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn discovery_and_sign_in_cancellation_commands() {
        let path = std::env::temp_dir().join(format!("lagom-oauth-worker-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, discovery: None, me, events };
        worker.quick(Cmd::Download { id: -1, section: "2".into() }).await;
        assert!(matches!(out.recv().await, Some(Event::Downloaded(Err(_)))));
        worker.quick(Cmd::SignIn(Provider::Google)).await;
        assert!(worker.sign_in.is_some());
        worker.quick(Cmd::CancelSignIn).await;
        assert!(worker.sign_in.is_none());
        assert!(matches!(out.recv().await, Some(Event::SignInCancelled)));
        worker.quick(Cmd::Discover("alice@gmail.com".into())).await;
        let (email, result) = worker.discovery.take().unwrap().await;
        assert_eq!(email, "alice@gmail.com");
        assert_eq!(result, Discovery::OAuth(Provider::Google));
        worker.signed_in(Err(Error::Cancelled));
        assert!(matches!(out.recv().await, Some(Event::SignInCancelled)));
        assert!(worker.store.accounts().unwrap().is_empty());
        worker.signed_in(Err(Error::Network("test error")));
        assert!(matches!(out.recv().await, Some(Event::SignedIn(Err(_)))));
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_stopped_worker_refuses_commands() {
        let (tx, rx) = unbounded_channel();
        let bridge = Bridge(tx);
        assert!(bridge.send(Cmd::Sync(None)));
        drop(rx);
        assert!(!bridge.send(Cmd::Sync(None)));
    }

    #[test]
    fn a_move_redirects_only_this_message_aliases() {
        let mut moved = HashMap::new();
        // The same newsletter in account A (INBOX 1, Archive 2) and B (INBOX 10, Archive 20).
        follow(&mut moved, 100, 1, "<n@x>", 2);
        follow(&mut moved, 200, 10, "<n@x>", 20);
        assert_eq!((moved[&100].0, moved[&200].0), (2, 20));
        // A's copy, now row 101 in Archive 2, goes back to INBOX 1: A's old id follows, B's not.
        follow(&mut moved, 101, 2, "<n@x>", 1);
        assert_eq!((moved[&100].0, moved[&101].0, moved[&200].0), (1, 1, 20));
    }
}

#[cfg(test)]
mod greenmail {
    use super::*;
    use lagom_core::{
        account::{Security, ServerConfig},
        imap_sync::ImapSync,
    };

    async fn next(events: &mut UnboundedReceiver<Event>, mut want: impl FnMut(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let e = events.recv().await.expect("worker running");
                if let Event::Sync(_, SyncState::Failed(err)) = &e {
                    panic!("sync failed: {err}");
                }
                if want(&e) {
                    return e;
                }
            }
        })
        .await
        .expect("event in time")
    }

    async fn synced(events: &mut UnboundedReceiver<Event>) {
        next(events, |e| matches!(e, Event::Sync(_, SyncState::Synced(_)))).await;
    }

    async fn op_ok(events: &mut UnboundedReceiver<Event>, op: u64) {
        if let Event::Op { error, .. } = next(events, |e| matches!(e, Event::Op { op: o, .. } if *o == op)).await {
            assert_eq!(error, None, "op {op}");
        }
    }

    /// Subjects per folder name, as the server has them now.
    async fn server(account: &Account) -> HashMap<String, Vec<String>> {
        let dir = std::env::temp_dir().join(format!("lagom-check-{}", uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(dir.join("check.sqlite3")).unwrap();
        store.save_account(account).unwrap();
        let mut imap = ImapSync::login(account, "test").await.unwrap();
        imap.sync(&mut store, 50).await.unwrap();
        let mut out = HashMap::new();
        for f in store.folders(&account.id, now()).unwrap() {
            let mut subjects: Vec<_> = store.list_messages(f.id, now(), 50, 0).unwrap().into_iter().map(|m| m.subject).collect();
            subjects.sort();
            out.insert(f.name, subjects);
        }
        let _ = std::fs::remove_dir_all(dir);
        out
    }

    fn uuid() -> String {
        format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
    }

    #[tokio::test]
    async fn worker_actions_reach_the_server() {
        if std::env::var("LAGOM_GREENMAIL").as_deref() != Ok("1") {
            eprintln!("skipped: set LAGOM_GREENMAIL=1; see crates/lagom-core/README.md");
            return;
        }
        // Never the owner's keychain entries or store.
        std::env::set_var("LAGOM_KEYCHAIN_SERVICE", "lagom-test");
        let dir = std::env::temp_dir().join(format!("lagom-worker-{}", uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lagom.sqlite3");
        let email = format!("worker-{}@lagom.test", uuid());
        let config = |port| ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: email.clone() };
        let account = Account::new(email.clone(), config(3143), config(3025)).unwrap();
        let (bridge, mut events) = start(path.clone());

        let _ = bridge.send(Cmd::AddAccount(Box::new(account.clone()), Zeroizing::new("test".into())));
        let Event::Added(added) = next(&mut events, |e| matches!(e, Event::Added(_))).await else { unreachable!() };
        assert_eq!(added, Ok(account.id.clone()));
        synced(&mut events).await;

        let queue = Store::open(&path).unwrap();
        for subject in ["Archive me", "Trash me", "File me", "Move me"] {
            let mail = smtp::Outgoing { to: vec![email.clone()], subject: subject.into(), text: "Body".into(), ..Default::default() };
            let id = queue.enqueue(&account, &mail, 0).unwrap();
            let _ = bridge.send(Cmd::Send(id));
            let Event::Sent { delivery, error, .. } = next(&mut events, |e| matches!(e, Event::Sent { .. })).await else { unreachable!() };
            assert_eq!(delivery, Delivery::Sent, "{error:?}");
        }
        let _ = bridge.send(Cmd::Sync(None));
        synced(&mut events).await;

        let store = Store::open(&path).unwrap();
        let folders = store.folders(&account.id, now()).unwrap();
        let inbox = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap().id;
        let id = |subject: &str| store.list_messages(inbox, now(), 50, 0).unwrap().into_iter().find(|m| m.subject == subject).unwrap().id;
        let (archive, trash, file, moving) = (id("Archive me"), id("Trash me"), id("File me"), id("Move me"));

        let _ = bridge.send(Cmd::Op { op: 10, id: archive, act: Act::Archive });
        op_ok(&mut events, 10).await;
        let _ = bridge.send(Cmd::Op { op: 11, id: trash, act: Act::Trash });
        op_ok(&mut events, 11).await;
        let _ = bridge.send(Cmd::Op { op: 12, id: file, act: Act::NewFolder("Projects".into()) });
        op_ok(&mut events, 12).await;
        let projects = store.folders(&account.id, now()).unwrap().into_iter().find(|f| f.name == "Projects").unwrap().id;
        // A second "new" folder of the same name reuses the first.
        assert!(projects > 0);
        let _ = bridge.send(Cmd::Op { op: 13, id: moving, act: Act::NewFolder("Projects".into()) });
        op_ok(&mut events, 13).await;

        let s = server(&account).await;
        assert_eq!(s["INBOX"], Vec::<String>::new());
        assert_eq!(s["Archive"], ["Archive me"]);
        assert_eq!(s["Projects"], ["File me", "Move me"]);
        assert!(s.iter().any(|(name, subjects)| name != "INBOX" && subjects.contains(&"Trash me".to_string()) && name.to_lowercase().contains("trash")));
        // Sent copies went to the server's Sent folder.
        assert!(s.iter().any(|(name, subjects)| name.to_lowercase().contains("sent") && subjects.len() == 4));

        // Undo uses the id the UI saw; the worker follows the move to the new row.
        let _ = bridge.send(Cmd::Op { op: 14, id: archive, act: Act::Move(inbox) });
        op_ok(&mut events, 14).await;
        assert_eq!(server(&account).await["INBOX"], ["Archive me"]);
        // And again, with the id the UI first saw: the alias still leads to the message.
        let _ = bridge.send(Cmd::Op { op: 15, id: archive, act: Act::Archive });
        op_ok(&mut events, 15).await;
        assert_eq!(server(&account).await["Archive"], ["Archive me"]);

        let _ = bridge.send(Cmd::RemoveAccount(account.id.clone()));
        // Earlier Changed events may still be queued: wait for the store itself.
        for _ in 0..100 {
            if store.accounts().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(store.accounts().unwrap().is_empty());
        assert!(account.password().is_err(), "keychain entry removed");
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! The mail worker. One background thread runs a Tokio runtime (the core's IMAP and SMTP need
//! it), owns the network and every store write, and works through `Cmd`s in order. The UI reads
//! the store itself and hears about changes through `Event`s, so a frame never waits on the network.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    time::Duration,
};

use lagom_core::{
    account::Account,
    imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp::{self, Outgoing},
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
    AddAccount(Box<Account>, Zeroizing<String>),
    RemoveAccount(String),
    /// One account, or all of them.
    Sync(Option<String>),
    /// IDLE saw a change: INBOX only.
    SyncInbox(String),
    /// `id` is the store row the UI saw. The worker follows it through earlier moves.
    Op { op: u64, id: i64, act: Act },
    MarkSeen(i64),
    Send { op: u64, account: String, mail: Box<Outgoing> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    Syncing,
    /// Unix time of the last finished sync.
    Synced(i64),
    Failed(String),
}

pub enum Event {
    /// The store changed: read it again.
    Changed,
    Sync(String, SyncState),
    Op { op: u64, error: Option<String> },
    /// The new account's id, or why it could not connect.
    Added(Result<String, String>),
    /// `delivered`: SMTP took the message, even if `error` says the Sent copy failed.
    Sent { op: u64, delivered: bool, error: Option<String> },
}

#[derive(Clone)]
pub struct Bridge(UnboundedSender<Cmd>);

impl Bridge {
    pub fn send(&self, cmd: Cmd) {
        // The worker only stops with the app.
        let _ = self.0.send(cmd);
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
    let mut w = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), me, events };
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
            cmd = rx.recv() => match cmd {
                Some(cmd) => w.handle(cmd, &mut rx).await,
                None => break,
            },
            _ = tick.tick() => w.sync(None, false, &mut rx).await,
        }
    }
}

struct Worker {
    store: Store,
    /// One logged-in session per account, reused between commands. Dropped after any error.
    sessions: HashMap<String, ImapSync>,
    idle: HashMap<String, tokio::task::JoinHandle<()>>,
    /// Old store id to new store id for messages this session moved, so undo finds them.
    moved: HashMap<i64, i64>,
    /// Commands that arrived during a sync and wait for it to finish.
    deferred: VecDeque<Cmd>,
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
            Cmd::Op { op, id, act } => {
                let error = self.op(id, act).await.err().map(|e| e.to_string());
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::MarkSeen(id) => {
                // A failure is corrected by the next sync; the UI already shows it read.
                if self.mark_seen(id).await.is_ok() {
                    self.emit(Event::Changed);
                }
            }
            Cmd::Send { op, account, mail } => {
                let (delivered, error) = match self.send(&account, &mail).await {
                    Ok(()) => (true, None),
                    Err(Error::SentCopy) => (true, Some(Error::SentCopy.to_string())),
                    Err(e) => (false, Some(e.to_string())),
                };
                self.emit(Event::Sent { op, delivered, error });
                self.emit(Event::Changed);
            }
            other => self.deferred.push_back(other),
        }
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
            // Gone already is fine; the local data goes either way.
            let _ = account.delete_password();
        }
        let _ = self.store.delete_account(id);
        self.emit(Event::Changed);
    }

    async fn sync(&mut self, only: Option<String>, inbox_only: bool, rx: &mut UnboundedReceiver<Cmd>) {
        let accounts = self.store.accounts().unwrap_or_default();
        for account in accounts.into_iter().filter(|a| only.as_ref().is_none_or(|id| *id == a.id)) {
            self.emit(Event::Sync(account.id.clone(), SyncState::Syncing));
            let state = match self.sync_account(&account, inbox_only, rx).await {
                Ok(()) => SyncState::Synced(now()),
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
        for folder in folders {
            // Triage keys pressed meanwhile run between folders.
            while let Ok(cmd) = rx.try_recv() {
                self.quick(cmd).await;
            }
            let mut session = self.take(account).await?;
            let result = session.sync_folder(&mut self.store, folder.id, DEFAULT_LIMIT).await;
            self.give(&account.id, session, &result);
            result?;
            self.emit(Event::Changed);
        }
        Ok(())
    }

    async fn op(&mut self, id: i64, act: Act) -> lagom_core::Result<()> {
        let mut id = id;
        while let Some(&next) = self.moved.get(&id) {
            id = next;
        }
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
                    let folder = session.create_folder(store, &name).await?;
                    session.move_message(store, id, folder).await?;
                    Some(folder)
                }
                Act::Snooze(_) => unreachable!("handled above"),
            };
            Ok(target)
        }
        .await;
        self.give(&account.id, session, &result);
        if let Some(target) = result? {
            if let Some(new) = self.store.find_message(target, &message.message_id)? {
                self.moved.insert(id, new);
            }
        }
        Ok(())
    }

    async fn mark_seen(&mut self, id: i64) -> lagom_core::Result<()> {
        let account = self.account(&self.store.message(id)?.summary.account)?;
        let mut session = self.take(&account).await?;
        let result = session.mark_seen(&mut self.store, id, true).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn send(&mut self, account: &str, mail: &Outgoing) -> lagom_core::Result<()> {
        let account = self.account(account)?;
        let password = account.password()?;
        smtp::send(&account, &password, mail, &mut self.store).await
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

fn role(store: &Store, account: &str, role: FolderRole) -> Option<i64> {
    store.folders(account, 0).ok()?.into_iter().find(|f| f.role == role).map(|f| f.id)
}

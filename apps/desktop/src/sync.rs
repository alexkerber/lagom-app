//! The mail worker. One background thread runs a Tokio runtime (the core's IMAP and SMTP need
//! it), owns the network and sync writes, and works through `Cmd`s in order. Compose commits
//! locally; the UI reads the store and hears about changes through `Event`s, without network waits.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    future::Future,
    pin::Pin,
    time::Duration,
};

use lagom_core::{
    account::Account,
    calendar::{Calendar, Event as CalendarEvent, NewEvent},
    oauth::{self, Provider, SignedIn},
    discovery::{self, Discovery},
    imap_sync::{ImapSync, DEFAULT_LIMIT},
    smtp,
    store::{FolderRole, Store},
    google::CreatedEvent,
    Error,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zeroize::Zeroizing;

mod tasks;
use tasks::{TaskCmd, TaskWorker};

/// Incremental sync of every account, besides IDLE on INBOX.
pub const SYNC_EVERY: Duration = Duration::from_secs(120);

/// What an optimistic UI action does on the server (or, for Later, in the store).
#[derive(Debug, Clone)]
pub enum Act {
    Archive,
    Read(bool),
    Trash,
    Inbox,
    Move(i64),
    NewFolder(String),
    /// Local only: until this Unix time, `None` to bring it back.
    Snooze(Option<i64>),
}

pub enum Cmd {
    #[cfg(test)]
    Pause(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>),
    /// Onboarding API; the UI consumes OpenUrl on its own thread.
    #[allow(dead_code)]
    SignIn(Provider),
    CancelSignIn,
    Reauthenticate(String),
    AllowCalendar(String),
    CreateEvent { op: u64, account: String, calendar: String, event: NewEvent },
    TasksEdited,
    TasksReauth(String),
    #[allow(dead_code)]
    Discover(String),
    AddAccount(Box<Account>, Zeroizing<String>),
    RemoveAccount(String),
    /// One account, or all of them.
    Sync(Option<String>),
    /// IDLE saw a change: INBOX only.
    SyncInbox(String),
    Drafts(String),
    /// `id` is the store row the UI saw. The worker follows it through earlier moves.
    Op { op: u64, id: i64, act: Act },
    EmptyTrash { op: u64, folder: i64 },
    CreateFolder { op: u64, account: String, name: String },
    MarkSeen(i64),
    Inline(i64),
    ForwardAttachments { draft: String, id: i64 },
    Download { id: i64, section: String },
    /// The message is already durable, including its Undo deadline.
    Send(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    Syncing,
    ReauthRequired,
    SigningIn,
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
    Reauthenticated { account: String, result: Result<(), String> },
    /// The store changed: read it again.
    Changed,
    CalendarSync(String, SyncState),
    EventCreated { op: u64, error: Option<String> },
    TasksSync(String, SyncState),
    NewMail(Vec<lagom_core::store::MessageSummary>),
    Sync(String, SyncState),
    Op { op: u64, error: Option<String> },
    /// The new account's id, or why it could not connect.
    Added(Result<String, String>),
    /// A MarkSeen finished, done or not.
    Seen(i64),
    Inline { id: i64, error: Option<String> },
    Downloaded(Result<PathBuf, String>),
    ForwardAttachments { draft: String, result: Result<Vec<lagom_core::smtp::OutgoingAttachment>, String> },
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

    #[cfg(test)]
    pub fn same_worker(&self, other: &Self) -> bool { self.0.same_channel(&other.0) }

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

async fn run(path: PathBuf, rx: UnboundedReceiver<Cmd>, me: UnboundedSender<Cmd>, events: UnboundedSender<Event>) {
    let store = match Store::open(&path) {
        Ok(store) => store,
        Err(e) => {
            let _ = events.send(Event::Sync(String::new(), SyncState::Failed(e.to_string())));
            return;
        }
    };
    if let Err(error) = store.recover_outbox().and_then(|_| store.recover_moves()) {
        let _ = events.send(Event::Sync(String::new(), SyncState::Failed(error.to_string())));
        return;
    }
    let _ = events.send(Event::Changed);
    let mut w = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
    for account in w.store.accounts().unwrap_or_default() {
        w.watch(&account);
    }
    work(w, rx).await;
}

async fn work(w: Worker, rx: UnboundedReceiver<Cmd>) {
    let store = match Store::open(w.store.path()) {
        Ok(store) => store,
        Err(e) => { w.emit(Event::Sync(String::new(), SyncState::Failed(e.to_string()))); return; }
    };
    let tasks = TaskWorker::new(store, w.me.clone(), w.events.clone());
    work_with_tasks(w, rx, tasks).await;
}

async fn work_with_tasks(mut w: Worker, mut rx: UnboundedReceiver<Cmd>, mut tasks: TaskWorker) {
    let (task_tx, mut task_rx) = unbounded_channel();
    let (mail_tx, mail_rx) = unbounded_channel();
    w.task_sync = Some(task_tx.clone());
    let mail = mail_work(w, mail_rx);
    tokio::pin!(mail);
    // Keep task timers and completions polled while a mail command awaits the network.
    loop {
        tokio::select! {
            _ = &mut mail => break,
            active = tasks.step(&mut task_rx) => if !active { break; },
            cmd = rx.recv() => match cmd {
                Some(Cmd::TasksEdited) => { let _ = task_tx.send(TaskCmd::Edited); },
                Some(cmd) => {
                    if let Cmd::RemoveAccount(id) | Cmd::Reauthenticate(id) | Cmd::AllowCalendar(id) = &cmd {
                        tasks.pause(id);
                    }
                    if mail_tx.send(cmd).is_err() { break; }
                },
                None => break,
            },
        }
    }
}

async fn mail_work(mut w: Worker, mut rx: UnboundedReceiver<Cmd>) {
    // The first tick is immediate: the launch sync.
    let mut tick = tokio::time::interval(SYNC_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry = tokio::time::interval(Duration::from_secs(5));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        w.finish_creates();
        if let Some(cmd) = w.deferred.pop_front() {
            w.handle(cmd, &mut rx).await;
            continue;
        }
        tokio::select! {
            Some(result) = w.event_creates.join_next_with_id(), if !w.event_creates.is_empty() => w.event_created(result),
            Some(result) = w.calendars.join_next_with_id(), if !w.calendars.is_empty() => w.calendar_finished(result),
            result = async {
                match &mut w.sign_in {
                    Some(flow) => flow.await.unwrap_or(Err(Error::Network("sign-in task failed"))),
                    None => std::future::pending().await,
                }
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
            _ = retry.tick() => { w.retry_events(); w.sync_drafts(None).await; },
            _ = tick.tick() => w.sync(None, false, &mut rx).await,
        }
    }
}

type CalendarResult = lagom_core::Result<Vec<(Calendar, Vec<CalendarEvent>)>>;
type CalendarJob = (Account, i64, i64, CalendarResult);

type Flow<T> = Pin<Box<dyn Future<Output = T> + Send>>;

enum CreateJob { Create(u64), Conference(String) }

struct PendingEvent {
    account: Account,
    calendar: String,
    created: CreatedEvent,
    cached: bool,
    deleted: bool,
    retry_at: tokio::time::Instant,
}

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
    sign_in: Option<tokio::task::JoinHandle<lagom_core::Result<SignedIn>>>,
    reauth: Option<String>,
    calendar_consent: bool,
    calendars: tokio::task::JoinSet<CalendarJob>,
    calendar_tasks: HashMap<String, tokio::task::AbortHandle>,
    event_creates: tokio::task::JoinSet<lagom_core::Result<CreatedEvent>>,
    create_tasks: HashMap<tokio::task::Id, (CreateJob, Account, String)>,
    // shortcut: retries live in this worker; persist them when offline calendar editing lands.
    pending_events: Vec<PendingEvent>,
    task_sync: Option<UnboundedSender<TaskCmd>>,
    reauth_required: HashSet<String>,
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
            Cmd::Sync(only) => {
                for account in self.store.accounts().unwrap_or_default().into_iter().filter(|a| only.as_ref().is_none_or(|id| *id == a.id)) {
                    self.sync_tasks(&account);
                }
                self.sync(only, false, rx).await;
            },
            Cmd::SyncInbox(account) => self.sync(Some(account), true, rx).await,
            quick => self.quick(quick).await,
        }
    }

    /// Commands that a running sync handles between folders, so a triage key never waits long.
    async fn quick(&mut self, cmd: Cmd) {
        let calendar_consent = matches!(&cmd, Cmd::AllowCalendar(_));
        match cmd {
            Cmd::Drafts(account) => self.sync_drafts(Some(&account)).await,
            Cmd::TasksEdited => { if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Edited); } },
            Cmd::TasksReauth(id) => self.require_reauth(&id),
            #[cfg(test)]
            Cmd::Pause(ready, resume) => {
                ready.send(()).unwrap();
                resume.recv().expect("resume fixture worker");
            }
            Cmd::SignIn(provider) => {
                self.cancel_sign_in();
                let events = self.events.clone();
                self.start_sign_in(oauth::sign_in(provider, move |url| {
                    let _ = events.send(Event::OpenUrl(url.into()));
                }));
            }
            Cmd::Reauthenticate(id) | Cmd::AllowCalendar(id) => {
                if self.reauth.as_ref() == Some(&id) { return; }
                self.cancel_sign_in();
                self.cancel_calendar(&id);
                if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Pause(id.clone())); }
                self.reauth = Some(id.clone());
                self.calendar_consent = calendar_consent;
                match self.account(&id) {
                    Ok(account) => {
                        let scopes = match self.store.account_scopes(&id) {
                            Ok(mut scopes) => {
                                if calendar_consent {
                                    if account.auth != lagom_core::account::Auth::OAuth(Provider::Google) {
                                        self.signed_in(Err(Error::Invalid("Google calendar requires a Google account")));
                                        return;
                                    }
                                    scopes.push(' ');
                                    scopes.push_str(oauth::GOOGLE_CALENDAR_SCOPES);
                                }
                                scopes
                            }
                            Err(e) => { self.signed_in(Err(e)); return; }
                        };
                        self.emit(if calendar_consent { Event::CalendarSync(id, SyncState::SigningIn) } else { Event::Sync(id, SyncState::SigningIn) });
                        let events = self.events.clone();
                        self.start_sign_in(async move {
                            oauth::reauthenticate(&account, &scopes, move |url| {
                                let _ = events.send(Event::OpenUrl(url.into()));
                            }).await
                        });
                    }
                    Err(error) => self.signed_in(Err(error)),
                }
            }
            Cmd::CancelSignIn => {
                if self.sign_in.is_some() { self.cancel_sign_in(); }
                else { self.emit(Event::SignInCancelled); }
            }
            Cmd::Discover(email) => {
                self.discovery = Some(Box::pin(async move {
                    let result = discovery::discover(&email).await;
                    (email, result)
                }));
            }
            Cmd::Op { op, id, act } => {
                let mut error = self.op(id, act).await.err().map(|e| e.to_string());
                if let Some(error) = &mut error {
                    if let Err(rollback) = self.store.cancel_move(id) { error.push_str(&format!("; local rollback failed: {rollback}")); }
                }
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::EmptyTrash { op, folder } => {
                let error = self.empty_trash(folder).await.err().map(|e| e.to_string());
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::CreateEvent { op, account, calendar, event } => {
                if let Err(error) = self.create_event(op, &account, calendar, event) {
                    self.emit(Event::EventCreated { op, error: Some(error.to_string()) });
                }
            }
            Cmd::CreateFolder { op, account, name } => {
                let error = self.create_folder(&account, &name).await.err().map(|e| e.to_string());
                self.emit(Event::Op { op, error });
                self.emit(Event::Changed);
            }
            Cmd::ForwardAttachments { draft, id } => {
                let result = self.forward_attachments(id).await.map_err(|e| e.to_string());
                self.emit(Event::ForwardAttachments { draft, result });
            }
            Cmd::Download { id, section } => {
                let result = self.download(id, &section).await.map_err(|e| e.to_string());
                self.emit(Event::Downloaded(result));
            }
            Cmd::Inline(id) => {
                let error = self.load_inline(id).await.err().map(|e| e.to_string());
                self.emit(Event::Inline { id, error });
            }
            Cmd::MarkSeen(id) => {
                // Either way the UI drops its override: the store says what the server has.
                let _ = self.mark_seen(id, true).await;
                self.emit(Event::Seen(id));
                self.emit(Event::Changed);
            }
            Cmd::Send(id) => { self.send(&id).await; }
            other => self.deferred.push_back(other),
        }
    }

    fn start_sign_in(&mut self, flow: impl Future<Output = lagom_core::Result<SignedIn>> + Send + 'static) {
        // OAuth must keep polling while the worker awaits another account's network work.
        self.sign_in = Some(tokio::spawn(flow));
    }

    fn cancel_sign_in(&mut self) {
        if let Some(flow) = self.sign_in.take() {
            flow.abort();
            self.signed_in(Err(Error::Cancelled));
        }
    }

    fn require_reauth(&mut self, id: &str) {
        self.cancel_calendar(id);
        if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Pause(id.into())); }
        self.reauth_required.insert(id.into());
        self.sessions.remove(id);
        if let Some(task) = self.idle.remove(id) { task.abort(); }
        if self.reauth.as_deref() != Some(id) { self.emit(Event::ReauthRequired(id.into())); }
    }

    fn signed_in(&mut self, result: lagom_core::Result<SignedIn>) {
        if let Some(id) = self.reauth.take() {
            let calendar_consent = std::mem::take(&mut self.calendar_consent);
            let result = result.and_then(|signed| {
                let account = self.account(&id)?;
                if signed.account != account { return Err(Error::Invalid("account changed during sign-in")); }
                self.store.set_account_scopes(&id, &signed.scopes)?;
                self.store.resume_outbox(&id, chrono::Local::now().timestamp_millis())?;
                self.reauth_required.remove(&id);
                self.sessions.remove(&id);
                self.watch(&account);
                self.deferred.push_back(Cmd::Sync(Some(id.clone())));
                self.emit(Event::Changed);
                Ok(())
            });
            if result.is_ok() || calendar_consent {
                if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Resume(id.clone())); }
            }
            if calendar_consent {
                let state = match &result {
                    Ok(()) | Err(Error::Cancelled) => SyncState::Synced(now()),
                    Err(e) => SyncState::Failed(e.to_string()),
                };
                self.emit(Event::CalendarSync(id.clone(), state));
                if result.is_err() { return; }
            }
            if result.is_err() { self.require_reauth(&id); }
            let result = match result {
                Err(Error::Cancelled) => return,
                other => other.map_err(|e| e.to_string()),
            };
            self.emit(Event::Reauthenticated { account: id, result });
            return;
        }
        let signed = match result {
            Ok(signed) => signed,
            Err(Error::Cancelled) => { self.emit(Event::SignInCancelled); return; }
            Err(error) => { self.emit(Event::SignedIn(Err(error.to_string()))); return; }
        };
        let account = signed.account;
        if let Err(error) = self.store.save_authorized_account(&account, &signed.scopes) {
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
    fn give<T>(&mut self, account: &str, mut session: ImapSync, result: &lagom_core::Result<T>) {
        if let Some(error) = session.take_refresh_error() {
            self.emit(Event::Sync(account.into(), SyncState::Failed(format!("Mail moved; refresh failed: {error}"))));
        } else if result.is_ok() {
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
        self.remove_with(id, Account::delete_password);
    }

    fn remove_with(&mut self, id: &str, delete_password: impl FnOnce(&Account) -> lagom_core::Result<()>) {
        self.cancel_calendar(id);
        if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Pause(id.into())); }
        if self.reauth.as_deref() == Some(id) { self.cancel_sign_in(); }
        if let Some(task) = self.idle.remove(id) {
            task.abort();
        }
        self.sessions.remove(id);
        if let Ok(account) = self.account(id) {
            // Keep the row while the secret stays: without it nobody could find the secret again.
            if delete_password(&account).is_err() {
                if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Resume(id.into())); }
                self.emit(Event::Sync(id.to_string(), SyncState::Failed("could not remove the saved password; try Remove again".into())));
                self.emit(Event::Changed);
                return;
            }
        }
        let _ = self.store.delete_account(id);
        self.reauth_required.remove(id);
        self.emit(Event::Changed);
    }

    async fn sync_drafts(&mut self, only: Option<&str>) {
        let accounts = match self.store.accounts() {
            Ok(accounts) => accounts,
            Err(e) => { self.emit(Event::Sync(String::new(), SyncState::Failed(e.to_string()))); return; }
        };
        let mut changed = false;
        for account in accounts.into_iter().filter(|a| only.is_none_or(|id| id == a.id)) {
            if self.reauth_required.contains(&account.id) { continue; }
            let now = chrono::Local::now().timestamp_millis();
            let ids = match self.store.due_drafts(&account.id, now) {
                Ok(ids) => ids,
                Err(e) => { self.emit(Event::Sync(account.id, SyncState::Failed(e.to_string()))); continue; }
            };
            if ids.is_empty() { continue; }
            changed = true;
            let mut failure = None;
            for id in ids {
                let result = if failure.is_some() { Ok(()) } else {
                    match self.take(&account).await {
                        Ok(mut session) => {
                            let result = session.sync_draft(&mut self.store, &id, now).await;
                            self.give(&account.id, session, &result);
                            result
                        }
                        Err(e) => Err(e),
                    }
                };
                if let Err(error) = result { failure = Some(error); }
                if let Some(error) = &failure {
                    if let Err(e) = self.store.draft_sync_failed(&id, error, now) {
                        self.emit(Event::Sync(account.id.clone(), SyncState::Failed(e.to_string())));
                    }
                    if matches!(error, Error::Invalid(_)) {
                        self.emit(Event::Sync(account.id.clone(), SyncState::Failed(format!("Draft kept locally: {error}"))));
                        failure = None;
                    }
                }
            }
            if let Some(error) = failure {
                if matches!(error, Error::ReauthRequired) { self.require_reauth(&account.id); }
                self.emit(Event::Sync(account.id.clone(), SyncState::Failed(format!("Draft kept locally: {error}"))));
            }
        }
        if changed { self.emit(Event::Changed); }
    }

    async fn sync(&mut self, only: Option<String>, inbox_only: bool, rx: &mut UnboundedReceiver<Cmd>) {
        self.sync_drafts(only.as_deref()).await;
        let accounts = self.store.accounts().unwrap_or_default();
        for account in accounts.into_iter().filter(|a| only.as_ref().is_none_or(|id| *id == a.id)) {
            if self.reauth_required.contains(&account.id) { continue; }
            if !inbox_only { self.sync_calendar(&account); }
            if self.reauth_required.contains(&account.id) { continue; }
            self.emit(Event::Sync(account.id.clone(), SyncState::Syncing));
            match self.store.due_outbox(&account.id, chrono::Local::now().timestamp_millis()) {
                Ok(ids) => for id in ids { self.send(&id).await; },
                Err(e) => self.emit(Event::Sync(account.id.clone(), SyncState::Failed(e.to_string()))),
            }
            if self.reauth_required.contains(&account.id) { continue; }
            let state = match self.sync_account(&account, inbox_only, rx).await {
                Ok(()) => SyncState::Synced(now()),
                Err(Error::ReauthRequired) => {
                    self.require_reauth(&account.id);
                    SyncState::ReauthRequired
                }
                Err(e) => SyncState::Failed(e.to_string()),
            };
            self.emit(Event::Sync(account.id.clone(), state));
            self.emit(Event::Changed);
        }
    }

    fn create_event(&mut self, op: u64, id: &str, calendar: String, event: NewEvent) -> lagom_core::Result<()> {
        let account = self.account(id)?;
        if account.auth != lagom_core::account::Auth::OAuth(Provider::Google) {
            return Err(Error::Invalid("choose a Google calendar"));
        }
        if !oauth::calendar_writable(&self.store.account_scopes(id)?) { return Err(Error::CalendarConsent); }
        if !self.store.calendars(id)?.iter().any(|c| c.remote_id == calendar && c.writable) {
            return Err(Error::Invalid("choose a writable calendar"));
        }
        event.event("validation".into())?;
        let (owned, remote) = (account.clone(), calendar.clone());
        self.start_create(op, account, calendar, async move { lagom_core::google::create_event(&owned, &remote, &event).await });
        Ok(())
    }

    fn start_create(&mut self, op: u64, account: Account, calendar: String,
        create: impl Future<Output = lagom_core::Result<CreatedEvent>> + Send + 'static) {
        let task = self.event_creates.spawn(create);
        self.create_tasks.insert(task.id(), (CreateJob::Create(op), account, calendar));
    }

    fn event_created(&mut self, completed: Result<(tokio::task::Id, lagom_core::Result<CreatedEvent>), tokio::task::JoinError>) {
        let task = match &completed { Ok((task, _)) => *task, Err(error) => error.id() };
        let Some((job, account, calendar)) = self.create_tasks.remove(&task) else { return };
        let result = completed.map_err(|_| Error::Network("event creation unconfirmed; check your calendar before retrying"))
            .and_then(|(_, result)| result);
        let remote_error = result.as_ref().err().map(ToString::to_string);
        if let CreateJob::Conference(id) = &job {
            if matches!(result, Err(Error::NotFound)) {
                if let Some(index) = self.pending_events.iter().position(|p| p.account == account && p.calendar == calendar && &p.created.event.remote_id == id) {
                    let mut pending = self.pending_events.remove(index);
                    pending.deleted = true;
                    pending.cached = false;
                    pending.created.conference_pending = false;
                    self.cancel_calendar(&account.id);
                    let result = self.cache_event(&mut pending);
                    if !pending.cached { self.pending_events.push(pending); }
                    self.calendar_synced(&account, result);
                }
                return;
            }
        }
        let result = result.and_then(|created| {
            // A fetch started before this response must not erase the newly cached event.
            self.cancel_calendar(&account.id);
            self.pending_events.retain(|p| p.account.id != account.id || p.calendar != calendar
                || p.created.event.remote_id != created.event.remote_id);
            let mut pending = PendingEvent { account: account.clone(), calendar, created, cached: false, deleted: false, retry_at: tokio::time::Instant::now() + Duration::from_secs(5) };
            let result = self.cache_event(&mut pending);
            if !pending.cached || pending.created.conference_pending { self.pending_events.push(pending); }
            result
        });
        if let CreateJob::Create(op) = job {
            self.emit(Event::EventCreated { op, error: remote_error });
        }
        self.calendar_synced(&account, result);
    }

    fn cache_event(&self, pending: &mut PendingEvent) -> lagom_core::Result<()> {
        if self.account(&pending.account.id)? != pending.account { return Err(Error::Invalid("calendar account changed")); }
        if pending.deleted {
            self.store.delete_calendar_event(&pending.account.id, &pending.calendar, &pending.created.event.remote_id)?;
        } else {
            self.store.insert_calendar_event(&pending.account.id, &pending.calendar, &pending.created.event)?;
        }
        pending.cached = true;
        Ok(())
    }

    fn finish_creates(&mut self) {
        while let Some(result) = self.event_creates.try_join_next_with_id() { self.event_created(result); }
    }

    fn retry_events(&mut self) {
        self.retry_events_with(|account, calendar, id| async move {
            lagom_core::google::fetch_created(&account, &calendar, &id).await
        });
    }

    fn retry_events_with<F, Fut>(&mut self, mut fetch: F)
    where F: FnMut(Account, String, String) -> Fut,
        Fut: Future<Output = lagom_core::Result<CreatedEvent>> + Send + 'static {
        for mut pending in std::mem::take(&mut self.pending_events) {
            match self.account(&pending.account.id) {
                Ok(account) if account != pending.account => continue,
                Err(Error::NotFound) => continue,
                Err(error) => {
                    self.calendar_synced(&pending.account, Err(error));
                    self.pending_events.push(pending);
                    continue;
                }
                _ => {}
            }
            if !pending.cached {
                self.cancel_calendar(&pending.account.id);
                let result = self.cache_event(&mut pending);
                self.calendar_synced(&pending.account, result);
            }
            if pending.created.conference_pending {
                let busy = self.create_tasks.values().any(|(job, account, calendar)| account == &pending.account
                    && calendar == &pending.calendar && matches!(job, CreateJob::Conference(id) if id == &pending.created.event.remote_id));
                if !busy && tokio::time::Instant::now() >= pending.retry_at && !self.reauth_required.contains(&pending.account.id) && self.reauth.as_ref() != Some(&pending.account.id)
                    && self.store.account_scopes(&pending.account.id).is_ok_and(|s| oauth::calendar_allowed(&s)) {
                    let (account, calendar, id) = (pending.account.clone(), pending.calendar.clone(), pending.created.event.remote_id.clone());
                    pending.retry_at = tokio::time::Instant::now() + Duration::from_secs(5);
                    let task = self.event_creates.spawn(fetch(account, calendar, id));
                    self.create_tasks.insert(task.id(), (CreateJob::Conference(pending.created.event.remote_id.clone()), pending.account.clone(), pending.calendar.clone()));
                }
            }
            if !pending.cached || pending.created.conference_pending { self.pending_events.push(pending); }
        }
    }

    fn sync_tasks(&self, account: &Account) {
        if self.reauth.as_deref() == Some(&account.id) || self.reauth_required.contains(&account.id) { return; }
        if let Some(tasks) = &self.task_sync { let _ = tasks.send(TaskCmd::Sync(account.clone())); }
    }

    fn sync_calendar(&mut self, account: &Account) {
        if self.pending_events.iter().any(|p| p.account.id == account.id && !p.cached) { return; }
        if self.reauth.as_deref() == Some(&account.id) || account.auth != lagom_core::account::Auth::OAuth(Provider::Google) { return; }
        match self.store.account_scopes(&account.id) {
            Ok(scopes) if oauth::calendar_allowed(&scopes) => {}
            Ok(_) => return,
            Err(error) => { self.calendar_synced(account, Err(error)); return; }
        }
        let now = now();
        let (start, end) = (now - 7 * 86400, now + 60 * 86400);
        let owned = account.clone();
        self.start_calendar(account.clone(), start, end, async move { lagom_core::google::fetch_calendar(&owned, start, end).await });
    }

    fn start_calendar(&mut self, account: Account, start: i64, end: i64,
        fetch: impl Future<Output = CalendarResult> + Send + 'static) {
        if self.calendar_tasks.contains_key(&account.id) { return; }
        let id = account.id.clone();
        let task = self.calendars.spawn(async move { (account, start, end, fetch.await) });
        self.calendar_tasks.insert(id, task);
    }

    fn cancel_calendar(&mut self, id: &str) {
        if let Some(task) = self.calendar_tasks.remove(id) { task.abort(); }
    }

    fn calendar_finished(&mut self, completed: Result<(tokio::task::Id, CalendarJob), tokio::task::JoinError>) {
        let task = match &completed { Ok((task, _)) => *task, Err(error) => error.id() };
        // A completed request can still be queued when removal or consent cancels it.
        let Some(id) = self.calendar_tasks.iter().find(|(_, handle)| handle.id() == task).map(|(id, _)| id.clone()) else { return };
        self.calendar_tasks.remove(&id);
        let Ok(account) = self.account(&id) else { return };
        let result = match completed {
            Ok((_, (fetched, start, end, result))) if fetched == account =>
                result.and_then(|calendars| self.store.replace_calendar_window(&id, start, end, &calendars)),
            Ok(_) => return,
            Err(_) => Err(Error::Network("calendar sync task failed")),
        };
        self.calendar_synced(&account, result);
    }

    fn calendar_synced(&mut self, account: &Account, result: lagom_core::Result<()>) {
        if matches!(result, Err(Error::CalendarConsent)) {
            let cleared = self.store.account_scopes(&account.id).and_then(|scopes| {
                let scopes = scopes.split_whitespace().filter(|s| !s.starts_with("https://www.googleapis.com/auth/calendar")).collect::<Vec<_>>().join(" ");
                self.store.set_account_scopes(&account.id, &scopes)
            });
            if let Err(e) = cleared {
                self.emit(Event::CalendarSync(account.id.clone(), SyncState::Failed(e.to_string())));
                return;
            }
        }
        if matches!(result, Err(Error::ReauthRequired)) { self.require_reauth(&account.id); }
        self.emit(Event::CalendarSync(account.id.clone(), match result {
            Ok(()) => SyncState::Synced(now()), Err(e) => SyncState::Failed(e.to_string()),
        }));
        self.emit(Event::Changed);
    }

    async fn sync_account(&mut self, account: &Account, inbox_only: bool, rx: &mut UnboundedReceiver<Cmd>) -> lagom_core::Result<()> {
        let mut session = self.take(account).await?;
        let listed = session.list_folders(&self.store).await;
        self.give(&account.id, session, &listed);
        let mut folders = listed?;
        self.emit(Event::Changed);
        // INBOX first, so the list fills where the user looks.
        folders.sort_by_key(|f| f.role != FolderRole::Inbox);
        if inbox_only {
            folders.retain(|f| f.role == FolderRole::Inbox);
        }
        // One bad folder must not keep the others from syncing.
        let mut first = None;
        for folder in folders {
            self.finish_creates();
            self.retry_events();
            // Triage keys pressed meanwhile run between folders.
            while let Ok(cmd) = rx.try_recv() {
                self.quick(cmd).await;
            }
            let result = self.sync_folder(account, folder.id).await;
            self.finish_creates();
            match result {
                Ok(()) => {
                    self.emit(Event::Changed);
                },
                Err(e) if e.is_account_wide() => return Err(e),
                Err(e) => {
                    first.get_or_insert(lagom_core::Error::Folder { folder: folder.name, source: Box::new(e) });
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    async fn sync_folder(&mut self, account: &Account, folder: i64) -> lagom_core::Result<()> {
        let before = self.store.folder(folder)?;
        let mut session = self.take(account).await?;
        // Folder I/O must leave the worker free to publish completed calendar requests.
        let mut store = Store::open(self.store.path())?;
        let result = {
            let sync = session.sync_folder(&mut store, folder, DEFAULT_LIMIT);
            tokio::pin!(sync);
            loop {
                tokio::select! {
                    result = &mut sync => break result,
                    Some(result) = self.event_creates.join_next_with_id(), if !self.event_creates.is_empty() => self.event_created(result),
                }
            }
        };
        self.give(&account.id, session, &result);
        result?;
        self.notify_new_mail(&before)
    }

    fn notify_new_mail(&self, before: &lagom_core::store::Folder) -> lagom_core::Result<()> {
        if before.role != FolderRole::Inbox { return Ok(()); }
        let after = self.store.folder(before.id)?;
        let messages = self.store.messages_since(before.id, before.last_uid)?;
        let messages = crate::notifications::new_mail(before, &after, messages);
        if !messages.is_empty() { self.emit(Event::NewMail(messages)); }
        Ok(())
    }

    async fn empty_trash(&mut self, folder: i64) -> lagom_core::Result<()> {
        let target = self.store.folder(folder)?;
        if target.role != FolderRole::Trash {
            return Err(Error::Invalid("not a Trash folder"));
        }
        let account = self.account(&target.account)?;
        let mut session = self.take(&account).await?;
        let result = session.empty_trash(&mut self.store, folder).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn op(&mut self, id: i64, act: Act) -> lagom_core::Result<()> {
        let id = self.resolve(id).await?;
        if let Act::Read(seen) = act { return self.mark_seen(id, seen).await; }
        if let Act::Snooze(until) = act {
            return self.store.snooze(id, until);
        }
        let message = self.store.message(id)?.summary;
        let account = self.account(&message.account)?;
        let before: Vec<_> = self.store.folders(&account.id, now())?.into_iter().filter(|f| f.role == FolderRole::Inbox).collect();
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
                Act::Inbox => {
                    let inbox = role(store, &account.id, FolderRole::Inbox).ok_or(Error::NotFound)?;
                    session.move_message(store, id, inbox).await?;
                    Some(inbox)
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
                Act::Snooze(_) | Act::Read(_) => unreachable!("handled above"),
            };
            Ok(target)
        }
        .await;
        self.give(&account.id, session, &result);
        // A move can finish the Inbox sync before a later folder fails.
        let notified = before.iter().try_for_each(|folder| self.notify_new_mail(folder));
        if let Some(target) = result?.filter(|_| !message.message_id.is_empty()) {
            follow(&mut self.moved, id, message.folder, &message.message_id, target);
        }
        if let Err(error) = notified {
            self.emit(Event::Sync(account.id, SyncState::Failed(error.to_string())));
        }
        Ok(())
    }

    async fn create_folder(&mut self, account: &str, name: &str) -> lagom_core::Result<()> {
        let folders = self.store.folders(account, now())?;
        if folders.is_empty() {
            return Err(Error::Invalid("Sync folders before creating a folder"));
        }
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
            self.sync_folder(&account, folder).await?;
        }
        self.store.find_message(folder, &message_id)?.ok_or(Error::NotFound)
    }

    async fn load_inline(&mut self, id: i64) -> lagom_core::Result<()> {
        let account = self.account(&self.store.message_summary(id)?.account)?;
        let mut session = self.take(&account).await?;
        let result = session.fetch_inline(&mut self.store, id).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn attachment_bytes(&mut self, id: i64, section: &str) -> lagom_core::Result<Vec<u8>> {
        if let Some(data) = self.store.cached_attachment(id, section)? { return Ok(data); }
        let account = self.account(&self.store.message_summary(id)?.account)?;
        let mut session = self.take(&account).await?;
        let result = session.fetch_attachment(&mut self.store, id, section).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn forward_attachments(&mut self, id: i64) -> lagom_core::Result<Vec<lagom_core::smtp::OutgoingAttachment>> {
        let id = self.resolve(id).await?;
        if !self.store.attachment_metadata_loaded(id)? {
            let m = self.store.message_summary(id)?;
            let account = self.account(&m.account)?;
            self.sync_folder(&account, m.folder).await?;
            // shortcut: legacy metadata backfill is bounded; retry after sync if this message is beyond that batch.
            if !self.store.attachment_metadata_loaded(id)? { return Err(Error::Invalid("sync attachments before forwarding")); }
        }
        let mut parts = Vec::new();
        for part in self.store.attachments(id)? {
            let data = self.attachment_bytes(id, &part.section).await?;
            parts.push(lagom_core::smtp::OutgoingAttachment { filename: part.filename, mime_type: part.mime_type, data });
        }
        Ok(parts)
    }

    async fn download(&mut self, id: i64, section: &str) -> lagom_core::Result<PathBuf> {
        let id = self.resolve(id).await?;
        let part = self.store.attachments(id)?.into_iter().find(|p| p.section == section).ok_or(Error::NotFound)?;
        let data = self.attachment_bytes(id, section).await?;
        lagom_core::attachment::save_download(&part.filename, &data)
    }

    async fn mark_seen(&mut self, id: i64, seen: bool) -> lagom_core::Result<()> {
        let id = self.resolve(id).await?;
        let account = self.account(&self.store.message(id)?.summary.account)?;
        let mut session = self.take(&account).await?;
        let result = session.mark_seen(&mut self.store, id, seen).await;
        self.give(&account.id, session, &result);
        result
    }

    async fn send(&mut self, id: &str) {
        if self.store.pending(id).is_ok_and(|p| self.reauth_required.contains(&p.account)) { return; }
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
                        self.require_reauth(&pending.account);
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
    async fn review_followup_read_rejection_preserves_flags_and_reports_failure() {
        use lagom_core::account::{Security, ServerConfig};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for status in ["NO", "BAD"] {
            for seen in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
                let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = BufReader::new(socket);
                    socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                    loop {
                        let mut line = String::new();
                        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                        let (tag, command) = line.trim_end().split_once(' ').unwrap();
                        let data = match command {
                            "LOGIN \"fixture\" \"fixture\"" => "",
                            "CAPABILITY" => "* CAPABILITY IMAP4rev1\r\n",
                            "SELECT \"INBOX\"" => "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n",
                            _ => {
                                assert_eq!(command, format!("UID STORE 1 {}FLAGS.SILENT (\\Seen)", if seen { "+" } else { "-" }));
                                socket.get_mut().write_all(format!("{tag} {status} refused\r\n").as_bytes()).await.unwrap();
                                break;
                            }
                        };
                        socket.get_mut().write_all(format!("{data}{tag} OK fixture\r\n").as_bytes()).await.unwrap();
                    }
                });
                let path = std::env::temp_dir().join(format!("lagom-read-rejection-{}.db", account.id));
                let mut store = Store::open(&path).unwrap();
                store.save_account(&account).unwrap();
                let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
                store.reset_uidvalidity(folder, 7).unwrap();
                let id = store.save_message(&lagom_core::store::Message { summary: lagom_core::store::MessageSummary {
                    account: account.id.clone(), folder, uid: 1, seen: !seen, flagged: true, ..Default::default()
                }, ..Default::default() }).unwrap();
                let (me, _rx) = unbounded_channel();
                let (events, mut out) = unbounded_channel();
                let session = ImapSync::login(&account, "fixture").await.unwrap();
                let mut worker = Worker { store, sessions: HashMap::from([(account.id.clone(), session)]), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
                worker.quick(Cmd::Op { op: 42, id, act: Act::Read(seen) }).await;
                let completion = out.recv().await.unwrap();
                let summary = worker.store.message(id).unwrap().summary;
                drop(worker);
                server.await.unwrap();
                std::fs::remove_file(path).unwrap();
                assert!(matches!(completion, Event::Op { op: 42, error: Some(_) }), "{status} must fail the operation");
                assert_eq!(summary.seen, !seen, "{status} must not change local read flags");
                assert!(summary.flagged);
            }
        }
    }

    #[tokio::test]
    async fn review_task_debounce_and_completion_run_while_mail_list_is_stalled() {
        use lagom_core::account::{Security, ServerConfig};
        use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, sync::oneshot};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.test".into(), config.clone(), config).unwrap();
        let (listing, listed) = oneshot::channel();
        let (release, released) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
            loop {
                let mut line = String::new();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                if command.starts_with("LIST ") {
                    listing.send(()).unwrap();
                    let _ = released.await;
                    break;
                }
                let data = if command == "CAPABILITY" { "* CAPABILITY IMAP4rev1\r\n" } else { "" };
                socket.get_mut().write_all(format!("{data}{tag} OK fixture\r\n").as_bytes()).await.unwrap();
            }
        });
        let path = std::env::temp_dir().join(format!("lagom-task-stall-{}.db", account.id));
        let store = Store::open(&path).unwrap();
        store.save_account(&account).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let mut google = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        google.auth = lagom_core::account::Auth::OAuth(Provider::Google);
        store.save_authorized_account(&google, oauth::GOOGLE_TASKS_SCOPE).unwrap();
        let list = store.default_task_list().unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("DELETE FROM task_lists WHERE id<>?1", [list]).unwrap();
        db.execute("UPDATE task_lists SET account=?1,remote_id='work',dirty=0", [&google.id]).unwrap();
        drop(db);
        let http = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("http://{}/", http.local_addr().unwrap());
        let http_server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in [
                r#"{"items":[{"id":"work","title":"Work"}]}"#, r#"{"id":"work","title":"Work"}"#, r#"{"items":[]}"#,
                r#"{"items":[{"id":"work","title":"Work"}]}"#, r#"{"id":"work","title":"Work"}"#,
                r#"{"id":"task","title":"Edited during IMAP"}"#, r#"{"items":[]}"#,
            ] {
                let (mut socket, _) = http.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") { head.push(socket.read_u8().await.unwrap()); }
                let head = String::from_utf8(head).unwrap();
                let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|s| s.parse().unwrap())).unwrap_or(0);
                let mut data = vec![0; length];
                socket.read_exact(&mut data).await.unwrap();
                requests.push(format!("{head}{}", String::from_utf8(data).unwrap()));
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let editor = Store::open(&path).unwrap();
        let session = ImapSync::login(&account, "fixture").await.unwrap();
        let (me, rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let worker = Worker { store, sessions: HashMap::from([(account.id.clone(), session)]), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::from([Cmd::Sync(Some(account.id.clone()))]), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let bridge = worker.me.clone();
        let mut tasks = TaskWorker::new(Store::open(&path).unwrap(), worker.me.clone(), worker.events.clone());
        tasks.endpoint = Some(endpoint);
        let run = work_with_tasks(worker, rx, tasks);
        tokio::pin!(run);
        let result = tokio::select! {
            _ = &mut run => panic!("worker exited"),
            result = async {
                listed.await.unwrap();
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if matches!(out.recv().await, Some(Event::TasksSync(id, SyncState::Synced(_))) if id == google.id) { break; }
                    }
                    editor.add_task(list, "Edited during IMAP", "", None).unwrap();
                    bridge.send(Cmd::TasksEdited).unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    bridge.send(Cmd::TasksEdited).unwrap();
                    let edited = tokio::time::Instant::now();
                    loop {
                        match out.recv().await.unwrap() {
                            Event::TasksSync(id, SyncState::Synced(_)) if id == google.id => break,
                            Event::TasksSync(_, SyncState::Failed(e)) => panic!("fixture sync failed: {e}"),
                            _ => {}
                        }
                    }
                    assert!(edited.elapsed() >= Duration::from_millis(400), "last edit must reset the debounce timer");
                    assert!(!editor.tasks(list, false).unwrap()[0].dirty);
                }).await
            } => result,
        };
        drop(release);
        server.await.unwrap();
        assert!(result.is_ok(), "Tasks debounce and completion must run before stalled IMAP returns");
        let requests = http_server.await.unwrap();
        assert!(requests[5].starts_with("POST /lists/work/tasks "));
        assert!(requests[5].contains("Edited during IMAP"));
        drop(editor);
        // The pinned worker still holds the database open here, and Windows cannot delete an open file.
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn calendar_jobs_deduplicate_and_discard_cancelled_results() {
        let path = std::env::temp_dir().join(format!("lagom-calendar-jobs-{}.db", std::process::id()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        worker.store.save_authorized_account(&account, oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
        let snapshot = vec![(Calendar { remote_id: "fixture".into(), name: "Fixture".into(), color: None, shown: true, writable: true },
            vec![CalendarEvent { remote_id: "fixture".into(), title: "Fixture".into(), start: "2026-11-09".into(), end: "2026-11-10".into(),
                start_utc: 1, end_utc: 2, all_day: true, location: None, join_url: None, html_link: None }])];
        worker.start_calendar(account.clone(), 0, 3, std::future::ready(Ok(snapshot.clone())));
        worker.start_calendar(account.clone(), 0, 3, async { panic!("duplicate fetch") });
        assert_eq!(worker.calendars.len(), 1);
        let stale = worker.calendars.join_next_with_id().await.unwrap();
        worker.cancel_calendar(&account.id);
        worker.start_calendar(account.clone(), 0, 3, std::future::ready(Ok(snapshot)));
        worker.calendar_finished(stale);
        assert!(worker.store.today_events("2026-11-09", 0, 3).unwrap().is_empty());
        assert_eq!(worker.store.calendar_window(&account.id).unwrap(), None);
        assert!(out.try_recv().is_err());
        assert_eq!(worker.calendar_tasks.len(), 1);
        let fresh = worker.calendars.join_next_with_id().await.unwrap();
        worker.calendar_finished(fresh);
        assert_eq!(worker.store.today_events("2026-11-09", 0, 3).unwrap().len(), 1);
        assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((0, 3)));
        assert!(worker.calendar_tasks.is_empty());
        worker.start_calendar(account.clone(), 400, 500, std::future::ready(Err(Error::Network("offline"))));
        let failed = worker.calendars.join_next_with_id().await.unwrap();
        worker.calendar_finished(failed);
        assert_eq!(worker.store.calendar_window(&account.id).unwrap(), Some((0, 3)));
        while out.try_recv().is_ok() {}
        worker.start_calendar(account.clone(), 0, 3, std::future::ready(Err(Error::CalendarConsent)));
        let stale = worker.calendars.join_next_with_id().await.unwrap();
        worker.cancel_calendar(&account.id);
        worker.calendar_finished(stale);
        assert!(oauth::calendar_allowed(&worker.store.account_scopes(&account.id).unwrap()));
        assert!(out.try_recv().is_err());
        // Removal must also ignore a result that was already ready, without accessing credentials.
        worker.start_calendar(account.clone(), 0, 3, std::future::ready(Ok(vec![])));
        let stale = worker.calendars.join_next_with_id().await.unwrap();
        worker.store.delete_account(&account.id).unwrap();
        worker.remove(&account.id);
        worker.calendar_finished(stale);
        assert!(worker.store.accounts().unwrap().is_empty());
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn stalled_calendar_page_does_not_block_mail_commands() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = std::env::temp_dir().join(format!("lagom-calendar-stall-{}.db", std::process::id()));
        let store = Store::open(&path).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let message = store.save_message(&lagom_core::store::Message { summary: lagom_core::store::MessageSummary {
            account: account.id.clone(), folder, uid: 1, ..Default::default()
        }, ..Default::default() }).unwrap();
        let (me, rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store, sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::from([account.id.clone()]), discovery: None, me: me.clone(), events };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (stalled, waiting) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") { request.push(stream.read_u8().await.unwrap()); }
            let body = r#"{"items":[],"nextPageToken":"slow-page"}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            drop(stream);
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") { request.push(stream.read_u8().await.unwrap()); }
            assert!(String::from_utf8(request).unwrap().contains("pageToken=slow-page"));
            stalled.send(()).unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0, "dropping the worker cancels the request");
        });
        let working = async {
            worker.start_calendar(account, 1, 2, async move {
                lagom_core::google::fetch_with(&url, 1, 2, |_| std::future::ready(Ok(Zeroizing::new("fixture".into())))).await
            });
            work(worker, rx).await;
        };
        let commands = async {
            waiting.await.unwrap();
            me.send(Cmd::CancelSignIn).unwrap();
            me.send(Cmd::Op { op: 41, id: message, act: Act::Snooze(Some(123)) }).unwrap();
            me.send(Cmd::Op { op: 42, id: -1, act: Act::Archive }).unwrap();
            me.send(Cmd::Download { id: -1, section: "1".into() }).unwrap();
            me.send(Cmd::Send("missing".into())).unwrap();
            me.send(Cmd::MarkSeen(-1)).unwrap();
            let mut completed = [false; 5];
            while completed.contains(&false) {
                match out.recv().await.unwrap() {
                    Event::SignInCancelled => completed[0] = true,
                    Event::Op { op: 41, error: None } => completed[1] = true,
                    Event::Op { op: 42, error: Some(_) } => completed[2] = true,
                    Event::Downloaded(Err(_)) => completed[3] = true,
                    Event::Seen(-1) => completed[4] = true,
                    Event::CalendarSync(_, _) => panic!("the server is still stalled"),
                    _ => {}
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! { _ = working => panic!("worker exited"), _ = commands => {} }
        }).await.expect("mail commands complete while the calendar server is stalled");
        tokio::time::timeout(Duration::from_secs(2), server).await.unwrap().unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.message(message).unwrap().summary.snoozed_until, Some(123));
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn folder_command_does_not_create_after_failed_initial_listing() {
        use lagom_core::account::{Security, ServerConfig};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut read = BufReader::new(read);
            write.write_all(b"* OK fixture\r\n").await.unwrap();
            let mut creates = 0;
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap() == 0 { break; }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let response = if command.starts_with("LOGIN ") {
                    format!("{tag} OK login\r\n")
                } else if command == "CAPABILITY" {
                    format!("* CAPABILITY IMAP4rev1 SPECIAL-USE\r\n{tag} OK capability\r\n")
                } else if command == "LIST \"\" \"*\" RETURN (SPECIAL-USE)" {
                    format!("* LIST () \"/\" \"INBOX\"\r\n{tag} NO listing failed\r\n")
                } else if command.starts_with("CREATE ") {
                    creates += 1;
                    format!("{tag} NO unexpected create\r\n")
                } else {
                    panic!("unexpected fixture command: {command}");
                };
                write.write_all(response.as_bytes()).await.unwrap();
            }
            creates
        });
        let path = std::env::temp_dir().join(format!("lagom-unlisted-folder-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let config = ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        worker.store.save_account(&account).unwrap();
        let mut session = ImapSync::login(&account, "fixture").await.unwrap();
        assert!(session.list_folders(&worker.store).await.is_err());
        assert!(worker.store.folders(&account.id, now()).unwrap().is_empty());
        worker.sessions.insert(account.id.clone(), session);
        worker.quick(Cmd::CreateFolder { op: 42, account: account.id, name: "Parent/Child".into() }).await;
        let Some(Event::Op { op: 42, error }) = out.recv().await else { panic!("expected folder result") };
        assert!(matches!(out.recv().await, Some(Event::Changed)));
        drop(worker);
        let creates = tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(creates, 0, "CREATE must wait for a known namespace");
        assert_eq!(error.as_deref(), Some("Sync folders before creating a folder"));
    }

    #[tokio::test]
    async fn folder_command_reports_validation_errors_without_connecting() {
        use lagom_core::account::{Security, ServerConfig};
        let path = std::env::temp_dir().join(format!("lagom-folder-worker-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let server = ServerConfig { host: "127.0.0.1".into(), port: 3143, security: Security::Plain, username: "test@lagom.test".into() };
        let account = Account::new("test@lagom.test".into(), server.clone(), server).unwrap();
        worker.store.save_account(&account).unwrap();
        worker.store.save_folder(&account.id, "INBOX", Some("."), FolderRole::Inbox).unwrap();
        for (name, expected) in [
            ("", "Enter a folder name"),
            ("  ", "Enter a folder name"),
            ("inbox", "A folder with this name already exists"),
            ("Parent.Child", "Folder names cannot contain the hierarchy delimiter"),
        ] {
            worker.quick(Cmd::CreateFolder { op: 42, account: account.id.clone(), name: name.into() }).await;
            let Some(Event::Op { op: 42, error }) = out.recv().await else { panic!("expected folder result") };
            assert_eq!(error.as_deref(), Some(expected), "{name:?}");
            assert!(matches!(out.recv().await, Some(Event::Changed)));
        }
        assert_eq!(worker.store.folders(&account.id, now()).unwrap().len(), 1);
        assert!(worker.sessions.is_empty());
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn reauthentication_progresses_while_another_account_syncs() {
        use lagom_core::account::{Auth, Security, ServerConfig};
        use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, sync::oneshot};
        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
            let account = Account::new("sync@example.test".into(), config.clone(), config).unwrap();
            let (listing, listed) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    socket.read_line(&mut line).await.unwrap();
                    let (tag, command) = line.trim_end().split_once(' ').unwrap();
                    if command.starts_with("LIST ") {
                        listing.send(()).unwrap();
                        released.await.unwrap();
                        socket.get_mut().write_all(format!("{tag} OK no folders\r\n").as_bytes()).await.unwrap();
                        break;
                    }
                    let data = match command {
                        "LOGIN \"fixture\" \"fixture\"" => "",
                        "CAPABILITY" => "* CAPABILITY IMAP4rev1\r\n",
                        _ => panic!("unexpected fixture command: {command}"),
                    };
                    socket.get_mut().write_all(format!("{data}{tag} OK done\r\n").as_bytes()).await.unwrap();
                }
            });
            let path = std::env::temp_dir().join(format!("lagom-reauth-sync-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            let (me, mut rx) = unbounded_channel();
            let (events, mut out) = unbounded_channel();
            let session = ImapSync::login(&account, "fixture").await.unwrap();
            let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::from([(account.id.clone(), session)]), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
            worker.store.save_account(&account).unwrap();
            let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
            let mut reauth = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
            reauth.auth = Auth::OAuth(Provider::Google);
            worker.store.save_account(&reauth).unwrap();
            worker.require_reauth(&reauth.id);
            worker.reauth = Some(reauth.id);
            let (callback, code) = oneshot::channel();
            let (complete, completed) = oneshot::channel();
            worker.start_sign_in(async move {
                code.await.unwrap();
                complete.send(()).unwrap();
                Err(Error::Cancelled)
            });
            tokio::join!(worker.handle(Cmd::Sync(Some(account.id.clone())), &mut rx), async {
                listed.await.unwrap();
                callback.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(1), completed).await
                    .expect("OAuth callback must progress before the other account finishes syncing").unwrap();
                release.send(()).unwrap();
            });
            assert!(std::iter::from_fn(|| out.try_recv().ok()).any(|event| matches!(event, Event::Sync(id, SyncState::Synced(_)) if id == account.id)));
            let _ = worker.sign_in.take().unwrap().await;
            drop(worker);
            server.await.unwrap();
            std::fs::remove_file(path).unwrap();
        }).await.expect("fixture completes without keychain or external servers");
    }

    #[tokio::test]
    async fn calendar_consent_and_scope_denial_do_not_pause_mail_or_delete_cached_events() {
        use lagom_core::account::Auth;
        let path = std::env::temp_dir().join(format!("lagom-calendar-{}-{}.db", std::process::id(), now()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let mut account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        account.auth = Auth::OAuth(Provider::Google);
        let scopes = format!("https://mail.google.com/ {}", oauth::GOOGLE_CALENDAR_SCOPES);
        worker.store.save_authorized_account(&account, &scopes).unwrap();
        worker.store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(lagom_core::calendar::Calendar {
            remote_id: "fixture".into(), name: "Fixture".into(), color: None, shown: true, writable: true,
        }, vec![lagom_core::calendar::Event {
            remote_id: "fixture".into(), title: "Fixture".into(), start: "2026-11-09".into(), end: "2026-11-10".into(),
            start_utc: 1, end_utc: 2, all_day: true, location: None, join_url: None, html_link: None,
        }])]).unwrap();
        worker.calendar_synced(&account, Err(Error::Network("offline")));
        assert_eq!(worker.store.account_scopes(&account.id).unwrap(), scopes);
        worker.calendar_synced(&account, Err(Error::CalendarConsent));
        let granted = worker.store.account_scopes(&account.id).unwrap();
        assert!(!oauth::calendar_allowed(&granted));
        assert!(oauth::has_scope(&granted, "https://mail.google.com/"));
        assert!(oauth::has_scope(&granted, "https://www.googleapis.com/auth/tasks"));
        assert_eq!(worker.store.today_events("2026-11-09", 0, 3).unwrap().len(), 1);
        assert!(worker.reauth_required.is_empty());
        while out.try_recv().is_ok() {}
        for error in [Error::Cancelled, Error::Network("consent failed")] {
            worker.quick(Cmd::AllowCalendar(account.id.clone())).await;
            assert!(matches!(out.try_recv().unwrap(), Event::CalendarSync(_, SyncState::SigningIn)));
            worker.sync_calendar(&account);
            assert!(out.try_recv().is_err(), "a tick must not hide the pending sign-in");
            worker.sign_in.take().unwrap().abort();
            worker.signed_in(Err(error));
            assert!(matches!(out.try_recv().unwrap(), Event::CalendarSync(_, _)));
            assert!(worker.reauth_required.is_empty());
            assert_eq!(worker.store.account_scopes(&account.id).unwrap(), granted);
        }
        worker.quick(Cmd::AllowCalendar(account.id.clone())).await;
        worker.sign_in.take().unwrap().abort();
        worker.signed_in(Ok(SignedIn { email: account.email.clone(), display_name: account.display_name.clone(), account: account.clone(), scopes: scopes.clone() }));
        worker.idle.remove(&account.id).unwrap().abort();
        assert_eq!(worker.store.account_scopes(&account.id).unwrap(), scopes);
        assert!(matches!(worker.deferred.pop_front(), Some(Cmd::Sync(Some(id))) if id == account.id));
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn reauthentication_cancellation_failure_and_success_keep_the_same_account() {
        use lagom_core::account::Auth;
        let path = std::env::temp_dir().join(format!("lagom-reauth-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, mut rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let mut account = Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        account.auth = Auth::OAuth(Provider::Google);
        worker.store.save_account(&account).unwrap();
        let folder = worker.store.save_folder(&account.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
        let mail = lagom_core::smtp::Outgoing { to: vec!["recipient@example.com".into()], ..Default::default() };
        let id = worker.store.enqueue(&account, &mail, 0).unwrap();
        worker.store.claim_send(&id, 0).unwrap();
        worker.store.send_failed(&id, &Error::ReauthRequired, 0).unwrap();
        worker.require_reauth(&account.id);
        assert!(matches!(out.try_recv().unwrap(), Event::ReauthRequired(id) if id == account.id));
        worker.sync(None, false, &mut rx).await;
        worker.send(&id).await;
        assert!(out.try_recv().is_err(), "paused accounts must not reach credentials or network");
        worker.quick(Cmd::Reauthenticate(account.id.clone())).await;
        assert!(matches!(out.try_recv().unwrap(), Event::Sync(id, SyncState::SigningIn) if id == account.id));
        worker.quick(Cmd::Reauthenticate(account.id.clone())).await;
        assert!(out.try_recv().is_err(), "double click keeps the same flow");
        worker.require_reauth(&account.id);
        assert!(out.try_recv().is_err(), "an older sync failure must not hide the pending flow");
        worker.quick(Cmd::CancelSignIn).await;
        assert!(worker.sign_in.is_none());
        assert!(worker.reauth.is_none());
        assert!(matches!(out.try_recv().unwrap(), Event::ReauthRequired(_)));
        assert!(worker.deferred.is_empty());
        for error in [Error::Invalid("Sign in with the same email address as this account"), Error::Network("sign-in timed out"), Error::Secret] {
            worker.quick(Cmd::Reauthenticate(account.id.clone())).await;
            out.try_recv().unwrap();
            worker.sign_in.take().unwrap().abort();
            worker.signed_in(Err(error));
            assert!(matches!(out.try_recv().unwrap(), Event::ReauthRequired(_)));
            assert!(matches!(out.try_recv().unwrap(), Event::Reauthenticated { account: id, result: Err(_) } if id == account.id));
            assert!(worker.reauth_required.contains(&account.id));
            assert!(worker.store.due_outbox(&account.id, i64::MAX).unwrap().is_empty());
        }
        worker.quick(Cmd::Reauthenticate(account.id.clone())).await;
        out.try_recv().unwrap();
        worker.sign_in.take().unwrap().abort();
        worker.signed_in(Ok(SignedIn { email: account.email.clone(), display_name: account.display_name.clone(), account: account.clone(), scopes: String::new() }));
        worker.idle.remove(&account.id).unwrap().abort();
        assert!(matches!(out.try_recv().unwrap(), Event::Changed));
        assert!(matches!(out.try_recv().unwrap(), Event::Reauthenticated { account: id, result: Ok(()) } if id == account.id));
        assert!(!worker.reauth_required.contains(&account.id));
        assert!(matches!(worker.deferred.pop_front(), Some(Cmd::Sync(Some(id))) if id == account.id));
        assert_eq!(worker.store.accounts().unwrap(), [account.clone()]);
        assert_eq!(worker.store.folders(&account.id, now()).unwrap()[0].id, folder);
        assert_eq!(worker.store.due_outbox(&account.id, i64::MAX).unwrap(), [id]);
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn discovery_and_sign_in_cancellation_commands() {
        let path = std::env::temp_dir().join(format!("lagom-oauth-worker-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (me, _rx) = unbounded_channel();
        let (events, mut out) = unbounded_channel();
        let mut worker = Worker { store: Store::open(&path).unwrap(), sessions: HashMap::new(), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
        worker.quick(Cmd::Download { id: -1, section: "2".into() }).await;
        assert!(matches!(out.recv().await, Some(Event::Downloaded(Err(_)))));
        worker.quick(Cmd::SignIn(Provider::Google)).await;
        assert!(worker.sign_in.is_some());
        worker.quick(Cmd::CancelSignIn).await;
        assert!(worker.sign_in.is_none());
        assert!(matches!(out.recv().await, Some(Event::SignInCancelled)));
        let (mut callback, code) = tokio::sync::oneshot::channel::<()>();
        worker.start_sign_in(async move {
            code.await.unwrap();
            panic!("cancelled sign-in must not finish");
        });
        tokio::task::yield_now().await;
        worker.quick(Cmd::CancelSignIn).await;
        tokio::time::timeout(Duration::from_secs(1), callback.closed()).await
            .expect("cancellation must drop the pending callback");
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

    #[tokio::test]
    async fn restore_and_empty_trash_commands_reach_the_server() {
        use lagom_core::account::{Security, ServerConfig};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for reject in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
            let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
                let mut deleted = false;
                let mut inbox = false;
                let mut restored = false;
                let mut read_flags = Vec::new();
                loop {
                    let mut line = String::new();
                    if socket.read_line(&mut line).await.unwrap() == 0 { break; }
                    let (tag, command) = line.trim_end().split_once(' ').unwrap();
                    let data = match command {
                        "LOGIN \"fixture\" \"fixture\"" => String::new(),
                        "CAPABILITY" => "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n".into(),
                        "SELECT \"Bin\"" | "SELECT \"INBOX\"" => {
                            inbox = command.contains("INBOX");
                            "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n".into()
                        }
                        "UID STORE 4 -FLAGS.SILENT (\\Deleted)" => String::new(),
                        "UID MOVE 4 \"INBOX\"" => { restored = true; String::new() }
                        "UID STORE 8 +FLAGS.SILENT (\\Seen)" => { read_flags.push(true); String::new() }
                        "UID STORE 8 -FLAGS.SILENT (\\Seen)" => { read_flags.push(false); String::new() }
                        "UID FETCH 8 (UID FLAGS)" => "* 1 FETCH (UID 8 FLAGS ())\r\n".into(),
                        "UID FETCH 5 (UID FLAGS)" => "* 1 FETCH (UID 5 FLAGS ())\r\n".into(),
                        "UID SEARCH ALL" => if inbox { "* SEARCH 8\r\n".into() } else if deleted { "* SEARCH\r\n".into() } else { "* SEARCH 5\r\n".into() },
                        command if command.starts_with("UID FETCH ") => {
                            let uid = if inbox { 8 } else { 5 };
                            assert!(command.starts_with(&format!("UID FETCH {uid} ")));
                            if command.contains("BODYSTRUCTURE") {
                                let header = "Subject: Fixture\r\nContent-Type: text/plain\r\n\r\n";
                                format!("* 1 FETCH (UID {uid} FLAGS () BODYSTRUCTURE (\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 4 1) BODY[HEADER] {{{}}}\r\n{header})\r\n", header.len())
                            } else {
                                assert!(command.contains("BODY.PEEK[TEXT]"));
                                format!("* 1 FETCH (UID {uid} BODY[TEXT] {{4}}\r\nbody)\r\n")
                            }
                        }
                        "UID STORE 5 +FLAGS.SILENT (\\Deleted)" => String::new(),
                        "UID EXPUNGE 5" => { deleted = true; String::new() }
                        _ => panic!("unexpected fixture command: {command}"),
                    };
                    let status = if reject && command.starts_with("UID STORE") { "NO" } else { "OK" };
                    socket.get_mut().write_all(format!("{data}{tag} {status} fixture\r\n").as_bytes()).await.unwrap();
                }
                assert_eq!(deleted, !reject);
                assert_eq!(restored, !reject);
                assert_eq!(read_flags, if reject { vec![] } else { vec![true, false] });
            });
            let path = std::env::temp_dir().join(format!("lagom-empty-trash-{}-{}.db", std::process::id(), account.id));
            let mut store = Store::open(&path).unwrap();
            store.save_account(&account).unwrap();
            let folder = store.save_folder(&account.id, "Bin", None, FolderRole::Trash).unwrap();
            let inbox = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
            store.reset_uidvalidity(folder, 7).unwrap();
            store.reset_uidvalidity(inbox, 7).unwrap();
            let restore = store.save_message(&lagom_core::store::Message { summary: lagom_core::store::MessageSummary {
                account: account.id.clone(), folder, uid: 4, ..Default::default()
            }, ..Default::default() }).unwrap();
            let (me, _rx) = unbounded_channel();
            let (events, mut out) = unbounded_channel();
            let session = ImapSync::login(&account, "fixture").await.unwrap();
            let mut worker = Worker { store, sessions: HashMap::from([(account.id.clone(), session)]), idle: HashMap::new(), moved: HashMap::new(), deferred: VecDeque::new(), sign_in: None, reauth: None, calendar_consent: false, calendars: tokio::task::JoinSet::new(), calendar_tasks: HashMap::new(), event_creates: tokio::task::JoinSet::new(), create_tasks: HashMap::new(), pending_events: Vec::new(), task_sync: None, reauth_required: HashSet::new(), discovery: None, me, events };
            if !reject {
                worker.quick(Cmd::Op { op: 41, id: restore, act: Act::Inbox }).await;
                assert!(matches!(out.recv().await, Some(Event::Op { op: 41, error: None })));
                assert!(matches!(out.recv().await, Some(Event::Changed)));
                assert_eq!(worker.store.known_uids(inbox).unwrap(), [8]);
                assert!(worker.store.message(restore).is_err());
                let id = worker.store.list_messages(inbox, now(), 1, 0).unwrap()[0].id;
                for seen in [true, false] {
                    worker.quick(Cmd::Op { op: 43, id, act: Act::Read(seen) }).await;
                    assert!(matches!(out.recv().await, Some(Event::Op { op: 43, error: None })));
                    assert!(matches!(out.recv().await, Some(Event::Changed)));
                    assert_eq!(worker.store.message(id).unwrap().summary.seen, seen);
                }
            }
            worker.quick(Cmd::EmptyTrash { op: 42, folder }).await;
            let Event::Op { op, error } = out.recv().await.unwrap() else { panic!("missing completion") };
            assert_eq!(op, 42);
            assert_eq!(error.is_some(), reject);
            assert!(matches!(out.recv().await, Some(Event::Changed)));
            assert_eq!(worker.sessions.contains_key(&account.id), !reject);
            drop(worker);
            server.await.unwrap();
            std::fs::remove_file(path).unwrap();
        }
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

#[cfg(test)]
#[path = "sync/notification_tests.rs"]
mod notification_tests;

#[cfg(test)]
#[path = "sync/mail_basics_tests.rs"]
mod mail_basics_tests;

#[cfg(test)]
mod create_tests;

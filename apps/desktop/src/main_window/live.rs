//! The window's side of real mail: store reads, worker events, optimistic moves and their undo.
//! The UI thread only reads the store (bounded, indexed queries) and never waits on the network.

use super::*;

/// What an in-flight worker command needs when it finishes.
enum Op {
    /// A move of this row (id in `Inbox::mail`). On failure the row comes back.
    Move(usize),
    /// Undo of a move: on failure only a message.
    Undo,
    /// The sheet that sent; on failure it becomes the draft again.
    Send(Entity<Compose>),
}

pub struct Live {
    bridge: Bridge,
    store: Rc<Store>,
    pub(super) accounts: Vec<Account>,
    pub(super) current: usize,
    sync: HashMap<String, SyncState>,
    ops: HashMap<u64, Op>,
    next_op: u64,
    /// A store change arrived while moves were in flight; read again once they finish.
    stale: bool,
    /// Rows per list read. Grows when the selection nears the end.
    limit: u32,
    /// Store ids marked read here, until the server confirms.
    seen: HashSet<i64>,
    _events: Task<()>,
    /// Wakes the window when the next message on Later is due.
    due: Option<(i64, Task<()>)>,
}

/// Rows per page of a list.
const PAGE: u32 = 200;

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// The sidebar name of an account: its display name, or its domain.
pub fn short_name(a: &Account) -> String {
    if a.display_name != a.email {
        return a.display_name.clone();
    }
    a.email.rsplit_once('@').map_or(a.email.clone(), |(_, d)| d.to_string())
}

impl Live {
    pub fn start(window: &mut Window, cx: &mut Context<MainWindow>) -> lagom_core::Result<Self> {
        let path = Store::default_path()?;
        let store = Store::open(&path)?;
        let (bridge, mut events) = crate::sync::start(path);
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.recv().await {
                // Several changes in a row read the store once.
                let mut batch = vec![event];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update_in(cx, |this, window, cx| this.on_events(batch, window, cx)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            bridge,
            accounts: store.accounts()?,
            store: Rc::new(store),
            current: 0,
            sync: HashMap::new(),
            ops: HashMap::new(),
            next_op: 0,
            stale: false,
            limit: PAGE,
            seen: HashSet::new(),
            _events: task,
            due: None,
        })
    }

    pub fn account(&self) -> Option<&Account> {
        self.accounts.get(self.current)
    }

    /// Full-text search for ⌘K in the current account: (store id, "Sender · Subject").
    pub fn search(&self) -> Option<crate::command_palette::Search> {
        let (store, account) = (self.store.clone(), self.account()?.id.clone());
        Some(Rc::new(move |q: &str| {
            let Some(q) = mail::fts_query(q) else { return Vec::new() };
            store
                .search(&account, &q, now(), 20, 0)
                .unwrap_or_default()
                .into_iter()
                .map(|m| (m.id, format!("{} · {}", if m.from_name.is_empty() { &m.from_address } else { &m.from_name }, m.subject)))
                .collect()
        }))
    }

    fn op(&mut self, op: Op) -> u64 {
        self.next_op += 1;
        self.ops.insert(self.next_op, op);
        self.next_op
    }
}

impl MainWindow {
    /// Reads the store for the account and list on screen. Waits while moves are in flight, so
    /// a read never shows a moved row back before the server has it.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        if !live.ops.values().all(|op| matches!(op, Op::Send(_))) {
            live.stale = true;
            return;
        }
        live.stale = false;
        live.accounts = live.store.accounts().unwrap_or_default();
        live.current = live.current.min(live.accounts.len().saturating_sub(1));
        let Some(account) = live.account().map(|a| a.id.clone()) else {
            self.inbox = Inbox::live();
            return;
        };
        let view = match self.inbox.open.map(|f| (&self.inbox.folders[f].kind, self.inbox.folders[f].store)) {
            None => mail::View::Inbox,
            Some((Kind::Later, _)) => mail::View::Later,
            Some((_, Some(id))) => mail::View::Folder(id),
            // A folder ⌘K just created: nothing on the server yet.
            Some((_, None)) => mail::View::Inbox,
        };
        match mail::load(&live.store, &account, view, live.limit, now()) {
            Ok(snap) => {
                let due = snap.next_due;
                self.inbox.apply(snap, &live.seen, chrono::Local::now());
                self.wake_at(due, cx);
            }
            Err(e) => self.notify(format!("Could not read mail: {e}"), cx),
        }
        cx.notify();
    }

    /// One timer for the next message on Later, not a polling loop.
    fn wake_at(&mut self, due: Option<i64>, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let Some(at) = due else {
            live.due = None;
            return;
        };
        if live.due.as_ref().is_some_and(|d| d.0 == at) {
            return;
        }
        let wait = Duration::from_secs((at - now()).max(0) as u64 + 1);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                if let Some(live) = &mut this.live {
                    live.due = None;
                }
                this.reload(cx);
            })
            .ok();
        });
        live.due = Some((at, task));
    }

    fn on_events(&mut self, events: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
        let mut read = false;
        for event in events {
            match event {
                Event::Changed => read = true,
                Event::Sync(account, state) => {
                    if let Some(live) = &mut self.live {
                        live.sync.insert(account, state);
                    }
                }
                Event::Op { op, error } => {
                    read = true;
                    let Some(done) = self.live.as_mut().and_then(|l| l.ops.remove(&op)) else { continue };
                    match (done, error) {
                        (_, None) => {}
                        (Op::Move(id), Some(e)) => {
                            if let Some((what, id, ix)) = self.inbox.revert(id) {
                                self.come_back(what, id, ix, false, cx);
                            }
                            self.notify(format!("Not moved: {e}"), cx);
                        }
                        (_, Some(e)) => self.notify(format!("Could not undo: {e}"), cx),
                    }
                }
                Event::Added(result) => match result {
                    Ok(id) => {
                        self.page = Page::Mail;
                        if let Some(live) = &mut self.live {
                            live.accounts = live.store.accounts().unwrap_or_default();
                            live.current = live.accounts.iter().position(|a| a.id == id).unwrap_or(0);
                        }
                        self.inbox = Inbox::live();
                        self.clear_motion();
                        window.focus(&self.focus);
                        read = true;
                    }
                    Err(e) => {
                        if let Page::AddAccount(add) = &self.page {
                            add.update(cx, |add, cx| add.failed(e, cx));
                        }
                    }
                },
                Event::Sent { op, delivered, error } => {
                    read = true;
                    let compose = match self.live.as_mut().and_then(|l| l.ops.remove(&op)) {
                        Some(Op::Send(compose)) => Some(compose),
                        _ => None,
                    };
                    match (delivered, error) {
                        (true, None) => self.notify("Sent", cx),
                        (true, Some(e)) => self.notify(format!("Sent. {e}"), cx),
                        (false, e) => {
                            // The text is not lost: R or C opens it again.
                            if self.draft.is_none() {
                                self.draft = compose;
                            }
                            self.notify(format!("Not sent: {}. C opens the draft.", e.unwrap_or_default()), cx);
                        }
                    }
                }
            }
        }
        if read || self.live.as_ref().is_some_and(|l| l.stale) {
            self.reload(cx);
        }
        cx.notify();
    }

    /// The worker half of an optimistic move.
    pub(super) fn server(&mut self, what: Move, id: usize, when: LaterWhen) {
        let Some(store) = self.inbox.mail[id].store else { return };
        let act = match what {
            Move::Done => Act::Archive,
            Move::Delete => Act::Trash,
            Move::Later => Act::Snooze(Some(when.at(chrono::Local::now()).timestamp())),
            Move::Folder(f) => match self.inbox.folders[f].store {
                Some(folder) => Act::Move(folder),
                None => Act::NewFolder(self.inbox.folders[f].full.clone()),
            },
        };
        let Some(live) = &mut self.live else { return };
        let op = live.op(Op::Move(id));
        live.bridge.send(Cmd::Op { op, id: store, act });
    }

    /// Undo on the server: the message goes back to the folder it was in, or off Later.
    pub(super) fn unserver(&mut self, what: Move, id: usize) {
        let m = &self.inbox.mail[id];
        let Some(store) = m.store else { return };
        let act = match what {
            Move::Later => Act::Snooze(None),
            Move::Done | Move::Delete | Move::Folder(_) => Act::Move(m.folder),
        };
        let Some(live) = &mut self.live else { return };
        let op = live.op(Op::Undo);
        live.bridge.send(Cmd::Op { op, id: store, act });
    }

    /// Before a frame: the selected message's body comes from the store once, and opening a
    /// message marks it read. One indexed row read, only when the selection changed.
    pub(super) fn open_current(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.inbox.current() else { return };
        let Some(live) = &mut self.live else { return };
        let Some(store) = self.inbox.mail[id].store else { return };
        if self.inbox.mail[id].body.is_empty() {
            self.inbox.mail[id].body = match live.store.message(store) {
                Ok(m) => mail::paragraphs(&m.body_text),
                Err(_) => vec!["This message is no longer in the local store.".into()],
            };
        }
        if self.inbox.mail[id].unread && matches!(self.page, Page::Mail) {
            self.inbox.mail[id].unread = false;
            if let Some(f) = self.inbox.open {
                self.inbox.folders[f].unread = self.inbox.folders[f].unread.saturating_sub(1);
            }
            live.seen.insert(store);
            live.bridge.send(Cmd::MarkSeen(store));
            cx.notify();
        }
    }

    /// Reads the next page when the selection nears the end of what is loaded.
    pub(super) fn near_end(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let len = self.inbox.list().len();
        if len as u32 >= live.limit && self.inbox.selected + 20 >= len {
            live.limit += PAGE;
            self.reload(cx);
        }
    }

    pub(super) fn open_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if let Some(f) = self.inbox.find_kind(kind) {
            self.open_box(Some(f), cx);
        }
    }

    pub(super) fn switch_account(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        if ix >= live.accounts.len() {
            return;
        }
        live.current = ix;
        live.limit = PAGE;
        // Folder indices belong to one account; history does too.
        self.inbox = Inbox::live();
        self.clear_motion();
        self.page = Page::Mail;
        self.reload(cx);
    }

    pub(super) fn sync_now(&mut self) {
        if let Some(live) = &self.live {
            live.bridge.send(Cmd::Sync(None));
        }
    }

    /// "Home · synced 10:12" for the current account, `None` in demo mode.
    pub(super) fn sync_line(&self) -> Option<SharedString> {
        let live = self.live.as_ref()?;
        Some(format!("{} · {}", short_name(live.account()?), self.sync_state()?).into())
    }

    /// "synced 10:12": the list footer has no room for the account name, the sidebar shows it.
    pub(super) fn sync_state(&self) -> Option<SharedString> {
        let live = self.live.as_ref()?;
        let account = live.account()?;
        let state = match live.sync.get(&account.id) {
            None | Some(SyncState::Syncing) => "syncing…".to_string(),
            Some(SyncState::Synced(at)) => {
                let at = chrono::DateTime::from_timestamp(*at, 0).map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string());
                format!("synced {}", at.unwrap_or_default())
            }
            Some(SyncState::Failed(e)) => format!("offline: {e}"),
        };
        Some(state.into())
    }

    /// ⌘K search hit: open its list, select it.
    pub(super) fn open_found(&mut self, store: i64, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let Ok(m) = live.store.message(store) else { return };
        let folder = self.inbox.folders.iter().position(|f| f.store == Some(m.summary.folder) && !f.gone);
        self.open_box(folder, cx);
        if let Some(ix) = self.inbox.list().iter().position(|&id| self.inbox.mail[id].store == Some(store)) {
            self.select(|inbox| inbox.select(ix), cx);
        }
    }

    pub(super) fn send_mail(&mut self, compose: Entity<Compose>, mail: lagom_core::smtp::Outgoing, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let Some(account) = live.account().map(|a| a.id.clone()) else {
            self.notify("Not sent: no account", cx);
            return;
        };
        let op = live.op(Op::Send(compose));
        live.bridge.send(Cmd::Send { op, account, mail: Box::new(mail) });
    }

    pub(super) fn add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.is_none() {
            return self.notify("Demo mode: start without LAGOM_DEMO to add an account", cx);
        }
        let can_cancel = self.live.as_ref().is_some_and(|l| !l.accounts.is_empty());
        let add = cx.new(|cx| AddAccount::new(can_cancel, window, cx));
        cx.subscribe(&add, |this, _, Connect(account, password): &Connect, _| {
            if let Some(live) = &this.live {
                live.bridge.send(Cmd::AddAccount(account.clone(), password.clone()));
            }
        })
        .detach();
        cx.subscribe_in(&add, window, |this, _, _: &gpui::DismissEvent, window, cx| {
            this.page = Page::Mail;
            window.focus(&this.focus);
            cx.notify();
        })
        .detach();
        test_hook(&add, window, cx);
        self.page = Page::AddAccount(add);
        cx.notify();
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.page = Page::Settings(None);
        cx.notify();
    }

    pub(crate) fn confirm_remove(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.page = Page::Settings(id);
        cx.notify();
    }

    /// Deletes the keychain secret and every local row of the account. The server keeps its mail.
    pub(crate) fn remove_account(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        live.bridge.send(Cmd::RemoveAccount(id.clone()));
        live.accounts.retain(|a| a.id != id);
        live.current = 0;
        self.inbox = Inbox::live();
        self.clear_motion();
        self.page = Page::Settings(None);
        cx.notify();
    }

    pub(crate) fn settings_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_account(window, cx);
    }
}

/// `LAGOM_TEST_ACCOUNT=email|password|imap host:port|smtp host:port|tls,starttls or plain`
/// fills the form and presses Connect: the real add-account path, for end-to-end runs.
/// Only with `LAGOM_DATA_DIR`, so it never writes into the owner's store.
fn test_hook(add: &Entity<AddAccount>, window: &mut Window, cx: &mut Context<MainWindow>) {
    let (Ok(spec), Some(_)) = (std::env::var("LAGOM_TEST_ACCOUNT"), std::env::var_os("LAGOM_DATA_DIR")) else { return };
    let parts: Vec<&str> = spec.split('|').collect();
    let [email, password, imap, smtp, security] = parts[..] else { return };
    let security = match security {
        "plain" => lagom_core::account::Security::Plain,
        "starttls" => lagom_core::account::Security::StartTls,
        _ => lagom_core::account::Security::Tls,
    };
    let split = |s: &str| s.rsplit_once(':').map(|(h, p)| (h.to_string(), p.to_string())).unwrap_or_default();
    let ((imap_host, imap_port), (smtp_host, smtp_port)) = (split(imap), split(smtp));
    let servers = crate::accounts::Servers { imap_host, imap_port, imap_security: security, smtp_host, smtp_port, smtp_security: security };
    let connect = std::env::var("LAGOM_SCREEN").as_deref() != Ok("add-account-form");
    add.update(cx, |add, cx| {
        add.fill(email, password, &servers, window, cx);
        if connect {
            add.connect(cx);
        }
    });
}

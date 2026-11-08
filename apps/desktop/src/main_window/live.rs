//! The window's side of real mail: store reads, worker events, optimistic moves and their undo.
//! The UI reads mail and commits compose edits locally; it never waits on the network.

use super::*;

/// What an in-flight worker command needs when it finishes.
enum Op {
    /// A move of this row (id in `Inbox::mail`). On failure the row comes back.
    Move(usize),
    /// Undo of a move: on failure only a message.
    Undo,
}

pub struct Live {
    bridge: Bridge,
    pub(super) store: Rc<Store>,
    pub(super) accounts: Vec<Account>,
    pub(super) current: usize,
    sync: HashMap<String, SyncState>,
    ops: HashMap<u64, Op>,
    next_op: u64,
    /// A store change arrived while moves were in flight; read again once they finish.
    stale: bool,
    /// Rows per list read. Grows when the selection nears the end.
    limit: u32,
    /// The ⌘K hit on screen: loaded even past `limit`, until another list opens.
    pub(super) pin: Option<i64>,
    /// The ⌘K hit to select once the next store read runs (it waits while moves are in flight).
    want: Option<i64>,
    /// Store ids marked read here, until the server confirms.
    seen: HashSet<i64>,
    pub(super) failed: Vec<lagom_core::outbox::Pending>,
    _events: Task<()>,
    /// Wakes the window when the next message on Later is due.
    due: Option<(i64, Task<()>)>,
}

/// Rows per page of a list.
const PAGE: u32 = 200;

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// The toast after the worker tried to send.
pub(super) fn sent_note(delivery: Delivery, error: Option<String>) -> String {
    match (delivery, error) {
        (Delivery::Sent, None) => "Sent".into(),
        (Delivery::Sent, Some(e)) => format!("Sent. {e}. The Sent copy will retry with backoff."),
        (Delivery::Maybe, _) => "Delivery uncertain. Kept in outbox; choose Send again or Edit draft.".into(),
        (Delivery::NotSent, e) => format!("Kept in outbox: {}. See send status for retry or editing.", e.unwrap_or_default()),
    }
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
                    return;
                }
            }
            // The worker stopped: nothing will answer the commands still pending.
            this.update(cx, |this, cx| this.worker_gone(cx)).ok();
        });
        Ok(Self {
            bridge,
            accounts: store.accounts()?,
            failed: store.failed_outbox()?,
            store: Rc::new(store),
            current: 0,
            sync: HashMap::new(),
            ops: HashMap::new(),
            next_op: 0,
            stale: false,
            limit: PAGE,
            pin: None,
            want: None,
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
        let Ok(accounts) = live.store.accounts() else { return };
        live.accounts = accounts;
        live.failed = live.store.failed_outbox().unwrap_or_default();
        self.drafts.retain(|c| c.read(cx).account_id().is_none_or(|id| live.accounts.iter().any(|a| a.id == id)));
        self.sending.retain(|c| c.read(cx).account_id().is_none_or(|id| live.accounts.iter().any(|a| a.id == id)));
        if self.compose.as_ref().is_some_and(|c| c.read(cx).account_id().is_some_and(|id| !live.accounts.iter().any(|a| a.id == id))) {
            self.compose = None;
        }
        if !live.ops.is_empty() {
            live.stale = true;
            return;
        }
        live.stale = false;
        live.current = live.current.min(live.accounts.len().saturating_sub(1));
        let Some(account) = live.account().map(|a| a.id.clone()) else {
            self.inbox = Inbox::live();
            self.close_popouts(None, cx);
            return;
        };
        let view = match self.inbox.open.map(|f| (&self.inbox.folders[f].kind, self.inbox.folders[f].store)) {
            None => mail::View::Inbox,
            Some((Kind::Later, _)) => mail::View::Later,
            Some((_, Some(id))) => mail::View::Folder(id),
            // A folder ⌘K just created: nothing on the server yet.
            Some((_, None)) => mail::View::Inbox,
        };
        match mail::load(&live.store, &account, view, live.limit, now(), live.pin) {
            Ok(snap) => {
                let due = snap.next_due;
                self.inbox.apply(snap, &live.seen, chrono::Local::now());
                let want = live.want.take();
                self.wake_at(due, cx);
                if let Some(want) = want {
                    self.select_found(want, cx);
                }
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
                Event::OpenUrl(url) => cx.open_url(&url),
                Event::Downloaded(result) => match result {
                    Ok(path) => cx.reveal_path(&path),
                    Err(error) => self.notify(format!("Could not download attachment: {error}"), cx),
                },
                Event::Discovered { .. } | Event::SignInCancelled => {},
                Event::ReauthRequired(account) => {
                    if let Some(live) = &mut self.live {
                        live.sync.insert(account, SyncState::Failed("Sign in again".into()));
                    }
                }
                Event::Changed => read = true,
                Event::Seen(id) => {
                    if let Some(live) = &mut self.live {
                        live.seen.remove(&id);
                    }
                }
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
                            self.inbox.back(id);
                            if let Some((what, id, ix)) = self.inbox.revert(id) {
                                self.come_back(what, id, ix, false, cx);
                            }
                            self.notify(format!("Not moved: {e}"), cx);
                        }
                        (_, Some(e)) => self.notify(format!("Could not undo: {e}"), cx),
                    }
                }
                Event::Added(result) | Event::SignedIn(result) => match result {
                    Ok(id) => {
                        self.page = Page::Mail;
                        if let Some(live) = &mut self.live {
                            live.accounts = live.store.accounts().unwrap_or_default();
                            live.current = live.accounts.iter().position(|a| a.id == id).unwrap_or(0);
                        }
                        self.inbox = Inbox::live();
                        self.close_popouts(None, cx);
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
                Event::Sent { delivery, error } => {
                    read = true;
                    self.notify(sent_note(delivery, error), cx);
                }
            }
        }
        if read || self.live.as_ref().is_some_and(|l| l.stale) {
            self.reload(cx);
        }
        cx.notify();
    }

    /// Pending sends remain in the outbox when the worker stops.
    pub(super) fn worker_gone(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let ops: Vec<_> = live.ops.drain().map(|(_, op)| op).collect();
        live.stale = false;
        for op in ops {
            match op {
                Op::Move(id) => {
                    self.inbox.back(id);
                    if let Some((what, id, ix)) = self.inbox.revert(id) {
                        self.come_back(what, id, ix, false, cx);
                    }
                }
                Op::Undo => {}
            }
        }
        self.notify("Mail stopped working. Restart Lagom; pending sends are saved in the outbox.", cx);
    }

    /// The worker half of an optimistic move.
    pub(super) fn server(&mut self, what: Move, id: usize, when: LaterWhen, cx: &mut Context<Self>) {
        let Some(store) = self.inbox.mail[id].store else { return };
        let Some(live) = &mut self.live else { return };
        let store_of = |kind| self.inbox.find_kind(kind).and_then(|f| self.inbox.folders[f].store).map(mail::Place::Folder);
        let (act, target) = match what {
            Move::Done => (Act::Archive, store_of(Kind::Done)),
            Move::Delete => (Act::Trash, store_of(Kind::Trash)),
            Move::Later => (Act::Snooze(Some(when.at(chrono::Local::now()).timestamp())), None),
            Move::Folder(f) => match self.inbox.folders[f].store {
                Some(folder) => (Act::Move(folder), Some(mail::Place::Folder(folder))),
                None => {
                    let name = self.inbox.folders[f].full.clone();
                    (Act::NewFolder(name.clone()), Some(mail::Place::New(name)))
                }
            },
        };
        self.inbox.left(id, target);
        let op = live.op(Op::Move(id));
        if !live.bridge.send(Cmd::Op { op, id: store, act }) {
            self.worker_gone(cx);
        }
    }

    /// Undo on the server: the message goes back to the folder it was in, or off Later.
    pub(super) fn unserver(&mut self, what: Move, id: usize, cx: &mut Context<Self>) {
        let Some(store) = self.inbox.mail[id].store else { return };
        if self.live.is_none() {
            return;
        }
        let back = self.inbox.back(id);
        let act = match what {
            Move::Later => Act::Snooze(None),
            Move::Done | Move::Delete | Move::Folder(_) => match back {
                mail::Place::Folder(id) => Act::Move(id),
                // The worker finds it by name: the create ran before this undo.
                mail::Place::New(name) => Act::NewFolder(name),
            },
        };
        let Some(live) = &mut self.live else { return };
        let op = live.op(Op::Undo);
        if !live.bridge.send(Cmd::Op { op, id: store, act }) {
            self.worker_gone(cx);
        }
    }

    pub(super) fn download_attachment(&mut self, id: usize, section: String, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let Some(store) = self.inbox.mail.get(id).and_then(|m| m.store) else { return };
        if !live.bridge.send(Cmd::Download { id: store, section }) {
            self.worker_gone(cx);
        }
    }

    /// Before a frame: the selected message's body comes from the store once, and opening a
    /// message marks it read. One indexed row read, only when the selection changed.
    pub(super) fn open_current(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.inbox.current() else { return };
        let Some(live) = &mut self.live else { return };
        let Some(store) = self.inbox.mail[id].store else { return };
        if self.inbox.mail[id].body.is_empty() {
            self.inbox.mail[id].body = match live.store.message(store).and_then(|m| Ok((m, live.store.attachments(store)?))) {
                Ok((m, parts)) => {
                    self.inbox.mail[id].attachments = parts;
                    self.inbox.mail[id].html = m.body_html.map(Into::into);
                    mail::paragraphs(&m.body_text)
                }
                Err(_) => vec!["This message is no longer in the local store.".into()],
            };
        }
        if self.inbox.mail[id].unread && matches!(self.page, Page::Mail) {
            self.inbox.mail[id].unread = false;
            if let Some(f) = self.inbox.open {
                self.inbox.folders[f].unread = self.inbox.folders[f].unread.saturating_sub(1);
            }
            live.seen.insert(store);
            // A stopped worker shows up through the event channel; nothing waits on this.
            let _ = live.bridge.send(Cmd::MarkSeen(store));
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
        live.pin = None;
        // Folder indices belong to one account; history does too.
        self.inbox = Inbox::live();
        self.close_popouts(None, cx);
        self.clear_motion();
        self.page = Page::Mail;
        self.reload(cx);
    }

    pub(super) fn sync_now(&mut self) {
        if let Some(live) = &self.live {
            let _ = live.bridge.send(Cmd::Sync(None));
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

    /// ⌘K search hit: open its list (Later when snoozed) and select it. The hit is pinned into
    /// that list, so the selection never lands on another row that Delete or Reply would hit.
    pub(super) fn open_found(&mut self, store: i64, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let Ok(m) = live.store.message(store) else { return self.notify("That message is gone", cx) };
        let list = if m.summary.snoozed_until.is_some_and(|t| t > now()) {
            self.inbox.find_kind(Kind::Later)
        } else {
            // INBOX is not in `folders`: no match means the inbox.
            self.inbox.folders.iter().position(|f| f.store == Some(m.summary.folder) && !f.gone)
        };
        self.open_box(list, cx);
        if let Some(live) = &mut self.live {
            (live.pin, live.want) = (Some(store), Some(store));
        }
        self.reload(cx);
    }

    fn select_found(&mut self, store: i64, cx: &mut Context<Self>) {
        match self.inbox.list().iter().position(|&id| self.inbox.mail[id].store == Some(store)) {
            Some(ix) => self.select(|inbox| inbox.select(ix), cx),
            None => self.notify("That message is not in this list any more", cx),
        }
    }

    pub(super) fn send_mail(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        if !live.bridge.send(Cmd::Send(id)) { self.worker_gone(cx); }
    }

    pub(super) fn retry_failed(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        match live.store.retry_send(id, chrono::Local::now().timestamp_millis()) {
            Ok(true) => self.send_mail(id.into(), cx),
            Ok(false) => self.notify("This message is already being sent", cx),
            Err(e) => self.notify(format!("Could not retry: {e}"), cx),
        }
        self.reload(cx);
    }

    pub(super) fn edit_failed(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        match live.store.undo_send(id) {
            Ok(true) => {
                match live.store.drafts() {
                    Ok(drafts) => {
                        if let Some(draft) = drafts.into_iter().find(|draft| draft.id == id) {
                            if let Some(account) = live.accounts.iter().find(|a| a.id == draft.account).cloned() {
                                let store = live.store.clone();
                                let compose = cx.new(|cx| Compose::restore(draft, account, store, window, cx));
                                self.watch_compose(&compose, window, cx);
                                if let Some(other) = self.compose.take() { self.drafts.push(other); }
                                compose.update(cx, |c, cx| c.reopen(window, cx));
                                self.compose = Some(compose);
                            }
                        }
                    }
                    Err(e) => self.notify(format!("Draft restored, but could not open it: {e}"), cx),
                }
            }
            Ok(false) => self.notify("This message is already being sent", cx),
            Err(e) => self.notify(format!("Could not restore draft: {e}"), cx),
        }
        self.reload(cx);
    }

    pub(super) fn restore_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let store = live.store.clone();
        let accounts = live.accounts.clone();
        match store.drafts() {
            Ok(drafts) => for draft in drafts {
                if let Some(account) = accounts.iter().find(|a| a.id == draft.account) {
                    let compose = cx.new(|cx| Compose::restore(draft, account.clone(), store.clone(), window, cx));
                    self.watch_compose(&compose, window, cx);
                    self.drafts.push(compose);
                }
            },
            Err(e) => self.notify(format!("Could not restore drafts: {e}"), cx),
        }
        window.focus(&self.focus);
    }

    pub(super) fn add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.is_none() {
            return self.notify("Demo mode: start without LAGOM_DEMO to add an account", cx);
        }
        let can_cancel = self.live.as_ref().is_some_and(|l| !l.accounts.is_empty());
        let add = cx.new(|cx| AddAccount::new(can_cancel, window, cx));
        cx.subscribe(&add, |this, add, Connect(account, password): &Connect, cx| {
            let sent = this.live.as_ref().is_some_and(|live| live.bridge.send(Cmd::AddAccount(account.clone(), password.clone())));
            if !sent {
                add.update(cx, |add, cx| add.failed("Mail stopped working. Restart Lagom.".into(), cx));
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
        if !live.bridge.send(Cmd::RemoveAccount(id.clone())) {
            return self.worker_gone(cx);
        }
        live.accounts.retain(|a| a.id != id);
        live.current = 0;
        self.inbox = Inbox::live();
        self.close_popouts(None, cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use lagom_core::account::{Security, ServerConfig};

    /// A demo window with a live side whose worker is `rx`: kept alive it never answers,
    /// dropped it has stopped.
    fn window(cx: &mut TestAppContext) -> (Entity<MainWindow>, &mut VisualTestContext, tokio::sync::mpsc::UnboundedReceiver<Cmd>) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::text_input::bind_keys(cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        let (bridge, rx) = crate::sync::Bridge::detached();
        let dir = std::env::temp_dir().join(format!("lagom-live-{}-{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: 3025, security: Security::Plain, username: "me@lagom.test".into() };
        let account = Account::new("me@lagom.test".into(), config.clone(), config).unwrap();
        let store = Rc::new(Store::open(dir.join("t.sqlite3")).unwrap());
        store.save_account(&account).unwrap();
        let live = Live {
            bridge,
            store,
            accounts: vec![account],
            current: 0,
            sync: HashMap::new(),
            ops: HashMap::new(),
            next_op: 0,
            stale: false,
            limit: PAGE,
            pin: None,
            want: None,
            seen: HashSet::new(),
            failed: Vec::new(),
            _events: Task::ready(()),
            due: None,
        };
        view.update(cx, |v, _| v.live = Some(live));
        (view, cx, rx)
    }

    #[gpui::test]
    fn virtual_list_navigation_loads_the_next_page(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, _rx) = window(cx);
        cx.simulate_resize(gpui::size(px(1440.), px(600.)));
        view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let a = &live.accounts[0];
            live.store.save_account(a).unwrap();
            let folder = live.store.save_folder(&a.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
            for uid in 1..=220 {
                let summary = MessageSummary { account: a.id.clone(), folder, uid, subject: format!("Message {uid}"), date: uid as i64, ..Default::default() };
                live.store.save_message(&Message { summary, body_text: "text".into(), body_html: None }).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            assert_eq!(v.inbox.list().len(), PAGE as usize);
            v.select(|inbox| inbox.select(179), cx);
        });
        cx.simulate_keystrokes("down");
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        view.read_with(cx, |v, _| {
            assert_eq!(v.inbox.selected, 180);
            assert_eq!(v.live.as_ref().unwrap().limit, PAGE * 2);
            assert_eq!(v.inbox.list().len(), 220);
            assert!(v.scroll.bounds_for_item(30).is_none());
            let y = row_y(180) + f32::from(v.scroll.offset().y);
            assert!(y >= -1. && y + ROW_H <= f32::from(v.scroll.bounds().size.height) + 1.);
        });
    }

    #[gpui::test]
    fn a_message_window_moves_an_older_hit_from_a_folder(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        // A folder with three messages, of which a read loads only the newest two.
        let store = view.update(cx, |v, _| {
            let live = v.live.as_mut().unwrap();
            live.limit = 2;
            let a = live.accounts[0].clone();
            live.store.save_account(&a).unwrap();
            let f = live.store.save_folder(&a.id, "Projects", Some("/"), FolderRole::Other).unwrap();
            let save = |uid: u32| {
                let summary = MessageSummary { account: a.id.clone(), folder: f, uid, subject: format!("Note {uid}"), date: uid as i64, ..Default::default() };
                live.store.save_message(&Message { summary, body_text: "text".into(), body_html: None }).unwrap()
            };
            let oldest = save(1);
            save(2);
            save(3);
            v.inbox = Inbox::live();
            oldest
        });
        // The oldest is a ⌘K hit, opened in its own window from the pinned folder.
        view.update(cx, |v, cx| {
            v.reload(cx);
            v.open_found(store, cx);
            // Selecting the last row grew the page; a hit further back stays past it.
            v.live.as_mut().unwrap().limit = 2;
        });
        view.update_in(cx, |v, window, cx| v.open_popout(window, cx));
        cx.run_until_parked();
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        // The inbox opens in the main window; the folder's cached rows still hold the hit.
        view.update(cx, |v, cx| v.open_box(None, cx));
        popout.update(cx, |p, _, cx| p.act(Move::Done, cx)).unwrap();
        cx.run_until_parked();
        // Opening it marked it read first; then the move.
        let cmds: Vec<Cmd> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Op { id, act: Act::Archive, .. } if *id == store)), "no archive of the hit");
        assert_eq!(view.read_with(cx, |v, cx| (v.popouts.len(), cx.windows().len())), (0, 1));
    }

    fn send_one(cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("c");
        cx.simulate_input("ana@example.com");
        cx.simulate_keystrokes("cmd-enter");
    }

    fn toast(view: &Entity<MainWindow>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |v, _| v.toast.as_ref().map(|t| t.0.to_string()).unwrap_or_default())
    }

    #[gpui::test]
    fn attachment_click_queues_work_and_failure_shows_toast(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            v.inbox.mail[0].store = Some(42);
            v.download_attachment(0, "2.1".into(), cx);
        });
        assert!(matches!(rx.try_recv(), Ok(Cmd::Download { id: 42, section }) if section == "2.1"));
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Downloaded(Err("fixture failure".into()))], window, cx));
        assert_eq!(toast(&view, cx), "Could not download attachment: fixture failure");
        assert_eq!(attachment_size(0), "0 B");
        assert_eq!(attachment_size(2048), "2 KB");
        assert_eq!(attachment_size(1_572_864), "1.5 MB");
    }

    #[gpui::test]
    fn a_stopped_worker_keeps_the_outbox_and_allows_quit(cx: &mut TestAppContext) {
        let (view, cx, rx) = window(cx);
        drop(rx);
        send_one(cx);
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        view.update(cx, |v, cx| v.worker_gone(cx));
        let count = view.read_with(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.due_outbox(&live.accounts[0].id, i64::MAX).unwrap().len()
        });
        assert_eq!(count, 1);
        assert!(view.read_with(cx, |v, _| v.drafts.is_empty()));
        assert!(toast(&view, cx).contains("outbox"));
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
    }

    #[gpui::test]
    fn quit_does_not_wait_for_an_in_flight_send(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        send_one(cx);
        let id = view.read_with(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            let id = live.store.due_outbox(&live.accounts[0].id, i64::MAX).unwrap().remove(0);
            assert!(live.store.claim_send(&id, i64::MAX).unwrap());
            id
        });
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.pending(&id).is_ok()));
    }

    #[gpui::test]
    fn a_failed_local_write_keeps_the_unsaved_draft(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Keep this text");
        view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.delete_account(&live.accounts[0].id).unwrap();
        });
        assert!(view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(toast(&view, cx).contains("Could not save draft"));
        assert_eq!(view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx)), "Keep this text");
        view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.save_account(&live.accounts[0]).unwrap();
        });
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), "Keep this text");
    }

    #[gpui::test]
    fn startup_restores_saved_edits_and_reply_headers(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, _| {
            let id = v.inbox.current().unwrap();
            v.inbox.mail[id].message_id = "<parent@example.com>".into();
            v.inbox.mail[id].thread_key = "<root@example.com>".into();
        });
        cx.simulate_keystrokes("r");
        cx.simulate_input("Draft **body**");
        let original = view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).outgoing(cx));
        assert_eq!(original.in_reply_to.as_deref(), Some("<parent@example.com>"));
        assert!(original.references.contains(&"<root@example.com>".into()));
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        let stored = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap());
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].text, "Draft **body**");
        view.update_in(cx, |v, window, cx| {
            v.compose = None;
            v.drafts.clear();
            v.restore_drafts(window, cx);
        });
        cx.simulate_keystrokes("c");
        assert_eq!(view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).outgoing(cx)), original);
        cx.simulate_keystrokes("cmd-backspace");
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap().is_empty()));
    }

    #[gpui::test]
    fn autosave_waits_for_a_pause_and_keeps_the_latest_edit(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("first");
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), "");
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.simulate_input(" latest");
        cx.executor().advance_clock(Duration::from_millis(200));
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), "");
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), "first latest");
    }

    #[gpui::test]
    fn failed_send_actions_retry_or_restore_for_editing(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        send_one(cx);
        let id = view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            let id = live.store.due_outbox(&live.accounts[0].id, i64::MAX).unwrap().remove(0);
            assert!(live.store.claim_send(&id, i64::MAX).unwrap());
            live.store.send_failed(&id, &lagom_core::Error::MaybeSent, 0).unwrap();
            id
        });
        view.update(cx, |v, cx| v.reload(cx));
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().failed[0].state.clone()), "uncertain");
        view.update(cx, |v, cx| v.retry_failed(&id, cx));
        assert!(matches!(rx.try_recv(), Ok(Cmd::Send(sent)) if sent == id));
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_ref().unwrap();
            assert!(live.store.claim_send(&id, i64::MAX).unwrap());
            live.store.send_failed(&id, &lagom_core::Error::Rejected, 0).unwrap();
            v.edit_failed(&id, window, cx);
        });
        assert!(view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).is_modal()));
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.pending(&id).is_err()));
        cx.simulate_keystrokes("cmd-backspace");
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap().is_empty()));
    }

    #[gpui::test]
    fn removing_an_account_releases_retained_drafts(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Saved draft");
        cx.simulate_keystrokes("escape");
        assert_eq!(view.read_with(cx, |v, _| v.drafts.len()), 1);
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_ref().unwrap();
            live.store.delete_account(&live.accounts[0].id).unwrap();
            v.on_events(vec![Event::Changed], window, cx);
        });
        assert!(view.read_with(cx, |v, _| v.drafts.is_empty()));
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
    }

    #[gpui::test]
    fn undo_returns_the_persisted_send_to_drafts(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        send_one(cx);
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap().is_empty()));
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap().len()), 1);
        assert!(view.read_with(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.due_outbox(&live.accounts[0].id, i64::MAX).unwrap().is_empty()
        }));
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
    }
}

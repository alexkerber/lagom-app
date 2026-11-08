//! The window's side of real mail: store reads, worker events, optimistic moves and their undo.
//! The UI reads mail and commits compose edits locally; it never waits on the network.

use super::*;

/// What an in-flight worker command needs when it finishes.
enum Op {
    /// A move of this row (id in `Inbox::mail`). On failure the row comes back.
    Move(usize),
    /// Undo of a move: on failure only a message.
    Undo,
    EmptyTrash(String),
    CreateFolder { account: String, name: String },
}

pub struct Live {
    bridge: Bridge,
    pub(super) store: Rc<Store>,
    pub(super) accounts: Vec<Account>,
    pub(super) current: usize,
    sync: HashMap<String, SyncState>,
    ops: HashMap<u64, Op>,
    next_op: Rc<std::cell::Cell<u64>>,
    pending: Rc<std::cell::RefCell<HashMap<u64, Option<String>>>>,
    /// The current list still needs a successful store read.
    stale: bool,
    /// Rows per list read. Grows when the selection nears the end.
    limit: u32,
    /// The ⌘K hit on screen: loaded even past `limit`, until another list opens.
    pub(super) pin: Option<i64>,
    /// The ⌘K hit to select once the next store read runs (it waits while moves are in flight).
    want: Option<i64>,
    notification_target: Option<crate::notifications::Target>,
    /// Store ids marked read here, until the server confirms.
    seen: HashSet<i64>,
    pub(super) failed: Vec<lagom_core::outbox::Pending>,
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

// Closing a window must not recover or replace an in-flight mail worker.
struct Mail {
    store: Rc<Store>,
    bridge: Bridge,
    next_op: Rc<std::cell::Cell<u64>>,
    // Destructive actions must wait for commands issued by a window that has since closed.
    // Keep the Empty Trash account so its guard and cleanup survive reopening too.
    pending: Rc<std::cell::RefCell<HashMap<u64, Option<String>>>>,
    _events: Task<()>,
}

impl gpui::Global for Mail {}

impl Mail {
    fn start(store: Store, cx: &mut App) {
        let (bridge, mut events) = crate::sync::start(store.path().to_path_buf());
        let task = cx.spawn(async move |cx| {
            while let Some(event) = events.recv().await {
                let mut batch = vec![event];
                while let Ok(more) = events.try_recv() { batch.push(more); }
                if cx.update(|cx| Self::dispatch(batch, cx)).is_err() { return; }
            }
            cx.update(|cx| {
                if let Some(handle) = cx.windows().into_iter().find_map(|w| w.downcast::<MainWindow>()) {
                    handle.update(cx, |this, _, cx| this.worker_gone(cx)).ok();
                }
            }).ok();
        });
        cx.set_global(Self { store: Rc::new(store), bridge, next_op: Rc::new(std::cell::Cell::new(0)), pending: Default::default(), _events: task });
    }

    fn dispatch(events: Vec<Event>, cx: &mut App) {
        if let Some(handle) = cx.windows().into_iter().find_map(|w| w.downcast::<MainWindow>()) {
            handle.update(cx, |this, window, cx| this.on_events(events, window, cx)).ok();
        } else {
            let messages = events.into_iter().filter_map(|event| match event {
                Event::NewMail(messages) => Some(messages),
                Event::Op { op, .. } => {
                    cx.global::<Self>().pending.borrow_mut().remove(&op);
                    None
                }
                _ => None,
            }).flatten().collect();
            notify_without_window(messages, cx);
        }
    }
}

pub(crate) fn notify_without_window(messages: Vec<lagom_core::store::MessageSummary>, cx: &mut App) {
    if messages.is_empty() { return; }
    let Some(mail) = cx.try_global::<Mail>() else { return };
    let store = mail.store.clone();
    match Panes::load(&store.path().with_file_name("window.json"), 1440.) {
        Ok(panes) if panes.notifications => show_mail(&store, messages, cx),
        Ok(_) => {},
        Err(error) => eprintln!("Could not read notification preferences: {error}"),
    }
}

fn show_mail(store: &Store, messages: Vec<lagom_core::store::MessageSummary>, cx: &mut App) {
    // Permission prompts and queued events can outlive a read, snooze or move.
    let mut ids = HashSet::new();
    let messages = messages.into_iter().filter_map(|m| {
        if !ids.insert(m.id) { return None; }
        let current = store.message_summary(m.id).ok()?;
        let folder = store.folder(current.folder).ok()?;
        (!current.seen && current.snoozed_until.is_none() && folder.role == lagom_core::store::FolderRole::Inbox)
            .then_some(current)
    }).collect();
    cx.global_mut::<crate::notifications::State>().service.show(messages);
}

impl Live {
    pub fn start(cx: &mut Context<MainWindow>) -> lagom_core::Result<Self> {
        if !cx.has_global::<Mail>() {
            Mail::start(Store::open_default()?, cx);
        }
        let mail = cx.global::<Mail>();
        let store = mail.store.clone();
        Ok(Self {
            bridge: mail.bridge.clone(),
            accounts: store.accounts()?,
            failed: store.failed_outbox()?,
            store,
            current: 0,
            sync: HashMap::new(),
            ops: HashMap::new(),
            next_op: mail.next_op.clone(),
            pending: mail.pending.clone(),
            stale: false,
            limit: PAGE,
            pin: None,
            want: None,
            notification_target: None,
            seen: HashSet::new(),
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
        let id = self.next_op.get() + 1;
        self.next_op.set(id);
        let account = match &op { Op::EmptyTrash(account) => Some(account.clone()), _ => None };
        self.pending.borrow_mut().insert(id, account);
        self.ops.insert(id, op);
        id
    }
}

impl MainWindow {
    pub(super) fn validate_folder_name(&self, name: &str) -> Result<(), String> {
        let local = self.inbox.folders.iter().filter(|f| !f.gone).map(|f| f.full.as_str());
        if let Some(live) = &self.live {
            let account = live.account().ok_or("Add an account before creating folders")?;
            let server = live.store.folders(&account.id, now()).map_err(|e| e.to_string())?;
            if server.is_empty() {
                return Err("Sync folders before creating a folder".into());
            }
            folders::validate_name(name, server.iter().filter_map(|f| f.delimiter.as_deref()), server.iter().map(|f| f.name.as_str()).chain(local))
        } else {
            folders::validate_name(name, std::iter::once("/"), local)
        }.map_err(str::to_string)
    }

    pub(super) fn create_folder(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.validate_folder_name(name)?;
        self.toast = None;
        self.inbox.create_folder(name);
        self.all_folders = true;
        self.folder_scroll.scroll_to_bottom();
        if let Some(live) = &mut self.live {
            let account = live.account().unwrap().id.clone();
            let op = live.op(Op::CreateFolder { account: account.clone(), name: name.into() });
            if !live.bridge.send(Cmd::CreateFolder { op, account, name: name.into() }) {
                self.worker_gone(cx);
            }
        }
        cx.notify();
        Ok(())
    }

    fn folder_failed(&mut self, account: &str, name: &str) {
        if self.live.as_ref().and_then(Live::account).is_some_and(|a| a.id == account) {
            self.inbox.remove_pending_folder(name);
        }
    }

    /// Reads the store for the account and list on screen. Waits while moves are in flight, so
    /// a read never shows a moved row back before the server has it.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        live.stale = true;
        let Ok(accounts) = live.store.accounts() else {
            self.confirm_empty_trash = false;
            return;
        };
        live.accounts = accounts;
        live.failed = live.store.failed_outbox().unwrap_or_default();
        self.drafts.retain(|c| c.read(cx).account_id().is_none_or(|id| live.accounts.iter().any(|a| a.id == id)));
        self.sending.retain(|c| c.read(cx).account_id().is_none_or(|id| live.accounts.iter().any(|a| a.id == id)));
        if self.compose.as_ref().is_some_and(|c| c.read(cx).account_id().is_some_and(|id| !live.accounts.iter().any(|a| a.id == id))) {
            self.compose = None;
        }
        if !live.ops.is_empty() {
            self.confirm_empty_trash = false;
            return;
        }
        live.current = live.current.min(live.accounts.len().saturating_sub(1));
        let Some(account) = live.account().map(|a| a.id.clone()) else {
            self.inbox = Inbox::live();
            self.close_popouts(None, cx);
            return;
        };
        let view = match self.inbox.open.map(|f| (&self.inbox.folders[f].kind, self.inbox.folders[f].store)) {
            None => mail::View::Inbox,
            Some((Kind::Later, _)) => mail::View::Later,
            Some((Kind::Trash, None)) => mail::View::Trash,
            Some((_, Some(id))) => mail::View::Folder(id),
            // A folder ⌘K just created: nothing on the server yet.
            Some((_, None)) => mail::View::Inbox,
        };
        match mail::load(&live.store, &account, view, live.limit, now(), live.pin) {
            Ok(snap) => {
                live.stale = false;
                let due = snap.next_due;
                self.inbox.apply(snap, &live.seen, chrono::Local::now());
                let want = live.want.take();
                self.wake_at(due, cx);
                if let Some(want) = want {
                    self.select_found(want, cx);
                }
            }
            Err(e) => {
                self.confirm_empty_trash = false;
                self.notify(format!("Could not read mail: {e}"), cx);
            }
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
        let mut new_mail = Vec::new();
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
                Event::NewMail(messages) => new_mail.extend(messages),
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
                    let Some(live) = &mut self.live else { continue };
                    let empty = live.pending.borrow_mut().remove(&op).flatten();
                    let Some(done) = live.ops.remove(&op).or_else(|| empty.map(Op::EmptyTrash)) else { continue };
                    match (done, error) {
                        (Op::EmptyTrash(account), None) => {
                            if self.live.as_ref().and_then(|l| l.account()).is_some_and(|a| a.id == account) {
                                self.inbox.history.clear();
                                self.close_popouts(None, cx);
                                self.clear_motion();
                            }
                            self.notify("Trash emptied", cx);
                        }
                        (Op::EmptyTrash(_), Some(e)) => self.notify(format!("Could not empty Trash: {e}"), cx),
                        (_, None) => {}
                        (Op::CreateFolder { account, name }, Some(e)) => {
                            self.folder_failed(&account, &name);
                            self.notify(format!("Could not create folder: {e}"), cx);
                        }
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
        if let Some(target) = self.live.as_mut().filter(|l| l.ops.is_empty()).and_then(|l| l.notification_target.take()) {
            self.open_notification(target, window, cx);
        }
        self.notify_mail(new_mail, window, cx);
        cx.notify();
    }

    fn notify_mail(&mut self, messages: Vec<lagom_core::store::MessageSummary>, window: &Window, cx: &mut Context<Self>) {
        if !self.panes.notifications || window.is_window_active() { return; }
        let Some(live) = &mut self.live else { return };
        let messages = messages.into_iter().filter(|m| {
            !live.seen.contains(&m.id) && !live.ops.values().any(|op| {
                matches!(op, Op::Move(id) if self.inbox.mail.get(*id).is_some_and(|row| row.store == Some(m.id)))
            })
        }).collect();
        show_mail(&live.store, messages, cx);
    }

    pub(crate) fn on_notification(&mut self, event: crate::notifications::Event, window: &mut Window, cx: &mut Context<Self>) {
        use crate::notifications::Event;
        match event {
            Event::Authorized(allowed) => {
                let pending = cx.global_mut::<crate::notifications::State>().service.authorized(allowed);
                self.notify_mail(pending, window, cx);
            }
            Event::Failed(error) => self.notify(error, cx),
            Event::Clicked(target) => self.open_notification(target, window, cx),
        }
    }

    fn open_notification(&mut self, target: crate::notifications::Target, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        window.activate_window();
        let Some(live) = &mut self.live else { return };
        if !live.ops.is_empty() {
            live.notification_target = Some(target);
            return;
        }
        let Some(account) = live.accounts.iter().position(|a| a.id == target.account) else { return };
        live.current = account;
        live.limit = PAGE;
        live.pin = None;
        live.want = None;
        self.page = Page::Mail;
        self.palette = None;
        // Preserve an unfinished compose sheet as a draft while revealing the clicked mail.
        if let Some(compose) = self.compose.take() {
            self.drafts.push(compose);
        }
        self.close_popouts(None, cx);
        self.inbox = Inbox::live();
        self.release(false, cx);
        self.clear_motion();
        self.reload(cx);
        if let Some(id) = target.message {
            if self.live.as_ref().is_some_and(|l| l.store.message(id).is_ok_and(|m| m.summary.account == target.account)) {
                self.open_found(id, cx);
            }
        }
        window.focus(&self.focus);
        cx.notify();
    }

    /// Pending sends remain in the outbox when the worker stops.
    pub(super) fn worker_gone(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        live.pending.borrow_mut().clear();
        let ops: Vec<_> = live.ops.drain().map(|(_, op)| op).collect();
        live.stale = true;
        for op in ops {
            match op {
                Op::Move(id) => {
                    self.inbox.back(id);
                    if let Some((what, id, ix)) = self.inbox.revert(id) {
                        self.come_back(what, id, ix, false, cx);
                    }
                }
                Op::Undo | Op::EmptyTrash(_) => {}
                Op::CreateFolder { account, name } => self.folder_failed(&account, &name),
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
            Move::Inbox => {
                let inbox = live.account().and_then(|a| live.store.folders(&a.id, now()).ok())
                    .and_then(|fs| fs.into_iter().find(|f| f.role == lagom_core::store::FolderRole::Inbox));
                (Act::Inbox, inbox.map(|f| mail::Place::Folder(f.id)))
            }
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
            Move::Inbox | Move::Done | Move::Delete | Move::Folder(_) => match back {
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
    pub(super) fn open_current(&mut self, mark_read: bool, cx: &mut Context<Self>) {
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
        if mark_read && self.inbox.mail[id].unread && matches!(self.page, Page::Mail) {
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

    pub(super) fn emptying_trash(&self) -> bool {
        self.live.as_ref().is_some_and(|l| l.pending.borrow().values().any(Option::is_some))
    }

    pub(super) fn can_empty_trash(&self) -> bool {
        self.inbox.open_kind() == Some(Kind::Trash)
            && self.live.as_ref().is_none_or(|l| l.ops.is_empty() && l.pending.borrow().is_empty() && !l.stale)
    }

    pub(super) fn empty_trash(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_empty_trash || !self.can_empty_trash() {
            self.confirm_empty_trash = false;
            return;
        }
        self.confirm_empty_trash = false;
        if let Some(live) = &mut self.live {
            let Some(folder) = self.inbox.open.and_then(|f| self.inbox.folders[f].store) else {
                self.notify("No Trash folder on the server yet", cx);
                return;
            };
            let Some(account) = live.account().map(|a| a.id.clone()) else { return };
            let op = live.op(Op::EmptyTrash(account));
            if !live.bridge.send(Cmd::EmptyTrash { op, folder }) { self.worker_gone(cx); }
        } else {
            self.inbox.trash.clear();
            self.inbox.history.clear();
            self.inbox.selected = 0;
            self.close_popouts(None, cx);
            self.clear_motion();
            self.notify("Trash emptied", cx);
        }
        cx.notify();
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
        self.folder_name = None;
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
        self.confirm_empty_trash = false;
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
        let dir = std::env::temp_dir().join(format!("lagom-live-{}-{:?}-{:?}", std::process::id(), std::thread::current().id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: 3025, security: Security::Plain, username: "me@lagom.test".into() };
        let account = Account::new("me@lagom.test".into(), config.clone(), config).unwrap();
        let store = Rc::new(Store::open(dir.join("t.sqlite3")).unwrap());
        store.save_account(&account).unwrap();
        store.save_folder(&account.id, "INBOX", Some("/"), lagom_core::store::FolderRole::Inbox).unwrap();
        cx.cx.update(|cx| cx.set_global(crate::notifications::State {
            service: crate::notifications::Service::new().0, _events: Task::ready(()),
        }));
        view.update(cx, |v, _| v.live = Some(fixture_live(store, bridge)));
        (view, cx, rx)
    }

    fn fixture_live(store: Rc<Store>, bridge: Bridge) -> Live {
        Live {
            bridge,
            store: store.clone(),
            accounts: store.accounts().unwrap(),
            current: 0,
            sync: HashMap::new(),
            ops: HashMap::new(),
            next_op: Rc::new(std::cell::Cell::new(0)),
            pending: Default::default(),
            stale: false,
            limit: PAGE,
            pin: None,
            want: None,
            notification_target: None,
            seen: HashSet::new(),
            failed: Vec::new(),
            due: None,
        }
    }

    fn incoming(v: &MainWindow, uid: u32) -> lagom_core::store::MessageSummary {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let live = v.live.as_ref().unwrap();
        let a = live.account().unwrap();
        let folder = live.store.save_folder(&a.id, "INBOX", None, FolderRole::Inbox).unwrap();
        let summary = MessageSummary { account: a.id.clone(), folder, uid, date: uid as i64, ..Default::default() };
        let id = live.store.save_message(&Message { summary, ..Default::default() }).unwrap();
        live.store.message(id).unwrap().summary
    }

    #[gpui::test]
    fn notifications_respect_focus_setting_and_changes_while_permission_waits(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (service, mut events) = crate::notifications::Service::new();
        let m = view.update(cx, |v, cx| {
            cx.global_mut::<crate::notifications::State>().service = service;
            incoming(v, 1)
        });
        view.update_in(cx, |_, window, _| window.activate_window());
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            assert!(window.is_window_active());
            v.on_events(vec![Event::NewMail(vec![m.clone()])], window, cx);
        });
        assert!(events.try_recv().is_err());
        cx.deactivate_window();
        view.update_in(cx, |v, window, cx| {
            v.toggle_notifications(cx);
            assert!(!v.panes.notifications);
            v.on_events(vec![Event::NewMail(vec![m.clone()])], window, cx);
        });
        assert!(events.try_recv().is_err());
        view.update_in(cx, |v, window, cx| {
            v.toggle_notifications(cx);
            v.on_events(vec![Event::NewMail(vec![m.clone()])], window, cx);
        });
        assert!(matches!(events.try_recv(), Ok(crate::notifications::Event::Authorized(true))));
        view.update_in(cx, |v, window, cx| {
            v.live.as_ref().unwrap().store.snooze(m.id, Some(i64::MAX)).unwrap();
            v.on_notification(crate::notifications::Event::Authorized(true), window, cx);
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 0);
        });
        // A later new message uses the existing authorization, without another prompt.
        view.update_in(cx, |v, window, cx| {
            let m = incoming(v, 2);
            v.on_events(vec![Event::NewMail(vec![m])], window, cx);
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1);
            v.on_notification(crate::notifications::Event::Failed("fixture notification error"), window, cx);
        });
        assert!(events.try_recv().is_err());
        assert!(toast(&view, cx).contains("fixture notification error"));
    }

    #[gpui::test]
    fn reopening_reuses_worker_and_preserves_in_flight_outbox(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lagom-reopen-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        cx.update(|cx| {
            bind_keys(cx);
            crate::text_input::bind_keys(cx);
            Mail::start(Store::open(dir.join("mail.sqlite3")).unwrap(), cx);
            crate::notifications::init(cx, |cx| crate::open_main_window(false, cx));
        });
        let first = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        let (bridge, old_op, weak) = first.update(cx, |view, _, cx| {
            let live = view.live.as_mut().unwrap();
            (live.bridge.clone(), live.op(Op::Undo), cx.entity().downgrade())
        }).unwrap();
        first.update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        let reopened = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        let store = reopened.update(cx, |view, _, _| {
            let live = view.live.as_mut().unwrap();
            assert!(bridge.same_worker(&live.bridge), "Dock reopen must reuse the running worker");
            assert!(live.op(Op::Undo) > old_op, "old completions must not match new operations");
            live.ops.clear();
            live.store.clone()
        }).unwrap();

        // Pause the real worker before adding fixtures so it cannot contact a server or keychain.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        assert!(bridge.send(Cmd::Pause(ready_tx, resume_rx)));
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: 3025, security: Security::Plain, username: "sender@example.com".into() };
        let account = Account::new("sender@example.com".into(), config.clone(), config).unwrap();
        store.save_account(&account).unwrap();
        let draft = lagom_core::outbox::Draft {
            id: "<in-flight@example.com>".into(), account: account.id.clone(),
            to: "recipient@example.com".into(), text: "Original mail".into(), ..Default::default()
        };
        store.save_draft(&draft).unwrap();
        let mail = lagom_core::smtp::Outgoing {
            to: vec![draft.to.clone()], text: draft.text.clone(), message_id: Some(draft.id.clone()), ..Default::default()
        };
        let id = store.enqueue(&account, &mail, 0).unwrap();
        assert!(store.claim_send(&id, i64::MAX).unwrap());
        reopened.update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        let sender = cx.update(|cx| cx.global::<crate::notifications::State>().service.event_sender());
        sender.send(crate::notifications::Event::Clicked(crate::notifications::Target { account: account.id.clone(), message: None })).unwrap();
        cx.run_until_parked();
        let handle = cx.update(|cx| {
            assert_eq!(cx.windows().len(), 1);
            cx.windows()[0].downcast::<MainWindow>().unwrap()
        });
        let op = handle.update(cx, |view, window, cx| {
            let live = view.live.as_ref().unwrap();
            assert!(bridge.same_worker(&live.bridge), "notification reopen must reuse the worker");
            assert!(Rc::ptr_eq(&store, &live.store));
            assert_eq!(store.pending(&id).unwrap().state, "ready", "reopen must not recover an active send");
            view.edit_failed(&id, window, cx);
            assert!(view.compose.is_none());
            assert_eq!(store.pending(&id).unwrap().mail.text, "Original mail");
            assert!(store.drafts().unwrap().is_empty());
            assert!(!store.retry_send(&id, i64::MAX).unwrap());
            assert!(!store.claim_send(&id, i64::MAX).unwrap());
            view.live.as_mut().unwrap().op(Op::Undo)
        }).unwrap();
        assert!(bridge.send(Cmd::Op { op, id: -1, act: Act::Snooze(None) }));
        store.delete_account(&account.id).unwrap();
        resume_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if handle.read_with(cx, |view, _| view.live.as_ref().unwrap().ops.is_empty()).unwrap() { break; }
            assert!(Instant::now() < deadline, "reopened window must receive worker completions");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[gpui::test]
    fn reopened_trash_waits_for_commands_from_the_closed_window(cx: &mut TestAppContext) {
        use lagom_core::store::FolderRole;
        let (view, cx, mut rx) = window(cx);
        let (store, bridge, next_op) = view.update(cx, |v, _| {
            let message = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            let trash = live.store.save_folder(&message.account, "Bin", Some("/"), FolderRole::Trash).unwrap();
            let mut message = live.store.message(message.id).unwrap();
            message.summary.id = 0;
            message.summary.folder = trash;
            live.store.save_message(&message).unwrap();
            (live.store.clone(), live.bridge.clone(), live.next_op.clone())
        });
        cx.cx.update(|cx| cx.set_global(Mail { store, bridge: bridge.clone(), next_op, pending: Default::default(), _events: Task::ready(()) }));
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        let mut handle = cx.cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        for action in 0..4 {
            handle.update(cx, |v, _, cx| {
                v.open_kind(Kind::Trash, cx);
                assert!(v.can_empty_trash());
                match action {
                    0 => v.create_folder("Pending", cx).unwrap(),
                    1 => v.act(Move::Inbox, cx),
                    _ => { v.confirm_empty_trash = true; v.empty_trash(cx); }
                }
            }).unwrap();
            let op = match rx.try_recv().unwrap() {
                Cmd::CreateFolder { op, .. } | Cmd::Op { op, act: Act::Inbox, .. } | Cmd::EmptyTrash { op, .. } => op,
                _ => panic!("expected folder, restore or empty command"),
            };
            let weak = handle.update(cx, |_, window, cx| {
                window.remove_window();
                cx.entity().downgrade()
            }).unwrap();
            cx.run_until_parked();
            assert!(weak.upgrade().is_none());
            handle = cx.cx.update(|cx| crate::open_main_window(false, cx).unwrap());
            handle.update(cx, |v, _, cx| {
                assert!(bridge.same_worker(&v.live.as_ref().unwrap().bridge));
                v.open_kind(Kind::Trash, cx);
                assert!(!v.can_empty_trash(), "reopen must retain pending command guards");
                v.confirm_empty_trash = true;
                v.empty_trash(cx);
                assert!(!v.confirm_empty_trash);
                if action >= 2 {
                    v.act(Move::Inbox, cx);
                    assert_eq!(v.inbox.list().len(), 1, "Restore must stay blocked while the old empty command is pending");
                }
            }).unwrap();
            assert!(rx.try_recv().is_err(), "no empty command while the old command is pending");
            if action == 0 || action == 3 {
                handle.update(cx, |_, window, _| window.remove_window()).unwrap();
                cx.run_until_parked();
            }
            cx.cx.update(|cx| Mail::dispatch(vec![Event::Op { op, error: Some("fixture refused".into()) }, Event::Changed], cx));
            if action == 0 || action == 3 {
                handle = cx.cx.update(|cx| crate::open_main_window(false, cx).unwrap());
            }
            handle.update(cx, |v, _, cx| {
                v.open_kind(Kind::Trash, cx);
                assert!(v.can_empty_trash(), "completion must release the guard even without a window");
                assert!(!v.emptying_trash());
                if action == 2 {
                    assert!(v.toast.as_ref().unwrap().0.contains("Could not empty Trash"));
                }
                assert!(v.create_folder("Parent/Child", cx).is_err(), "namespace validation survives reopen");
            }).unwrap();
            assert!(rx.try_recv().is_err());
        }
    }

    #[gpui::test]
    fn reopened_empty_trash_completion_closes_new_popouts(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        let (store, bridge, next_op, pending, message) = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let folder = live.store.save_folder(account, "Trash", None, FolderRole::Trash).unwrap();
            let message = live.store.save_message(&Message { summary: MessageSummary {
                account: account.clone(), folder, uid: 1, seen: true, ..Default::default()
            }, ..Default::default() }).unwrap();
            v.inbox = Inbox::live();
            v.open_kind(Kind::Trash, cx);
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            let live = v.live.as_ref().unwrap();
            (live.store.clone(), live.bridge.clone(), live.next_op.clone(), live.pending.clone(), message)
        });
        let Cmd::EmptyTrash { op, .. } = rx.try_recv().unwrap() else { panic!("missing empty") };
        cx.cx.update(|cx| cx.set_global(Mail { store: store.clone(), bridge, next_op, pending, _events: Task::ready(()) }));
        let weak = view.downgrade();
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        let handle = cx.cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        let popout = handle.update(cx, |v, window, cx| {
            v.open_kind(Kind::Trash, cx);
            v.open_popout(window, cx);
            v.popouts[0].1
        }).unwrap();
        assert!(popout.read_with(cx, |_, _| ()).is_ok());
        popout.update(cx, |v, _, cx| v.act(Move::Inbox, cx)).unwrap();
        assert!(rx.try_recv().is_err(), "popout Restore must wait for the old empty command");
        store.delete_message(message).unwrap();
        cx.cx.update(|cx| Mail::dispatch(vec![Event::Op { op, error: None }], cx));
        cx.run_until_parked();
        assert!(popout.read_with(cx, |_, _| ()).is_err(), "completion must close popouts opened after reopening");
        handle.read_with(cx, |v, _| {
            assert!(v.inbox.list().is_empty());
            assert!(v.popouts.is_empty());
            assert!(v.can_empty_trash());
            assert_eq!(v.toast.as_ref().unwrap().0, "Trash emptied");
        }).unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[gpui::test]
    fn new_mail_posts_after_main_window_closes(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (message, store, bridge) = view.update(cx, |v, _| {
            let message = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            (message, live.store.clone(), live.bridge.clone())
        });
        cx.cx.update(|cx| {
            cx.set_global(Mail { store: store.clone(), bridge, next_op: Rc::new(std::cell::Cell::new(0)), pending: Default::default(), _events: Task::ready(()) });
            crate::notifications::init(cx, |cx| crate::open_main_window(false, cx));
        });
        let weak = view.downgrade();
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        cx.cx.update(|cx| Mail::dispatch(vec![Event::Changed, Event::NewMail(vec![message.clone()])], cx));
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert!(cx.windows().is_empty(), "new mail and permission must not reopen the window");
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1, "new mail must notify without a main window");
        });
        let sender = cx.cx.update(|cx| cx.global::<crate::notifications::State>().service.event_sender());
        sender.send(crate::notifications::Event::Clicked(crate::notifications::Target {
            account: message.account, message: Some(message.id),
        })).unwrap();
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert_eq!(cx.windows().len(), 1);
            let reopened = cx.windows()[0].downcast::<MainWindow>().unwrap().read(cx).unwrap();
            assert_eq!(reopened.inbox.mail[reopened.inbox.current().unwrap()].store, Some(message.id));
        });
    }

    #[gpui::test]
    fn closed_window_notifications_respect_setting_and_revalidation(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (message, store, bridge) = view.update(cx, |v, _| {
            let message = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            (message, live.store.clone(), live.bridge.clone())
        });
        cx.cx.update(|cx| {
            cx.set_global(Mail { store: store.clone(), bridge, next_op: Rc::new(std::cell::Cell::new(0)), pending: Default::default(), _events: Task::ready(()) });
            crate::notifications::init(cx, |_| panic!("new mail must not reopen a window"));
        });
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        let path = store.path().with_file_name("window.json");
        let mut panes = Panes::new(1440.);
        panes.notifications = false;
        panes.save(&path).unwrap();
        cx.cx.update(|cx| Mail::dispatch(vec![Event::NewMail(vec![message.clone()])], cx));
        cx.run_until_parked();
        cx.cx.update(|cx| assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 0));

        panes.notifications = true;
        panes.save(&path).unwrap();
        cx.cx.update(|cx| Mail::dispatch(vec![Event::NewMail(vec![message.clone()])], cx));
        store.snooze(message.id, Some(i64::MAX)).unwrap();
        cx.run_until_parked();
        cx.cx.update(|cx| assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 0, "permission must revalidate pending mail"));

        store.snooze(message.id, None).unwrap();
        let db = rusqlite::Connection::open(store.path()).unwrap();
        db.execute("UPDATE messages SET body_text=x'ff',body_html=x'ff' WHERE id=?1", [message.id]).unwrap();
        assert!(store.message(message.id).is_err());
        cx.cx.update(|cx| Mail::dispatch(vec![Event::NewMail(vec![message])], cx));
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert!(cx.windows().is_empty());
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1, "authorized delivery must read only metadata");
        });
    }

    #[gpui::test]
    fn notification_click_survives_closed_main_window(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (target, store, bridge) = view.update(cx, |v, _| {
            let m = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            (crate::notifications::Target { account: m.account, message: Some(m.id) }, live.store.clone(), live.bridge.clone())
        });
        cx.cx.update(|cx| crate::notifications::init(cx, move |cx| {
            let handle = crate::open_main_window(true, cx)?;
            handle.update(cx, |v, _, cx| {
                if v.live.is_none() {
                    v.live = Some(fixture_live(store.clone(), bridge.clone()));
                    v.inbox = Inbox::live();
                    v.reload(cx);
                }
            })?;
            Ok(handle)
        }));
        let message = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.message_summary(target.message.unwrap()).unwrap());
        let sender = cx.cx.update(|cx| {
            let service = &mut cx.global_mut::<crate::notifications::State>().service;
            service.authorized(true);
            service.show(vec![message]);
            service.event_sender()
        });
        let weak = view.downgrade();
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();
        assert!(weak.upgrade().is_none(), "closed window must release its view");
        sender.send(crate::notifications::Event::Authorized(true)).unwrap();
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert!(cx.windows().is_empty(), "permission callbacks must not reopen mail");
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1);
        });
        assert!(sender.send(crate::notifications::Event::Clicked(target.clone())).is_ok(), "click listener must survive closing the main window");
        cx.run_until_parked();
        cx.cx.update(|cx| {
            let windows = cx.windows();
            assert_eq!(windows.len(), 1);
            let reopened = windows[0].downcast::<MainWindow>().unwrap();
            let v = reopened.read(cx).unwrap();
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, target.message);
        });
        sender.send(crate::notifications::Event::Clicked(crate::notifications::Target { message: None, ..target })).unwrap();
        cx.run_until_parked();
        cx.cx.update(|cx| {
            let windows = cx.windows();
            assert_eq!(windows.len(), 1, "later clicks must reuse the main window");
            let v = windows[0].downcast::<MainWindow>().unwrap().read(cx).unwrap();
            assert!(v.live.as_ref().unwrap().pin.is_none());
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1);
        });
    }

    #[gpui::test]
    fn notification_revalidation_does_not_read_message_bodies(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.deactivate_window();
        view.update_in(cx, |v, window, cx| {
            let m = incoming(v, 1);
            let live = v.live.as_mut().unwrap();
            let db = rusqlite::Connection::open(live.store.path()).unwrap();
            db.execute("UPDATE messages SET body_text=x'ff',body_html=x'ff' WHERE id=?1", [m.id]).unwrap();
            assert!(live.store.message(m.id).is_err());
            assert_eq!(live.store.message_summary(m.id).unwrap(), m);
            cx.global_mut::<crate::notifications::State>().service.authorized(true);
            v.on_events(vec![Event::NewMail(vec![m])], window, cx);
            assert_eq!(cx.global::<crate::notifications::State>().service.posted_count(), 1);
        });
    }

    #[gpui::test]
    fn inactive_inbox_does_not_mark_new_mail_read(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        cx.deactivate_window();
        let id = view.update(cx, |v, cx| {
            let id = incoming(v, 1).id;
            v.inbox = Inbox::live();
            v.reload(cx);
            id
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.inbox.mail[v.inbox.current().unwrap()].unread));
        assert!(rx.try_recv().is_err());
        view.update_in(cx, |_, window, _| window.activate_window());
        cx.run_until_parked();
        assert!(matches!(rx.try_recv(), Ok(Cmd::MarkSeen(read)) if read == id));
    }

    #[gpui::test]
    fn notification_click_switches_account_pins_old_message_and_summary_opens_inbox(cx: &mut TestAppContext) {
        use crate::notifications::Target;
        let (view, cx, _rx) = window(cx);
        let target = view.update(cx, |v, cx| {
            let live = v.live.as_mut().unwrap();
            let a = live.accounts[0].clone();
            let second = Account::new("other@example.test".into(), a.imap, a.smtp).unwrap();
            live.store.save_account(&second).unwrap();
            live.accounts.push(second.clone());
            live.current = 1;
            let first = incoming(v, 1).id;
            for uid in 2..=210 { incoming(v, uid); }
            v.live.as_mut().unwrap().current = 0;
            v.open_settings(cx);
            Target { account: second.id, message: Some(first) }
        });
        view.update_in(cx, |v, window, cx| {
            v.on_notification(crate::notifications::Event::Clicked(target.clone()), window, cx);
            assert!(matches!(v.page, Page::Mail));
            assert_eq!(v.live.as_ref().unwrap().current, 1);
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, target.message);
            assert_eq!(v.live.as_ref().unwrap().pin, target.message);
            v.on_notification(crate::notifications::Event::Clicked(Target { message: None, ..target }), window, cx);
            assert!(v.inbox.open.is_none());
            assert!(v.live.as_ref().unwrap().pin.is_none());
        });
    }

    #[gpui::test]
    fn notification_click_waits_for_pending_moves_and_preserves_compose(cx: &mut TestAppContext) {
        use crate::notifications::Target;
        let (view, cx, _rx) = window(cx);
        cx.simulate_keystrokes("r");
        cx.simulate_input("Unfinished fixture draft");
        let target = view.update(cx, |v, _| {
            let m = incoming(v, 1);
            v.live.as_mut().unwrap().ops.insert(1, Op::Undo);
            Target { account: m.account, message: Some(m.id) }
        });
        view.update_in(cx, |v, window, cx| {
            v.on_notification(crate::notifications::Event::Clicked(target.clone()), window, cx);
            assert!(v.live.as_ref().unwrap().notification_target.is_some());
            assert!(v.compose.is_some());
            v.on_events(vec![Event::Op { op: 1, error: None }], window, cx);
            assert!(v.live.as_ref().unwrap().notification_target.is_none());
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, target.message);
            assert!(v.compose.is_none());
            assert_eq!(v.drafts.len(), 1);
            assert_eq!(v.drafts[0].read(cx).body_text(cx), "Unfinished fixture draft");
            assert!(!v.hold_exit(cx));
        });
    }

    #[gpui::test]
    fn inline_folder_creation_keeps_trash_guarded_until_reload(cx: &mut TestAppContext) {
        use lagom_core::store::FolderRole;
        let (view, cx, mut rx) = window(cx);
        let trash = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let trash = live.store.save_folder(&live.accounts[0].id, "Bin", Some("/"), FolderRole::Trash).unwrap();
            v.inbox = Inbox::live();
            v.open_kind(Kind::Trash, cx);
            assert!(v.can_empty_trash());
            v.confirm_empty_trash = true;
            trash
        });
        view.update_in(cx, |v, window, cx| v.new_folder(window, cx));
        cx.simulate_input("Taxes");
        cx.simulate_keystrokes("enter");
        let Cmd::CreateFolder { op, account, name } = rx.try_recv().unwrap() else { panic!("create command") };
        view.update(cx, |v, cx| {
            assert!(v.folder_name.is_none());
            assert!(!v.can_empty_trash());
            v.reload(cx);
            assert!(v.live.as_ref().unwrap().stale);
            assert!(!v.confirm_empty_trash);
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            assert!(rx.try_recv().is_err());
            v.live.as_ref().unwrap().store.save_folder(&account, &name, Some("/"), FolderRole::Other).unwrap();
        });
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::Op { op, error: None }, Event::Changed], window, cx);
            assert!(!v.live.as_ref().unwrap().stale);
            assert_eq!(v.inbox.folders[v.inbox.open.unwrap()].store, Some(trash));
            assert_eq!(v.inbox.folders.iter().filter(|f| f.full == "Taxes" && f.listed()).count(), 1);
            assert!(v.can_empty_trash());
            assert!(!v.confirm_empty_trash);
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
        });
        assert!(matches!(rx.try_recv().unwrap(), Cmd::EmptyTrash { folder, .. } if folder == trash));
        assert!(rx.try_recv().is_err());
    }

    #[gpui::test]
    fn search_in_second_trash_displays_and_empties_that_folder(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        let trash = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let first = live.store.save_folder(account, "Deleted Items", None, FolderRole::Trash).unwrap();
            let trash = live.store.save_folder(account, "Trash", None, FolderRole::Trash).unwrap();
            let mut hit = 0;
            for folder in [first, trash] {
                hit = live.store.save_message(&Message { summary: MessageSummary {
                    account: account.clone(), folder, uid: 1, seen: true, ..Default::default()
                }, ..Default::default() }).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.open_found(hit, cx);
            assert_eq!(v.inbox.folders[v.inbox.open.unwrap()].store, Some(trash));
            assert_eq!(v.inbox.list().iter().map(|&id| v.inbox.mail[id].store).collect::<Vec<_>>(), [Some(hit)]);
            v.confirm_empty_trash = true;
            v.reload(cx);
            assert!(v.confirm_empty_trash);
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].folder, mail::Place::Folder(trash));
            v.empty_trash(cx);
            trash
        });
        assert!(matches!(rx.try_recv().unwrap(), Cmd::EmptyTrash { folder, .. } if folder == trash));
        assert!(rx.try_recv().is_err());
    }

    #[gpui::test]
    fn empty_trash_waits_for_pending_restore_and_the_selected_folder(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        let (trash, hit) = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            live.store.save_folder(account, "INBOX", None, FolderRole::Inbox).unwrap();
            let first = live.store.save_folder(account, "Deleted Items", None, FolderRole::Trash).unwrap();
            let trash = live.store.save_folder(account, "Trash", None, FolderRole::Trash).unwrap();
            let mut hit = 0;
            for (folder, uid) in [(first, 1), (first, 2), (trash, 1)] {
                hit = live.store.save_message(&Message { summary: MessageSummary {
                    account: account.clone(), folder, uid, seen: true, ..Default::default()
                }, ..Default::default() }).unwrap();
            }
            v.inbox = Inbox::live();
            v.open_kind(Kind::Trash, cx);
            v.act(Move::Inbox, cx);
            v.open_found(hit, cx);
            assert_eq!(v.inbox.folders[v.inbox.open.unwrap()].store, Some(trash));
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].folder, mail::Place::Folder(first));
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            (trash, hit)
        });
        let Cmd::Op { op, act: Act::Inbox, .. } = rx.try_recv().unwrap() else { panic!("missing restore") };
        assert!(rx.try_recv().is_err(), "must not empty a different folder from the displayed rows");
        cx.run_until_parked();
        assert!(cx.debug_bounds("Empty Trash").is_none());
        assert!(cx.debug_bounds("Delete all").is_none());
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx));
        view.read_with(cx, |v, _| {
            assert!(!v.confirm_empty_trash, "a stale confirmation must not survive the reload");
            assert_eq!(v.inbox.list().iter().map(|&id| v.inbox.mail[id].store).collect::<Vec<_>>(), [Some(hit)]);
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("Empty Trash").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        let at = cx.debug_bounds("Delete all").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::EmptyTrash { folder, .. } if folder == trash));
        assert!(rx.try_recv().is_err());
    }

    #[gpui::test]
    fn trash_popout_restores_an_older_hit_after_its_page_is_unloaded(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        let store = view.update(cx, |v, cx| {
            let live = v.live.as_mut().unwrap();
            live.limit = 2;
            let account = &live.accounts[0].id;
            live.store.save_folder(account, "INBOX", None, FolderRole::Inbox).unwrap();
            let folder = live.store.save_folder(account, "Trash", None, FolderRole::Trash).unwrap();
            let mut oldest = 0;
            for uid in 1..=3 {
                let id = live.store.save_message(&Message { summary: MessageSummary {
                    account: account.clone(), folder, uid, date: uid as i64, seen: true, ..Default::default()
                }, ..Default::default() }).unwrap();
                if uid == 1 { oldest = id; }
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.open_found(oldest, cx);
            v.live.as_mut().unwrap().limit = 2;
            oldest
        });
        view.update_in(cx, |v, window, cx| v.open_popout(window, cx));
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        view.update(cx, |v, cx| {
            v.open_box(None, cx);
            v.open_kind(Kind::Trash, cx);
            assert!(!v.inbox.trash.contains(&v.inbox.by_store[&store]));
        });
        let mut popup = VisualTestContext::from_window(popout.into(), cx);
        popup.run_until_parked();
        assert!(popup.debug_bounds("Done").is_none(), "Trash must keep its restore action outside the loaded page");
        let at = popup.debug_bounds("Move to Inbox").unwrap().center();
        popup.simulate_click(at, Default::default());
        popup.run_until_parked();
        assert!(matches!(rx.try_recv().unwrap(), Cmd::Op { id, act: Act::Inbox, .. } if id == store));
        assert!(rx.try_recv().is_err());
        assert!(view.read_with(cx, |v, _| v.popouts.is_empty()));
    }

    #[gpui::test]
    fn trash_confirmation_restore_and_worker_completion(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        let folder = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            live.store.save_folder(account, "INBOX", None, FolderRole::Inbox).unwrap();
            let folder = live.store.save_folder(account, "[Provider]/Bin", Some("/"), FolderRole::Trash).unwrap();
            live.store.save_message(&Message { summary: MessageSummary {
                account: account.clone(), folder, uid: 5, seen: true, ..Default::default()
            }, ..Default::default() }).unwrap();
            v.inbox = Inbox::live();
            // Open before the first folder merge: the role still resolves to the provider folder.
            v.open_kind(Kind::Trash, cx);
            assert_eq!(v.inbox.list().len(), 1);
            assert_eq!(v.inbox.folders[v.inbox.open.unwrap()].store, Some(folder));
            assert_eq!(v.list_title().as_ref(), "Trash");
            v.empty_trash(cx);
            assert!(!v.emptying_trash());
            v.confirm_empty_trash = true;
            v.open_box(None, cx);
            assert!(!v.confirm_empty_trash);
            v.open_kind(Kind::Trash, cx);
            v.act(Move::Inbox, cx);
            folder
        });
        let Cmd::Op { op, act: Act::Inbox, .. } = rx.try_recv().unwrap() else { panic!("missing restore") };
        assert!(rx.try_recv().is_err(), "no empty command before confirmation");
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx));
        let op = view.update(cx, |v, cx| {
            assert_eq!(v.inbox.list().len(), 1, "failed restore comes back");
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            assert!(v.emptying_trash());
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            v.act(Move::Inbox, cx);
            v.live.as_ref().unwrap().next_op.get()
        });
        assert!(matches!(rx.try_recv().unwrap(), Cmd::EmptyTrash { op: id, folder: f } if id == op && f == folder));
        assert!(rx.try_recv().is_err(), "no duplicate or move while emptying");
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx));
        view.update(cx, |v, cx| {
            assert!(!v.emptying_trash());
            assert_eq!(v.inbox.list().len(), 1);
            assert!(v.toast.as_ref().unwrap().0.contains("Could not empty Trash"));
            v.confirm_empty_trash = true;
            v.empty_trash(cx);
            let id = v.inbox.mail[v.inbox.current().unwrap()].store.unwrap();
            v.live.as_ref().unwrap().store.delete_message(id).unwrap();
        });
        let Cmd::EmptyTrash { op, .. } = rx.try_recv().unwrap() else { panic!("missing empty") };
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Op { op, error: None }], window, cx));
        view.read_with(cx, |v, _| {
            assert!(!v.emptying_trash());
            assert!(v.inbox.list().is_empty());
            assert!(v.inbox.history.is_empty());
        });
    }

    #[gpui::test]
    fn folder_creation_waits_for_the_accounts_namespace(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            let live = v.live.as_mut().unwrap();
            let mut account = live.accounts[0].clone();
            account.id = "unlisted-account".into();
            live.store.save_account(&account).unwrap();
            live.accounts.push(account.clone());
            v.switch_account(1, cx);
            let n = v.inbox.folders.len();
            assert_eq!(v.create_folder("Parent/Child", cx), Err("Sync folders before creating a folder".into()));
            assert_eq!(v.inbox.folders.len(), n);
            assert!(rx.try_recv().is_err());
            assert!(v.live.as_ref().unwrap().ops.is_empty());
            v.live.as_ref().unwrap().store.save_folder(&account.id, "INBOX", None, lagom_core::store::FolderRole::Inbox).unwrap();
            v.create_folder("Parent/Child", cx).unwrap();
            assert!(matches!(rx.try_recv().unwrap(), Cmd::CreateFolder { name, .. } if name == "Parent/Child"));
        });
    }

    #[gpui::test]
    fn folder_creation_is_optimistic_and_adopted_without_duplicates(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let f = view.update(cx, |v, cx| {
            v.create_folder("Taxes", cx).unwrap();
            let f = v.inbox.folders.len() - 1;
            assert!(v.inbox.folders[f].listed());
            assert!(v.all_folders);
            assert!(v.create_folder("taxes", cx).is_err());
            v.reload(cx);
            assert!(v.inbox.folders[f].listed());
            assert!(v.live.as_ref().unwrap().stale);
            f
        });
        let Cmd::CreateFolder { op, account, name } = rx.try_recv().unwrap() else { panic!("create command") };
        assert_eq!(name, "Taxes");
        assert!(rx.try_recv().is_err());
        let id = view.update_in(cx, |v, window, cx| {
            let store = &v.live.as_ref().unwrap().store;
            let id = store.save_folder(&account, &name, Some("/"), lagom_core::store::FolderRole::Other).unwrap();
            v.on_events(vec![Event::Op { op, error: None }, Event::Changed], window, cx);
            id
        });
        view.read_with(cx, |v, _| {
            assert_eq!(v.inbox.folders[f].store, Some(id));
            assert_eq!(v.inbox.folders.iter().filter(|f| f.full == "Taxes" && f.listed()).count(), 1);
            assert!(v.live.as_ref().unwrap().ops.is_empty());
        });
    }

    #[gpui::test]
    fn failed_folder_creation_removes_the_row_and_allows_retry(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            v.create_folder("Taxes", cx).unwrap();
            v.inbox.open(Some(v.inbox.folders.len() - 1));
        });
        let Cmd::CreateFolder { op, .. } = rx.try_recv().unwrap() else { panic!("create command") };
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::Op { op, error: Some("refused".into()) }], window, cx);
            assert!(!v.inbox.folders.last().unwrap().listed());
            assert_eq!(v.inbox.open, None);
            assert!(v.live.as_ref().unwrap().ops.is_empty());
        });
        assert!(toast(&view, cx).contains("Could not create folder: refused"));
        view.update(cx, |v, cx| v.create_folder("Taxes", cx).unwrap());
        let Cmd::CreateFolder { op, account, .. } = rx.try_recv().unwrap() else { panic!("retry command") };
        view.update_in(cx, |v, window, cx| {
            let id = v.live.as_ref().unwrap().store.save_folder(&account, "Taxes", None, lagom_core::store::FolderRole::Other).unwrap();
            v.on_events(vec![Event::Op { op, error: None }], window, cx);
            assert_eq!(v.inbox.folders.last().unwrap().store, Some(id));
            assert_eq!(v.inbox.folders.iter().filter(|f| f.full == "Taxes" && f.listed()).count(), 1);
        });
    }

    #[gpui::test]
    fn stopped_worker_rolls_back_pending_folders(cx: &mut TestAppContext) {
        let (view, cx, rx) = window(cx);
        view.update(cx, |v, cx| v.create_folder("Pending", cx).unwrap());
        drop(rx);
        view.update(cx, |v, cx| {
            v.create_folder("Disconnected", cx).unwrap();
            assert!(v.inbox.folders.iter().filter(|f| ["Pending", "Disconnected"].contains(&f.full.as_str())).all(|f| !f.listed()));
            assert!(v.live.as_ref().unwrap().ops.is_empty());
        });
        assert!(toast(&view, cx).contains("Mail stopped working"));
    }

    #[gpui::test]
    fn folder_failure_does_not_remove_another_accounts_folder(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| v.create_folder("Taxes", cx).unwrap());
        let Cmd::CreateFolder { op, .. } = rx.try_recv().unwrap() else { panic!("create command") };
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_mut().unwrap();
            let mut other = live.accounts[0].clone();
            other.id = "other-account".into();
            live.store.save_account(&other).unwrap();
            live.store.save_folder(&other.id, "INBOX", Some("/"), lagom_core::store::FolderRole::Inbox).unwrap();
            live.accounts.push(other);
            v.switch_account(1, cx);
            v.create_folder("Taxes", cx).unwrap();
            v.on_events(vec![Event::Op { op, error: Some("refused".into()) }], window, cx);
            assert!(v.inbox.folders.last().unwrap().listed());
        });
    }

    #[gpui::test]
    fn folder_validation_includes_hidden_server_folders_and_delimiters(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            live.store.save_folder(&live.accounts[0].id, "INBOX", Some("."), lagom_core::store::FolderRole::Inbox).unwrap();
            let n = v.inbox.folders.len();
            for name in ["", "  ", "inbox", "Parent.Child"] {
                assert!(v.create_folder(name, cx).is_err(), "{name:?}");
            }
            assert_eq!(v.inbox.folders.len(), n);
            assert!(rx.try_recv().is_err());
            v.create_folder("Parent/Child", cx).unwrap();
        });
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

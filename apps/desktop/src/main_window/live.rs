//! The window's side of real mail: store reads, worker events, optimistic moves and their undo.
//! The UI reads mail and commits compose edits locally; it never waits on the network.

use super::*;

/// What an in-flight worker command needs when it finishes.
enum Op {
    /// Store identity survives account switches while the command is in flight.
    Move(i64),
    Read { account: String, store: i64 },
    /// Undo of a move: on failure only a message.
    Undo(i64),
    EmptyTrash(String),
    CreateFolder { account: String, name: String },
}

pub struct Live {
    pub(super) bridge: Bridge,
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
    more: bool,
    /// The ⌘K hit on screen: loaded even past `limit`, until another list opens.
    pub(super) pin: Option<i64>,
    /// The ⌘K hit to select once the next store read runs (it waits while moves are in flight).
    want: Option<i64>,
    notification_target: Option<crate::notifications::Target>,
    task_target: Option<i64>,
    move_target: Option<i64>,
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
    match panes::Writes::load(&store.path().with_file_name("window.json"), 1440., cx) {
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
            more: false,
            pin: None,
            want: None,
            notification_target: None,
            task_target: None,
            move_target: None,
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

    pub(super) fn event_op(&self) -> u64 {
        let id = self.next_op.get() + 1;
        self.next_op.set(id);
        id
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
    pub(super) fn prepare_move(&mut self, cx: &mut Context<Self>) -> bool {
        let members = self.inbox.selected_members();
        let accounts: HashSet<_> = members.iter().map(|&id| self.inbox.mail[id].account.clone()).collect();
        // shortcut: custom-folder moves use one account at a time, add per-account destinations for mixed selections.
        if accounts.len() > 1 {
            self.notify("Select messages from one account to move to a folder", cx);
            return false;
        }
        let Some(live) = &self.live else { return true };
        let Some(account) = accounts.iter().next() else { return true };
        if live.account().is_some_and(|a| a.id == *account) { return true; }
        let selected: HashSet<_> = members.iter().filter_map(|&id| self.inbox.mail[id].store).collect();
        let Some(&first) = selected.iter().next() else { return false };
        if !live.ops.is_empty() { self.notify("Wait for the pending operation before choosing a folder", cx); return false; }
        let summaries = match selected.iter().map(|&id| live.store.message_summary(id)).collect::<lagom_core::Result<Vec<_>>>() {
            Ok(summaries) => summaries,
            Err(e) => { self.notify(format!("Could not read selection: {e}"), cx); return false; }
        };
        self.open_found(first, cx);
        let Some(live) = &mut self.live else { return false };
        // Selected rows can straddle a page after switching accounts.
        for summary in summaries {
            let id = self.inbox.intern(&summary, &live.seen, chrono::Local::now());
            if !self.inbox.raw_list().contains(&id) { self.inbox.list_mut().push(id); }
        }
        self.inbox.regroup();
        self.inbox.chosen = Some(self.inbox.list().iter().copied().filter(|&id|
            self.inbox.members(id).iter().any(|&member| self.inbox.mail[member].store.is_some_and(|id| selected.contains(&id)))).collect());
        true
    }

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
        self.sidebar_scroll.scroll_to_item(1);
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
        self.reload_draft_rows(cx);
        self.reload_tasks(cx);
        let Some(live) = &mut self.live else { return };
        live.stale = true;
        let Ok(accounts) = live.store.accounts() else {
            self.confirm_empty_trash = false;
            return;
        };
        live.accounts = accounts;
        if self.inbox.account_filter.as_ref().is_some_and(|id| !live.accounts.iter().any(|a| &a.id == id)) {
            self.inbox.account_filter = None;
        }
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
            live.more = false;
            self.inbox = Inbox::live();
            self.reading_thread = None;
            self.close_popouts(None, cx);
            return;
        };
        let view = match self.inbox.open.map(|f| (&self.inbox.folders[f].kind, self.inbox.folders[f].store)) {
            None => mail::View::Inbox,
            Some((Kind::Later, _)) => mail::View::Later,
            Some((Kind::Done, _)) => mail::View::Role(lagom_core::store::FolderRole::Archive),
            Some((Kind::Sent, _)) => mail::View::Role(lagom_core::store::FolderRole::Sent),
            Some((Kind::Trash, None)) => mail::View::Trash,
            Some((_, Some(id))) => mail::View::Folder(id),
            // A folder ⌘K just created: nothing on the server yet.
            Some((_, None)) => mail::View::Inbox,
        };
        self.inbox.group = self.panes.group_conversations;
        let pins = self.inbox.selected_members().into_iter().filter_map(|id| self.inbox.mail[id].store).chain(live.pin).collect();
        let result = mail::load_accounts(&live.store, &live.accounts, &account, self.inbox.account_filter.as_deref(), view, live.limit, now(), pins, self.inbox.group);
        match result {
            Ok(snap) => {
                live.stale = false;
                live.more = snap.more;
                self.reading_dirty = true;
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
                Event::ForwardAttachments { draft, result } => {
                    for compose in self.compose.iter().chain(&self.drafts) {
                        compose.update(cx, |c, cx| c.forwarded(&draft, &result, cx));
                    }
                }
                Event::Downloaded(result) => match result {
                    Ok(path) => cx.reveal_path(&path),
                    Err(error) => self.notify(format!("Could not download attachment: {error}"), cx),
                },
                Event::Discovered { .. } | Event::SignInCancelled => {},
                Event::ReauthRequired(account) => {
                    if let Some(live) = &mut self.live {
                        live.sync.insert(account, SyncState::ReauthRequired);
                    }
                }
                Event::Reauthenticated { account, result } => {
                    if let Some(live) = &mut self.live {
                        if !live.accounts.iter().any(|a| a.id == account) { continue; }
                        live.sync.insert(account, if result.is_ok() { SyncState::Syncing } else { SyncState::ReauthRequired });
                    }
                    match result {
                        Ok(()) => read = true,
                        Err(error) => self.notify(format!("Could not sign in: {error}"), cx),
                    }
                }
                Event::EventCreated { op, error } => {
                    self.event_created(op, error, window, cx);
                    read = true;
                }
                Event::TasksSync(account, state) => {
                    self.today.task_sync.insert(account, state);
                    read = true;
                }
                Event::CalendarSync(account, state) => {
                    self.today.calendar_sync.insert(account, state);
                    read = true;
                }
                Event::Inline { id: store, error } => {
                    if let Some(mail) = self.inbox.mail.iter_mut().find(|m| m.store == Some(store)) {
                        mail.inline_pending = false;
                        mail.inline_retry_after = error.as_ref().map(|_| std::time::Instant::now() + std::time::Duration::from_secs(60));
                    }
                    if let Some(error) = error { self.notify(format!("Could not load inline images: {error}"), cx); }
                    else if let Some(id) = self.inbox.mail.iter().position(|m| m.store == Some(store)) {
                        self.inbox.mail[id].body.clear();
                        self.open_message(id, false, cx);
                        if let Some(body) = self.expanded.get(&id) {
                            body.update(cx, |body, cx| body.refresh(&self.inbox.mail[id], cx));
                        }
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
                        if state == SyncState::SigningIn || !matches!(live.sync.get(&account), Some(SyncState::ReauthRequired | SyncState::SigningIn)) {
                            live.sync.insert(account, state);
                        }
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
                                self.inbox.batches.clear();
                                self.close_popouts(None, cx);
                                self.clear_motion();
                            }
                            self.notify("Trash emptied", cx);
                        }
                        (Op::EmptyTrash(_), Some(e)) => self.notify(format!("Could not empty Trash: {e}"), cx),
                        (Op::Undo(store), None) => {
                            if let Some(&id) = self.inbox.by_store.get(&store) {
                                let mail = &mut self.inbox.mail[id];
                                if let (Some(old), mail::Place::Folder(folder)) = (mail.store, &mail.folder) {
                                    if matches!(live.store.message_summary(old), Err(lagom_core::Error::NotFound)) {
                                        if let Ok(Some(new)) = live.store.find_message(*folder, &mail.message_id) {
                                            // MOVE replaces store rows; keep the UI id used by selection and history.
                                            self.inbox.by_store.remove(&old);
                                            self.inbox.by_store.insert(new, id);
                                            mail.store = Some(new);
                                        }
                                    }
                                }
                            }
                        }
                        (_, None) => {}
                        (Op::CreateFolder { account, name }, Some(e)) => {
                            self.folder_failed(&account, &name);
                            self.notify(format!("Could not create folder: {e}"), cx);
                        }
                        (Op::Read { account, store }, Some(e)) => {
                            if let Some(id) = self.inbox.by_store.get(&store).copied().filter(|&id| self.inbox.mail[id].account == account) {
                                self.inbox.revert(id, Some(op));
                            }
                            self.notify(format!("Could not change read status: {e}"), cx);
                        }
                        (Op::Move(store), Some(e)) => {
                            if let Some(&id) = self.inbox.by_store.get(&store) {
                                self.inbox.back(id);
                                if let Some((what, id, ix)) = self.inbox.revert(id, Some(op)) {
                                    self.come_back(what, id, ix, false, cx);
                                }
                            }
                            self.notify(format!("Not moved: {e}"), cx);
                        }
                        (_, Some(e)) => self.notify(format!("Could not undo: {e}"), cx),
                    }
                }
                Event::Added(result) | Event::SignedIn(result) => match result {
                    Ok(id) => {
                        self.set_page(Page::Mail, cx);
                        if let Some(live) = &mut self.live {
                            live.accounts = live.store.accounts().unwrap_or_default();
                            live.current = live.accounts.iter().position(|a| a.id == id).unwrap_or(0);
                        }
                        self.inbox = Inbox::live();
                        self.reading_thread = None;
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
        if let Some(id) = self.live.as_mut().filter(|l| l.ops.is_empty()).and_then(|l| l.task_target.take()) {
            self.open_task_mail(id, cx);
        }
        if let Some(store) = self.live.as_mut().filter(|l| l.ops.is_empty()).and_then(|l| l.move_target.take()) {
            self.move_message(store, window, cx);
        }
        self.notify_mail(new_mail, window, cx);
        cx.notify();
    }

    pub(super) fn pending_moves(&self) -> HashSet<i64> {
        self.live.iter().flat_map(|live| live.ops.values()).filter_map(|op| match op {
            Op::Move(store) => Some(*store),
            _ => None,
        }).collect()
    }

    fn notify_mail(&mut self, messages: Vec<lagom_core::store::MessageSummary>, window: &Window, cx: &mut Context<Self>) {
        if !self.panes.notifications || window.is_window_active() { return; }
        let pending = self.pending_moves();
        let Some(live) = &mut self.live else { return };
        let messages = messages.into_iter().filter(|m| !live.seen.contains(&m.id) && !pending.contains(&m.id)).collect();
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

    pub(super) fn open_task_mail(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.demo && self.live.is_none() {
            let target = self.today.tasks.iter().find(|task| task.id == id).and_then(|task|
                self.inbox.mail.iter().position(|mail| task.mail_account.as_ref() == Some(&mail.account)
                    && task.mail_message_id.as_ref() == Some(&mail.message_id)));
            if let Some(target) = target {
                self.open_box(None, cx);
                if let Some(index) = self.inbox.items.iter().position(|&id| id == target) {
                    self.select(|inbox| inbox.select(index), cx);
                }
            }
            return;
        }
        let Some(live) = &mut self.live else { return };
        if !live.ops.is_empty() {
            live.task_target = Some(id);
            return;
        }
        let result = live.store.task_mail(id).and_then(|id| id.map(|id| live.store.message_summary(id)).transpose());
        match result {
            Ok(Some(mail)) => {
                let Some(account) = live.accounts.iter().position(|a| a.id == mail.account) else {
                    self.notify("The linked account is no longer available", cx);
                    return;
                };
                self.switch_account(account, cx);
                self.open_found(mail.id, cx);
            }
            Ok(None) => self.notify("The linked mail is no longer available locally", cx),
            Err(e) => self.notify(format!("Could not open linked mail: {e}"), cx),
        }
    }

    pub(super) fn open_notification(&mut self, target: crate::notifications::Target, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        window.activate_window();
        let Some(live) = &mut self.live else { return };
        if !live.ops.is_empty() {
            live.notification_target = Some(target);
            return;
        }
        let Some(account) = live.accounts.iter().position(|a| a.id == target.account) else { return };
        if let Some(id) = target.message {
            match live.store.message_summary(id) {
                Ok(message) if message.account == target.account => {}
                Ok(_) | Err(lagom_core::Error::NotFound) => {
                    self.notify("That message is no longer available locally", cx);
                    return;
                }
                Err(e) => { self.notify(format!("Could not open message: {e}"), cx); return; }
            }
        }
        live.current = account;
        live.limit = PAGE;
        live.pin = None;
        live.want = None;
        self.set_page(Page::Mail, cx);
        self.palette = None;
        // Preserve an unfinished compose sheet as a draft while revealing the clicked mail.
        if let Some(compose) = self.compose.take() {
            self.drafts.push(compose);
        }
        self.close_popouts(None, cx);
        self.inbox = Inbox::live();
        self.reading_thread = None;
        self.release(false, cx);
        self.clear_motion();
        self.reload(cx);
        if let Some(id) = target.message { self.open_found(id, cx); }
        window.focus(&self.focus);
        cx.notify();
    }

    /// Pending sends remain in the outbox when the worker stops.
    pub(super) fn worker_gone(&mut self, cx: &mut Context<Self>) {
        self.event_worker_gone();
        let Some(live) = &mut self.live else { return };
        live.pending.borrow_mut().clear();
        live.move_target = None;
        let rollback = live.store.with_short_lock_wait(Store::recover_moves);
        let mut ops: Vec<_> = live.ops.drain().collect();
        ops.sort_by_key(|(op, _)| std::cmp::Reverse(*op));
        live.stale = true;
        for (op, action) in ops {
            match action {
                Op::Move(store) => {
                    if let Some(&id) = self.inbox.by_store.get(&store) {
                        self.inbox.back(id);
                        if let Some((what, id, ix)) = self.inbox.revert(id, Some(op)) {
                            self.come_back(what, id, ix, false, cx);
                        }
                    }
                }
                Op::Read { account, store } => {
                    if let Some(id) = self.inbox.by_store.get(&store).copied().filter(|&id| self.inbox.mail[id].account == account) {
                        self.inbox.revert(id, Some(op));
                    }
                }
                Op::Undo(_) | Op::EmptyTrash(_) => {}
                Op::CreateFolder { account, name } => self.folder_failed(&account, &name),
            }
        }
        let mut error = "Mail stopped working. Restart Lagom; pending sends are saved in the outbox.".to_string();
        if let Err(rollback) = rollback { error.push_str(&format!(" Local rollback failed: {rollback}")); }
        self.notify(error, cx);
    }

    /// The worker half of an optimistic move.
    pub(super) fn server(&mut self, what: Move, id: usize, when: LaterWhen,
        destinations: &mut HashMap<String, Vec<lagom_core::store::Folder>>, cx: &mut Context<Self>) -> bool {
        let Some(store) = self.inbox.mail[id].store else { return true };
        let Some(live) = &mut self.live else { return true };
        if let Move::Read(seen) = what {
            live.seen.remove(&store);
            let op = live.op(Op::Read { account: self.inbox.mail[id].account.clone(), store });
            if let Some(history) = self.inbox.history.iter_mut().rev().find(|h| h.1 == id) { history.4 = Some(op); }
            let sent = live.bridge.send(Cmd::Op { op, id: store, act: Act::Read(seen) });
            if !sent { self.worker_gone(cx); }
            return sent;
        }
        let account = &self.inbox.mail[id].account;
        if !destinations.contains_key(account) {
            match live.store.folder_metadata(account) {
                Ok(folders) => { destinations.insert(account.clone(), folders); }
                Err(e) => {
                    self.inbox.revert(id, None);
                    self.notify(format!("Could not read destination: {e}"), cx);
                    return false;
                }
            }
        }
        let folders = &destinations[account];
        let store_of = |role| folders.iter().find(|f| f.role == role).map(|f| mail::Place::Folder(f.id));
        let (act, target) = match what {
            Move::Read(_) => unreachable!("handled above"),
            Move::Inbox => {
                (Act::Inbox, store_of(lagom_core::store::FolderRole::Inbox))
            }
            Move::Done => (Act::Archive, store_of(lagom_core::store::FolderRole::Archive)),
            Move::Delete => (Act::Trash, store_of(lagom_core::store::FolderRole::Trash)),
            Move::Later => (Act::Snooze(Some(when.at(chrono::Local::now()).timestamp())), None),
            Move::Folder(f) => match self.inbox.folders[f].store {
                Some(folder) if folders.iter().any(|f| f.id == folder) => (Act::Move(folder), Some(mail::Place::Folder(folder))),
                Some(_) => {
                    self.inbox.revert(id, None);
                    self.notify("Choose a folder in the message's account", cx);
                    return false;
                }
                None => {
                    let name = self.inbox.folders[f].full.clone();
                    (Act::NewFolder(name.clone()), Some(mail::Place::New(name)))
                }
            },
        };
        self.inbox.left(id, target);
        self.inbox.mail[id].snoozed_until = match act { Act::Snooze(until) => until, _ => None };
        let op = live.op(Op::Move(store));
        if let Some(history) = self.inbox.history.iter_mut().rev().find(|h| h.1 == id) { history.4 = Some(op); }
        let sent = live.bridge.send(Cmd::Op { op, id: store, act });
        if !sent { self.worker_gone(cx); }
        sent
    }

    /// Undo on the server: the message goes back to the folder it was in, or off Later.
    pub(super) fn unserver(&mut self, what: Move, id: usize, cx: &mut Context<Self>) {
        let Some(store) = self.inbox.mail[id].store else { return };
        if self.live.is_none() {
            return;
        }
        if matches!(what, Move::Read(_)) {
            let seen = !self.inbox.mail[id].unread;
            let live = self.live.as_mut().unwrap();
            live.seen.remove(&store);
            let op = live.op(Op::Read { account: self.inbox.mail[id].account.clone(), store });
            if !live.bridge.send(Cmd::Op { op, id: store, act: Act::Read(seen) }) { self.worker_gone(cx); }
            return;
        }
        let back = self.inbox.back(id);
        let act = match what {
            Move::Read(_) => unreachable!("handled above"),
            Move::Later => Act::Snooze(self.inbox.mail[id].snoozed_until),
            Move::Inbox | Move::Done | Move::Delete | Move::Folder(_) => match back {
                mail::Place::Folder(id) => Act::Move(id),
                // The worker finds it by name: the create ran before this undo.
                mail::Place::New(name) => Act::NewFolder(name),
            },
        };
        let Some(live) = &mut self.live else { return };
        let op = live.op(Op::Undo(store));
        if !live.bridge.send(Cmd::Op { op, id: store, act }) {
            self.worker_gone(cx);
            return;
        }
        if what != Move::Later {
            if let Some(until) = self.inbox.mail[id].snoozed_until {
                let op = live.op(Op::Undo(store));
                if !live.bridge.send(Cmd::Op { op, id: store, act: Act::Snooze(Some(until)) }) { self.worker_gone(cx); }
            }
        }
    }

    pub(super) fn toggle_read_on(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let seen = self.inbox.members(id).iter().any(|&id| self.inbox.mail[id].unread);
        self.act_on(id, Move::Read(seen), window, cx);
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
        let Some(id) = self.inbox.single() else {
            self.reading_thread = None;
            self.reading_members.clear();
            self.expanded.clear();
            return;
        };
        let changed = self.reading_thread != Some(id);
        if changed || self.reading_dirty {
            let was_single = self.reading_members.len() <= 1;
            if changed { self.expanded.clear(); }
            self.reading_dirty = false;
            self.reading_members = self.inbox.members(id);
            if self.inbox.group {
                if let Some(live) = &self.live {
                    let m = &self.inbox.mail[id];
                    let folder = m.folder.clone();
                    match live.store.thread_messages(&m.account, &m.thread_id) {
                        Ok(mut rows) => {
                            // Prefer the visible folder's copy; actions still use every folder member.
                            rows.sort_by_key(|m| (mail::Place::Folder(m.folder) != folder, m.id));
                            let mut seen = HashSet::new();
                            rows.retain(|m| m.message_id.is_empty() || seen.insert(m.message_id.clone()));
                            self.reading_members = rows.iter().map(|m| self.inbox.intern(m, &live.seen, chrono::Local::now())).collect();
                        }
                        Err(e) => self.notify(format!("Could not read conversation: {e}"), cx),
                    }
                }
            }
            self.reading_members.sort_by_key(|&id| (self.inbox.mail[id].date, self.inbox.mail[id].store.unwrap_or(id as i64)));
            self.reading_thread = Some(id);
            self.expanded.retain(|id, _| self.reading_members.contains(id));
            if (changed || was_single) && self.inbox.group && self.reading_members.len() > 1 {
                let pin = self.live.as_ref().and_then(|live| live.pin);
                let newest = self.reading_members.iter().find(|&&id| pin.is_some() && self.inbox.mail[id].store == pin)
                    .or_else(|| self.reading_members.iter().rev().find(|&&id| self.inbox.mail[id].unread))
                    .or_else(|| self.reading_members.last()).copied();
                if let Some(id) = newest {
                    self.toggle_message(id, mark_read, cx);
                    if let Some(ix) = self.reading_members.iter().position(|&member| member == id) { self.reading_scroll.scroll_to_top_of_item(ix); }
                }
            }
        }
        if !self.inbox.group || self.reading_members.len() <= 1 {
            self.open_message(id, mark_read, cx);
        } else if mark_read {
            let expanded: Vec<_> = self.expanded.keys().copied().collect();
            for id in expanded { self.open_message(id, true, cx); }
        }
    }

    pub(super) fn open_message(&mut self, id: usize, mark_read: bool, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let Some(store) = self.inbox.mail[id].store else { return };
        if self.inbox.mail[id].body.is_empty() {
            self.inbox.mail[id].body = match live.store.message(store).and_then(|m| Ok((m, live.store.attachments(store)?, live.store.inline_parts(store)?))) {
                Ok((m, parts, inline)) => {
                    let body = mail::paragraphs(&lagom_core::mime::readable_text(&m.body_text, m.body_html.as_deref()));
                    self.inbox.mail[id].inline = std::sync::Arc::new(inline);
                    self.inbox.mail[id].attachments = parts;
                    self.inbox.mail[id].html = m.body_html.map(Into::into);
                    body
                }
                Err(_) => vec!["This message is no longer in the local store.".into()],
            };
        }
        if !self.inbox.mail[id].inline_pending
            && self.inbox.mail[id].inline_retry_after.is_none_or(|after| std::time::Instant::now() >= after)
            && live.store.inline_loaded(store).is_ok_and(|loaded| !loaded)
            && self.inbox.mail[id].html.as_deref().is_some_and(|h| !lagom_core::inline::referenced_ids(h).is_empty()) {
            self.inbox.mail[id].inline_pending = live.bridge.send(Cmd::Inline(store));
        }
        // Reading a filtered list must not consume the next unread row on every reload.
        if mark_read && !self.inbox.unread_only && !self.inbox.keep_unread.contains(&id) && self.inbox.mail[id].unread && matches!(self.page, Page::Mail) {
            self.inbox.set_unread(id, false);
            live.seen.insert(store);
            // A stopped worker shows up through the event channel; nothing waits on this.
            let _ = live.bridge.send(Cmd::MarkSeen(store));
            cx.notify();
        }
    }

    pub(super) fn has_more(&self) -> bool {
        self.live.as_ref().is_some_and(|live| live.more)
    }

    pub(super) fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.has_more() { return; }
        if let Some(live) = &mut self.live { live.limit += PAGE; }
        self.reload(cx);
    }

    /// Reads the next page when the selection nears the end of what is loaded.
    pub(super) fn near_end(&mut self, cx: &mut Context<Self>) {
        if self.inbox.selected + 20 >= self.inbox.list().len() { self.load_more(cx); }
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
            self.inbox.regroup();
            self.inbox.history.clear();
            self.inbox.batches.clear();
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
        if self.live.is_none() {
            if let Some(account) = mail::demo_accounts().get(ix) {
                self.inbox.account_filter = Some(account.id.clone());
                self.open_box(None, cx);
            }
            return;
        }
        let live = self.live.as_mut().unwrap();
        if ix >= live.accounts.len() { return; }
        let account = live.accounts[ix].id.clone();
        live.current = ix;
        self.folder_name = None;
        live.limit = PAGE;
        live.pin = None;
        // Folder indices belong to one account; history does too.
        self.inbox = Inbox::live();
        self.inbox.account_filter = Some(account);
        self.reading_thread = None;
        self.close_popouts(None, cx);
        self.clear_motion();
        self.set_page(Page::Mail, cx);
        self.reload(cx);
    }

    pub(super) fn sync_now(&mut self) {
        if let Some(live) = &self.live {
            let _ = live.bridge.send(Cmd::Sync(None));
        }
    }

    /// "Personal · synced 10:12" for the current account, `None` in demo mode.
    pub(super) fn sync_line(&self) -> Option<SharedString> {
        let live = self.live.as_ref()?;
        Some(format!("{} · {}", short_name(live.account()?), self.sync_state()?).into())
    }

    pub(super) fn sync_icon(&self) -> &'static str {
        let Some(live) = &self.live else { return "icons/cloud-synced.svg" };
        live.account().and_then(|a| live.sync.get(&a.id)).map_or("icons/cloud-syncing.svg", polish::sync_icon)
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
            Some(SyncState::ReauthRequired) => "Sign in again".into(),
            Some(SyncState::SigningIn) => "Signing in…".into(),
        };
        Some(state.into())
    }

    pub(super) fn reauth_control(&self, ix: usize, slot: &'static str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let live = self.live.as_ref()?;
        let account = live.accounts.get(ix)?;
        let signing_in = match live.sync.get(&account.id)? {
            SyncState::ReauthRequired => false,
            SyncState::SigningIn => true,
            _ => return None,
        };
        let id = account.id.clone();
        let t = &self.theme;
        Some(icon_button(t, slot, if signing_in { "icons/cancel.svg" } else { "icons/access.svg" },
            if signing_in { "Cancel sign-in" } else { "Sign in again" }).id((slot, ix))
            .debug_selector(move || format!("{slot}-{ix}"))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.reauthenticate(&id, cx);
            })).into_any_element())
    }

    fn reauthenticate(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        let cmd = match live.sync.get(id) {
            Some(SyncState::ReauthRequired) => Cmd::Reauthenticate(id.into()),
            Some(SyncState::SigningIn) => Cmd::CancelSignIn,
            _ => return,
        };
        if !live.bridge.send(cmd) { self.worker_gone(cx); }
    }

    /// ⌘K search hit: open its list (Later when snoozed) and select it. The hit is pinned into
    /// that list, so the selection never lands on another row that Delete or Reply would hit.
    pub(super) fn open_found(&mut self, store: i64, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let Ok(m) = live.store.message(store) else { return self.notify("That message is gone", cx) };
        if live.account().is_none_or(|a| a.id != m.summary.account) {
            if let Some(ix) = live.accounts.iter().position(|a| a.id == m.summary.account) { self.switch_account(ix, cx); }
        }
        self.inbox.account_filter = Some(m.summary.account.clone());
        let list = if m.summary.snoozed_until.is_some_and(|t| t > now()) {
            self.inbox.find_kind(Kind::Later)
        } else {
            // INBOX is not in `folders`: no match means the inbox.
            self.inbox.folders.iter().position(|f| f.store == Some(m.summary.folder) && !f.gone)
        };
        self.inbox.unread_only = false;
        self.open_box(list, cx);
        if let Some(live) = &mut self.live {
            (live.pin, live.want) = (Some(store), Some(store));
        }
        self.reload(cx);
    }

    pub(super) fn move_message(&mut self, store: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = &mut self.live else { return };
        if !live.ops.is_empty() {
            live.move_target = Some(store);
            return;
        }
        if live.store.message_summary(store).is_err() {
            self.notify("That message is no longer available locally", cx);
            return;
        }
        self.open_found(store, cx);
        if self.live.as_ref().is_some_and(|l| !l.stale && l.want.is_none())
            && self.inbox.single().is_some_and(|id| self.inbox.members(id).iter().any(|&id| self.inbox.mail[id].store == Some(store))) {
            self.open_palette("move ", window, cx);
        }
    }

    fn select_found(&mut self, store: i64, cx: &mut Context<Self>) {
        match self.inbox.list().iter().position(|&id| self.inbox.members(id).iter().any(|&member| self.inbox.mail[member].store == Some(store))) {
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
        match store.owned_drafts() {
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
            this.set_page(Page::Mail, cx);
            window.focus(&this.focus);
            cx.notify();
        })
        .detach();
        test_hook(&add, window, cx);
        self.set_page(Page::AddAccount(add), cx);
        cx.notify();
    }

    pub(super) fn prepare_signatures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let demo = mail::demo_accounts();
        let accounts = self.live.as_ref().map_or(&demo[..], |live| &live.accounts);
        self.signatures.retain(|id, _| accounts.iter().any(|a| &a.id == id));
        for account in accounts {
            if self.signatures.contains_key(&account.id) { continue; }
            let text = match self.live.as_ref().map_or_else(|| Ok(String::new()), |live| live.store.signature(&account.id)) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let input = cx.new(|cx| TextInput::new(window, cx).markdown().bordered().placeholder("Signature, added to new mail, replies and forwards").with_text(&text));
            let id = account.id.clone();
            let focus = input.read(cx).focus_handle(cx);
            cx.on_blur(&focus, window, move |this, _, cx| {
                let Some(input) = this.signatures.get(&id) else { return };
                if let Some(live) = &this.live {
                    if let Err(e) = live.store.with_short_lock_wait(|store| store.set_signature(&id, input.read(cx).text())) {
                        this.notify(format!("Could not save signature: {e}"), cx);
                    }
                }
            }).detach();
            self.signatures.insert(account.id.clone(), input);
        }
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.confirm_empty_trash = false;
        self.set_page(Page::Settings(None), cx);
        cx.notify();
    }

    pub(crate) fn confirm_remove(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.set_page(Page::Settings(id), cx);
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
        self.reading_thread = None;
        self.close_popouts(None, cx);
        self.clear_motion();
        self.set_page(Page::Settings(None), cx);
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

    #[gpui::test]
    fn review_normal_startup_restores_saved_pages(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lagom-startup-pages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let store = Rc::new(Store::open(dir.join("mail.sqlite3")).unwrap());
        let (bridge, _rx) = Bridge::detached();
        cx.update(|cx| cx.set_global(Mail {
            store, bridge, next_op: Rc::new(std::cell::Cell::new(0)), pending: Default::default(), _events: Task::ready(()),
        }));
        let path = dir.join("window.json");
        for page in [StartPage::Calendar, StartPage::Home, StartPage::Todo, StartPage::Mail] {
            let mut panes = Panes::new(1440.);
            panes.start_page = page;
            panes.save(&path).unwrap();
            let handle = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
            handle.update(cx, |v, window, _| {
                assert!(!v.demo);
                assert_eq!(v.preferences_path.as_ref(), Some(&path));
                assert!(matches!((&v.page, page), (Page::Calendar, StartPage::Calendar) | (Page::Home, StartPage::Home) | (Page::Todo, StartPage::Todo) | (Page::Mail, StartPage::Mail)));
                window.remove_window();
            }).unwrap();
            cx.run_until_parked();
        }
        let handle = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        handle.update(cx, |v, _, cx| v.save_panes(cx)).unwrap();
        cx.run_until_parked();
        let (resume, wait) = tokio::sync::oneshot::channel();
        cx.update(|cx| {
            let tail = cx.background_executor().spawn(async move { wait.await.unwrap(); });
            cx.global_mut::<panes::Writes>().tail = Some(tail);
        });
        handle.update(cx, |v, window, cx| {
            v.show_section(2, window, cx);
            window.remove_window();
        }).unwrap();
        cx.run_until_parked();
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Mail);
        let handle = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        handle.update(cx, |v, window, cx| {
            assert!(matches!(v.page, Page::Calendar));
            v.show_section(3, window, cx);
            window.remove_window();
        }).unwrap();
        resume.send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Todo);
        cx.update(|cx| cx.remove_global::<Mail>());
        for entry in std::fs::read_dir(&dir).unwrap() { std::fs::remove_file(entry.unwrap().path()).unwrap(); }
        std::fs::remove_dir(dir).unwrap();
    }

    /// A demo window with a live side whose worker is `rx`: kept alive it never answers,
    /// dropped it has stopped.
    fn window(cx: &mut TestAppContext) -> (Entity<MainWindow>, &mut VisualTestContext, tokio::sync::mpsc::UnboundedReceiver<Cmd>) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::text_input::bind_keys(cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
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
        view.update(cx, |v, cx| { v.live = Some(fixture_live(store, bridge)); v.page = Page::Mail; cx.notify(); });
        (view, cx, rx)
    }

    #[gpui::test]
    fn legacy_newsletter_reads_and_quotes_visible_text(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let mut summary = incoming(v, 1);
            summary.from_address = "sender@example.test".into();
            v.live.as_ref().unwrap().store.save_message(&lagom_core::store::Message {
                summary, body_text: "96 * * sup { font-size: 100% !important; }\n\n\nhttps://tracking.example.test/click".into(),
                body_html: Some(include_str!("../../../../crates/lagom-core/tests/fixtures/newsletter.html").into()),
            }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select(0);
            let selected = v.inbox.single().unwrap();
            v.open_message(selected, false, cx);
            assert_eq!(v.inbox.mail[selected].body, ["Hello & welcome", "Read the news"]);
        });
        cx.simulate_keystrokes("r");
        let quote = view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx));
        assert!(quote.ends_with("> Hello & welcome\n> \n> Read the news"), "{quote:?}");
        assert!(!quote.contains("font-size") && !quote.contains("tracking.example.test"));
    }

    #[gpui::test]
    fn legacy_inline_failure_waits_before_retry_and_completion_survives_summary_reload(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut summary = incoming(v, 1);
            summary.has_attachments = true;
            let id = summary.id;
            store.save_message(&lagom_core::store::Message {
                summary, body_text: "Inline fixture".into(), body_html: Some("<img src='cid:logo@test'>".into()),
            }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select(0);
            let selected = v.inbox.single().unwrap();
            v.open_message(selected, false, cx);
            v.reload(cx);
            v.open_message(selected, false, cx);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Inline(row)) if row == id));
            assert!(rx.try_recv().is_err());
            v.on_events(vec![Event::Inline { id, error: Some("offline".into()) }], window, cx);
            assert!(!v.inbox.mail[selected].body.is_empty());
            assert!(!v.inbox.mail[selected].inline_pending);
            assert!(rx.try_recv().is_err());
        });
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(rx.try_recv().is_err(), "render must not retry a failed inline load");
        view.update_in(cx, |v, window, cx| {
            let selected = v.inbox.single().unwrap();
            let store = v.live.as_ref().unwrap().store.clone();
            let id = v.inbox.mail[selected].store.unwrap();
            v.reload(cx);
            v.open_current(false, cx);
            assert!(rx.try_recv().is_err(), "summary reload must retain the retry gate");
            assert!(v.inbox.mail[selected].inline_retry_after.unwrap() > std::time::Instant::now());
            v.inbox.mail[selected].inline_retry_after = Some(std::time::Instant::now());
            v.open_current(false, cx);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Inline(row)) if row == id));
            v.open_message(selected, false, cx);
            assert!(rx.try_recv().is_err());
            let db = rusqlite::Connection::open(store.path()).unwrap();
            db.execute("INSERT INTO inline_parts VALUES (?1,'logo@test','image/png',x'00ff')", [id]).unwrap();
            db.execute("UPDATE messages SET inline_loaded=1 WHERE id=?1", [id]).unwrap();
            v.on_events(vec![Event::Inline { id, error: None }], window, cx);
            assert!(v.inbox.mail[selected].inline_retry_after.is_none());
            assert_eq!(v.inbox.mail[selected].inline[0].data, [0, 255]);
            v.reload(cx);
            assert_eq!(v.inbox.mail[selected].inline[0].data, [0, 255]);
            assert!(rx.try_recv().is_err());
        });
    }

    #[gpui::test]
    fn calendar_creation_cancel_dispatch_and_completion_reload_immediately(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            v.demo = false;
            let live = v.live.as_mut().unwrap();
            live.accounts[0].auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            let account = &live.accounts[0];
            live.store.save_authorized_account(account, lagom_core::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
            let calendars = [("a", true), ("b", true), ("read-only", false)].map(|(id, writable)| (lagom_core::calendar::Calendar {
                remote_id: id.into(), name: "Fixture calendar".into(), color: None, shown: true, writable,
            }, vec![]));
            live.store.replace_calendar_window(&account.id, 0, i64::MAX, &calendars).unwrap();
            v.show_section(2, window, cx);
        });
        cx.simulate_keystrokes("n");
        cx.simulate_input("Discard fixture");
        cx.simulate_keystrokes("escape");
        assert!(rx.try_recv().is_err(), "Esc must not send any command");
        cx.simulate_keystrokes("n");
        cx.simulate_input("Created fixture");
        cx.run_until_parked();
        assert!(cx.debug_bounds("event-calendar-0").is_some());
        assert!(cx.debug_bounds("event-calendar-1").is_some());
        assert!(cx.debug_bounds("event-calendar-2").is_none());
        let calendar = cx.debug_bounds("event-calendar-1").unwrap().center();
        cx.simulate_click(calendar, Default::default());
        let meet = cx.debug_bounds("Add Google Meet").unwrap().center();
        cx.simulate_click(meet, Default::default());
        for (field, value) in [("event-field-2", "09:00"), ("event-field-4", "09:45")] {
            let at = cx.debug_bounds(field).unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_keystrokes("secondary-a");
            cx.simulate_input(value);
        }
        cx.simulate_keystrokes("enter enter");
        let (mut op, account, calendar, draft) = match rx.try_recv().unwrap() {
            Cmd::CreateEvent { op, account, calendar, event } => (op, account, calendar, event),
            _ => panic!("expected calendar command"),
        };
        assert_eq!(calendar, "b");
        assert!(draft.meet);
        assert!(rx.try_recv().is_err(), "repeated Return must not duplicate a request");
        cx.simulate_input("Do not edit while saving");
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::EventCreated { op, error: Some("Fixture refused".into()) }], window, cx));
        cx.simulate_keystrokes("enter");
        match rx.try_recv().unwrap() {
            Cmd::CreateEvent { op: retry, event, .. } => { op = retry; assert_eq!(event.title, "Created fixture"); }
            _ => panic!("failed save must retain the original draft"),
        }
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::EventCreated { op: op + 1, error: None }], window, cx);
            assert!(v.week.editor.is_some(), "unrelated completions cannot close the sheet");
            let mut event = draft.event("created".into()).unwrap();
            event.join_url = None;
            v.live.as_ref().unwrap().store.insert_calendar_event(&account, &calendar, &event).unwrap();
            v.on_events(vec![Event::EventCreated { op, error: None }], window, cx);
            assert!(v.week.editor.is_none());
            assert!(v.today.events.iter().any(|e| e.event.remote_id == "created"));
        });
        cx.run_until_parked();
        // Keep the fixture's morning event visible independently of the current time.
        view.update(cx, |v, cx| { v.week.scroll_to_now = false; v.week.scroll.set_offset(gpui::point(px(0.), px(-480.))); cx.notify(); });
        cx.run_until_parked();
        let day = chrono::Datelike::weekday(&chrono::Local::now()).num_days_from_monday();
        let event = cx.debug_bounds(format!("week-event-{day}-0").leak()).unwrap();
        cx.simulate_click(event.center(), Default::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("event-detail").is_some());
        assert!(cx.debug_bounds("Join").is_none());
        view.update_in(cx, |v, window, cx| {
            let mut event = draft.event("created".into()).unwrap();
            event.join_url = Some("https://meet.example.test/fixture".into());
            let store = &v.live.as_ref().unwrap().store;
            let mut other = event.clone();
            other.start_utc -= 3600;
            other.join_url = Some("https://meet.example.test/other-calendar".into());
            store.insert_calendar_event(&account, "a", &other).unwrap();
            store.insert_calendar_event(&account, &calendar, &event).unwrap();
            v.on_events(vec![Event::Changed], window, cx);
            assert_eq!(v.week.detail.as_ref().unwrap().event, event, "an open detail must receive the completed conference");
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("event-detail").is_some());
        assert!(cx.debug_bounds("Join").is_some());
        assert!(rx.try_recv().is_err(), "completion must display the cached event without fetching");
        view.update_in(cx, |v, window, cx| {
            v.live.as_ref().unwrap().store.delete_calendar_event(&account, &calendar, "created").unwrap();
            v.on_events(vec![Event::Changed], window, cx);
            assert!(v.week.detail.is_none(), "a deleted event must not leave a stale detail or select a different calendar");
        });
    }

    #[gpui::test]
    fn shift_u_uses_batch_read_path_and_rolls_back_rejection(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=3 { incoming(v, uid); }
            v.panes.group_conversations = false;
            v.inbox = Inbox::live();
            v.reload(cx);
            v.filter_unread(true, cx);
            v.inbox.select_all();
        });
        cx.simulate_keystrokes("shift-u");
        let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(commands.len(), 3);
        let events = commands.into_iter().map(|cmd| match cmd {
            Cmd::Op { op, act: Act::Read(true), .. } => Event::Op { op, error: Some("fixture rejected read".into()) },
            _ => panic!("Shift+U must use the batch read command"),
        }).collect();
        assert!(view.read_with(cx, |v, _| v.inbox.list().is_empty()));
        view.update_in(cx, |v, window, cx| v.on_events(events, window, cx));
        view.read_with(cx, |v, _| {
            assert_eq!(v.inbox.list().len(), 3);
            assert!(v.inbox.list().iter().all(|&id| v.inbox.mail[id].unread));
        });
    }

    #[gpui::test]
    fn review_home_count_excludes_pending_moves(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            for uid in 1..=7 { incoming(v, uid); }
            v.panes.group_conversations = false;
            v.inbox = Inbox::live();
            v.reload(cx);
            let moved = v.inbox.mail[v.inbox.current().unwrap()].store.unwrap();
            v.act(Move::Done, cx);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Op { id, act: Act::Archive, .. }) if id == moved));
            v.show_home(window, cx);
            assert_eq!(v.today.unread, 6);
        });
    }

    #[gpui::test]
    fn review_home_rejects_deferred_click_after_move_replaces_store_row(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            incoming(v, 1);
            incoming(v, 2);
            v.panes.group_conversations = false;
            v.inbox = Inbox::live();
            v.reload(cx);
            let moved = v.inbox.mail[v.inbox.current().unwrap()].store.unwrap();
            let store = v.live.as_ref().unwrap().store.clone();
            let account = store.message_summary(moved).unwrap().account;
            v.act(Move::Done, cx);
            let Cmd::Op { op, id, act: Act::Archive } = rx.try_recv().unwrap() else { panic!("missing move") };
            assert_eq!(id, moved);
            v.show_home(window, cx);
            v.open_notification(crate::notifications::Target { account: account.clone(), message: Some(moved) }, window, cx);
            assert!(v.live.as_ref().unwrap().notification_target.is_some());
            // IMAP MOVE replaces the source row with a destination row during sync.
            let mut message = store.message(moved).unwrap();
            message.summary.folder = store.save_folder(&account, "Archive", None, lagom_core::store::FolderRole::Archive).unwrap();
            let replacement = store.save_message(&message).unwrap();
            store.delete_message(moved).unwrap();
            assert_ne!(replacement, moved);
            v.on_events(vec![Event::Op { op, error: None }], window, cx);
            assert!(matches!(v.page, Page::Home), "a stale target must not open the remaining Inbox message");
            assert!(v.live.as_ref().unwrap().notification_target.is_none());
            assert!(v.toast.as_ref().unwrap().0.contains("no longer available"));
        });
        cx.run_until_parked();
        assert!(rx.try_recv().is_err(), "rejecting the click must not mark another message read");
    }

    #[gpui::test]
    fn review_home_calendar_consent_survives_cached_events_and_mixed_accounts(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let account = view.update_in(cx, |v, window, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut account = store.accounts().unwrap().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            store.save_account(&account).unwrap();
            let day = chrono::Local::now().date_naive();
            let start = super::super::agenda::day_start(day, &chrono::Local);
            store.replace_calendar_window(&account.id, start - 7 * 86400, start + 60 * 86400,
                &[(lagom_core::calendar::Calendar { remote_id: "primary".into(), name: "Fixture".into(), color: None, shown: true, writable: false },
                    vec![lagom_core::calendar::Event { remote_id: "cached".into(), title: "Cached event".into(), start: day.to_string(), end: day.succ_opt().unwrap().to_string(),
                        start_utc: start, end_utc: start + 86400, all_day: true, location: None, join_url: None, html_link: None }])]).unwrap();
            store.set_account_scopes(&account.id, "").unwrap();
            v.on_events(vec![Event::CalendarSync(account.id.clone(), SyncState::Failed("Calendar permission revoked".into())), Event::Changed], window, cx);
            v.show_section(2, window, cx);
            account
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("allow-calendar-0").is_some(), "cached events must not hide revoked consent");
        assert!(cx.debug_bounds("week-grid").is_some());
        view.update_in(cx, |v, window, cx| {
            let store = &v.live.as_ref().unwrap().store;
            let mut other = account.clone();
            other.id = "second-calendar".into();
            other.email = "second@example.test".into();
            store.save_authorized_account(&other, lagom_core::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
            v.on_events(vec![Event::Changed], window, cx);
        });
        cx.run_until_parked();
        let ix = view.read_with(cx, |v, _| v.today.calendar_accounts.iter().position(|(a, _)| a.id == account.id).unwrap());
        let button = cx.debug_bounds(format!("allow-calendar-{ix}").leak()).expect("another authorized account must not hide consent");
        cx.simulate_click(button.center(), Default::default());
        assert!(matches!(rx.try_recv(), Ok(Cmd::AllowCalendar(id)) if id == account.id));
    }

    #[gpui::test]
    fn review_home_calendar_reopened_offline_uses_successful_fetch_bounds(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_mut().unwrap();
            let mut account = live.store.accounts().unwrap().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            live.store.save_authorized_account(&account, lagom_core::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
            let fetched = chrono::Local::now().timestamp() - 90 * 86400;
            live.store.replace_calendar_window(&account.id, fetched - 7 * 86400, fetched + 60 * 86400, &[]).unwrap();
            live.store = Rc::new(Store::open(live.store.path()).unwrap());
            v.show_section(2, window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("week-unsynced").is_some(), "today has never been fetched");
        assert!(cx.debug_bounds("week-grid").is_none());
    }

    #[gpui::test]
    fn home_unread_opens_inbox_and_default_tasks_include_future_tasks(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            for uid in 1..=7 { incoming(v, uid); }
            let store = &v.live.as_ref().unwrap().store;
            let list = store.default_task_list().unwrap();
            store.add_task(list, "Future task", "", Some("2099-12-31".into())).unwrap();
            v.reload(cx);
            v.show_home(window, cx);
            assert_eq!(v.today.unread, 7);
            assert_eq!(v.today.tasks.len(), 1);
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("home-unread").unwrap().center();
        cx.simulate_click(at, Default::default());
        view.read_with(cx, |v, _| {
            assert!(matches!(v.page, Page::Mail));
            assert!(v.inbox.open.is_none());
        });
        cx.simulate_keystrokes("secondary-4");
        view.read_with(cx, |v, _| { assert!(matches!(v.page, Page::Todo)); assert_eq!(v.today.tasks[0].title, "Future task"); });
        cx.simulate_keystrokes("secondary-1");
        assert_eq!(view.read_with(cx, |v, _| v.today.tasks.len()), 1);
    }

    #[gpui::test]
    fn review_shortcuts_share_batch_reads_and_require_one_compose_target(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=2 {
                let m = incoming(v, uid);
                v.live.as_ref().unwrap().store.set_flags(m.folder, m.uid, true, false).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
        });
        cx.simulate_keystrokes("f shift-r");
        assert!(view.read_with(cx, |v, _| v.compose.is_none()));
        cx.simulate_keystrokes("shift-u");
        let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| matches!(c, Cmd::Op { act: Act::Read(false), .. })));
        view.read_with(cx, |v, _| assert_eq!(v.inbox.batches.last().unwrap().len(), 2));
    }

    #[gpui::test]
    fn review_popout_toggle_reaches_a_filtered_message(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let target = view.update_in(cx, |v, window, cx| {
            let m = incoming(v, 1);
            v.live.as_ref().unwrap().store.set_flags(m.folder, m.uid, true, false).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.open_popout(window, cx);
            v.filter_unread(true, cx);
            assert!(v.inbox.list().is_empty());
            m.id
        });
        while rx.try_recv().is_ok() {}
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        cx.update_window(popout.into(), |_, window, cx| {
            window.draw(cx).clear();
            window.dispatch_action(Box::new(ToggleRead), cx);
        }).unwrap();
        cx.run_until_parked();
        assert!(matches!(rx.try_recv(), Ok(Cmd::Op { id, act: Act::Read(false), .. }) if id == target));
        view.read_with(cx, |v, _| assert_eq!(v.inbox.batches.last().unwrap().len(), 1));
    }

    #[gpui::test]
    fn review_read_callbacks_and_undo_survive_account_switch(cx: &mut TestAppContext) {
        for (completion, before_switch) in [(None, false), (Some(false), false), (Some(true), false), (Some(true), true)] {
            let (view, cx, mut rx) = window(cx);
            let (source, other) = view.update(cx, |v, cx| {
                let source = incoming(v, 1);
                let live = v.live.as_ref().unwrap();
                live.store.set_flags(source.folder, source.uid, true, true).unwrap();
                let mut other = live.accounts[0].clone();
                other.id = "other-read-account".into();
                other.email = "other@example.com".into();
                live.store.save_account(&other).unwrap();
                let folder = live.store.save_folder(&other.id, "INBOX", None, lagom_core::store::FolderRole::Inbox).unwrap();
                let other = live.store.save_message(&lagom_core::store::Message {
                    summary: lagom_core::store::MessageSummary { account: other.id, folder, uid: 1, seen: true, ..Default::default() },
                    ..Default::default()
                }).unwrap();
                v.inbox = Inbox::live();
                v.reload(cx);
                v.toggle_read(cx);
                (source, other)
            });
            let Cmd::Op { op, id, act: Act::Read(false) } = rx.try_recv().unwrap() else { panic!("missing unread command") };
            assert_eq!(id, source.id);
            view.update_in(cx, |v, window, cx| {
                if completion == Some(true) && before_switch {
                    v.live.as_ref().unwrap().store.set_flags(source.folder, source.uid, false, true).unwrap();
                    v.on_events(vec![Event::Op { op, error: None }], window, cx);
                }
                v.switch_account(1, cx);
                if completion.is_some() && !before_switch {
                    if completion == Some(true) {
                        v.live.as_ref().unwrap().store.set_flags(source.folder, source.uid, false, true).unwrap();
                    }
                    v.on_events(vec![Event::Op { op, error: (completion == Some(false)).then(|| "fixture refusal".into()) }], window, cx);
                } else if completion.is_none() {
                    v.worker_gone(cx);
                    v.reload(cx);
                }
                v.undo(cx);
                assert!(v.inbox.history.is_empty());
                assert!(v.inbox.batches.is_empty());
                assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(other));
                assert!(!v.inbox.mail[v.inbox.current().unwrap()].unread);
                let store = &v.live.as_ref().unwrap().store;
                assert!(store.message_summary(other).unwrap().seen);
                assert_eq!(store.message_summary(source.id).unwrap().seen, completion != Some(true));
            });
            assert!(rx.try_recv().is_err(), "Undo after switching accounts must not queue a read change");
        }
    }

    #[gpui::test]
    fn review_two_failed_toggles_preserve_confirmed_read_state_after_navigation(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let source = view.update(cx, |v, cx| {
            let source = incoming(v, 1);
            let store = &v.live.as_ref().unwrap().store;
            store.set_flags(source.folder, source.uid, true, true).unwrap();
            store.save_folder(&source.account, "Archive", None, lagom_core::store::FolderRole::Archive).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.toggle_read(cx);
            v.toggle_read(cx);
            v.open_kind(Kind::Done, cx);
            source
        });
        for seen in [false, true] {
            let Cmd::Op { op, id, act: Act::Read(value) } = rx.try_recv().unwrap() else { panic!("missing read command") };
            assert_eq!((id, value), (source.id, seen));
            view.update_in(cx, |v, window, cx| {
                v.on_events(vec![Event::Op { op, error: Some("fixture refusal".into()) }, Event::Changed], window, cx);
                let m = v.live.as_ref().unwrap().store.message_summary(source.id).unwrap();
                assert!(m.seen && m.flagged, "failed requests must retain the last server-confirmed flags");
            });
        }
        view.update(cx, |v, cx| {
            v.open_box(None, cx);
            assert!(!v.inbox.mail[v.inbox.current().unwrap()].unread);
            assert!(v.inbox.history.is_empty());
        });
    }

    #[gpui::test]
    fn review_explicit_composition_preserves_saved_reply(cx: &mut TestAppContext) {
        for keys in ["f", "shift-r"] {
            let (view, cx, mut rx) = window(cx);
            let source = view.update(cx, |v, cx| {
                let first = incoming(v, 1);
                let source = incoming(v, 2);
                let store = &v.live.as_ref().unwrap().store;
                let mut message = store.message(source.id).unwrap();
                message.summary.subject = "Selected message".into();
                message.summary.from_address = "sender@example.com".into();
                message.summary.to = "me@lagom.test, team@example.com".into();
                message.summary.cc = "copy@example.com".into();
                message.body_text = "Selected body".into();
                store.save_message(&message).unwrap();
                v.inbox = Inbox::live();
                v.reload(cx);
                v.select_found(first.id, cx);
                source.id
            });
            cx.simulate_keystrokes("r");
            cx.simulate_input("Saved reply");
            cx.simulate_keystrokes("escape");
            let saved = view.read_with(cx, |v, cx| v.drafts[0].read(cx).outgoing(cx));
            view.update(cx, |v, cx| v.select_found(source, cx));
            while rx.try_recv().is_ok() {}
            cx.simulate_keystrokes(keys);
            view.read_with(cx, |v, cx| {
                assert_eq!(v.drafts.len(), 1, "an explicit composition must leave the saved reply intact");
                assert_eq!(v.drafts[0].read(cx).outgoing(cx), saved);
                let mail = v.compose.as_ref().unwrap().read(cx).outgoing(cx);
                assert_ne!(mail.message_id, saved.message_id);
                assert!(mail.text.contains("Selected body"));
                if keys == "f" {
                    assert_eq!(mail.subject, "Fwd: Selected message");
                    assert!(mail.to.is_empty() && mail.cc.is_empty());
                    assert!(mail.in_reply_to.is_none());
                } else {
                    assert_eq!(mail.subject, "Re: Selected message");
                    assert_eq!(mail.to, ["sender@example.com", "team@example.com"]);
                    assert_eq!(mail.cc, ["copy@example.com"]);
                }
            });
            if keys == "f" {
                let Cmd::ForwardAttachments { draft, id } = rx.try_recv().unwrap() else { panic!("missing forward download") };
                assert_eq!(id, source);
                view.update_in(cx, |v, window, cx| {
                    let parts = vec![lagom_core::smtp::OutgoingAttachment { filename: "selected.bin".into(), mime_type: "application/octet-stream".into(), data: vec![1, 2] }];
                    v.on_events(vec![Event::ForwardAttachments { draft, result: Ok(parts) }], window, cx);
                    assert_eq!(v.compose.as_ref().unwrap().read(cx).outgoing(cx).attachments[0].data, [1, 2]);
                });
            }
        }
    }

    #[gpui::test]
    fn review_forward_completion_saves_in_background_under_write_lock(cx: &mut TestAppContext) {
        for closed in [false, true] {
            let (view, cx, mut rx) = window(cx);
            view.update(cx, |v, cx| { incoming(v, 1); v.inbox = Inbox::live(); v.reload(cx); });
            cx.simulate_keystrokes("f");
            let draft = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|cmd| match cmd {
                Cmd::ForwardAttachments { draft, .. } => Some(draft), _ => None,
            }).unwrap();
            if closed { cx.simulate_keystrokes("escape"); }
            let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
            let lock = rusqlite::Connection::open(store.path()).unwrap();
            lock.execute_batch("BEGIN IMMEDIATE").unwrap();
            let data = vec![127; 2 * 1024 * 1024];
            let parts = vec![lagom_core::smtp::OutgoingAttachment { filename: "large.bin".into(), mime_type: "application/octet-stream".into(), data: data.clone() }];
            let started = Instant::now();
            view.update_in(cx, |v, window, cx| v.on_events(vec![Event::ForwardAttachments { draft: draft.clone(), result: Ok(parts) }], window, cx));
            let elapsed = started.elapsed();
            lock.execute_batch("ROLLBACK").unwrap();
            assert!(elapsed < Duration::from_secs(1), "completion blocked on a SQLite writer for {elapsed:?}");
            cx.run_until_parked();
            let saved = store.drafts().unwrap().into_iter().find(|d| d.id == draft).unwrap();
            assert!(saved.forward_source.is_none());
            assert_eq!(saved.attachments[0].data, data);
        }
    }

    #[gpui::test]
    fn unread_toggle_queues_flags_updates_store_and_undo_survives_render(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let store_id = view.update(cx, |v, cx| {
            let m = incoming(v, 1);
            v.live.as_ref().unwrap().store.set_flags(m.folder, m.uid, true, true).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.open_current(true, cx);
            m.id
        });
        cx.simulate_keystrokes("shift-u");
        let op = match rx.try_recv().unwrap() {
            Cmd::Op { op, id, act: Act::Read(false) } => { assert_eq!(id, store_id); op }
            _ => panic!("missing unread command"),
        };
        view.update_in(cx, |v, window, cx| {
            assert!(v.live.as_ref().unwrap().store.message_summary(store_id).unwrap().seen);
            let m = v.live.as_ref().unwrap().store.message_summary(store_id).unwrap();
            v.live.as_ref().unwrap().store.set_flags(m.folder, m.uid, false, m.flagged).unwrap();
            v.open_current(true, cx);
            assert!(v.inbox.mail[v.inbox.current().unwrap()].unread);
            v.on_events(vec![Event::Op { op, error: None }, Event::Changed], window, cx);
            v.open_current(true, cx);
            assert!(v.inbox.mail[v.inbox.current().unwrap()].unread);
        });
        assert!(rx.try_recv().is_err(), "redraw must not mark it read again");
        cx.simulate_keystrokes("secondary-z");
        assert!(matches!(rx.try_recv().unwrap(), Cmd::Op { id, act: Act::Read(true), .. } if id == store_id));
        view.read_with(cx, |v, _| {
            let m = v.live.as_ref().unwrap().store.message_summary(store_id).unwrap();
            assert!(!m.seen && m.flagged);
        });
        view.update(cx, |v, cx| { v.toggle_read(cx); v.inbox.keep_unread.clear(); v.open_message(v.inbox.current().unwrap(), true, cx); });
        assert!(matches!(rx.try_recv().unwrap(), Cmd::Op { act: Act::Read(false), .. }));
        assert!(matches!(rx.try_recv().unwrap(), Cmd::MarkSeen(id) if id == store_id));
    }

    #[gpui::test]
    fn forward_signature_attachments_and_draft_reopen(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let source = view.update(cx, |v, cx| {
            let m = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            let mut original = live.store.message(m.id).unwrap();
            original.summary.subject = "Files".into();
            original.summary.from_address = "sender@example.com".into();
            original.summary.to = "me@lagom.test, colleague@example.com".into();
            original.summary.cc = "copy@example.com".into();
            original.body_text = "Original body".into();
            live.store.save_message(&original).unwrap();
            live.store.set_signature(&m.account, "**Team**").unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            m.id
        });
        cx.simulate_keystrokes("f");
        let draft = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|cmd| match cmd {
            Cmd::ForwardAttachments { draft, id } => { assert_eq!(id, source); Some(draft) }, _ => None,
        }).unwrap();
        cx.simulate_input("recipient@example.com");
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| v.compose.is_some()), "pending attachments block Send");
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::ForwardAttachments { draft: draft.clone(), result: Err("fixture failure".into()) }], window, cx);
            assert!(v.live.as_ref().unwrap().store.due_outbox(&v.live.as_ref().unwrap().accounts[0].id, i64::MAX).unwrap().is_empty());
            let parts = vec![lagom_core::smtp::OutgoingAttachment { filename: "data.bin".into(), mime_type: "application/octet-stream".into(), data: vec![0, 255] }];
            v.on_events(vec![Event::ForwardAttachments { draft: draft.clone(), result: Ok(parts) }], window, cx);
        });
        let original = view.read_with(cx, |v, cx| {
            let outgoing = v.compose.as_ref().unwrap().read(cx).outgoing(cx);
            assert_eq!(outgoing.subject, "Fwd: Files");
            assert_eq!(outgoing.text.matches("-- \n**Team**").count(), 1);
            assert!(outgoing.text.contains("> Original body"));
            assert!(outgoing.in_reply_to.is_none());
            assert_eq!(outgoing.attachments[0].data, [0, 255]);
            outgoing
        });
        cx.simulate_keystrokes("escape");
        view.update_in(cx, |v, window, cx| { v.drafts.clear(); v.restore_drafts(window, cx); });
        cx.simulate_keystrokes("c");
        assert_eq!(view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).outgoing(cx)), original);
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        view.read_with(cx, |v, _| assert_eq!(v.live.as_ref().unwrap().store.pending(&draft).unwrap().mail, original));
    }

    #[gpui::test]
    fn review_signature_fields_save_each_account_on_blur(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let (store, first, second) = view.update(cx, |v, cx| {
            let live = v.live.as_mut().unwrap();
            let first = live.accounts[0].id.clone();
            let config = live.accounts[0].imap.clone();
            let other = Account::new("other@example.com".into(), config.clone(), config).unwrap();
            live.store.save_account(&other).unwrap();
            live.store.set_signature(&first, "First").unwrap();
            live.store.set_signature(&other.id, "Second").unwrap();
            live.accounts.push(other.clone());
            let store = live.store.clone();
            v.open_settings(cx);
            (store, first, other.id)
        });
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        for id in [&first, &second] {
            assert!(cx.debug_bounds(format!("signature-{id}").leak()).is_some());
        }
        view.update_in(cx, |v, window, cx| window.focus(&v.signatures[&first].read(cx).focus_handle(cx)));
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("First edited");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("Team");
        assert_eq!(store.signature(&first).unwrap(), "First");
        view.update_in(cx, |v, window, cx| window.focus(&v.signatures[&second].read(cx).focus_handle(cx)));
        cx.run_until_parked();
        assert_eq!(store.signature(&first).unwrap(), "First edited\nTeam");
        assert_eq!(store.signature(&second).unwrap(), "Second");
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("Second edited");
        assert_eq!(store.signature(&second).unwrap(), "Second");
        view.update_in(cx, |v, window, _| window.focus(&v.focus));
        cx.run_until_parked();
        assert_eq!(store.signature(&first).unwrap(), "First edited\nTeam");
        assert_eq!(store.signature(&second).unwrap(), "Second edited");
        view.update_in(cx, |v, window, cx| {
            v.signatures.clear();
            v.prepare_signatures(window, cx);
            assert_eq!(v.signatures[&first].read(cx).text(), "First edited\nTeam");
            assert_eq!(v.signatures[&second].read(cx).text(), "Second edited");
        });
    }

    #[gpui::test]
    fn signature_settings_and_reply_all_use_the_message_account(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            let m = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            let mut message = live.store.message(m.id).unwrap();
            message.summary.from_address = "sender@example.com".into();
            message.summary.to = "ME@lagom.test, colleague@example.com, sender@example.com".into();
            message.summary.cc = "COLLEAGUE@example.com, copy@example.com".into();
            live.store.save_message(&message).unwrap();
            let config = live.accounts[0].imap.clone();
            let other = Account::new("other@example.com".into(), config.clone(), config).unwrap();
            live.store.save_account(&other).unwrap();
            live.store.set_signature(&other.id, "Other signature").unwrap();
            v.open_settings(cx);
            v.prepare_signatures(window, cx);
            window.focus(&v.signatures[&m.account].read(cx).focus_handle(cx));
            v.signatures[&m.account].update(cx, |input, cx| input.set_text("Team", cx));
        });
        cx.run_until_parked();
        view.update_in(cx, |v, window, _| {
            let live = v.live.as_ref().unwrap();
            assert_eq!(live.store.signature(&live.accounts[0].id).unwrap(), "");
            window.focus(&v.focus);
        });
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_ref().unwrap();
            assert_eq!(live.store.signature(&live.accounts[0].id).unwrap(), "Team");
            v.page = Page::Mail;
            v.inbox = Inbox::live();
            v.reload(cx);
            let live = v.live.as_mut().unwrap();
            live.current = live.accounts.iter().position(|a| a.email == "other@example.com").unwrap();
            window.focus(&v.focus);
        });
        cx.simulate_keystrokes("shift-r");
        view.read_with(cx, |v, cx| {
            assert_eq!(v.compose.as_ref().unwrap().read(cx).account_id(), Some(v.live.as_ref().unwrap().accounts[0].id.as_str()));
            let mail = v.compose.as_ref().unwrap().read(cx).outgoing(cx);
            assert_eq!(mail.to, ["sender@example.com", "colleague@example.com"]);
            assert_eq!(mail.cc, ["copy@example.com"]);
            assert!(mail.text.starts_with("\n\n-- \nTeam\n\n> On "));
        });
    }

    #[gpui::test]
    fn review_followup_undo_rebinds_selection_after_store_ids_change(cx: &mut TestAppContext) {
        for group in [false, true] {
            for count in [1, 2] {
                let (view, cx, mut rx) = window(cx);
                view.update_in(cx, |v, window, cx| {
                    let store = v.live.as_ref().unwrap().store.clone();
                    let account = v.live.as_ref().unwrap().accounts[0].id.clone();
                    let archive = store.save_folder(&account, "Archive", None, lagom_core::store::FolderRole::Archive).unwrap();
                    for uid in 1..=count {
                        let mut m = incoming(v, uid);
                        m.message_id = format!("message-{uid}@test");
                        m.subject = format!("Message {uid}");
                        store.save_message(&lagom_core::store::Message { summary: m, body_text: "Restored body".into(), ..Default::default() }).unwrap();
                    }
                    v.panes.group_conversations = group;
                    v.inbox = Inbox::live();
                    v.reload(cx);
                    v.inbox.select_all();
                    v.act(Move::Done, cx);
                    let mut moved = Vec::new();
                    let mut events = Vec::new();
                    while let Ok(cmd) = rx.try_recv() {
                        if let Cmd::Op { op, id, act: Act::Archive } = cmd {
                            let mut mail = store.message(id).unwrap();
                            let source = mail.summary.folder;
                            mail.summary.folder = archive;
                            let new = store.save_message(&mail).unwrap();
                            assert_ne!(new, id);
                            store.delete_message(id).unwrap();
                            moved.push((id, new, source));
                            events.push(Event::Op { op, error: None });
                        }
                    }
                    assert_eq!(moved.len(), count as usize);
                    v.on_events(events, window, cx);
                    assert!(v.inbox.list().is_empty());
                    v.undo(cx);
                    assert_eq!(v.inbox.selected_members().len(), count as usize);
                    let mut restored = HashSet::new();
                    let mut events = Vec::new();
                    while let Ok(cmd) = rx.try_recv() {
                        if let Cmd::Op { op, id, act: Act::Move(folder) } = cmd {
                            let (_, current, source) = moved.iter().find(|m| m.0 == id).unwrap();
                            assert_eq!(folder, *source);
                            let mut mail = store.message(*current).unwrap();
                            mail.summary.folder = folder;
                            mail.summary.uid += 100;
                            let new = store.save_message(&mail).unwrap();
                            assert_ne!(new, id);
                            store.delete_message(*current).unwrap();
                            restored.insert(new);
                            events.push(Event::Op { op, error: None });
                        }
                    }
                    assert_eq!(restored.len(), count as usize);
                    v.live.as_mut().unwrap().limit = 1;
                    v.on_events(events, window, cx);
                    let selected: HashSet<_> = v.inbox.selected_members().iter().map(|&id| v.inbox.mail[id].store.unwrap()).collect();
                    assert_eq!(selected, restored, "Undo must retain the selection through the completion reload");
                    v.open_current(false, cx);
                    if count == 1 {
                        assert!(v.inbox.single().is_some());
                        assert_eq!(v.reading_members.len(), 1);
                        assert_eq!(v.inbox.mail[v.inbox.single().unwrap()].body, ["Restored body"]);
                    }
                    v.act(Move::Delete, cx);
                    let deleted: HashSet<_> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|cmd| match cmd {
                        Cmd::Op { id, act: Act::Trash, .. } => Some(id), _ => None,
                    }).collect();
                    assert_eq!(deleted, restored);
                });
            }
        }
    }

    #[gpui::test]
    fn review_followup_html_task_uses_sole_selection(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            for uid in 1..=2 {
                let mut m = incoming(v, uid);
                m.message_id = format!("message-{uid}@test");
                m.subject = format!("Message {uid}");
                store.save_message(&lagom_core::store::Message { summary: m, body_html: Some("<p>Body</p>".into()), ..Default::default() }).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select(0);
            let selected = v.inbox.current().unwrap();
            v.inbox.click(1, false, true);
            v.inbox.click(1, false, true);
            assert_ne!(v.inbox.current(), Some(selected));
            v.open_current(false, cx);
            assert_eq!(v.reading_thread, Some(selected));
            html::WebHost::new_task(v, cx);
            let tasks = store.tasks(store.default_task_list().unwrap(), false).unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].mail_message_id.as_deref(), Some(v.inbox.mail[selected].message_id.as_str()));
            assert_eq!(tasks[0].title, v.inbox.mail[selected].subject);
        });
    }

    #[gpui::test]
    fn review_followup_popout_reveals_read_message_before_acting(cx: &mut TestAppContext) {
        for what in [Move::Done, Move::Later, Move::Delete] {
            let (view, cx, mut rx) = window(cx);
            let target = view.update_in(cx, |v, window, cx| {
                let mut m = incoming(v, 1);
                m.seen = true;
                v.live.as_ref().unwrap().store.save_message(&lagom_core::store::Message { summary: m.clone(), ..Default::default() }).unwrap();
                v.inbox = Inbox::live();
                v.reload(cx);
                v.open_popout(window, cx);
                v.filter_unread(true, cx);
                assert!(v.inbox.list().is_empty());
                assert_eq!(v.inbox.raw_list().len(), 1);
                m.id
            });
            while rx.try_recv().is_ok() {}
            let popout = view.read_with(cx, |v, _| v.popouts[0].1);
            popout.update(cx, |p, _, cx| p.act(what, cx)).unwrap();
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|cmd| match cmd {
                Cmd::Op { id, act, .. } => Some((id, act)), _ => None,
            }).collect();
            assert_eq!(commands.len(), 1, "{what:?} must reach the worker");
            assert_eq!(commands[0].0, target);
            assert!(matches!((what, &commands[0].1), (Move::Done, Act::Archive) | (Move::Later, Act::Snooze(Some(_))) | (Move::Delete, Act::Trash)));
            assert!(!toast(&view, cx).contains("not in a loaded list"));
        }
    }

    #[gpui::test]
    fn review_failed_read_preserves_later_delete_undo(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            for uid in 1..=2 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            v.toggle_read(cx);
            let reads = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
            v.act(Move::Delete, cx);
            let deletes = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
            for cmd in reads {
                let Cmd::Op { op, .. } = cmd else { panic!("expected read") };
                v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx);
            }
            assert_eq!(v.inbox.batches.len(), 1);
            assert!(v.inbox.history.iter().all(|h| h.0 == Move::Delete));
            for cmd in deletes {
                let Cmd::Op { op, .. } = cmd else { panic!("expected delete") };
                v.on_events(vec![Event::Op { op, error: None }], window, cx);
            }
            v.undo(cx);
            assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|cmd| matches!(cmd, Cmd::Op { act: Act::Move(_), .. })).count(), 2);
        });
    }

    #[gpui::test]
    fn review_old_read_failure_cannot_revert_a_new_read(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            incoming(v, 1);
            v.inbox = Inbox::live();
            v.reload(cx);
            v.toggle_read(cx);
            let Cmd::Op { op, .. } = rx.try_recv().unwrap() else { panic!("expected read") };
            v.undo(cx);
            v.toggle_read(cx);
            v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx);
            assert!(!v.inbox.mail[0].unread);
            assert_eq!(v.inbox.batches.len(), 1);
            assert_eq!(v.inbox.history.len(), 1);
            v.undo(cx);
            assert!(v.inbox.mail[0].unread);
        });
    }

    #[gpui::test]
    fn review_stopped_worker_reverts_read_and_delete_in_reverse_order(cx: &mut TestAppContext) {
        let (view, cx, rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=2 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            v.toggle_read(cx);
            v.act(Move::Delete, cx);
        });
        drop(rx);
        view.update(cx, |v, cx| {
            v.worker_gone(cx);
            assert!(v.inbox.history.is_empty() && v.inbox.batches.is_empty());
            assert_eq!(v.inbox.items.len(), 2);
            assert!(v.inbox.mail.iter().all(|m| m.unread));
        });
    }

    #[gpui::test]
    fn review_sync_keeps_selection_past_page_boundary(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=200 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            let selected = v.inbox.selected_members();
            incoming(v, 201);
            v.reload(cx);
            assert_eq!(v.inbox.selected_members(), selected);
            assert_eq!(v.inbox.list().len(), 201);
            v.act(Move::Done, cx);
            assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|cmd| matches!(cmd, Cmd::Op { act: Act::Archive, .. })).count(), 200);
        });
    }

    #[gpui::test]
    fn review_sync_drops_selected_messages_that_leave_the_list(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let removed = incoming(v, 1);
            let snoozed = incoming(v, 2);
            incoming(v, 3);
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            let live = v.live.as_ref().unwrap();
            live.store.delete_message(removed.id).unwrap();
            live.store.snooze(snoozed.id, Some(now() + 3600)).unwrap();
            v.reload(cx);
            assert_eq!(v.inbox.selected_members().len(), 1);
            assert!(v.inbox.selected_members().iter().all(|&id| ![removed.id, snoozed.id].contains(&v.inbox.mail[id].store.unwrap())));
        });
    }

    #[gpui::test]
    fn review_search_reveals_read_target_under_unread_filter(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            let mut target = incoming(v, 1);
            target.seen = true;
            v.live.as_ref().unwrap().store.save_message(&lagom_core::store::Message { summary: target.clone(), ..Default::default() }).unwrap();
            incoming(v, 2);
            v.inbox = Inbox::live();
            v.reload(cx);
            v.filter_unread(true, cx);
            v.open_found(target.id, cx);
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(target.id));
            v.act(Move::Delete, cx);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Op { id, act: Act::Trash, .. }) if id == target.id));
        });
    }

    #[gpui::test]
    fn review_trash_does_not_inherit_hidden_unread_filter(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let mut target = incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            target.folder = live.store.save_folder(&target.account, "Trash", None, lagom_core::store::FolderRole::Trash).unwrap();
            target.seen = true;
            let target = live.store.save_message(&lagom_core::store::Message { summary: target, ..Default::default() }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.filter_unread(true, cx);
            v.open_box(v.inbox.find_kind(Kind::Trash), cx);
            assert!(!v.inbox.unread_only);
            assert!(v.inbox.list().iter().any(|&id| v.inbox.mail[id].store == Some(target)));
        });
    }

    #[gpui::test]
    fn review_cmd_toggle_reads_sole_selection_not_cursor(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=2 {
                let summary = incoming(v, uid);
                v.live.as_ref().unwrap().store.save_message(&lagom_core::store::Message {
                    summary, body_text: format!("Body {uid}"), body_html: Some(format!("<p>Body {uid}</p>")), ..Default::default()
                }).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select(0);
            let selected = v.inbox.current().unwrap();
            v.inbox.click(1, false, true);
            v.inbox.click(1, false, true);
            assert_ne!(v.inbox.current(), Some(selected));
            v.open_current(false, cx);
            assert_eq!(v.reading_thread, Some(selected));
            assert_eq!(v.inbox.mail[selected].body, ["Body 2"]);
            assert_eq!(v.inbox.mail[selected].html.as_ref().map(|html| html.as_ref()), Some("<p>Body 2</p>"));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("Reply").is_some());
    }

    #[gpui::test]
    fn mail_sync_status_has_distinct_embedded_icons(cx: &mut TestAppContext) {
        use gpui::AssetSource;
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, _| {
            assert_eq!(v.sync_icon(), "icons/cloud-syncing.svg");
            for (state, path) in [(SyncState::Synced(0), "icons/cloud-synced.svg"),
                (SyncState::Failed("fixture".into()), "icons/cloud-failed.svg"),
                (SyncState::ReauthRequired, "icons/cloud-failed.svg"),
                (SyncState::SigningIn, "icons/cloud-syncing.svg"), (SyncState::Syncing, "icons/cloud-syncing.svg")] {
                let live = v.live.as_mut().unwrap();
                live.sync.insert(live.accounts[0].id.clone(), state);
                assert_eq!(v.sync_icon(), path);
                assert!(crate::Assets.load(path).unwrap().is_some());
            }
        });
    }

    #[gpui::test]
    fn unread_pages_remain_reachable_after_filter_hides_loaded_rows(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=205 {
                let mut m = incoming(v, uid);
                m.seen = uid > 2;
                m.subject = format!("Message {uid}");
                v.live.as_ref().unwrap().store.save_message(&lagom_core::store::Message { summary: m, ..Default::default() }).unwrap();
            }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.filter_unread(true, cx);
            assert!(v.inbox.list().is_empty());
            assert!(v.has_more());
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("Load more").unwrap().center();
        cx.simulate_click(at, Default::default());
        view.read_with(cx, |v, _| {
            assert_eq!(v.inbox.list().len(), 2);
            assert!(!v.has_more());
            assert!(v.inbox.list().iter().all(|&id| v.inbox.mail[id].unread));
        });
    }

    #[gpui::test]
    fn multi_selection_queues_actions_and_one_undo_for_all_rows(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            for uid in 1..=3 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            v.open_current(true, cx);
            assert!(v.reading_members.is_empty());
            assert!(v.inbox.selected_members().iter().all(|&id| v.inbox.mail[id].unread));
        });
        assert!(rx.try_recv().is_err(), "the summary must not load or mark one message read");
        view.update(cx, |v, cx| v.act(Move::Done, cx));
        let moved = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
            Cmd::Op { id, act: Act::Archive, .. } => id,
            _ => panic!("expected archive"),
        }).collect::<Vec<_>>();
        assert_eq!(moved.len(), 3);
        assert!(view.read_with(cx, |v, _| v.inbox.list().is_empty()));
        view.update(cx, |v, cx| v.undo(cx));
        let restored = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
            Cmd::Op { id, act: Act::Move(_), .. } => id,
            _ => panic!("expected restore"),
        }).collect::<Vec<_>>();
        assert_eq!(restored, moved);
        assert_eq!(view.read_with(cx, |v, _| v.inbox.list().len()), 3);
        assert!(view.read_with(cx, |v, _| v.inbox.history.is_empty() && v.inbox.batches.is_empty()));
    }

    #[gpui::test]
    fn manual_read_flags_undo_failure_and_stopped_worker(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let ids = view.update(cx, |v, cx| {
            for uid in 1..=3 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            v.inbox.mail[1].unread = false;
            v.toggle_read(cx);
            assert!(v.inbox.mail.iter().all(|m| !m.unread));
            v.inbox.mail.iter().map(|m| m.store.unwrap()).collect::<Vec<_>>()
        });
        let marked = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
            Cmd::Op { id, act: Act::Read(true), .. } => id,
            _ => panic!("expected mark read"),
        }).collect::<Vec<_>>();
        assert_eq!(marked, [ids[0], ids[2]]);
        view.update(cx, |v, cx| v.undo(cx));
        let restored = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
            Cmd::Op { id, act: Act::Read(false), .. } => id,
            _ => panic!("expected mark unread"),
        }).collect::<Vec<_>>();
        assert_eq!(restored, marked);
        view.update_in(cx, |v, window, cx| {
            v.inbox.select_all();
            v.toggle_read(cx);
            let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
            let Cmd::Op { op, .. } = commands[0] else { panic!("expected operation") };
            v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx);
            assert!(v.inbox.mail[0].unread);
            assert!(!v.inbox.mail[2].unread);
            v.undo(cx);
            assert!(v.inbox.mail[2].unread);
        });
        drop(rx);
        view.update(cx, |v, cx| {
            v.inbox.select_all();
            v.toggle_read(cx);
            assert!(v.inbox.mail[0].unread && v.inbox.mail[2].unread);
            assert!(!v.inbox.mail[1].unread);
        });
    }

    #[gpui::test]
    fn calendar_updates_preserve_unread_selection_and_pending_batch(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            for uid in 1..=3 { incoming(v, uid); }
            v.inbox = Inbox::live();
            v.reload(cx);
            v.filter_unread(true, cx);
            v.inbox.select_all();
            let selected = v.inbox.selected_members();
            let account = v.live.as_ref().unwrap().accounts[0].id.clone();
            v.on_events(vec![Event::CalendarSync(account.clone(), SyncState::Synced(0)), Event::Changed], window, cx);
            assert!(v.inbox.unread_only);
            assert_eq!(v.inbox.selected_members(), selected);
            v.toggle_read(cx);
            let marked: HashSet<_> = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
                Cmd::Op { id, act: Act::Read(true), .. } => id,
                _ => panic!("expected mark read"),
            }).collect();
            assert_eq!(marked.len(), 3);
            v.on_events(vec![Event::CalendarSync(account, SyncState::Synced(1)), Event::Changed], window, cx);
            assert!(v.inbox.list().is_empty());
            assert_eq!(v.inbox.batches.len(), 1);
            v.undo(cx);
            assert_eq!(v.inbox.selected_members(), selected);
            let restored: HashSet<_> = std::iter::from_fn(|| rx.try_recv().ok()).map(|cmd| match cmd {
                Cmd::Op { id, act: Act::Read(false), .. } => id,
                _ => panic!("expected mark unread"),
            }).collect();
            assert_eq!(restored, marked);
        });
    }

    #[gpui::test]
    fn task_edits_notify_sync_after_commit_and_status_refresh_keeps_local_tasks(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let id = view.update_in(cx, |v, window, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut account = store.accounts().unwrap().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            store.save_authorized_account(&account, lagom_core::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
            v.reload(cx);
            v.show_section(3, window, cx);
            let list = store.default_task_list().unwrap();
            assert!(v.save_task(super::tasks::TaskChange::Add { list, title: "Durable before sync".into() }, cx));
            assert_eq!(store.tasks(list, false).unwrap()[0].title, "Durable before sync");
            assert!(matches!(rx.try_recv().unwrap(), Cmd::TasksEdited));
            v.on_events(vec![Event::TasksSync(account.id.clone(), SyncState::Synced(1)), Event::Changed], window, cx);
            assert_eq!(v.today.task_sync.get(&account.id), Some(&SyncState::Synced(1)));
            assert_eq!(store.tasks(list, false).unwrap().len(), 1);
            v.on_events(vec![Event::TasksSync(account.id.clone(), SyncState::Failed("offline".into()))], window, cx);
            assert_eq!(v.today.task_sync.get(&account.id), Some(&SyncState::Failed("offline".into())));
            account.id
        });
        cx.run_until_parked();
        let selector: &'static str = Box::leak(format!("tasks-sync-{id}").into_boxed_str());
        assert!(cx.debug_bounds(selector).is_some(), "Tasks status belongs on Todo");
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Home)));
        // GPUI retains debug selectors across frames; check absence in a fresh test window.
        let home = cx.cx.update(|cx| cx.open_window(Default::default(), |_, _| view.clone()).unwrap());
        let mut home = VisualTestContext::from_window(home.into(), cx);
        home.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(home.debug_bounds("home-tasks").is_some());
        assert!(home.debug_bounds(selector).is_none(), "Home keeps its calendar status");
    }

    #[gpui::test]
    fn synced_task_completion_keeps_keyboard_selection_visible(cx: &mut TestAppContext) {
        for (remaining, expanded) in [(true, false), (false, false), (true, true)] {
            let (view, cx, _rx) = window(cx);
            let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
            let list = store.default_task_list().unwrap();
            let fallback = remaining.then(|| store.add_task(list, "Still open", "", None).unwrap());
            let task = store.add_task(list, "Completed elsewhere", "", None).unwrap();
            if expanded { store.set_done(store.add_task(list, "Already done", "", None).unwrap().id, true).unwrap(); }
            view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
            cx.run_until_parked();
            if expanded {
                let at = cx.debug_bounds(format!("completed-{list}").leak()).unwrap().center();
                cx.simulate_click(at, Default::default());
            }
            let row = format!("task-{}", task.id).leak();
            let at = cx.debug_bounds(row).unwrap().center();
            cx.simulate_click(at, Default::default());
            let worker_store = Store::open(store.path()).unwrap();
            worker_store.set_done(task.id, true).unwrap();
            view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Changed], window, cx));
            cx.run_until_parked();
            cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
            view.read_with(cx, |v, _| assert!(v.today.tasks.iter().find(|t| t.id == task.id).unwrap().done_at.is_some()));
            cx.simulate_keystrokes("enter");
            if remaining {
                cx.simulate_keystrokes("secondary-a");
                cx.simulate_input("Renamed visible task");
                cx.simulate_keystrokes("enter");
                let expected = if expanded { task.id } else { fallback.unwrap().id };
                assert_eq!(store.tasks(list, true).unwrap().iter().find(|t| t.id == expected).unwrap().title, "Renamed visible task");
            }
            view.update_in(cx, |v, window, _| assert!(v.focus.is_focused(window), "Enter must not focus a hidden editor"));
            assert!(store.tasks(list, true).unwrap().iter().find(|t| t.id == task.id).unwrap().done_at.is_some());
        }
    }

    #[gpui::test]
    fn review_tasks_scope_loss_offers_consent_with_calendar_still_authorized(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let id = view.update_in(cx, |v, window, cx| {
            let store = &v.live.as_ref().unwrap().store;
            let mut account = store.accounts().unwrap().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            let scopes = lagom_core::oauth::GOOGLE_CALENDAR_SCOPES.split_whitespace()
                .filter(|s| *s != lagom_core::oauth::GOOGLE_TASKS_SCOPE).collect::<Vec<_>>().join(" ");
            store.save_authorized_account(&account, &scopes).unwrap();
            v.reload(cx);
            v.show_section(3, window, cx);
            assert!(v.today.calendar_accounts[0].1);
            account.id
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("allow-calendar-0").is_none());
        let selector: &'static str = Box::leak(format!("allow-tasks-{id}").into_boxed_str());
        let button = cx.debug_bounds(selector).expect("Tasks consent action");
        cx.simulate_click(button.center(), Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::AllowCalendar(account) if account == id));
        cx.run_until_parked();
        let button = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(button.center(), Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::CancelSignIn));
    }

    #[gpui::test]
    fn today_offers_calendar_access_for_existing_google_accounts(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let store = &v.live.as_ref().unwrap().store;
            let mut account = store.accounts().unwrap().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            store.save_account(&account).unwrap();
            store.add_task(store.default_task_list().unwrap(), "Fixture task", "", None).unwrap();
            v.reload(cx);
            v.show_home(window, cx);
        });
        cx.run_until_parked();
        let consent = cx.debug_bounds("Show your Google Calendar").expect("Today must offer calendar access");
        assert!(cx.debug_bounds("home-tasks").is_some());
        assert!(cx.debug_bounds("calendar-heading").is_none(), "unconsented accounts get only a quiet row");
        let button = consent.center();
        cx.simulate_click(button, Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::AllowCalendar(_)));
        view.update_in(cx, |v, window, cx| {
            let store = &v.live.as_ref().unwrap().store;
            let account = store.accounts().unwrap().remove(0);
            store.set_account_scopes(&account.id, lagom_core::oauth::GOOGLE_CALENDAR_SCOPES).unwrap();
            let day = chrono::Local::now().date_naive();
            let start = day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
            store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(lagom_core::calendar::Calendar {
                remote_id: "primary".into(), name: "Fixture calendar".into(), color: None, shown: true, writable: true,
            }, vec![lagom_core::calendar::Event {
                remote_id: "fixture".into(), title: "Fixture event".into(), start: day.to_string(), end: day.succ_opt().unwrap().to_string(),
                start_utc: start, end_utc: start + 86400, all_day: true, location: None, join_url: None, html_link: None,
            }, lagom_core::calendar::Event {
                remote_id: "fixture-next".into(), title: "Next fixture event".into(), start: day.to_string(), end: day.succ_opt().unwrap().to_string(),
                start_utc: start, end_utc: start + 86400, all_day: true, location: None, join_url: None, html_link: None,
            }])]).unwrap();
            v.on_events(vec![Event::CalendarSync(account.id, SyncState::Synced(0)), Event::Changed], window, cx);
            assert_eq!(v.today.events.len(), 2);
            assert!(v.today.calendar_accounts[0].1);
        });
        cx.run_until_parked();
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        // GPUI retains old debug bounds; check that the old consent position no longer dispatches.
        cx.simulate_click(button, Default::default());
        assert!(rx.try_recv().is_err());
        let event = cx.debug_bounds("agenda-event-0").unwrap();
        let groups = cx.debug_bounds("home-tasks").unwrap();
        let next = cx.debug_bounds("agenda-event-1").unwrap();
        assert!(event.bottom() <= next.top(), "events stay in agenda order");
        assert!(event.right() <= groups.left() || next.bottom() <= groups.top(), "columns sit alongside or stack");
    }

    #[gpui::test]
    fn local_tasks_commit_while_mail_operation_is_stalled(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let store = view.update(cx, |v, cx| {
            incoming(v, 1);
            v.inbox = Inbox::live();
            v.reload(cx);
            v.act(Move::Done, cx);
            v.live.as_ref().unwrap().store.clone()
        });
        assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|cmd| matches!(cmd, Cmd::Op { .. })));
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Saved during mail wait");
        cx.simulate_keystrokes("enter");
        let list = store.default_task_list().unwrap();
        assert_eq!(store.tasks(list, false).unwrap().len(), 1);
        cx.simulate_keystrokes("secondary-z");
        assert!(store.tasks(list, true).unwrap().is_empty());
        cx.simulate_keystrokes("t");
        cx.simulate_input("Complete during mail wait");
        cx.simulate_keystrokes("enter space");
        assert!(store.tasks(list, false).unwrap().is_empty());
        assert!(store.tasks(list, true).unwrap()[0].done_at.is_some());
        assert!(view.read_with(cx, |v, _| !v.live.as_ref().unwrap().ops.is_empty()));
        assert!(rx.try_recv().is_err(), "task writes must not use the mail queue");
    }

    #[gpui::test]
    fn local_task_busy_write_returns_quickly_and_keeps_editor_text(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        let fixture = rusqlite::Connection::open(store.path()).unwrap();
        let list = store.default_task_list().unwrap();
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Keep locked text");
        fixture.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let started = std::time::Instant::now();
        cx.simulate_keystrokes("enter");
        let elapsed = started.elapsed();
        assert!(toast(&view, cx).contains("database is locked"));
        assert!(store.tasks(list, true).unwrap().is_empty());
        fixture.execute_batch("ROLLBACK;").unwrap();
        cx.simulate_input(" and retry");
        cx.simulate_keystrokes("enter");
        assert_eq!(store.tasks(list, false).unwrap()[0].title, "Keep locked text and retry");
        assert!(elapsed < Duration::from_millis(750), "task write blocked for {elapsed:?}");

        for key in ["secondary-z", "space"] {
            fixture.execute_batch("BEGIN IMMEDIATE;").unwrap();
            let started = std::time::Instant::now();
            cx.simulate_keystrokes(key);
            let elapsed = started.elapsed();
            assert!(toast(&view, cx).contains("database is locked"));
            assert_eq!(store.tasks(list, false).unwrap().len(), 1);
            fixture.execute_batch("ROLLBACK;").unwrap();
            assert!(elapsed < Duration::from_millis(750), "{key} blocked for {elapsed:?}");
        }
        cx.simulate_keystrokes("space");
        assert!(store.tasks(list, false).unwrap().is_empty());
        assert!(store.tasks(list, true).unwrap()[0].done_at.is_some());
    }

    #[gpui::test]
    fn local_task_failed_write_keeps_editor_text(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        let fixture = rusqlite::Connection::open(store.path()).unwrap();
        fixture.execute_batch("CREATE TRIGGER refuse_task_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'fixture write failed'); END;").unwrap();
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Keep submitted text");
        cx.simulate_keystrokes("enter");
        assert!(toast(&view, cx).contains("fixture write failed"));
        fixture.execute_batch("DROP TRIGGER refuse_task_write;").unwrap();
        cx.simulate_input(" and retry");
        cx.simulate_keystrokes("enter");
        let tasks = store.tasks(store.default_task_list().unwrap(), false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Keep submitted text and retry");
        fixture.execute_batch("CREATE TRIGGER refuse_task_write BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT, 'fixture write failed'); END;").unwrap();
        cx.simulate_keystrokes("secondary-z");
        assert!(toast(&view, cx).contains("fixture write failed"), "failed Undo must keep the error toast");
        assert_eq!(store.tasks(tasks[0].list, false).unwrap().len(), 1);
        cx.simulate_keystrokes("enter secondary-a");
        cx.simulate_input("Keep renamed text");
        cx.simulate_keystrokes("enter");
        assert!(toast(&view, cx).contains("fixture write failed"));
        fixture.execute_batch("DROP TRIGGER refuse_task_write;").unwrap();
        cx.simulate_input(" and retry");
        cx.simulate_keystrokes("enter");
        assert_eq!(store.tasks(tasks[0].list, false).unwrap()[0].title, "Keep renamed text and retry");
    }

    #[gpui::test]
    fn review_task_controls_follow_page_and_visible_tasks(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("new-list").is_none());
        assert!(cx.debug_bounds("task-hint").is_none());
        let task = store.add_task(store.default_task_list().unwrap(), "Fixture task", "", None).unwrap();
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("new-list").is_none());
        assert!(cx.debug_bounds("task-hint").is_none());
        store.set_done(task.id, true).unwrap();
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("new-list").is_some());
        assert!(cx.debug_bounds("task-hint").is_none());
    }

    #[gpui::test]
    fn local_task_list_failed_writes_keep_editor_text(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        let fixture = rusqlite::Connection::open(store.path()).unwrap();
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        for rename in [false, true] {
            if rename {
                cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
                let at = cx.debug_bounds(format!("list-menu-{}", store.task_lists().unwrap().last().unwrap().id).leak()).unwrap().center();
                cx.simulate_click(at, Default::default());
            }
            let selector = if rename { format!("rename-list-{}", store.task_lists().unwrap().last().unwrap().id) } else { "new-list".into() };
            cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
            let at = cx.debug_bounds(selector.leak()).unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_keystrokes("secondary-a");
            cx.simulate_input(if rename { "Renamed list" } else { "New list" });
            let operation = if rename { "UPDATE" } else { "INSERT" };
            fixture.execute_batch(&format!("CREATE TRIGGER refuse_list_write BEFORE {operation} ON task_lists BEGIN SELECT RAISE(ABORT, 'fixture list write failed'); END;")).unwrap();
            cx.simulate_keystrokes("enter");
            assert!(toast(&view, cx).contains("fixture list write failed"));
            fixture.execute_batch("DROP TRIGGER refuse_list_write;").unwrap();
            cx.simulate_input(" and retry");
            cx.simulate_keystrokes("enter");
            let lists = store.task_lists().unwrap();
            assert_eq!(lists.len(), 3);
            assert_eq!(lists.last().unwrap().name, if rename { "Renamed list and retry" } else { "New list and retry" });
        }
    }

    #[gpui::test]
    fn local_task_enter_is_durable_before_quit(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Durable on Enter");
        cx.simulate_keystrokes("enter");
        let path = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.path().to_path_buf());
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.tasks(reopened.default_task_list().unwrap(), false).unwrap().len(), 1);
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.tasks(reopened.default_task_list().unwrap(), false).unwrap()[0].title, "Durable on Enter");
    }

    #[gpui::test]
    fn local_task_closed_window_cannot_consume_new_window_write(cx: &mut TestAppContext) {
        let (old, cx, mut rx) = window(cx);
        old.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Old window task");
        cx.simulate_keystrokes("enter");
        assert!(!old.update(cx, |v, cx| v.hold_exit(cx)));
        let (store, bridge) = old.read_with(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            (live.store.clone(), live.bridge.clone())
        });
        old.update_in(cx, |_, window, _| window.remove_window());
        drop(old);
        let (view, cx) = cx.cx.add_window_view(|window, cx| {
            let mut v = MainWindow::new(true, window, cx);
            v.live = Some(fixture_live(store.clone(), bridge));
            v.show_home(window, cx);
            v
        });
        cx.simulate_keystrokes("t");
        cx.simulate_input("New window task");
        cx.simulate_keystrokes("enter");
        assert!(rx.try_recv().is_err(), "closed windows leave no task acknowledgements");
        view.update_in(cx, |v, window, cx| v.on_events(vec![Event::Changed], window, cx));
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        let reopened = Store::open(store.path()).unwrap();
        let tasks = reopened.tasks(reopened.default_task_list().unwrap(), false).unwrap();
        assert_eq!(tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Old window task", "New window task"]);
    }

    #[gpui::test]
    fn review_task_link_waits_for_failed_move(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let (task, mail) = view.update(cx, |v, cx| {
            let mut mail = incoming(v, 1);
            mail.message_id = "<pending-task@test>".into();
            let store = &v.live.as_ref().unwrap().store;
            store.save_message(&lagom_core::store::Message { summary: mail.clone(), ..Default::default() }).unwrap();
            let task = store.task_from_mail(store.default_task_list().unwrap(), &mail).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.act(Move::Done, cx);
            (task.id, mail.id)
        });
        let op = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|cmd| match cmd {
            Cmd::Op { op, .. } => Some(op), _ => None,
        }).unwrap();
        view.update_in(cx, |v, window, cx| {
            v.show_home(window, cx);
            v.open_task_mail(task, cx);
            assert!(!v.inbox.mail.is_empty(), "pending move still needs its rollback row");
            assert!(matches!(v.page, Page::Home));
            v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx);
            assert!(matches!(v.page, Page::Mail));
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(mail));
        });
    }

    #[gpui::test]
    fn review_quit_commits_submitted_task_add_and_rename(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("t");
        cx.simulate_input("Submitted task");
        cx.simulate_keystrokes("enter");
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        view.read_with(cx, |v, _| {
            let store = Store::open(v.live.as_ref().unwrap().store.path()).unwrap();
            assert_eq!(store.tasks(store.default_task_list().unwrap(), false).unwrap()[0].title, "Submitted task");
        });
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        cx.simulate_keystrokes("enter secondary-a");
        cx.simulate_input("Submitted rename");
        cx.simulate_keystrokes("enter");
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        view.read_with(cx, |v, _| {
            let store = Store::open(v.live.as_ref().unwrap().store.path()).unwrap();
            assert_eq!(store.tasks(store.default_task_list().unwrap(), false).unwrap()[0].title, "Submitted rename");
        });
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        view.read_with(cx, |v, _| {
            let store = Store::open(v.live.as_ref().unwrap().store.path()).unwrap();
            let tasks = store.tasks(store.default_task_list().unwrap(), false).unwrap();
            assert_eq!(tasks.len(), 1, "quit must not repeat a committed write");
            assert_eq!(tasks[0].title, "Submitted rename");
        });
    }

    #[gpui::test]
    fn review_today_undo_cancels_pending_send(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        send_one(cx);
        assert!(matches!(rx.try_recv(), Ok(Cmd::Drafts(_))));
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("secondary-z");
        assert!(view.read_with(cx, |v, _| v.compose.is_some() && v.sending.is_empty()));
        cx.executor().advance_clock(Duration::from_secs(6));
        cx.run_until_parked();
        assert!(rx.try_recv().is_err(), "undone send must never reach delivery");
        view.read_with(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            assert!(live.store.due_outbox(&live.accounts[0].id, i64::MAX).unwrap().is_empty());
            assert_eq!(live.store.drafts().unwrap().len(), 1);
        });
    }

    #[gpui::test]
    fn task_key_creates_link_in_selected_default_and_undo_works_from_today(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (store, mail, default) = view.update(cx, |v, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut mail = incoming(v, 1);
            mail.subject = "Follow up".into();
            mail.message_id = "<task@test>".into();
            store.save_message(&lagom_core::store::Message { summary: mail.clone(), ..Default::default() }).unwrap();
            let default = store.task_lists().unwrap()[1].id;
            v.inbox = Inbox::live();
            v.reload(cx);
            v.save_task(TaskChange::DefaultList(default), cx);
            (store, mail, default)
        });
        cx.simulate_keystrokes("t");
        let tasks = store.tasks(default, false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Follow up");
        assert_eq!(tasks[0].mail_message_id.as_deref(), Some("<task@test>"));
        assert_eq!(tasks[0].mail_account.as_deref(), Some(mail.account.as_str()));
        assert!(view.read_with(cx, |v, _| v.toast.as_ref().is_some_and(|t| t.1)));
        view.update_in(cx, |v, window, cx| v.show_home(window, cx));
        cx.simulate_keystrokes("secondary-z");
        assert!(store.tasks(default, true).unwrap().is_empty());
        assert!(store.tasks_for_mail(&mail.account, &mail.message_id).unwrap().is_empty());
    }

    #[gpui::test]
    fn task_link_finds_moved_mail_in_another_account_and_reports_missing_mail(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let (store, task, moved) = view.update(cx, |v, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut original = incoming(v, 1);
            original.message_id = "<linked@test>".into();
            original.subject = "Linked message".into();
            store.save_message(&lagom_core::store::Message { summary: original.clone(), ..Default::default() }).unwrap();
            let task = store.task_from_mail(store.default_task_list().unwrap(), &original).unwrap();
            store.delete_message(original.id).unwrap();
            original.folder = store.save_folder(&original.account, "Archive", Some("/"), lagom_core::store::FolderRole::Archive).unwrap();
            let moved = store.save_message(&lagom_core::store::Message { summary: original.clone(), ..Default::default() }).unwrap();
            let mut other = v.live.as_ref().unwrap().accounts[0].clone();
            other.id = "other-task-account".into();
            other.email = "other@lagom.test".into();
            store.save_account(&other).unwrap();
            original.account = other.id.clone();
            original.folder = store.save_folder(&other.id, "INBOX", None, lagom_core::store::FolderRole::Inbox).unwrap();
            store.save_message(&lagom_core::store::Message { summary: original, ..Default::default() }).unwrap();
            v.reload(cx);
            v.switch_account(1, cx);
            (store, task, moved)
        });
        view.update_in(cx, |v, window, cx| {
            v.show_home(window, cx);
            v.open_task_mail(task.id, cx);
            assert!(matches!(v.page, Page::Mail));
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(moved));
            assert_eq!(v.live.as_ref().unwrap().account().unwrap().id, task.mail_account.as_deref().unwrap());
            assert_eq!(v.inbox.open_kind(), Some(Kind::Done));
        });
        store.delete_message(moved).unwrap();
        view.update(cx, |v, cx| v.open_task_mail(task.id, cx));
        assert!(toast(&view, cx).contains("no longer available locally"));
        assert_eq!(store.tasks(task.list, true).unwrap(), [task]);
    }

    #[gpui::test]
    fn today_without_accounts_inline_add_rename_reorder_complete_and_palette(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.update_in(cx, |v, window, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            store.delete_account(&v.live.as_ref().unwrap().accounts[0].id).unwrap();
            v.reload(cx);
            v.show_home(window, cx);
            store
        });
        cx.simulate_keystrokes("secondary-k");
        cx.simulate_input("New task");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.simulate_input("First task");
        cx.simulate_keystrokes("enter");
        let first = view.read_with(cx, |v, _| v.task_undo.unwrap());
        cx.simulate_keystrokes("t");
        cx.simulate_input("Second task");
        cx.simulate_keystrokes("enter");
        let second = view.read_with(cx, |v, _| v.task_undo.unwrap());
        let list = store.default_task_list().unwrap();
        assert_eq!(store.tasks(list, false).unwrap().len(), 2);
        cx.simulate_keystrokes("alt-down");
        assert_eq!(store.tasks(list, false).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [second, first]);
        cx.simulate_keystrokes("enter secondary-a");
        cx.simulate_input("Renamed task");
        cx.simulate_keystrokes("enter");
        assert_eq!(store.tasks(list, false).unwrap()[1].title, "Renamed task");
        cx.simulate_keystrokes("up space");
        assert_eq!(store.tasks(list, false).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [first]);
        cx.simulate_keystrokes("t");
        cx.simulate_input("Not saved");
        cx.simulate_keystrokes("escape");
        assert_eq!(store.tasks(list, true).unwrap().len(), 2);
    }

    #[gpui::test]
    fn task_from_html_reading_and_text_fields_do_not_trigger_mail_shortcut(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.update(cx, |v, cx| {
            let store = v.live.as_ref().unwrap().store.clone();
            let mut mail = incoming(v, 1);
            mail.message_id = "<html-task@test>".into();
            mail.subject = "HTML task".into();
            store.save_message(&lagom_core::store::Message { summary: mail, body_html: Some("<p>Message</p>".into()), ..Default::default() }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            store
        });
        view.update(cx, |v, cx| html::WebHost::new_task(v, cx));
        assert_eq!(store.tasks(store.default_task_list().unwrap(), false).unwrap().len(), 1);
        assert_eq!(store.tasks(store.default_task_list().unwrap(), false).unwrap()[0].title, "HTML task");
        view.update_in(cx, |v, window, cx| v.open_palette("", window, cx));
        cx.simulate_input("t");
        assert_eq!(store.tasks(store.default_task_list().unwrap(), false).unwrap().len(), 1);
        assert_eq!(view.read_with(cx, |v, cx| v.palette.as_ref().unwrap().read(cx).query(cx)), "t");
    }

    #[gpui::test]
    fn task_list_controls_and_default_setting_move_tasks_on_delete(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds("new-list").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_input("Custom list");
        cx.simulate_keystrokes("enter");
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        let list = store.task_lists().unwrap().last().unwrap().id;
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds(format!("list-menu-{list}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds(format!("rename-list-{list}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("Renamed list");
        cx.simulate_keystrokes("enter");
        assert_eq!(store.task_lists().unwrap().last().unwrap().name, "Renamed list");
        let task = store.add_task(list, "Keep when deleting", "", None).unwrap();
        cx.simulate_keystrokes("secondary-,");
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let default = store.task_lists().unwrap()[1].id;
        let at = cx.debug_bounds(format!("default-task-list-{default}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        assert_eq!(store.default_task_list().unwrap(), default);
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert_eq!(view.read_with(cx, |v, _| v.today.default), Some(default));
        let at = cx.debug_bounds(format!("list-menu-{list}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds(format!("delete-list-{list}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(store.task_lists().unwrap().iter().any(|l| l.id == list));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds("confirm-delete-list").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(!store.task_lists().unwrap().iter().any(|l| l.id == list));
        assert_eq!(store.tasks(default, false).unwrap()[0].id, task.id);
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        let at = cx.debug_bounds(format!("complete-task-{}", task.id).leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(store.tasks(default, false).unwrap().is_empty());
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
            more: false,
            pin: None,
            want: None,
            notification_target: None,
            task_target: None,
            move_target: None,
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
            (live.bridge.clone(), live.op(Op::Undo(0)), cx.entity().downgrade())
        }).unwrap();
        first.update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
        let reopened = cx.update(|cx| crate::open_main_window(false, cx).unwrap());
        let store = reopened.update(cx, |view, _, _| {
            let live = view.live.as_mut().unwrap();
            assert!(bridge.same_worker(&live.bridge), "Dock reopen must reuse the running worker");
            assert!(live.op(Op::Undo(0)) > old_op, "old completions must not match new operations");
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
            view.live.as_mut().unwrap().op(Op::Undo(0))
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
    fn inactive_conversation_marks_only_expanded_mail_read_after_activation(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        cx.deactivate_window();
        let ids = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let folder = live.store.folders(account, 0).unwrap()[0].id;
            let ids: Vec<_> = (1..=2).map(|uid| {
                let mut m = lagom_core::mime::parse(format!("From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <m{uid}@test>\r\nReferences: <root@test>\r\n\r\nBody {uid}").as_bytes()).unwrap();
                m.summary.account = account.clone();
                m.summary.folder = folder;
                m.summary.uid = uid;
                m.summary.date = uid as i64;
                live.store.save_message(&m).unwrap()
            }).collect();
            v.inbox = Inbox::live();
            v.reading_thread = None;
            v.reload(cx);
            ids
        });
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert_eq!(v.reading_members.len(), 2);
            assert_eq!(v.expanded.len(), 1);
            assert!(v.reading_members.iter().all(|&id| v.inbox.mail[id].unread));
        });
        assert!(rx.try_recv().is_err());
        view.update_in(cx, |_, window, _| window.activate_window());
        cx.run_until_parked();
        assert!(matches!(rx.try_recv(), Ok(Cmd::MarkSeen(read)) if read == ids[1]));
        assert!(rx.try_recv().is_err());
        view.read_with(cx, |v, _| {
            assert!(v.inbox.mail[v.reading_members[0]].unread);
            assert!(!v.inbox.mail[v.reading_members[1]].unread);
        });
    }

    #[gpui::test]
    fn notification_click_expands_the_target_inside_a_conversation(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let target = view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let folder = live.store.folders(account, 0).unwrap()[0].id;
            let mut target = None;
            for uid in 1..=2 {
                let mut m = lagom_core::mime::parse(format!("From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <m{uid}@test>\r\nReferences: <root@test>\r\n\r\nBody {uid}").as_bytes()).unwrap();
                m.summary.account = account.clone();
                m.summary.folder = folder;
                m.summary.uid = uid;
                m.summary.date = uid as i64;
                let id = live.store.save_message(&m).unwrap();
                if uid == 1 { target = Some(crate::notifications::Target { account: account.clone(), message: Some(id) }); }
            }
            target.unwrap()
        });
        view.update_in(cx, |v, window, cx| {
            v.on_notification(crate::notifications::Event::Clicked(target.clone()), window, cx);
            v.open_current(true, cx);
            assert_eq!(v.reading_members.len(), 2);
            assert_eq!(v.expanded.len(), 1);
            let expanded = *v.expanded.keys().next().unwrap();
            assert_eq!(v.inbox.mail[expanded].store, target.message);
        });
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
        let quote = view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx));
        cx.simulate_input("Unfinished fixture draft");
        let target = view.update(cx, |v, _| {
            let m = incoming(v, 1);
            v.live.as_mut().unwrap().ops.insert(1, Op::Undo(0));
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
            assert_eq!(v.drafts[0].read(cx).body_text(cx), format!("Unfinished fixture draft{quote}"));
            assert!(!v.hold_exit(cx));
        });
    }

    #[gpui::test]
    fn reauthentication_actions_and_events_preserve_selection_and_retry_state(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let account = view.update(cx, |v, _| v.live.as_ref().unwrap().accounts[0].id.clone());
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::ReauthRequired(account.clone()), Event::Sync(account.clone(), SyncState::Failed("expired".into()))], window, cx);
            assert_eq!(v.sync_state().unwrap().as_ref(), "Sign in again");
            assert_eq!(v.sync_icon(), "icons/cloud-failed.svg");
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("account-reauth-0").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::Reauthenticate(id) if id == account));
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::Sync(account.clone(), SyncState::SigningIn), Event::Sync(account.clone(), SyncState::Synced(0))], window, cx);
            assert_eq!(v.sync_state().unwrap().as_ref(), "Signing in…");
            assert_eq!(v.sync_icon(), "icons/cloud-syncing.svg");
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("account-reauth-0").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::CancelSignIn));
        view.update_in(cx, |v, window, cx| {
            v.on_events(vec![Event::ReauthRequired(account.clone()), Event::Reauthenticated { account: account.clone(), result: Err("Sign in with the same email address as this account".into()) }], window, cx);
            assert_eq!(v.sync_state().unwrap().as_ref(), "Sign in again");
            assert_eq!(v.sync_icon(), "icons/cloud-failed.svg");
            assert!(v.toast.as_ref().unwrap().0.contains("same email address"));
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("account-reauth-0").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(matches!(rx.try_recv().unwrap(), Cmd::Reauthenticate(id) if id == account));
        view.update_in(cx, |v, window, cx| {
            let mut second = v.live.as_ref().unwrap().accounts[0].clone();
            second.id = "second-fixture-account".into();
            second.email = "second@example.com".into();
            v.live.as_ref().unwrap().store.save_account(&second).unwrap();
            v.live.as_mut().unwrap().accounts.push(second);
            v.live.as_mut().unwrap().current = 1;
            v.on_events(vec![Event::Reauthenticated { account: account.clone(), result: Ok(()) }, Event::Sync(account.clone(), SyncState::Synced(0))], window, cx);
            let live = v.live.as_ref().unwrap();
            assert_eq!(live.current, 1);
            assert_eq!(live.sync[&account], SyncState::Synced(0));
            assert!(v.reauth_control(0, "test", cx).is_none());
            assert!(v.reauth_control(1, "test", cx).is_none());
        });
    }

    #[gpui::test]
    fn reading_deduplicates_folder_copies_but_actions_keep_their_ids(cx: &mut TestAppContext) {
        use lagom_core::store::FolderRole;
        let (view, cx, mut rx) = window(cx);
        let (inbox_id, archive_id) = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let inbox = live.store.folders(account, 0).unwrap()[0].id;
            let archive = live.store.save_folder(account, "All Mail", None, FolderRole::Archive).unwrap();
            let mut m = lagom_core::mime::parse(b"From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <copy@test>\r\n\r\nBody").unwrap();
            m.summary.account = account.clone();
            m.summary.folder = archive;
            m.summary.uid = 1;
            let archive_id = live.store.save_message(&m).unwrap();
            m.summary.folder = inbox;
            let inbox_id = live.store.save_message(&m).unwrap();
            v.inbox = Inbox::live();
            v.reading_thread = None;
            v.reload(cx);
            v.open_current(true, cx);
            assert_eq!(v.reading_members.len(), 1);
            assert_eq!(v.inbox.mail[v.reading_members[0]].store, Some(inbox_id));
            assert!(v.expanded.is_empty());
            let archive = v.inbox.folders.iter().position(|f| f.store == Some(archive)).unwrap();
            v.inbox.open = Some(archive);
            v.reload(cx); v.open_current(true, cx);
            assert_eq!(v.reading_members.len(), 1);
            assert_eq!(v.inbox.mail[v.reading_members[0]].store, Some(archive_id));
            v.inbox.open = None;
            v.reload(cx); v.open_current(true, cx);
            (inbox_id, archive_id)
        });
        while rx.try_recv().is_ok() {}
        view.update(cx, |v, cx| v.act(Move::Delete, cx));
        let moved: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|cmd| match cmd { Cmd::Op { id, .. } => Some(id), _ => None }).collect();
        assert_eq!(moved, [inbox_id]);
        assert!(!moved.contains(&archive_id));
    }

    #[gpui::test]
    fn reading_keeps_distinct_messages_without_message_ids(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let folder = live.store.folders(account, 0).unwrap()[0].id;
            for uid in 1..=2 {
                let mut m = lagom_core::mime::parse(b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Topic\r\n\r\nBody").unwrap();
                m.summary.account = account.clone(); m.summary.folder = folder; m.summary.uid = uid;
                live.store.save_message(&m).unwrap();
            }
            v.inbox = Inbox::live(); v.reading_thread = None;
            v.reload(cx); v.open_current(true, cx);
            assert_eq!(v.inbox.list().len(), 1);
            assert_eq!(v.reading_members.len(), 2);
            assert!(v.reading_members.iter().all(|&id| v.inbox.mail[id].message_id.is_empty()));
        });
    }

    #[gpui::test]
    fn first_sent_reply_opens_a_body_without_advancing_on_receipts(cx: &mut TestAppContext) {
        use lagom_core::store::FolderRole;
        let (view, cx, mut rx) = window(cx);
        let mut reply = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let inbox = live.store.folders(account, 0).unwrap()[0].id;
            let sent = live.store.save_folder(account, "Sent", None, FolderRole::Sent).unwrap();
            let mut m = lagom_core::mime::parse(b"From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <root@test>\r\n\r\nRoot").unwrap();
            m.summary.account = account.clone(); m.summary.folder = inbox; m.summary.uid = 1; m.summary.date = 1;
            live.store.save_message(&m).unwrap();
            m.summary.folder = sent; m.summary.message_id = "reply@test".into(); m.summary.thread_refs = "root@test".into(); m.summary.date = 2;
            v.inbox = Inbox::live(); v.reading_thread = None;
            v.reload(cx); v.open_current(true, cx);
            assert_eq!(v.reading_members.len(), 1);
            assert!(v.expanded.is_empty());
            m
        });
        view.update(cx, |v, cx| {
            let selected = v.reading_thread;
            let reply_id = v.live.as_ref().unwrap().store.save_message(&reply).unwrap();
            v.reload(cx); v.open_current(true, cx);
            assert_eq!(v.reading_thread, selected);
            assert_eq!(v.reading_members.len(), 2);
            assert_eq!(v.expanded.len(), 1);
            let expanded = *v.expanded.keys().next().unwrap();
            assert_eq!(v.inbox.mail[expanded].store, Some(reply_id));
            reply.summary.seen = true;
            v.live.as_ref().unwrap().store.save_message(&reply).unwrap();
            while rx.try_recv().is_ok() {}
            v.reload(cx); v.open_current(true, cx);
            assert!(v.expanded.contains_key(&expanded));
            assert_eq!(v.expanded.len(), 1);
            assert!(rx.try_recv().is_err());
            v.toggle_message(expanded, true, cx);
            v.reload(cx); v.open_current(true, cx);
            assert!(v.expanded.is_empty(), "a deliberate collapse survives a reload");
        });
    }

    #[gpui::test]
    fn popout_actions_follow_the_row_after_a_new_reply(cx: &mut TestAppContext) {
        for what in [Move::Done, Move::Later, Move::Delete] {
            for away in [false, true] {
                let (view, cx, mut rx) = window(cx);
                let (root, mut reply) = view.update(cx, |v, cx| {
                    let live = v.live.as_ref().unwrap();
                    let account = &live.accounts[0].id;
                    let folder = live.store.folders(account, 0).unwrap()[0].id;
                    let mut m = lagom_core::mime::parse(b"From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <root@test>\r\n\r\nRoot").unwrap();
                    m.summary.account = account.clone(); m.summary.folder = folder; m.summary.uid = 1; m.summary.date = 1;
                    let root = live.store.save_message(&m).unwrap();
                    v.inbox = Inbox::live(); v.reading_thread = None;
                    v.reload(cx);
                    (root, m)
                });
                view.update_in(cx, |v, window, cx| v.open_popout(window, cx));
                let (original, popout) = view.read_with(cx, |v, _| v.popouts[0]);
                reply.summary.uid = 2; reply.summary.date = 2;
                reply.summary.message_id = "reply@test".into(); reply.summary.thread_refs = "root@test".into();
                let reply = view.update_in(cx, |v, window, cx| {
                    let reply = v.live.as_ref().unwrap().store.save_message(&reply).unwrap();
                    v.on_events(vec![Event::Changed], window, cx);
                    v.on_events(vec![Event::Changed], window, cx);
                    assert_eq!(v.inbox.list().len(), 1);
                    assert_ne!(v.inbox.current(), Some(original));
                    assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(reply));
                    if away { v.open_kind(Kind::Later, cx); }
                    reply
                });
                while rx.try_recv().is_ok() {}
                popout.update(cx, |p, _, cx| p.act(what, cx)).unwrap();
                let moved: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|cmd| match cmd { Cmd::Op { id, .. } => Some(id), _ => None }).collect();
                assert_eq!(moved, [root, reply], "{what:?}, away={away}");
                view.read_with(cx, |v, _| {
                    assert!(v.inbox.list().is_empty());
                    assert!(v.popouts.is_empty());
                });
            }
        }
    }

    #[gpui::test]
    fn review_custom_folder_batch_does_not_wait_for_a_writer(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            for uid in 1..=12 { incoming(v, uid); }
            let live = v.live.as_ref().unwrap();
            let target = live.store.save_folder(&live.accounts[0].id, "Projects", None, lagom_core::store::FolderRole::Other).unwrap();
            let lock = rusqlite::Connection::open(live.store.path()).unwrap();
            v.panes.group_conversations = false;
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            let folder = v.inbox.folders.iter().position(|f| f.store == Some(target)).unwrap();
            lock.execute_batch("BEGIN IMMEDIATE").unwrap();
            let started = std::time::Instant::now();
            v.act(Move::Folder(folder), cx);
            let elapsed = started.elapsed();
            lock.execute_batch("ROLLBACK").unwrap();
            assert!(elapsed < Duration::from_millis(750), "batch blocked on a writer for {elapsed:?}");
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(commands.len(), 12);
            assert!(commands.iter().all(|cmd| matches!(cmd, Cmd::Op { act: Act::Move(id), .. } if *id == target)));
            assert!(v.inbox.list().is_empty());
            v.reload(cx);
            assert!(v.inbox.list().is_empty(), "reload must preserve queued moves");
            let events = commands.into_iter().map(|cmd| match cmd {
                Cmd::Op { op, .. } => Event::Op { op, error: Some("fixture refused".into()) },
                _ => unreachable!(),
            }).collect();
            lock.execute_batch("BEGIN IMMEDIATE").unwrap();
            let started = std::time::Instant::now();
            v.on_events(events, window, cx);
            let elapsed = started.elapsed();
            lock.execute_batch("ROLLBACK").unwrap();
            assert!(elapsed < Duration::from_millis(750), "batch rollback blocked on a writer for {elapsed:?}");
            assert_eq!(v.inbox.list().len(), 12);
        });
    }

    #[gpui::test]
    fn review_custom_folder_move_queues_old_id_while_undo_is_pending(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let mut original = incoming(v, 1);
            original.message_id = "queued@example.test".into();
            let live = v.live.as_ref().unwrap();
            let store = live.store.clone();
            store.save_message(&lagom_core::store::Message { summary: original.clone(), ..Default::default() }).unwrap();
            let target = store.save_folder(&original.account, "Projects", None, lagom_core::store::FolderRole::Other).unwrap();
            let next = store.save_folder(&original.account, "Next", None, lagom_core::store::FolderRole::Other).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            let folder = v.inbox.folders.iter().position(|f| f.store == Some(target)).unwrap();
            v.act(Move::Folder(folder), cx);
            let Cmd::Op { op, id, act: Act::Move(destination) } = rx.try_recv().unwrap() else { panic!("expected move") };
            assert_eq!((id, destination), (original.id, target));
            let mut moved = store.message(id).unwrap();
            moved.summary.folder = target;
            let replacement = store.save_message(&moved).unwrap();
            store.delete_message(id).unwrap();
            assert_ne!(replacement, id);
            v.on_events(vec![Event::Op { op, error: None }], window, cx);
            v.undo(cx);
            assert!(v.prepare_move(cx));
            let folder = v.inbox.folders.iter().position(|f| f.store == Some(next)).unwrap();
            v.act(Move::Folder(folder), cx);
            assert!(matches!(rx.try_recv().unwrap(), Cmd::Op { id, act: Act::Move(folder), .. } if id == original.id && folder == original.folder));
            assert!(matches!(rx.try_recv().expect("new move must reach worker alias resolution"), Cmd::Op { id, act: Act::Move(folder), .. } if id == original.id && folder == next));
            assert!(rx.try_recv().is_err());
        });
    }

    #[gpui::test]
    fn custom_folder_batch_queues_persistence_and_restores_refused_messages(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let messages: Vec<_> = (1..=2).map(|uid| incoming(v, uid)).collect();
            let live = v.live.as_ref().unwrap();
            let target = live.store.save_folder(&live.accounts[0].id, "Projects", None, lagom_core::store::FolderRole::Other).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            let folder = v.inbox.folders.iter().position(|f| f.store == Some(target)).unwrap();
            v.act(Move::Folder(folder), cx);
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(commands.len(), 2);
            let store = v.live.as_ref().unwrap().store.clone();
            assert!(store.list_messages(target, now(), 10, 0).unwrap().is_empty());
            for message in &messages {
                assert_eq!(store.message(message.id).unwrap().summary.folder, message.folder);
            }
            for command in commands {
                let Cmd::Op { op, id, act: Act::Move(destination) } = command else { panic!("expected move") };
                assert_eq!(destination, target);
                store.begin_move(id, destination).unwrap();
                assert_eq!(store.message_summary(id).unwrap().folder, target);
                store.cancel_move(id).unwrap();
                v.on_events(vec![Event::Op { op, error: Some("fixture refused".into()) }], window, cx);
                assert_ne!(v.live.as_ref().unwrap().store.message_summary(id).unwrap().folder, target);
            }
            assert!(v.live.as_ref().unwrap().store.list_messages(target, now(), 10, 0).unwrap().is_empty());
            assert_eq!(v.inbox.list().len(), 2);
            assert!(v.toast.as_ref().unwrap().0.contains("Not moved: fixture refused"));
        });
    }

    #[gpui::test]
    fn thread_actions_send_every_folder_member_and_undo_them(cx: &mut TestAppContext) {
        use lagom_core::store::FolderRole;
        let (view, cx, mut rx) = window(cx);
        let ids = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let account = &live.accounts[0].id;
            let inbox = live.store.folders(account, 0).unwrap()[0].id;
            let sent = live.store.save_folder(account, "Sent", None, FolderRole::Sent).unwrap();
            live.store.save_folder(account, "Target", None, FolderRole::Other).unwrap();
            let ids: Vec<_> = (1..=3).map(|uid| {
                let mut m = lagom_core::mime::parse(format!("From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Topic\r\nMessage-ID: <m{uid}@test>\r\nReferences: <root@test>\r\n\r\nBody {uid}").as_bytes()).unwrap();
                m.summary.account = account.clone();
                m.summary.folder = if uid == 2 { sent } else { inbox };
                m.summary.uid = uid;
                m.summary.date = uid as i64;
                live.store.save_message(&m).unwrap()
            }).collect();
            v.inbox = Inbox::live();
            v.reading_thread = None;
            v.reload(cx);
            assert_eq!(v.inbox.list().len(), 1);
            v.open_current(true, cx);
            assert_eq!(v.reading_members.iter().map(|&id| v.inbox.mail[id].store.unwrap()).collect::<Vec<_>>(), ids);
            assert_eq!(v.expanded.len(), 1);
            let newest = *v.expanded.keys().next().unwrap();
            assert_eq!(v.inbox.mail[newest].store, Some(ids[2]));
            assert!(v.inbox.members(v.inbox.current().unwrap()).iter().any(|&id| v.inbox.mail[id].unread));
            ids
        });
        while rx.try_recv().is_ok() {}
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_ref().unwrap();
            let summary = live.store.message(ids[2]).unwrap().summary;
            live.store.set_flags(summary.folder, summary.uid, true, false).unwrap();
            v.on_events(vec![Event::Seen(ids[2]), Event::Changed], window, cx);
            v.open_current(true, cx);
            assert_eq!(v.expanded.len(), 1);
            let expanded = *v.expanded.keys().next().unwrap();
            assert_eq!(v.inbox.mail[expanded].store, Some(ids[2]));
            assert!(rx.try_recv().is_err(), "a read receipt must not open the next unread message");
        });
        let target = view.read_with(cx, |v, _| v.inbox.folders.iter().position(|f| f.full == "Target").unwrap());
        for what in [Move::Done, Move::Later, Move::Delete, Move::Folder(target)] {
            view.update(cx, |v, cx| v.act(what, cx));
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let moved: Vec<_> = commands.iter().filter_map(|cmd| match cmd { Cmd::Op { id, .. } => Some(*id), _ => None }).collect();
            assert_eq!(moved, [ids[0], ids[2]]);
            assert_eq!(view.read_with(cx, |v, _| v.inbox.list().len()), 0);
            view.update(cx, |v, cx| v.undo(cx));
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let restored: Vec<_> = commands.iter().filter_map(|cmd| match cmd { Cmd::Op { id, act: Act::Move(_) | Act::Snooze(None), .. } => Some(*id), _ => None }).collect();
            assert_eq!(restored, moved);
            assert_eq!(view.read_with(cx, |v, _| v.inbox.list().len()), 1);
        }
        view.update(cx, |v, cx| {
            let row = v.inbox.current().unwrap();
            for id in v.inbox.members(row) { v.inbox.mail[id].snoozed_until = Some(i64::MAX); }
            v.act(Move::Done, cx);
        });
        while rx.try_recv().is_ok() {}
        view.update(cx, |v, cx| v.undo(cx));
        let restored_snoozes = std::iter::from_fn(|| rx.try_recv().ok()).filter(|cmd| matches!(cmd, Cmd::Op { act: Act::Snooze(Some(i64::MAX)), .. })).count();
        assert_eq!(restored_snoozes, 2);
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
    fn review_unified_navigation_visits_every_message_in_date_order(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        for group in [false, true] {
            let (view, cx, _rx) = window(cx);
            let expected = view.update(cx, |v, cx| {
                let live = v.live.as_ref().unwrap();
                let mut ids = Vec::new();
                for (ix, account) in mail::demo_accounts().iter().enumerate() {
                    live.store.save_account(account).unwrap();
                    let folder = live.store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
                    let count = if ix == 0 { 400 } else { 200 };
                    for uid in 1..=count {
                        let summary = MessageSummary { account: account.id.clone(), folder, uid,
                            date: if ix == 0 { 1000 + uid as i64 } else { uid as i64 }, seen: true, ..Default::default() };
                        ids.push((summary.date, live.store.save_message(&Message { summary, ..Default::default() }).unwrap()));
                    }
                }
                ids.sort_by_key(|row| std::cmp::Reverse(*row));
                v.panes.group_conversations = group;
                v.inbox = Inbox::live();
                v.reload(cx);
                ids.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
            });
            for (ix, id) in expected.iter().enumerate() {
                if ix > 0 { cx.simulate_keystrokes("j"); }
                view.read_with(cx, |v, _| {
                    assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(*id), "row {ix}, grouped={group}");
                });
            }
            assert!(!view.read_with(cx, |v, _| v.has_more()));
        }
    }

    #[gpui::test]
    fn review_later_navigation_visits_every_message_in_date_order(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        for group in [false, true] {
            let (view, cx, _rx) = window(cx);
            let expected = view.update(cx, |v, cx| {
                let live = v.live.as_ref().unwrap();
                let account = &live.accounts[0];
                live.store.save_account(account).unwrap();
                let folder = live.store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
                let mut ids = Vec::new();
                let now = chrono::Local::now().timestamp();
                for uid in 1..=400 {
                    let summary = MessageSummary { account: account.id.clone(), folder, uid,
                        date: uid as i64 / 2, seen: true, ..Default::default() };
                    let id = live.store.save_message(&Message { summary, ..Default::default() }).unwrap();
                    live.store.snooze(id, Some(now + 3600 + uid as i64)).unwrap();
                    ids.push(id);
                }
                ids.reverse();
                v.panes.group_conversations = group;
                v.inbox = Inbox::live();
                v.reload(cx);
                v.open_box(v.inbox.find_kind(Kind::Later), cx);
                ids
            });
            for (ix, id) in expected.iter().enumerate() {
                if ix > 0 { cx.simulate_keystrokes("j"); }
                view.read_with(cx, |v, _| {
                    assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].store, Some(*id), "Later row {ix}, grouped={group}");
                });
            }
            assert!(!view.read_with(cx, |v, _| v.has_more()));
        }
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
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
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
        let quote = view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx));
        cx.simulate_input("Keep this text");
        view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.delete_account(&live.accounts[0].id).unwrap();
        });
        assert!(view.update(cx, |v, cx| v.hold_exit(cx)));
        assert!(toast(&view, cx).contains("Could not save local edits"));
        assert_eq!(view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx)), format!("Keep this text{quote}"));
        view.update(cx, |v, _| {
            let live = v.live.as_ref().unwrap();
            live.store.save_account(&live.accounts[0]).unwrap();
        });
        assert!(!view.update(cx, |v, cx| v.hold_exit(cx)));
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), format!("Keep this text{quote}"));
    }

    #[gpui::test]
    fn draft_summary_reload_yields_with_large_attachments(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            let store = &v.live.as_ref().unwrap().store;
            for index in 0..5 {
                store.save_draft(&lagom_core::outbox::Draft {
                    id: format!("<large-{index}@example.com>"), account: v.live.as_ref().unwrap().accounts[0].id.clone(),
                    subject: format!("Draft {index}"), attachments: vec![lagom_core::smtp::OutgoingAttachment {
                        filename: "large.bin".into(), mime_type: "application/octet-stream".into(), data: vec![255; 5 * 1024 * 1024],
                    }], ..Default::default()
                }).unwrap();
            }
            let started = Instant::now();
            v.reload(cx);
            eprintln!("draft reload returned in {:?}", started.elapsed());
            assert!(v.draft_rows.is_empty(), "summary reads must yield to the UI executor");
        });
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.draft_rows.len()), 5);
        view.update(cx, |v, cx| {
            v.reload_draft_rows(cx);
            let store = &v.live.as_ref().unwrap().store;
            for row in &v.draft_rows { store.delete_draft(&row.id).unwrap(); }
            v.reload_draft_rows(cx);
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.draft_rows.is_empty()), "the latest reload must win");
    }

    #[gpui::test]
    fn opening_foreign_draft_yields_before_reading_attachments_or_waiting_for_a_writer(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
        let account = view.read_with(cx, |v, _| v.live.as_ref().unwrap().accounts[0].id.clone());
        let id = "<large-foreign@example.com>";
        store.save_draft(&lagom_core::outbox::Draft {
            id: id.into(), account, text: "Foreign text".into(), attachments: vec![lagom_core::smtp::OutgoingAttachment {
                filename: "large.bin".into(), mime_type: "application/octet-stream".into(), data: vec![255; 5 * 1024 * 1024],
            }], ..Default::default()
        }).unwrap();
        let writer = rusqlite::Connection::open(store.path()).unwrap();
        writer.execute("UPDATE draft_sync SET owned=0 WHERE id=?1", [id]).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        view.update_in(cx, |v, window, cx| {
            v.open_draft(id, window, cx);
            assert!(v.compose.is_none(), "opening must yield before loading the draft");
            assert!(v.toast.is_none(), "opening must not attempt a write on the UI thread");
        });
        writer.execute_batch("ROLLBACK").unwrap();
        cx.run_until_parked();
        view.read_with(cx, |v, cx| {
            let compose = v.compose.as_ref().expect("draft should open after acquisition").read(cx);
            assert_eq!(compose.body_text(cx), "Foreign text");
            assert_eq!(compose.outgoing(cx).attachments[0].data, vec![255; 5 * 1024 * 1024]);
        });
        assert!(writer.query_row("SELECT owned FROM draft_sync WHERE id=?1", [id], |r| r.get::<_, bool>(0)).unwrap());
    }

    #[gpui::test]
    fn pending_draft_open_cannot_replace_a_newer_action(cx: &mut TestAppContext) {
        for action in ["draft", "page", "compose"] {
            let (view, cx, _rx) = window(cx);
            view.update_in(cx, |v, window, cx| {
                let live = v.live.as_ref().unwrap();
                for id in ["<first@example.com>", "<second@example.com>"] {
                    live.store.save_draft(&lagom_core::outbox::Draft {
                        id: id.into(), account: live.accounts[0].id.clone(), ..Default::default()
                    }).unwrap();
                }
                v.open_draft("<first@example.com>", window, cx);
                match action {
                    "draft" => v.open_draft("<second@example.com>", window, cx),
                    "page" => v.set_page(Page::Home, cx),
                    _ => v.open_compose(false, window, cx),
                }
            });
            let newer = view.read_with(cx, |v, _| v.compose.as_ref().map(|c| c.entity_id()));
            cx.run_until_parked();
            view.read_with(cx, |v, cx| match action {
                "draft" => assert_eq!(v.compose.as_ref().unwrap().read(cx).draft_id(), "<second@example.com>"),
                "page" => { assert!(matches!(v.page, Page::Home)); assert!(v.compose.is_none()); }
                _ => assert_eq!(v.compose.as_ref().map(|c| c.entity_id()), newer),
            });
        }
    }

    #[gpui::test]
    fn drafts_view_opens_foreign_bcc_and_reuses_unsaved_compose(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        let id = "<foreign@example.com>";
        view.update_in(cx, |v, window, cx| {
            let live = v.live.as_ref().unwrap();
            let draft = lagom_core::outbox::Draft { id: id.into(), account: live.accounts[0].id.clone(),
                to: "to@example.com".into(), bcc: "private@example.com".into(), subject: "Foreign draft".into(),
                text: "Foreign text".into(), ..Default::default() };
            live.store.save_draft(&draft).unwrap();
            let db = rusqlite::Connection::open(live.store.path()).unwrap();
            db.execute("UPDATE draft_sync SET owned=0,revision=(SELECT revision FROM drafts WHERE id=?1) WHERE id=?1", [id]).unwrap();
            v.restore_drafts(window, cx);
            assert!(v.drafts.is_empty(), "foreign drafts are opened explicitly");
            v.show_drafts(window, cx);
            assert!(matches!(v.page, Page::Drafts));
        });
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.draft_rows.len()), 1);
        view.update_in(cx, |v, window, cx| v.open_draft(id, window, cx));
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            let compose = v.compose.as_ref().unwrap().clone();
            assert_eq!(compose.read(cx).outgoing(cx).bcc, ["private@example.com"]);
            assert_eq!(compose.read(cx).body_text(cx), "Foreign text");
            compose.update(cx, |c, cx| c.focus_body(window, cx));
        });
        cx.simulate_input("Local edits ");
        cx.simulate_keystrokes("escape");
        let expected = view.read_with(cx, |v, cx| v.drafts[0].read(cx).outgoing(cx));
        assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|cmd| matches!(cmd, Cmd::Drafts(_))));
        view.update_in(cx, |v, window, cx| {
            let entity = v.drafts[0].entity_id();
            v.open_draft(id, window, cx);
            assert_eq!(v.compose.as_ref().unwrap().entity_id(), entity);
            assert_eq!(v.compose.as_ref().unwrap().read(cx).outgoing(cx), expected);
        });
        cx.simulate_keystrokes("cmd-backspace");
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.draft_rows.is_empty()));
        assert!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap().is_empty()));
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
        assert_eq!(stored[0].text, original.text);
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
    fn review_popout_menu_keeps_unsaved_and_failed_drafts(cx: &mut TestAppContext) {
        for (action, failed_save) in [("Reply all", false), ("Forward", false), ("Reply all", true), ("Forward", true)] {
            let (view, cx, _rx) = window(cx);
            cx.simulate_keystrokes("secondary-o j r");
            cx.simulate_input("Unsaved edits");
            let (draft, outgoing, stored, popout) = view.read_with(cx, |v, cx| {
                let draft = v.compose.as_ref().unwrap();
                (draft.entity_id(), draft.read(cx).outgoing(cx), v.live.as_ref().unwrap().store.drafts().unwrap(), v.popouts[0].1)
            });
            assert_ne!(stored[0].text, outgoing.text);
            let store = view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.clone());
            let writer = rusqlite::Connection::open(store.path()).unwrap();
            if failed_save {
                writer.execute_batch("BEGIN IMMEDIATE").unwrap();
                view.update(cx, |v, cx| assert!(!store.with_short_lock_wait(|_| {
                    Ok(v.compose.as_ref().unwrap().update(cx, |c, cx| c.save(cx)))
                }).unwrap()));
            }
            let mut popup = VisualTestContext::from_window(popout.into(), cx);
            popup.run_until_parked();
            let at = popup.debug_bounds("message-menu-0").unwrap().center();
            popup.simulate_click(at, Default::default());
            popup.run_until_parked();
            let at = popup.debug_bounds(format!("menu-{action}").leak()).unwrap().center();
            popup.simulate_click(at, Default::default());
            view.read_with(cx, |v, cx| {
                let current = v.compose.as_ref().unwrap();
                assert_eq!(current.entity_id(), draft, "{action} replaced the open composer");
                assert_eq!(current.read(cx).outgoing(cx), outgoing);
                assert_eq!(v.live.as_ref().unwrap().store.drafts().unwrap(), stored);
            });
            if failed_save {
                writer.execute_batch("ROLLBACK").unwrap();
                view.update(cx, |v, cx| assert!(v.compose.as_ref().unwrap().update(cx, |c, cx| c.save(cx))));
            } else {
                cx.executor().advance_clock(Duration::from_millis(300));
                cx.run_until_parked();
            }
            assert_eq!(store.drafts().unwrap()[0].text, outgoing.text);
        }
    }

    #[gpui::test]
    fn autosave_waits_for_a_pause_and_keeps_the_latest_edit(cx: &mut TestAppContext) {
        let (view, cx, _rx) = window(cx);
        cx.simulate_keystrokes("r");
        let quote = view.read_with(cx, |v, cx| v.compose.as_ref().unwrap().read(cx).body_text(cx));
        cx.simulate_input("first");
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), quote);
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.simulate_input(" latest");
        cx.executor().advance_clock(Duration::from_millis(200));
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), quote);
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.live.as_ref().unwrap().store.drafts().unwrap()[0].text.clone()), format!("first latest{quote}"));
    }

    #[gpui::test]
    fn failed_send_actions_retry_or_restore_for_editing(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        send_one(cx);
        assert!(matches!(rx.try_recv(), Ok(Cmd::Drafts(_))));
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
    fn review_accounts(v: &mut MainWindow, cx: &mut Context<MainWindow>) -> (i64, i64) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let first = incoming(v, 1).id;
        incoming(v, 2);
        incoming(v, 3);
        let live = v.live.as_ref().unwrap();
        live.store.save_folder(&live.accounts[0].id, "Personal files", None, FolderRole::Other).unwrap();
        let mut other = live.accounts[0].clone();
        other.id = "work-review".into();
        other.email = "work@example.test".into();
        live.store.save_account(&other).unwrap();
        let folder = live.store.save_folder(&other.id, "INBOX", None, FolderRole::Inbox).unwrap();
        live.store.save_folder(&other.id, "Work files", None, FolderRole::Other).unwrap();
        let second = live.store.save_message(&Message { summary: MessageSummary {
            account: other.id, folder, uid: 1, date: 10, ..Default::default()
        }, ..Default::default() }).unwrap();
        v.panes.group_conversations = false;
        v.inbox = Inbox::live();
        v.reload(cx);
        (first, second)
    }

    #[gpui::test]
    fn review_move_failure_after_account_switch_keeps_other_mail(cx: &mut TestAppContext) {
        for worker_gone in [false, true] {
            let (view, cx, mut rx) = window(cx);
            let other = view.update(cx, |v, cx| {
                let (first, other) = review_accounts(v, cx);
                v.open_found(first, cx);
                v.act(Move::Done, cx);
                v.switch_account(1, cx);
                other
            });
            let Cmd::Op { op, .. } = rx.try_recv().unwrap() else { panic!("missing move") };
            view.update_in(cx, |v, window, cx| {
                if worker_gone { v.worker_gone(cx); v.reload(cx); }
                else { v.on_events(vec![Event::Op { op, error: Some("fixture refusal".into()) }], window, cx); }
                assert_eq!(v.inbox.mail[v.inbox.single().unwrap()].store, Some(other));
                assert!(v.inbox.history.is_empty());
            });
        }
    }

    #[gpui::test]
    fn review_message_move_waits_for_the_requested_message(cx: &mut TestAppContext) {
        for cross_account in [false, true] {
            let (view, cx, mut rx) = window(cx);
            let target = view.update_in(cx, |v, window, cx| {
                let (first, other) = review_accounts(v, cx);
                let target = if cross_account { other } else { first };
                let row = v.inbox.list().iter().position(|&id| v.inbox.mail[id].store != Some(target)
                    && v.inbox.mail[id].account != "work-review").unwrap();
                v.inbox.select(row);
                v.act(Move::Done, cx);
                let id = v.inbox.by_store[&target];
                v.message_action(id, polish::MessageAction::Move, window, cx);
                assert!(v.palette.is_none(), "Move must wait for its requested selection");
                target
            });
            let Cmd::Op { op, .. } = rx.try_recv().unwrap() else { panic!("missing move") };
            view.update_in(cx, |v, window, cx| {
                v.on_events(vec![Event::Op { op, error: Some("fixture refusal".into()) }], window, cx);
                assert!(v.palette.is_some());
                assert_eq!(v.inbox.mail[v.inbox.single().unwrap()].store, Some(target));
            });
            cx.simulate_keystrokes("enter");
            assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|cmd|
                matches!(cmd, Cmd::Op { id, act: Act::Move(_), .. } if id == target)));
        }
    }

    #[gpui::test]
    fn review_deferred_move_does_not_replace_a_missing_target(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            let (target, _) = review_accounts(v, cx);
            v.inbox.select(0);
            v.act(Move::Done, cx);
            let id = v.inbox.by_store[&target];
            v.message_action(id, polish::MessageAction::Move, window, cx);
            assert!(v.palette.is_none());
            v.live.as_ref().unwrap().store.delete_message(target).unwrap();
            let Cmd::Op { op, .. } = rx.try_recv().unwrap() else { panic!("missing move") };
            v.on_events(vec![Event::Op { op, error: None }], window, cx);
            assert!(v.palette.is_none());
            assert!(v.toast.as_ref().unwrap().0.contains("no longer available"));
            assert!(rx.try_recv().is_err());
        });
    }

    #[gpui::test]
    fn review_generic_palette_prepares_move_on_entry(cx: &mut TestAppContext) {
        for typed in [false, true] {
            let (view, cx, mut rx) = window(cx);
            let target = view.update_in(cx, |v, window, cx| {
                let (_, target) = review_accounts(v, cx);
                let row = v.inbox.list().iter().position(|&id| v.inbox.mail[id].store == Some(target)).unwrap();
                v.inbox.select(row);
                v.open_palette("", window, cx);
                target
            });
            if typed { cx.simulate_input("MOVE Work files"); }
            else { cx.simulate_keystrokes("down down down down down enter"); }
            cx.run_until_parked();
            view.read_with(cx, |v, _| {
                assert_eq!(v.live.as_ref().unwrap().account().unwrap().id, "work-review");
                assert_eq!(v.inbox.mail[v.inbox.single().unwrap()].store, Some(target));
            });
            cx.simulate_keystrokes("enter");
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            view.read_with(cx, |v, _| assert!(commands.iter().any(|cmd| match cmd {
                Cmd::Op { id, act: Act::Move(folder), .. } => *id == target
                    && v.live.as_ref().unwrap().store.folder(*folder).unwrap().account == "work-review",
                _ => false,
            })));
        }
    }

    #[gpui::test]
    fn review_bulk_destinations_do_not_read_messages(cx: &mut TestAppContext) {
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            review_accounts(v, cx);
            for uid in 4..=200 { incoming(v, uid); }
            v.reload(cx);
            v.load_more(cx);
            let live = v.live.as_ref().unwrap();
            for account in &live.accounts {
                live.store.save_folder(&account.id, "Archive", None, lagom_core::store::FolderRole::Archive).unwrap();
            }
            let fixture = rusqlite::Connection::open(live.store.path()).unwrap();
            // Destination routing must work without reading or counting any message rows.
            fixture.execute_batch("ALTER TABLE messages RENAME TO fixture_messages;").unwrap();
            v.inbox.select_all();
            v.act(Move::Done, cx);
            assert_eq!(v.live.as_ref().unwrap().ops.len(), 201);
            for mail in &v.inbox.mail {
                let mail::Place::Folder(folder) = mail.folder else { panic!("missing destination") };
                let destination = v.live.as_ref().unwrap().store.folder(folder).unwrap();
                assert_eq!(destination.account, mail.account);
                assert_eq!(destination.role, lagom_core::store::FolderRole::Archive);
            }
        });
        assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|cmd| matches!(cmd, Cmd::Op { act: Act::Archive, .. })).count(), 201);
    }

    #[gpui::test]
    fn unified_actions_keep_message_account_and_single_account_hides_controls(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        view.update(cx, |v, cx| {
            incoming(v, 1);
            v.inbox = Inbox::live();
            v.reload(cx);
        });
        let live = view.update(cx, |v, _| v.live.take().unwrap());
        let (view, cx) = cx.cx.add_window_view(|window, cx| {
            let mut v = MainWindow::with_screen(true, "inbox", window, cx);
            v.live = Some(live);
            v.inbox = Inbox::live();
            v.reload(cx);
            v
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("Inbox-account-0").is_none());
        assert!(cx.debug_bounds("account-bar-0").is_none());
        let (message, source, archive, account) = view.update(cx, |v, cx| {
            let live = v.live.as_ref().unwrap();
            let mut account = live.accounts[0].clone();
            account.id = "work-fixture".into();
            account.email = "work@example.com".into();
            live.store.save_account(&account).unwrap();
            let source = live.store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
            let archive = live.store.save_folder(&account.id, "Archive", None, FolderRole::Archive).unwrap();
            let message = live.store.save_message(&Message { summary: MessageSummary {
                account: account.id.clone(), folder: source, uid: 1, date: 10, from_address: "sender@example.com".into(),
                ..Default::default()
            }, body_text: "Fixture".into(), body_html: None }).unwrap();
            v.reload(cx);
            assert_eq!(v.inbox.count(), 2);
            assert_eq!(v.inbox.list().len(), 2);
            let row = v.inbox.list().iter().position(|&id| v.inbox.mail[id].store == Some(message)).unwrap();
            v.inbox.select(row);
            assert_ne!(v.live.as_ref().unwrap().account().unwrap().id, account.id);
            (message, source, archive, account.id)
        });
        view.update(cx, |v, cx| {
            let selected = v.inbox.single().unwrap();
            v.act(Move::Done, cx);
            assert_eq!(v.inbox.mail[selected].folder, mail::Place::Folder(archive));
            assert_eq!(v.inbox.mail[selected].account, account);
            v.undo(cx);
            assert_eq!(v.inbox.mail[selected].folder, mail::Place::Folder(source));
            assert_eq!(v.inbox.current(), Some(selected));
        });
        let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(commands.iter().any(|cmd| matches!(cmd, Cmd::Op { id, act: Act::Archive, .. } if *id == message)));
        assert!(commands.iter().any(|cmd| matches!(cmd, Cmd::Op { id, act: Act::Move(folder), .. } if *id == message && *folder == source)));
    }

    #[gpui::test]
    fn unified_move_palette_uses_selected_account_and_rejects_mixed_selection(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, _rx) = window(cx);
        view.update(cx, |v, cx| {
            incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            let mut other = live.accounts[0].clone();
            other.id = "other-fixture".into();
            other.email = "other@example.com".into();
            live.store.save_account(&other).unwrap();
            let folder = live.store.save_folder(&other.id, "INBOX", None, FolderRole::Inbox).unwrap();
            live.store.save_folder(&other.id, "Work files", None, FolderRole::Other).unwrap();
            let message = live.store.save_message(&Message { summary: MessageSummary {
                account: other.id.clone(), folder, uid: 1, date: 10, ..Default::default()
            }, body_text: "Fixture".into(), body_html: None }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            v.inbox.select_all();
            assert!(!v.prepare_move(cx));
            assert!(v.palette.is_none());
            let row = v.inbox.list().iter().position(|&id| v.inbox.mail[id].store == Some(message)).unwrap();
            v.inbox.select(row);
            assert!(v.prepare_move(cx));
            assert_eq!(v.live.as_ref().unwrap().account().unwrap().id, other.id);
            assert_eq!(v.inbox.mail[v.inbox.single().unwrap()].store, Some(message));
            assert!(v.inbox.folders.iter().any(|f| f.full == "Work files" && !f.gone));
        });
    }

    #[gpui::test]
    fn unified_popout_action_rebinds_store_id_after_account_switch(cx: &mut TestAppContext) {
        use lagom_core::store::{FolderRole, Message, MessageSummary};
        let (view, cx, mut rx) = window(cx);
        view.update_in(cx, |v, window, cx| {
            incoming(v, 1);
            let live = v.live.as_ref().unwrap();
            let personal = live.accounts[0].id.clone();
            let empty = live.store.save_folder(&personal, "Empty", None, FolderRole::Other).unwrap();
            let mut other = live.accounts[0].clone();
            other.id = "other-fixture".into();
            live.store.save_account(&other).unwrap();
            let inbox = live.store.save_folder(&other.id, "INBOX", None, FolderRole::Inbox).unwrap();
            live.store.save_folder(&other.id, "Archive", None, FolderRole::Archive).unwrap();
            let message = live.store.save_message(&Message { summary: MessageSummary {
                account: other.id.clone(), folder: inbox, uid: 1, date: 10, ..Default::default()
            }, body_text: "Fixture".into(), body_html: None }).unwrap();
            v.inbox = Inbox::live();
            v.reload(cx);
            let id = v.inbox.by_store[&message];
            let folder = v.inbox.folders.iter().position(|f| f.store == Some(empty)).unwrap();
            v.open_box(Some(folder), cx);
            assert!(!v.inbox.raw_list().contains(&id));
            v.act_on(id, Move::Done, window, cx);
            assert_eq!(v.live.as_ref().unwrap().account().unwrap().id, other.id);
            let commands: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(commands.iter().any(|cmd| matches!(cmd, Cmd::Op { id, act: Act::Archive, .. } if *id == message)));
            assert!(!commands.iter().any(|cmd| matches!(cmd, Cmd::Op { id, .. } if *id != message)));
        });
    }

}

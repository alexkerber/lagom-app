use super::*;
use gpui::Focusable;
use lagom_core::store::Store;
use lagom_core::tasks::{Task as LocalTask, TaskList};

static DATE_LOCALE: std::sync::LazyLock<chrono::Locale> = std::sync::LazyLock::new(|| {
    // shortcut: unsupported locale tags use English; expand matching when language preferences are added.
    sys_locale::get_locale().and_then(|locale| locale.replace('-', "_").parse().ok()).unwrap_or(chrono::Locale::en_US)
});

pub(crate) enum TaskChange {
    Add { list: i64, title: String },
    FromMail(lagom_core::store::MessageSummary),
    Rename(i64, String),
    Complete(i64),
    Delete(i64),
    Move { id: i64, list: i64, position: i64 },
    AddList(String),
    RenameList(i64, String),
    DeleteList(i64),
    DefaultList(i64),
}

impl TaskChange {
    fn apply(self, store: &Store) -> lagom_core::Result<Option<i64>> {
        match self {
            Self::Add { list, title } => return store.add_task(list, &title, "", None).map(|t| Some(t.id)),
            Self::FromMail(mail) => return store.task_from_mail(store.default_task_list()?, &mail).map(|t| Some(t.id)),
            Self::Rename(id, title) => store.rename_task(id, &title)?,
            Self::Complete(id) => store.set_done(id, true)?,
            Self::Delete(id) => store.delete_task(id)?,
            Self::Move { id, list, position } => store.move_task(id, list, position)?,
            Self::AddList(name) => { store.add_task_list(&name)?; }
            Self::RenameList(id, name) => store.rename_task_list(id, &name)?,
            Self::DeleteList(id) => store.delete_task_list(id)?,
            Self::DefaultList(id) => store.set_default_task_list(id)?,
        }
        Ok(None)
    }
}

#[derive(Default)]
pub(super) struct Today {
    pub(super) lists: Vec<TaskList>,
    pub(super) default: Option<i64>,
    pub(super) events: Vec<lagom_core::calendar::AgendaEvent>,
    pub(super) calendar_accounts: Vec<(lagom_core::account::Account, bool)>,
    pub(super) calendar_sync: std::collections::HashMap<String, crate::sync::SyncState>,
    task_access: std::collections::HashSet<String>,
    pub(super) task_sync: std::collections::HashMap<String, crate::sync::SyncState>,
    pub(super) tasks: Vec<LocalTask>,
    pub(super) unread: usize,
    completed: HashSet<i64>,
    menu: Option<TaskMenu>,
    menu_focus: Option<FocusHandle>,
    menu_index: usize,
    focused: bool,
    leaving: Vec<LocalTask>,
    selected: Option<i64>,
    editor: Option<(Edit, Entity<TextInput>)>,
    pub(super) scroll: ScrollHandle,
    bounds: Rc<std::cell::RefCell<HashMap<i64, gpui::Bounds<Pixels>>>>,
    day: String,
    midnight: Option<Task<()>>,
}

impl Today {
    fn demo(mail: &mut [Mail]) -> Self {
        let linked = mail.iter_mut().find(|m| m.subject == "Board meeting moved to Thursday");
        let link = linked.map(|mail| {
            mail.message_id = "<board-meeting@demo.invalid>".into();
            (mail.account.clone(), mail.message_id.clone(), mail.subject.clone())
        });
        let tasks = [(1, "Prepare for the board meeting", false), (1, "Pick up groceries", false),
            (1, "Book a table", true), (2, "Review the project brief", false), (2, "Plan next week", false)]
            .into_iter().enumerate().map(|(i, (list, title, done))| {
                let link = link.as_ref().filter(|_| i == 0);
                LocalTask { id: i as i64 + 1, list, remote_id: None, title: title.into(), notes: String::new(),
                    due: (i == 0 || i == 3).then(|| (chrono::Local::now().date_naive() - chrono::Duration::days(i64::from(i == 3))).to_string()), done_at: done.then_some(1), position: Some(i as i64),
                    mail_account: link.map(|l| l.0.clone()), mail_message_id: link.map(|l| l.1.clone()), mail_subject: link.map(|l| l.2.clone()),
                    created: 0, updated: 0, dirty: false, deleted: false }
            }).collect();
        Self { lists: ["Personal", "Work"].into_iter().enumerate().map(|(i, name)| TaskList {
            id: i as i64 + 1, account: None, remote_id: None, name: name.into(), position: i as i64, google_default: false,
        }).collect(), default: Some(1), tasks, ..Default::default() }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TaskMenu { List(i64), Task(i64), Move(i64), DeleteList(i64) }

#[derive(Clone, Copy, PartialEq)]
enum Edit { Add(i64), Rename(i64), AddList, RenameList(i64) }

#[derive(Clone, Copy)]
enum MenuAction { Edit(Edit), Menu(TaskMenu), Delete(i64), DeleteList(i64), Move(i64, i64), Cancel }
struct MenuItem { id: String, label: String, icon: &'static str, danger: bool, action: MenuAction }

actions!(tasks, [Next, Prev, Rename, Complete, Earlier, Later, OpenTaskMenu]);

pub(super) fn bind_keys(cx: &mut App) {
    let context = Some("Tasks && !TextInput && !TaskMenu");
    cx.bind_keys([
        KeyBinding::new("t", NewTask, context),
        KeyBinding::new("down", Next, context),
        KeyBinding::new("j", Next, context),
        KeyBinding::new("up", Prev, context),
        KeyBinding::new("k", Prev, context),
        KeyBinding::new("enter", Rename, context),
        KeyBinding::new("alt-enter", OpenTaskMenu, context),
        KeyBinding::new("space", Complete, context),
        KeyBinding::new("alt-up", Earlier, context),
        KeyBinding::new("alt-down", Later, context),
    ]);
}

impl MainWindow {
    pub(super) fn close_task_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.today.menu.take().is_none() && self.today.editor.take().is_none() { return false; }
        window.focus(&self.focus);
        cx.notify();
        true
    }

    pub(super) fn reload_tasks(&mut self, cx: &mut Context<Self>) {
        self.reload_agenda(cx);
        self.reload_unread(cx);
        if matches!(self.page, Page::Calendar) { self.reload_week(cx); }
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let Some(live) = &self.live else {
            if self.demo && self.today.day.is_empty() {
                let demo = Today::demo(&mut self.inbox.mail);
                self.today.lists = demo.lists;
                self.today.default = demo.default;
                self.today.tasks = demo.tasks;
            }
            self.today.day = day;
            return;
        };
        let snapshot = (|| -> lagom_core::Result<_> {
            let mut access = std::collections::HashSet::new();
            for (account, _) in &self.today.calendar_accounts {
                if lagom_core::oauth::tasks_allowed(&live.store.account_scopes(&account.id)?) { access.insert(account.id.clone()); }
            }
            let lists = live.store.task_lists()?;
            let tasks: Vec<LocalTask> = if matches!(self.page, Page::Home) { live.store.today_tasks(&day)? }
                else { lists.iter().map(|list| live.store.tasks(list.id, matches!(self.page, Page::Todo))).collect::<lagom_core::Result<Vec<_>>>()?.into_iter().flatten().collect() };
            let default = if lists.is_empty() { None } else { Some(live.store.default_task_list()?) };
            Ok((lists, default, tasks, access))
        })();
        match snapshot {
            Ok((lists, default, mut tasks, access)) => {
                self.today.task_access = access;
                self.today.task_sync.retain(|id, _| self.today.calendar_accounts.iter().any(|(a, _)| a.id == *id));
                tasks.sort_by_key(|task| (lists.iter().position(|l| l.id == task.list), task.position, task.id));
                self.today.lists = lists;
                self.today.default = default;
                self.today.tasks = tasks;
                self.today.day = day;
                if !self.visible_tasks().iter().any(|t| Some(t.id) == self.today.selected) {
                    self.today.selected = self.visible_tasks().first().map(|t| t.id);
                }
            }
            Err(e) => self.notify(format!("Could not read tasks: {e}"), cx),
        }
    }

    pub(super) fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.clear_motion();
        self.set_page(Page::Home, cx);
        self.reset_task_focus();
        self.reload_tasks(cx);
        window.focus(&self.focus);
        cx.notify();
    }

    pub(crate) fn save_task(&mut self, change: TaskChange, cx: &mut Context<Self>) -> bool {
        let Some(live) = &self.live else {
            if self.demo { return self.save_demo_task(change, cx); }
            self.notify("Could not save the task. Restart lagom and try again.", cx);
            return false;
        };
        // shortcut: task writes can block the UI for 250 ms on locks; move off-thread if this budget becomes too large.
        match live.store.with_short_lock_wait(|store| change.apply(store)) {
            Ok(created) => {
                let sync = live.store.accounts().unwrap_or_default().iter().any(|a|
                    a.auth == lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google)
                    && live.store.account_scopes(&a.id).is_ok_and(|s| lagom_core::oauth::tasks_allowed(&s)));
                if sync { let _ = live.bridge.send(crate::sync::Cmd::TasksEdited); }
                self.reload_tasks(cx);
                if let Some(id) = created {
                    self.notify("Task created", cx);
                    self.task_undo = Some(id);
                    if self.today.selected.is_none() { self.today.selected = Some(id); }
                    self.today.focused = true;
                    if let Some(toast) = &mut self.toast { toast.1 = true; }
                }
                cx.notify();
                true
            }
            Err(e) => {
                self.notify(format!("Could not save task: {e}"), cx);
                false
            }
        }
    }

    pub(super) fn task_from_mail(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(mail) = self.inbox.mail.get(id) else { return };
        self.save_task(TaskChange::FromMail(lagom_core::store::MessageSummary {
            account: mail.account.clone(), message_id: mail.message_id.clone(), subject: mail.subject.clone(),
            ..Default::default()
        }), cx);
    }

    pub(super) fn new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.page, Page::Mail) {
            if let Some(id) = self.inbox.single() {
                self.task_from_mail(id, cx);
                return;
            }
        }
        if !matches!(self.page, Page::Home | Page::Todo) { self.show_home(window, cx); }
        if let Some(list) = self.today.default { self.edit_task(Edit::Add(list), window, cx); }
    }

    fn edit_task(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.task_input(edit) {
            window.focus(&input.focus_handle(cx));
            return;
        }
        let text = match edit {
            Edit::Rename(id) => self.today.tasks.iter().find(|t| t.id == id).map(|t| t.title.as_str()),
            Edit::RenameList(id) => self.today.lists.iter().find(|l| l.id == id).map(|l| l.name.as_str()),
            _ => Some(""),
        };
        let Some(text) = text else { return };
        let placeholder = match edit { Edit::AddList | Edit::RenameList(_) => "List name", _ => "Task title" };
        let input = cx.new(|cx| TextInput::new(window, cx).placeholder(placeholder).with_text(text).bordered());
        cx.subscribe_in(&input, window, move |this, input, event: &TextInputEvent, window, cx| {
            match event {
                TextInputEvent::Enter => {
                    let title = input.read(cx).text().trim().to_string();
                    if title.is_empty() { this.notify("Enter a name", cx); return; }
                    let change = match edit {
                        Edit::Add(list) => TaskChange::Add { list, title },
                        Edit::Rename(id) => TaskChange::Rename(id, title),
                        Edit::AddList => TaskChange::AddList(title),
                        Edit::RenameList(id) => TaskChange::RenameList(id, title),
                    };
                    if this.save_task(change, cx) {
                        this.today.editor = None;
                        window.focus(&this.focus);
                    }
                }
                TextInputEvent::Escape => {
                    this.today.editor = None;
                    window.focus(&this.focus);
                }
                TextInputEvent::Changed => {}
            }
            cx.notify();
        }).detach();
        window.focus(&input.focus_handle(cx));
        self.today.editor = Some((edit, input));
        cx.notify();
    }

    fn select_task(&mut self, next: bool, cx: &mut Context<Self>) {
        let tasks = self.visible_tasks();
        let index = tasks.iter().position(|t| Some(t.id) == self.today.selected);
        let index = match index { Some(i) if next => (i + 1).min(tasks.len().saturating_sub(1)), Some(i) => i.saturating_sub(1), None => 0 };
        self.today.selected = tasks.get(index).map(|t| t.id);
        self.today.focused = self.today.selected.is_some();
        if let Some(row) = self.today.selected.and_then(|id| self.today.bounds.borrow().get(&id).copied()) {
            let viewport = self.today.scroll.bounds();
            let mut offset = self.today.scroll.offset();
            if row.top() < viewport.top() { offset.y += viewport.top() - row.top(); }
            else if row.bottom() > viewport.bottom() { offset.y -= row.bottom() - viewport.bottom(); }
            self.today.scroll.set_offset(offset);
        }
        cx.notify();
    }

    fn reorder_task(&mut self, down: bool, cx: &mut Context<Self>) {
        let tasks = self.visible_tasks();
        let Some(index) = tasks.iter().position(|t| Some(t.id) == self.today.selected) else { return };
        let other = if down { index.checked_add(1) } else { index.checked_sub(1) };
        let Some(other) = other.and_then(|i| tasks.get(i)) else { return };
        let task = tasks[index];
        if other.list != task.list { return; }
        let position = other.position.unwrap_or(0) + i64::from(down);
        self.save_task(TaskChange::Move { id: task.id, list: task.list, position }, cx);
    }

    fn task_input(&self, edit: Edit) -> Option<Entity<TextInput>> {
        self.today.editor.as_ref().filter(|(target, _)| *target == edit).map(|(_, input)| input.clone())
    }

    pub(super) fn task_keys<E: InteractiveElement>(element: E, cx: &mut Context<Self>) -> E {
        element
            .on_action(cx.listener(|this, _: &Next, _, cx| this.select_task(true, cx)))
            .on_action(cx.listener(|this, _: &Prev, _, cx| this.select_task(false, cx)))
            .on_action(cx.listener(|this, _: &Earlier, _, cx| this.reorder_task(false, cx)))
            .on_action(cx.listener(|this, _: &Later, _, cx| this.reorder_task(true, cx)))
            .on_action(cx.listener(|this, _: &OpenTaskMenu, window, cx| {
                if let Some(id) = this.today.selected { this.open_task_menu(TaskMenu::Task(id), window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Rename, window, cx| {
                if let Some(id) = this.today.selected { this.edit_task(Edit::Rename(id), window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Complete, _, cx| {
                if let Some(id) = this.today.selected { this.complete_task(id, cx); }
            }))
    }

    fn save_demo_task(&mut self, change: TaskChange, cx: &mut Context<Self>) -> bool {
        let next = self.today.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let mut created = None;
        match change {
            TaskChange::Add { list, title } => {
                if !self.today.lists.iter().any(|l| l.id == list) || title.trim().is_empty() { return false; }
                self.today.tasks.push(LocalTask { id: next, list, title, notes: String::new(), due: None, done_at: None,
                    position: Some(next), remote_id: None, mail_account: None, mail_message_id: None, mail_subject: None,
                    created: 0, updated: 0, dirty: false, deleted: false });
                created = Some(next);
            }
            TaskChange::FromMail(mail) => {
                let Some(list) = self.today.default else { return false };
                let title = if mail.subject.trim().is_empty() { "(no subject)".into() } else { mail.subject.clone() };
                if !self.save_demo_task(TaskChange::Add { list, title }, cx) { return false; }
                let task = self.today.tasks.last_mut().unwrap();
                if !mail.message_id.is_empty() {
                    task.mail_account = Some(mail.account); task.mail_message_id = Some(mail.message_id); task.mail_subject = Some(mail.subject);
                }
            }
            TaskChange::Rename(id, title) => { if let Some(task) = self.today.tasks.iter_mut().find(|t| t.id == id) { task.title = title; } else { return false; } }
            TaskChange::Complete(id) => { if let Some(task) = self.today.tasks.iter_mut().find(|t| t.id == id) { task.done_at = Some(chrono::Local::now().timestamp()); } else { return false; } }
            TaskChange::Delete(id) => self.today.tasks.retain(|t| t.id != id),
            TaskChange::Move { id, list, position } => {
                if !self.today.lists.iter().any(|l| l.id == list) { return false; }
                for task in &mut self.today.tasks { if task.list == list && task.position.unwrap_or(0) >= position { task.position = Some(task.position.unwrap_or(0) + 1); } }
                if let Some(task) = self.today.tasks.iter_mut().find(|t| t.id == id) { task.list = list; task.position = Some(position); }
                self.today.tasks.sort_by_key(|t| (t.list, t.position, t.id));
            }
            TaskChange::AddList(name) => {
                let id = self.today.lists.iter().map(|l| l.id).max().unwrap_or(0) + 1;
                self.today.lists.push(TaskList { id, name, position: id, account: None, remote_id: None, google_default: false });
                if self.today.default.is_none() { self.today.default = Some(id); }
            }
            TaskChange::RenameList(id, name) => { if let Some(list) = self.today.lists.iter_mut().find(|l| l.id == id) { list.name = name; } else { return false; } }
            TaskChange::DeleteList(id) => {
                let Some(default) = self.today.default.filter(|&d| d != id) else { return false };
                if self.today.lists.iter().any(|l| l.id == id && l.google_default) { return false; }
                for task in &mut self.today.tasks { if task.list == id { task.list = default; } }
                self.today.lists.retain(|l| l.id != id);
            }
            TaskChange::DefaultList(id) => self.today.default = Some(id),
        }
        if !self.today.tasks.iter().any(|t| Some(t.id) == self.today.selected) {
            self.today.selected = self.visible_tasks().first().map(|t| t.id);
        }
        if let Some(id) = created {
            if self.today.selected.is_none() { self.today.selected = Some(id); }
            self.today.focused = true;
            self.notify("Task created", cx);
            self.task_undo = Some(id);
            if let Some(toast) = &mut self.toast { toast.1 = true; }
        }
        cx.notify();
        true
    }

    pub(super) fn reset_task_focus(&mut self) {
        self.today.focused = false;
        self.today.menu = None;
        self.today.editor = None;
        self.today.scroll.set_offset(Point::default());
    }

    fn visible_tasks(&self) -> Vec<&LocalTask> {
        if matches!(self.page, Page::Home) { return today_tasks(&self.today.tasks, self.today.default, &self.today.day); }
        self.today.lists.iter().flat_map(|list| self.today.tasks.iter().filter(move |t| t.list == list.id && t.done_at.is_none())
            .chain(self.today.tasks.iter().filter(move |t| t.list == list.id && t.done_at.is_some() && self.today.completed.contains(&list.id)))).collect()
    }

    fn complete_task(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(mut task) = self.today.tasks.iter().find(|t| t.id == id && t.done_at.is_none()).cloned() else { return };
        let visible = self.visible_tasks();
        let index = visible.iter().position(|t| t.id == id).unwrap_or(0);
        if !self.save_task(TaskChange::Complete(id), cx) { return; }
        task.done_at = Some(chrono::Local::now().timestamp());
        self.today.leaving.push(task);
        let visible = self.visible_tasks();
        self.today.selected = visible.get(index.min(visible.len().saturating_sub(1))).map(|t| t.id);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MOTION).await;
            this.update(cx, |this, cx| { this.today.leaving.retain(|t| t.id != id); cx.notify(); }).ok();
        }).detach();
        cx.notify();
    }

    pub(super) fn task_hint(&self, window: &Window) -> Option<Div> {
        (self.today.focused && self.today.selected.is_some() && self.today.editor.is_none() && self.focus.is_focused(window))
            .then(|| mono("↑ ↓ select · Space complete · Enter rename · ⌥↑ ⌥↓ reorder", self.theme.ink2).debug_selector(|| "task-hint".into()))
    }

    pub(super) fn add_task_row(&self, list: Option<i64>, home: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        let row = div().id(("add-task", list.unwrap_or(0) as usize)).min_h(px(36.)).px(px(8.)).flex().items_center().gap(px(10.));
        if let Some(input) = list.and_then(|id| self.task_input(Edit::Add(id))) { return row.child(div().flex_1().child(input)); }
        pressable(row, &t, false).debug_selector(move || if home { "new-task".into() } else { format!("add-task-{}", list.unwrap_or(0)) })
            .rounded(px(radius::CONTROL)).text_color(t.ink2)
            .child(icon("icons/plus.svg", t.ink2, 16.)).child(if home { "New task" } else { "Add task" })
            .tooltip(move |_, cx| cx.new(|_| Tip(t, "New task · T".into())).into())
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(list) = list { this.edit_task(Edit::Add(list), window, cx); }
                else { this.edit_task(Edit::AddList, window, cx); }
            }))
    }

    pub(super) fn task_row(&self, task: &LocalTask, leaving: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let id = task.id;
        let done = task.done_at.is_some();
        let selected = self.today.focused && self.today.selected == Some(id);
        let row = pressable(div().id(("task", id as usize)), &t, selected).group("task-row")
            .debug_selector(move || format!("task-{id}")).flex().flex_shrink_0().items_center().gap(px(10.))
            .h(px(40.)).px(px(8.)).rounded(px(radius::CONTROL)).when(selected, |d| d.bg(t.sel))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.today.selected = Some(id); this.today.focused = true;
                if this.today.editor.is_none() { window.focus(&this.focus); } cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                if e.click_count == 2 { this.edit_task(Edit::Rename(id), window, cx); }
            }))
            .child(pressable(div().id("complete-task"), &t, false).debug_selector(move || format!("complete-task-{id}"))
                .size(px(18.)).flex_shrink_0().flex().items_center().justify_center().border_1().border_color(t.ink2).rounded_full()
                .when(done, |d| d.child(icon("icons/done.svg", t.ink2, 16.)))
                .tooltip(move |_, cx| cx.new(|_| Tip(t, "Complete task · Space".into())).into())
                .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.complete_task(id, cx); })))
            .child(div().debug_selector(move || format!("task-title-{id}")).flex_1().min_w(px(0.)).map(|d| match self.task_input(Edit::Rename(id)) {
                Some(input) => d.child(input), None => d.one_line().when(done, |d| d.text_color(t.ink2).line_through()).child(task.title.clone()),
            }))
            .when(task.mail_message_id.is_some(), |d| d.child(icon_button(&t, "task-mail", "icons/mail.svg", "Open mail")
                .debug_selector(move || format!("task-mail-{id}"))
                .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.open_task_mail(id, cx); }))))
            .when_some(task.due.as_deref().and_then(|due| due_label(due, &self.today.day, *DATE_LOCALE)),
                |d, (label, overdue)| d.child(due_chip(label, overdue, &t)))
            .when(matches!(self.page, Page::Todo), |d| d.child(icon_button(&t, "task-menu", "icons/more.svg", "Task actions · ⌥Enter")
                .debug_selector(move || format!("task-menu-{id}")).opacity(if selected { 1. } else { 0.2 }).group_hover("task-row", |s| s.opacity(1.))
                .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.open_task_menu(TaskMenu::Task(id), window, cx); }))));
        if leaving {
            row.with_animation(("task-complete", id as usize), Animation::new(MOTION), |row, delta| row.opacity(1. - ease_out_quint()(delta)).h(px(40. * (1. - spring(delta)).max(0.))).overflow_hidden()).into_any_element()
        } else {
            let bounds = self.today.bounds.clone();
            // Record the nested row's bounds so keyboard selection scrolls only when it leaves the viewport.
            div().flex_shrink_0().on_children_prepainted(move |children, _, _| {
                if let Some(row) = children.first() { bounds.borrow_mut().insert(id, *row); }
            }).child(row).into_any_element()
        }
    }

    pub(super) fn home_tasks(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let mut tasks = today_tasks(&self.today.tasks, self.today.default, &self.today.day);
        tasks.extend(self.today.leaving.iter());
        tasks.sort_by_key(|t| {
            let due = t.due.as_deref().filter(|due| due.get(..10).unwrap_or(due) <= self.today.day.as_str());
            (due.is_none(), due, t.position, t.id)
        });
        div().debug_selector(|| "home-tasks".into()).min_w(px(0.)).flex().flex_col().gap(px(4.))
            .child(div().font_weight(FontWeight::SEMIBOLD).mb(px(8.)).child("To do"))
            .when(tasks.is_empty() && self.today.leaving.is_empty(), |d| d.child(div().py(px(16.)).flex().flex_col().gap(px(8.))
                .child(icon("icons/todo.svg", t.ink2, 32.)).child(mono("Nothing to do today", t.ink2))))
            .children(tasks.into_iter().map(|task| self.task_row(task, self.today.leaving.iter().any(|t| t.id == task.id), cx)))
            .child(self.add_task_row(self.today.default, true, cx))
            .children(self.task_input(Edit::AddList).map(|input| div().child(input)))
    }

    fn open_task_menu(&mut self, menu: TaskMenu, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Todo) { return; }
        let focus = self.today.menu_focus.get_or_insert_with(|| cx.focus_handle());
        window.focus(focus);
        self.today.menu = Some(menu);
        self.today.menu_index = 0;
        cx.notify();
    }

    fn task_menu_items(&self) -> Vec<MenuItem> {
        let Some(menu) = self.today.menu else { return vec![] };
        let mut items = Vec::new();
        let mut add = |id: String, label: String, icon, danger, action| items.push(MenuItem { id, label, icon, danger, action });
        match menu {
            TaskMenu::List(id) | TaskMenu::Task(id) => {
                let list = matches!(menu, TaskMenu::List(_));
                add(format!("rename-{}-{id}", if list { "list" } else { "task" }), "Rename".into(), "icons/rename.svg", false,
                    MenuAction::Edit(if list { Edit::RenameList(id) } else { Edit::Rename(id) }));
                if !list { add("move-task".into(), "Move to list".into(), "icons/move.svg", false, MenuAction::Menu(TaskMenu::Move(id))); }
                if !list || (self.today.default != Some(id) && !self.today.lists.iter().any(|l| l.id == id && l.google_default)) {
                    add(format!("delete-{}-{id}", if list { "list" } else { "task" }), if list { "Delete list".into() } else { "Delete".into() },
                        "icons/trash.svg", true, if list { MenuAction::Menu(TaskMenu::DeleteList(id)) } else { MenuAction::Delete(id) });
                }
            }
            TaskMenu::Move(id) => {
                for list in self.today.lists.iter().filter(|l| !self.today.tasks.iter().any(|t| t.id == id && t.list == l.id)) {
                    add(format!("move-target-{}", list.id), list.name.clone(), "icons/todo.svg", false, MenuAction::Move(id, list.id));
                }
            }
            TaskMenu::DeleteList(id) => {
                add("confirm-delete-list".into(), "Delete list".into(), "icons/trash.svg", true, MenuAction::DeleteList(id));
                add("cancel-delete-list".into(), "Cancel".into(), "icons/close.svg", false, MenuAction::Cancel);
            }
        }
        items
    }

    fn choose_task_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.task_menu_items().get(index).map(|i| i.action) else { return };
        match action {
            MenuAction::Menu(menu) => { self.open_task_menu(menu, window, cx); return; }
            MenuAction::Edit(edit) => { self.today.menu = None; self.edit_task(edit, window, cx); return; }
            MenuAction::Delete(id) => { if !self.save_task(TaskChange::Delete(id), cx) { return; } }
            MenuAction::DeleteList(id) => { if !self.save_task(TaskChange::DeleteList(id), cx) { return; } }
            MenuAction::Move(id, list) => {
                let position = self.today.tasks.iter().filter(|t| t.list == list).filter_map(|t| t.position).max().unwrap_or(-1) + 1;
                if !self.save_task(TaskChange::Move { id, list, position }, cx) { return; }
            }
            MenuAction::Cancel => {}
        }
        self.today.menu = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn task_menu(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let menu = self.today.menu?;
        let t = self.theme;
        Some(div().id("task-actions").debug_selector(|| "task-actions".into()).absolute().top(px(58.)).right(px(24.)).w(px(240.))
            .occlude().p(px(6.)).rounded(px(radius::CONTROL)).border_1().border_color(t.line).bg(t.raised)
            .flex().flex_col().gap(px(4.)).key_context("TaskMenu").track_focus(self.today.menu_focus.as_ref()?)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| { this.today.menu = None; window.focus(&this.focus); cx.notify(); }))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => { this.today.menu = None; window.focus(&this.focus); }
                    "down" => this.today.menu_index = (this.today.menu_index + 1).min(this.task_menu_items().len().saturating_sub(1)),
                    "up" => this.today.menu_index = this.today.menu_index.saturating_sub(1),
                    "enter" | "space" => this.choose_task_menu(this.today.menu_index, window, cx),
                    _ => return,
                }
                cx.stop_propagation(); cx.notify();
            }))
            .when(matches!(menu, TaskMenu::DeleteList(_)), |d| d.child(div().p(px(8.)).child("Delete this list? Tasks move to the default list.")))
            .children(self.task_menu_items().into_iter().enumerate().map(|(ix, item)| {
                pressable(div().id(SharedString::from(item.id.clone())), &t, ix == self.today.menu_index)
                    .debug_selector(move || item.id.clone()).flex().items_center().gap(px(8.)).px(px(8.)).min_h(px(32.)).rounded(px(radius::KEY))
                    .when(ix == self.today.menu_index, |d| d.bg(t.sel))
                    .text_color(if item.danger { t.danger } else { t.ink })
                    .child(icon(item.icon, if item.danger { t.danger } else { t.ink2 }, 16.)).child(item.label)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| { if *hovered { this.today.menu_index = ix; cx.notify(); } }))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose_task_menu(ix, window, cx)))
            })))
    }

    pub(super) fn tasks_view(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        if day != self.today.day { self.reload_tasks(cx); }
        if self.today.midnight.is_none() {
            self.today.midnight = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(60)).await;
                this.update(cx, |this, cx| { this.today.midnight = None; cx.notify(); }).ok();
            }));
        }
        self.today.bounds.borrow_mut().clear();
        if matches!(self.page, Page::Home) { return self.home_view(window, cx); }
        let t = self.theme;
        // shortcut: Todo lays out every visible task, virtualize when large lists need bounded layout.
        let mut groups = div().id("today-groups").debug_selector(|| "today-groups".into()).flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.today.scroll).flex().flex_col().gap(px(24.));
        if self.today.lists.is_empty() {
            groups = groups.items_center().justify_center().gap(px(12.))
                .child(icon("icons/todo.svg", t.ink2, 32.))
                .child(div().debug_selector(|| "No lists yet".into()).text_color(t.ink2).child("No lists yet"));
        }
        for list in &self.today.lists {
            let id = list.id;
            let mut group = div().id(("task-list", id as usize)).debug_selector(move || format!("task-list-{id}"))
                .flex().flex_col().gap(px(4.)).flex_shrink_0()
                .child(div().flex().items_center().gap(px(8.))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).map(|d| match self.task_input(Edit::RenameList(id)) {
                        Some(input) => d.child(input), None => d.child(list.name.clone()),
                    }))
                    .child(icon_button(&t, "list-menu", "icons/more.svg", "List actions")
                        .debug_selector(move || format!("list-menu-{id}"))
                        .on_click(cx.listener(move |this, _, window, cx| { this.open_task_menu(TaskMenu::List(id), window, cx); }))));
            let mut rows: Vec<_> = self.today.tasks.iter().filter(|task| task.list == id && task.done_at.is_none())
                .chain(self.today.leaving.iter().filter(|task| task.list == id)).collect();
            rows.sort_by_key(|task| (task.position, task.id));
            for task in rows {
                group = group.child(self.task_row(task, self.today.leaving.iter().any(|t| t.id == task.id), cx));
            }
            group = group.child(self.add_task_row(Some(id), false, cx));
            let done: Vec<_> = self.today.tasks.iter().filter(|task| task.list == id && task.done_at.is_some()
                && !self.today.leaving.iter().any(|leaving| leaving.id == task.id)).collect();
            if !done.is_empty() {
                let expanded = self.today.completed.contains(&id);
                group = group.child(pressable(div().id(("completed", id as usize)), &t, false)
                    .debug_selector(move || format!("completed-{id}")).flex().items_center().gap(px(8.)).h(px(32.)).text_color(t.ink2)
                    .child(icon(if expanded { "icons/chevron.svg" } else { "icons/chevron-right.svg" }, t.ink2, 16.))
                    .child(format!("Completed ({})", done.len()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.today.completed.remove(&id) { this.today.completed.insert(id); }
                        if !this.visible_tasks().iter().any(|t| Some(t.id) == this.today.selected) {
                            this.today.selected = None; this.today.focused = false;
                        }
                        cx.notify();
                    })));
                if expanded { for task in done { group = group.child(self.task_row(task, false, cx)); } }
            }
            groups = groups.child(group);
        }
        div().relative().flex_1().min_w(px(0.)).p(px(24.)).flex().flex_col().gap(px(16.))
            .child(div().flex().items_center().justify_between()
                .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(size::TITLE)).child("Todo"))
                .child(icon_button(&t, "new-list", "icons/plus.svg", "New list")
                    .on_click(cx.listener(|this, _, window, cx| this.edit_task(Edit::AddList, window, cx)))))
            .children(self.task_input(Edit::AddList).map(|input| div().child(input)))
                .when(matches!(self.page, Page::Todo), |d| d.children(self.today.calendar_accounts.iter().filter_map(|(account, _)| {
                    let state = self.today.task_sync.get(&account.id);
                    let text = task_sync_label(self.today.task_access.contains(&account.id), state)?;
                    let id = account.id.clone();
                    let signing_in = self.today.calendar_sync.get(&id) == Some(&crate::sync::SyncState::SigningIn);
                    let allowed = self.today.task_access.contains(&id);
                    Some(div().flex().items_center().gap(px(8.)).debug_selector({ let id = id.clone(); move || format!("tasks-sync-{id}") })
                        .child(icon(if matches!(state, Some(crate::sync::SyncState::Failed(_))) { "icons/cloud-off.svg" } else { "icons/cloud.svg" }, t.ink2, 16.))
                        .child(mono(account.email.clone(), t.ink2))
                        .when(allowed, |d| d.child(mono(text.to_owned(), t.ink2)))
                        .when(!allowed, |d| d.child(button(&t, if signing_in { "Cancel sign-in" } else { "Allow Google Tasks access" }, None)
                            .debug_selector({ let id = id.clone(); move || format!("allow-tasks-{id}") })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let cmd = if signing_in { crate::sync::Cmd::CancelSignIn } else { crate::sync::Cmd::AllowCalendar(id.clone()) };
                                if let Some(live) = &this.live {
                                    if live.bridge.send(cmd) {
                                        this.today.calendar_sync.insert(id.clone(), if signing_in { crate::sync::SyncState::Synced(0) } else { crate::sync::SyncState::SigningIn });
                                    } else { this.worker_gone(cx); }
                                }
                                cx.notify();
                            })))))
                })))

            .child(groups)
            .children(self.task_hint(window))
            .children(self.task_menu(cx))
    }

}

fn due_label(due: &str, day: &str, locale: chrono::Locale) -> Option<(String, bool)> {
    use chrono::{Datelike, NaiveDate};
    let due = NaiveDate::parse_from_str(due.get(..10)?, "%Y-%m-%d").ok()?;
    let day = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let label = match (due - day).num_days() {
        -1 => "Yesterday".into(),
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        2..=6 => due.format_localized("%A", locale).to_string(),
        _ => due.format_localized(if due.year() == day.year() { "%-d %b" } else { "%-d %b %Y" }, locale).to_string(),
    };
    Some((label, due < day))
}

fn due_chip(label: String, overdue: bool, t: &Theme) -> Div {
    mono(label.clone(), if overdue { t.danger } else { t.ink2 }).debug_selector(move || format!("due-{label}"))
        .flex_shrink_0().px(px(6.)).py(px(3.)).rounded(px(radius::KEY))
        .bg(if overdue { Rgba { a: 0.1, ..t.danger } } else { t.subtle })
}

fn today_tasks<'a>(tasks: &'a [LocalTask], default: Option<i64>, day: &str) -> Vec<&'a LocalTask> {
    let due = |task: &&LocalTask| task.done_at.is_none() && !task.deleted
        && task.due.as_deref().is_some_and(|due| due.get(..10).unwrap_or(due) <= day);
    let mut result: Vec<_> = tasks.iter().filter(due).collect();
    result.sort_by_key(|t| (&t.due, t.position, t.id));
    result.extend(tasks.iter().filter(|t| t.done_at.is_none() && !t.deleted && Some(t.list) == default && !due(t)).take(5));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn review_due_dates_are_relative_then_localized_without_unneeded_years() {
        use chrono::Locale;
        for (due, label, overdue) in [
            ("2026-11-08", "Yesterday", true), ("2026-11-09T23:00:00Z", "Today", false),
            ("2026-11-10", "Tomorrow", false), ("2026-11-11", "Wednesday", false),
            ("2026-11-15", "Sunday", false), ("2026-11-16", "16 Nov", false),
            ("2026-11-07", "7 Nov", true), ("2027-01-12", "12 Jan 2027", false),
            ("2025-11-12", "12 Nov 2025", true),
        ] {
            assert_eq!(due_label(due, "2026-11-09", Locale::en_US), Some((label.into(), overdue)));
        }
        assert_eq!(due_label("2026-11-12", "2026-11-01", Locale::en_US), Some(("12 Nov".into(), false)));
        assert_eq!(due_label("2026-12-12", "2026-11-09", Locale::fr_FR), Some(("12 déc.".into(), false)));
        assert_eq!(due_label("2026-11-11", "2026-11-09", Locale::sv_SE), Some(("onsdag".into(), false)));
        assert_eq!(due_label("2027-01-01", "2026-12-31", Locale::en_US), Some(("Tomorrow".into(), false)));
        assert_eq!(due_label("2028-02-29", "2028-03-01", Locale::en_US), Some(("Yesterday".into(), true)));
        for due in ["", "2026-02-30", "not a date", "🗓🗓🗓"] {
            assert!(due_label(due, "2026-11-09", Locale::en_US).is_none());
        }
        assert!(due_label("2026-11-09", "", Locale::en_US).is_none());
    }

    #[test]
    fn review_only_overdue_chips_use_faint_danger_colors_in_both_themes() {
        for t in [Theme::light(DEFAULT_ACCENT), Theme::dark(DEFAULT_ACCENT)] {
            for overdue in [false, true] {
                let mut chip = due_chip("Date".into(), overdue, &t);
                assert_eq!(chip.style().text.as_ref().unwrap().color, Some(if overdue { t.danger } else { t.ink2 }.into()));
                assert_eq!(chip.style().background, Some(if overdue { Rgba { a: 0.1, ..t.danger } } else { t.subtle }.into()));
            }
        }
    }

    #[gpui::test]
    fn review_today_renders_overdue_today_then_rest_with_due_chips(cx: &mut TestAppContext) {
        for dark in [false, true] {
            let (view, cx) = cx.add_window_view(|window, cx| {
                let mut v = MainWindow::with_screen(true, "home", window, cx);
                v.theme = if dark { Theme::dark(DEFAULT_ACCENT) } else { Theme::light(DEFAULT_ACCENT) };
                v.today.tasks.reverse();
                v
            });
            cx.run_until_parked();
            let overdue = cx.debug_bounds("task-4").unwrap();
            let today = cx.debug_bounds("task-1").unwrap();
            let rest = cx.debug_bounds("task-2").unwrap();
            assert!(overdue.bottom() <= today.top() && today.bottom() <= rest.top());
            assert_eq!(view.read_with(cx, |v, _| v.visible_tasks().iter().map(|t| t.id).collect::<Vec<_>>()), [4, 1, 2]);
            assert!(cx.debug_bounds("due-Yesterday").is_some());
            assert!(cx.debug_bounds("due-Today").is_some());
        }
    }

    #[test]
    fn today_prioritizes_due_tasks_then_five_default_tasks_without_duplicates() {
        let mut tasks = Today::demo(&mut []).tasks;
        let day = "2026-11-09";
        for task in &mut tasks { task.due = None; }
        tasks[0].due = Some("2026-11-09T00:00:00Z".into());
        tasks[3].due = Some("2026-11-08".into());
        tasks[4].due = Some("2099-01-01".into());
        for id in 6..=15 {
            let mut task = tasks[1].clone(); task.id = id; tasks.push(task);
        }
        let selected = today_tasks(&tasks, Some(1), day);
        assert_eq!(selected.iter().map(|t| t.id).collect::<Vec<_>>(), [4, 1, 2, 6, 7, 8, 9]);
        assert!(selected.iter().all(|t| t.done_at.is_none()));
        assert_eq!(today_tasks(&tasks, None, day).iter().map(|t| t.id).collect::<Vec<_>>(), [4, 1]);
        tasks[1].due = Some("2099-01-01".into());
        assert!(today_tasks(&tasks, Some(1), day).iter().any(|t| t.id == 2), "the default-list fallback includes future tasks");
    }

    #[gpui::test]
    fn demo_inline_add_rename_completion_and_disclosure_persist_between_pages(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("task-3").is_none(), "completed starts collapsed");
        assert!(cx.debug_bounds("Rename").is_none());
        assert!(cx.debug_bounds("task-hint").is_none());
        let at = cx.debug_bounds("completed-1").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("task-3").is_some());
        let at = cx.debug_bounds("add-task-1").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_input("Fixture task"); cx.simulate_keystrokes("enter");
        let id = view.read_with(cx, |v, _| { assert!(v.today.editor.is_none(), "editor closed after adding"); assert_eq!(v.today.tasks.last().unwrap().title, "Fixture task"); v.today.tasks.last().unwrap().id });
        cx.run_until_parked();
        let at = cx.debug_bounds(format!("task-{id}").leak()).unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        view.update_in(cx, |v, window, _| { assert!(v.today.focused, "task focus flag"); assert!(v.today.editor.is_none(), "editor closed"); assert!(v.focus.is_focused(window), "root focused"); });
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("task-hint").is_some());
        cx.simulate_keystrokes("enter secondary-a"); cx.simulate_input("Renamed fixture"); cx.simulate_keystrokes("enter space");
        view.read_with(cx, |v, _| {
            let task = v.today.tasks.iter().find(|t| t.id == id).unwrap();
            assert_eq!(task.title, "Renamed fixture"); assert!(task.done_at.is_some());
            assert!(v.today.leaving.iter().any(|t| t.id == id));
        });
        cx.simulate_keystrokes("secondary-1 secondary-4");
        view.read_with(cx, |v, _| assert!(v.today.tasks.iter().any(|t| t.id == id && t.done_at.is_some())));
    }

    #[gpui::test]
    fn task_menu_keyboard_moves_and_deletes_with_list_confirmation(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        cx.run_until_parked();
        let at = cx.debug_bounds("task-2").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_keystrokes("alt-enter down enter enter");
        assert_eq!(view.read_with(cx, |v, _| v.today.tasks.iter().find(|t| t.id == 2).unwrap().list), 2);
        cx.simulate_keystrokes("alt-enter down down enter");
        assert!(view.read_with(cx, |v, _| !v.today.tasks.iter().any(|t| t.id == 2)));
        view.update_in(cx, |v, window, cx| v.open_task_menu(TaskMenu::List(2), window, cx));
        cx.simulate_keystrokes("down enter");
        assert!(view.read_with(cx, |v, _| v.today.lists.len() == 2));
        cx.simulate_keystrokes("down enter");
        assert!(view.read_with(cx, |v, _| v.today.lists.len() == 2), "Cancel preserves the list");
        view.update_in(cx, |v, window, cx| v.open_task_menu(TaskMenu::List(2), window, cx));
        cx.simulate_keystrokes("down enter enter");
        view.read_with(cx, |v, _| {
            assert_eq!(v.today.lists.len(), 1);
            assert!(v.today.tasks.iter().all(|t| t.list == 1));
        });
    }

    #[gpui::test]
    fn completed_row_exits_in_place_on_today_and_todo(cx: &mut TestAppContext) {
        for (screen, first, next) in [("home", 4, 1), ("todo", 1, 2)] {
            let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, screen, window, cx));
            cx.run_until_parked();
            let first_selector = format!("task-{first}").leak();
            let next_selector = format!("task-{next}").leak();
            let before = cx.debug_bounds(first_selector).unwrap().top();
            view.update(cx, |v, cx| v.complete_task(first, cx));
            cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
            let leaving = cx.debug_bounds(first_selector).unwrap();
            let following = cx.debug_bounds(next_selector).unwrap();
            assert_eq!(leaving.top(), before);
            assert!(leaving.top() < following.top());
            assert!(view.read_with(cx, |v, _| v.today.tasks.iter().find(|t| t.id == first).unwrap().done_at.is_some()));
        }
    }

    #[gpui::test]
    fn keyboard_selection_reaches_tasks_below_the_fold(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        view.update(cx, |v, cx| {
            let sample = v.today.tasks[0].clone();
            v.today.tasks = (1..=40).map(|id| { let mut task = sample.clone(); task.id = id; task.title = format!("Task {id}"); task }).collect();
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..40 { cx.simulate_keystrokes("down"); cx.run_until_parked(); }
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert_eq!(view.read_with(cx, |v, _| v.today.selected), Some(40));
        let row = cx.debug_bounds("task-40").unwrap();
        let viewport = cx.debug_bounds("today-groups").unwrap();
        assert!(row.top() >= viewport.top() && row.bottom() <= viewport.bottom(), "row {row:?}, viewport {viewport:?}, offset {:?}", view.read_with(cx, |v, _| v.today.scroll.offset()));
    }

    #[gpui::test]
    fn close_window_cancels_task_editor_without_removing_main(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        cx.run_until_parked();
        let original = view.read_with(cx, |v, _| v.today.tasks.iter().find(|t| t.id == 2).unwrap().title.clone());
        let at = cx.debug_bounds("task-menu-2").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_keystrokes("secondary-w");
        assert!(view.read_with(cx, |v, _| v.today.menu.is_none()));
        cx.simulate_click(at, Default::default());
        cx.simulate_keystrokes("enter secondary-a");
        cx.simulate_input("Uncommitted title");
        cx.simulate_keystrokes("secondary-w");
        view.read_with(cx, |v, _| {
            assert!(v.today.editor.is_none());
            assert_eq!(v.today.tasks.iter().find(|t| t.id == 2).unwrap().title, original);
        });
        assert_eq!(cx.cx.update(|cx| cx.windows().len()), 1);
    }

    #[gpui::test]
    fn task_menu_shortcut_on_today_preserves_task_focus(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "home", window, cx));
        cx.run_until_parked();
        let at = cx.debug_bounds("task-4").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.simulate_keystrokes("alt-enter");
        view.update_in(cx, |v, window, _| {
            assert!(v.today.menu.is_none());
            assert!(v.focus.is_focused(window));
            assert_eq!(v.today.selected, Some(4));
        });
        cx.simulate_keystrokes("escape down enter");
        assert!(view.read_with(cx, |v, _| matches!(v.today.editor, Some((Edit::Rename(1), _)))));
    }

    #[gpui::test]
    fn reopening_task_rename_restores_editor_focus(cx: &mut TestAppContext) {
        reopening_rename_restores_editor_focus(cx, Edit::Rename(2), "task-menu-2");
    }

    #[gpui::test]
    fn reopening_list_rename_restores_editor_focus(cx: &mut TestAppContext) {
        reopening_rename_restores_editor_focus(cx, Edit::RenameList(2), "list-menu-2");
    }

    fn reopening_rename_restores_editor_focus(cx: &mut TestAppContext, edit: Edit, menu: &'static str) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        for finish in ["escape", "enter"] {
            cx.run_until_parked();
            let at = cx.debug_bounds(menu).unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_keystrokes("enter secondary-a");
            cx.simulate_input("Unsaved name");
            let input = view.read_with(cx, |v, _| v.task_input(edit).unwrap());
            cx.run_until_parked();
            let at = cx.debug_bounds(menu).unwrap().center();
            cx.simulate_click(at, Default::default());
            view.update_in(cx, |v, window, _| assert!(v.today.menu_focus.as_ref().unwrap().is_focused(window)));
            cx.simulate_keystrokes("enter");
            view.update_in(cx, |v, window, cx| {
                assert!(v.today.menu.is_none());
                assert_eq!(v.task_input(edit).unwrap(), input);
                assert!(input.focus_handle(cx).is_focused(window));
                assert_eq!(input.read(cx).text(), "Unsaved name");
            });
            cx.simulate_input(" continued");
            assert_eq!(input.read_with(cx, |input, _| input.text().to_owned()), "Unsaved name continued");
            cx.simulate_keystrokes(finish);
            view.update_in(cx, |v, window, _| {
                assert!(v.today.editor.is_none());
                assert!(v.focus.is_focused(window));
                let (name, original) = match edit {
                    Edit::Rename(id) => (v.today.tasks.iter().find(|t| t.id == id).unwrap().title.as_str(), "Pick up groceries"),
                    Edit::RenameList(id) => (v.today.lists.iter().find(|l| l.id == id).unwrap().name.as_str(), "Work"),
                    _ => unreachable!(),
                };
                assert_eq!(name, if finish == "enter" { "Unsaved name continued" } else { original });
            });
        }
    }

    #[gpui::test]
    fn double_click_in_task_editor_preserves_text_selection_and_undo(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        for screen in ["home", "todo"] {
            let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, screen, window, cx));
            cx.run_until_parked();
            let at = cx.debug_bounds("task-2").unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_keystrokes("enter secondary-a");
            cx.simulate_input("Buy groceries");
            cx.run_until_parked();
            let input = view.read_with(cx, |v, _| v.task_input(Edit::Rename(2)).unwrap());
            let row = cx.debug_bounds("task-2").unwrap();
            let at = gpui::point(row.left() + px(50.), row.center().y);
            cx.simulate_event(MouseDownEvent { position: at, button: MouseButton::Left, click_count: 2, ..Default::default() });
            view.read_with(cx, |v, cx| {
                assert_eq!(v.task_input(Edit::Rename(2)).unwrap(), input);
                assert_eq!(input.read(cx).text(), "Buy groceries");
            });
            cx.simulate_input("Fresh");
            assert_eq!(input.read_with(cx, |input, _| input.text().to_owned()), "Fresh groceries");
            cx.simulate_keystrokes("secondary-z secondary-z");
            assert_eq!(input.read_with(cx, |input, _| input.text().to_owned()), "Pick up groceries");
            cx.simulate_keystrokes("secondary-shift-z secondary-shift-z enter");
            view.read_with(cx, |v, _| {
                assert!(v.today.editor.is_none());
                assert_eq!(v.today.tasks.iter().find(|t| t.id == 2).unwrap().title, "Fresh groceries");
            });
        }
    }

    #[gpui::test]
    fn double_click_renames_inline_and_escape_preserves_title(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        cx.run_until_parked();
        let at = cx.debug_bounds("task-2").unwrap().center();
        cx.simulate_event(MouseDownEvent { position: at, button: MouseButton::Left, click_count: 2, ..Default::default() });
        assert!(view.read_with(cx, |v, _| matches!(v.today.editor, Some((Edit::Rename(2), _)))));
        cx.simulate_keystrokes("secondary-a"); cx.simulate_input("Discard this"); cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.today.editor.is_none()));
        assert_eq!(view.read_with(cx, |v, _| v.today.tasks.iter().find(|t| t.id == 2).unwrap().title.clone()), "Pick up groceries");
    }

    #[gpui::test]
    fn review_todo_demo_has_sample_lists_and_a_working_mail_link(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "todo", window, cx));
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(v.live.is_none() && v.preferences_path.is_none());
            assert_eq!(v.today.lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Personal", "Work"]);
            assert_eq!(v.today.tasks.iter().filter(|t| t.list == 1).count(), 3);
            assert_eq!(v.today.tasks.iter().filter(|t| t.list == 2).count(), 2);
            assert_eq!(v.today.tasks.iter().filter(|t| t.done_at.is_some()).count(), 1);
        });
        assert!(cx.debug_bounds("task-list-1").is_some());
        assert!(cx.debug_bounds("task-list-2").is_some());
        let at = cx.debug_bounds("task-mail-1").unwrap().center();
        cx.simulate_click(at, Default::default());
        view.read_with(cx, |v, _| {
            assert!(matches!(v.page, Page::Mail));
            assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].subject, "Board meeting moved to Thursday");
        });
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        assert_eq!(view.read_with(cx, |v, _| v.today.tasks.len()), 5);
    }

    #[gpui::test]
    fn review_mail_button_follows_task_title_and_opens_mail_on_both_pages(cx: &mut TestAppContext) {
        for dark in [false, true] {
            for screen in ["home", "todo"] {
                let (view, cx) = cx.add_window_view(|window, cx| {
                    let mut v = MainWindow::with_screen(true, screen, window, cx);
                    v.theme = if dark { Theme::dark(DEFAULT_ACCENT) } else { Theme::light(DEFAULT_ACCENT) };
                    v
                });
                cx.run_until_parked();
                let title = cx.debug_bounds("task-title-1").unwrap();
                let mail = cx.debug_bounds("task-mail-1").unwrap();
                let due = cx.debug_bounds("due-Today").unwrap();
                assert!(title.right() <= mail.left() && mail.right() <= due.left());
                assert_eq!(mail.size, gpui::size(px(28.), px(28.)));
                assert!(cx.debug_bounds("task-mail-2").is_none());
                cx.simulate_click(mail.center(), Default::default());
                view.read_with(cx, |v, _| {
                    assert!(matches!(v.page, Page::Mail));
                    assert_eq!(v.inbox.mail[v.inbox.current().unwrap()].subject, "Board meeting moved to Thursday");
                    assert!(v.today.editor.is_none());
                });
            }
        }
    }

    #[gpui::test]
    fn review_todo_header_and_list_buttons_work_at_rest_in_both_themes(cx: &mut TestAppContext) {
        cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
        for dark in [false, true] {
            let (view, cx) = cx.add_window_view(|window, cx| {
                let mut v = MainWindow::with_screen(true, "todo", window, cx);
                v.theme = if dark { Theme::dark(DEFAULT_ACCENT) } else { Theme::light(DEFAULT_ACCENT) };
                v
            });
            cx.run_until_parked();
            let new = cx.debug_bounds("new-list").unwrap();
            for (list, menu) in [("task-list-1", "list-menu-1"), ("task-list-2", "list-menu-2")] {
                let list = cx.debug_bounds(list).unwrap();
                let menu = cx.debug_bounds(menu).unwrap();
                assert_eq!(menu.right(), list.right());
                assert_eq!(menu.size, gpui::size(px(28.), px(28.)));
                assert!(new.bottom() <= menu.top());
            }
            let at = cx.debug_bounds("list-menu-2").unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.run_until_parked();
            view.read_with(cx, |v, _| {
                let items = v.task_menu_items();
                assert_eq!(items.iter().map(|i| (i.label.as_str(), i.icon)).collect::<Vec<_>>(),
                    [("Rename", "icons/rename.svg"), ("Delete list", "icons/trash.svg")]);
            });
            let at = cx.debug_bounds("rename-list-2").unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_keystrokes("secondary-a"); cx.simulate_input("Renamed fixture"); cx.simulate_keystrokes("enter");
            assert_eq!(view.read_with(cx, |v, _| v.today.lists[1].name.clone()), "Renamed fixture");
            cx.run_until_parked();
            let at = cx.debug_bounds("list-menu-2").unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.run_until_parked();
            let at = cx.debug_bounds("delete-list-2").unwrap().center();
            cx.simulate_click(at, Default::default());
            assert!(view.read_with(cx, |v, _| matches!(v.today.menu, Some(TaskMenu::DeleteList(2)))));
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            let at = cx.debug_bounds("new-list").unwrap().center();
            cx.simulate_click(at, Default::default());
            cx.simulate_input("Created fixture"); cx.simulate_keystrokes("enter");
            assert_eq!(view.read_with(cx, |v, _| v.today.lists.last().unwrap().name.clone()), "Created fixture");
        }
    }

    #[gpui::test]
    fn review_todo_no_lists_shows_centered_add_list(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = MainWindow::with_screen(true, "todo", window, cx);
            view.demo = false;
            view.today.lists.clear();
            view.today.tasks.clear();
            view
        });
        cx.run_until_parked();
        let empty = cx.debug_bounds("No lists yet").expect("empty state");
        let icon = cx.debug_bounds("icon-icons/todo.svg").unwrap();
        assert_eq!(icon.size, gpui::size(px(32.), px(32.)));
        assert!(icon.bottom() <= empty.top());
        let groups = cx.debug_bounds("today-groups").unwrap();
        assert!((empty.center().x - groups.center().x).abs() < px(1.));
        assert!((empty.center().y - groups.center().y).abs() < px(30.));
        let at = cx.debug_bounds("new-list").unwrap().center();
        cx.simulate_click(at, Default::default());
        view.read_with(cx, |v, _| assert!(matches!(v.today.editor, Some((Edit::AddList, _)))));
    }
}

fn task_sync_label(allowed: bool, state: Option<&crate::sync::SyncState>) -> Option<&str> {
    if !allowed { return Some("Allow Google Tasks access"); }
    match state {
        Some(crate::sync::SyncState::Synced(_)) => Some("Synced with Google Tasks"),
        Some(crate::sync::SyncState::Failed(error)) => Some(error.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod sync_status_tests {
    use super::*;
    use crate::sync::SyncState;

    #[test]
    fn tasks_status_never_claims_sync_before_success_or_after_scope_loss() {
        assert_eq!(task_sync_label(true, None), None);
        assert_eq!(task_sync_label(true, Some(&SyncState::Synced(1))), Some("Synced with Google Tasks"));
        assert_eq!(task_sync_label(true, Some(&SyncState::Failed("offline".into()))), Some("offline"));
        assert_eq!(task_sync_label(false, Some(&SyncState::Synced(1))), Some("Allow Google Tasks access"));
    }
}

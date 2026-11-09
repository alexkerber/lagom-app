use super::*;
use gpui::Focusable;
use lagom_core::store::Store;
use lagom_core::tasks::{Task as LocalTask, TaskList};

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
    tasks: Vec<LocalTask>,
    selected: Option<i64>,
    editor: Option<(Edit, Entity<TextInput>)>,
    scroll: ScrollHandle,
    day: String,
    midnight: Option<Task<()>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Edit { Add(i64), Rename(i64), AddList, RenameList(i64) }

actions!(tasks, [Next, Prev, Rename, Complete, Earlier, Later]);

pub(super) fn bind_keys(cx: &mut App) {
    let context = Some("Today && !TextInput");
    cx.bind_keys([
        KeyBinding::new("t", NewTask, context),
        KeyBinding::new("down", Next, context),
        KeyBinding::new("j", Next, context),
        KeyBinding::new("up", Prev, context),
        KeyBinding::new("k", Prev, context),
        KeyBinding::new("enter", Rename, context),
        KeyBinding::new("space", Complete, context),
        KeyBinding::new("alt-up", Earlier, context),
        KeyBinding::new("alt-down", Later, context),
    ]);
}

impl MainWindow {
    pub(super) fn reload_tasks(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let snapshot = (|| -> lagom_core::Result<_> {
            Ok((live.store.task_lists()?, live.store.default_task_list()?, live.store.today_tasks(&day)?))
        })();
        match snapshot {
            Ok((lists, default, mut tasks)) => {
                tasks.sort_by_key(|task| (lists.iter().position(|l| l.id == task.list), task.position, task.id));
                self.today.lists = lists;
                self.today.default = Some(default);
                self.today.tasks = tasks;
                self.today.day = day;
                if !self.today.tasks.iter().any(|t| Some(t.id) == self.today.selected) {
                    self.today.selected = self.today.tasks.first().map(|t| t.id);
                }
            }
            Err(e) => self.notify(format!("Could not read tasks: {e}"), cx),
        }
    }

    pub(super) fn show_today(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.clear_motion();
        self.page = Page::Today;
        self.reload_tasks(cx);
        window.focus(&self.focus);
        cx.notify();
    }

    pub(crate) fn save_task(&mut self, change: TaskChange, cx: &mut Context<Self>) -> bool {
        let Some(live) = &self.live else {
            self.notify("Tasks need the local store; unavailable in demo mode", cx);
            return false;
        };
        // shortcut: task writes can block the UI for 250 ms on locks; move off-thread if this budget becomes too large.
        match live.store.with_short_lock_wait(|store| change.apply(store)) {
            Ok(created) => {
                self.reload_tasks(cx);
                if let Some(id) = created {
                    self.notify("Task created", cx);
                    self.task_undo = Some(id);
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
        self.show_today(window, cx);
        if let Some(list) = self.today.default { self.edit_task(Edit::Add(list), window, cx); }
    }

    fn edit_task(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
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
        let index = self.today.tasks.iter().position(|t| Some(t.id) == self.today.selected).unwrap_or(0);
        let index = if next { (index + 1).min(self.today.tasks.len().saturating_sub(1)) } else { index.saturating_sub(1) };
        self.today.selected = self.today.tasks.get(index).map(|t| t.id);
        if let Some(task) = self.today.tasks.get(index) {
            let group = self.today.lists.iter().position(|l| l.id == task.list).unwrap_or(0);
            let empty = self.today.lists[..group].iter().filter(|l| !self.today.tasks.iter().any(|t| t.list == l.id)).count();
            self.today.scroll.scroll_to_item(index + group + empty + 1);
        }
        cx.notify();
    }

    fn reorder_task(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(index) = self.today.tasks.iter().position(|t| Some(t.id) == self.today.selected) else { return };
        let other = if down { index.checked_add(1) } else { index.checked_sub(1) };
        let Some(other) = other.and_then(|i| self.today.tasks.get(i)) else { return };
        let task = &self.today.tasks[index];
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
            .on_action(cx.listener(|this, _: &Rename, window, cx| {
                if let Some(id) = this.today.selected { this.edit_task(Edit::Rename(id), window, cx); }
            }))
            .on_action(cx.listener(|this, _: &Complete, _, cx| {
                if let Some(id) = this.today.selected { this.save_task(TaskChange::Complete(id), cx); }
            }))
    }

    pub(super) fn tasks_view(&mut self, cx: &mut Context<Self>) -> Div {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        if day != self.today.day { self.reload_tasks(cx); }
        if self.today.midnight.is_none() {
            use chrono::TimeZone;
            let now = chrono::Local::now();
            let tomorrow = now.date_naive().succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap();
            let next = chrono::Local.from_local_datetime(&tomorrow).earliest()
                .unwrap_or_else(|| chrono::Local.from_utc_datetime(&tomorrow));
            let delay = (next - now).to_std().unwrap_or(Duration::from_secs(60));
            self.today.midnight = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| { this.today.midnight = None; cx.notify(); }).ok();
            }));
        }
        let t = self.theme;
        // shortcut: Today lays out every open task, virtualize when large task lists need bounded layout.
        let mut groups = div().id("today-groups").flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.today.scroll).flex().flex_col().gap(px(4.));
        for list in &self.today.lists {
            let list_id = list.id;
            let mut group = div().id(("task-list", list_id as usize)).flex().flex_col().gap(px(4.)).mt(px(16.)).flex_shrink_0()
                .child(div().flex().items_center().gap(px(8.))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD)
                        .map(|d| match self.task_input(Edit::RenameList(list_id)) {
                            Some(input) => d.child(input), None => d.child(list.name.clone()),
                        }))
                    .child(button(&t, "Add task", None).on_click(cx.listener(move |this, _, window, cx| this.edit_task(Edit::Add(list_id), window, cx))))
                    .child(button(&t, "Rename", None).debug_selector(move || format!("rename-list-{list_id}")).on_click(cx.listener(move |this, _, window, cx| this.edit_task(Edit::RenameList(list_id), window, cx))))
                    .when(self.today.default != Some(list_id), |d| d.child(button(&t, "Delete list", None).debug_selector(move || format!("delete-list-{list_id}"))
                        .on_click(cx.listener(move |this, _, _, cx| { this.save_task(TaskChange::DeleteList(list_id), cx); })))));
            group = group.children(self.task_input(Edit::Add(list_id)).map(|input| div().child(input)));
            let tasks: Vec<_> = self.today.tasks.iter().filter(|task| task.list == list_id).collect();
            groups = groups.child(group);
            if tasks.is_empty() { groups = groups.child(mono("No open tasks for today", t.ink2).py(px(8.))); }
            for task in tasks {
                let id = task.id;
                let selected = self.today.selected == Some(id);
                groups = groups.child(pressable(div().id(("task", id as usize)), &t, selected)
                    .flex().flex_shrink_0().items_center().gap(px(10.)).min_h(px(40.)).px(px(8.)).rounded(px(radius::CONTROL))
                    .when(selected, |d| d.bg(t.sel))
                    .on_click(cx.listener(move |this, _, window, cx| { this.today.selected = Some(id); if this.today.editor.is_none() { window.focus(&this.focus); } cx.notify(); }))
                    .child(pressable(div().id("complete-task"), &t, false).debug_selector(move || format!("complete-task-{id}"))
                        .size(px(20.)).border_1().border_color(t.ink2).rounded(px(radius::KEY))
                        .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.save_task(TaskChange::Complete(id), cx); })))
                    .child(div().flex_1().min_w(px(0.)).one_line()
                        .map(|d| match self.task_input(Edit::Rename(id)) {
                            Some(input) => d.child(input), None => d.child(task.title.clone()),
                        }))
                    .when_some(task.due.clone(), |d, due| d.child(mono(due, t.ink2)))
                    .when(task.mail_message_id.is_some(), |d| d.child(button(&t, "Open mail", None)
                        .child(icon("icons/link.svg", t.ink2, 14.))
                        .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.open_task_mail(id, cx); }))))
                    .child(button(&t, "Rename", None).on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation(); this.edit_task(Edit::Rename(id), window, cx);
                    }))));
            }
        }
        div().flex_1().min_w(px(0.)).p(px(24.)).flex().flex_col().gap(px(16.))
            .child(div().id("today").flex_1().min_h(px(0.)).flex().flex_col().gap(px(16.))
                .child(div().flex().items_center().justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(size::TITLE)).child("Today"))
                    .child(button(&t, "Add list", None).on_click(cx.listener(|this, _, window, cx| this.edit_task(Edit::AddList, window, cx)))))
                .children(self.task_input(Edit::AddList).map(|input| div().child(input)))
                .child(groups)
                .child(mono("↑ ↓ select · Space complete · Enter rename · ⌥↑ ⌥↓ reorder", t.ink2)))
    }
}

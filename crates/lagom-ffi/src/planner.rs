//! Today, Todo and Calendar for mobile: local tasks and read-only cached events from core.
use super::*;
use chrono::{Local, NaiveDate, TimeZone};
use lagom_core::{calendar::AgendaEvent, tasks::{Task, TaskList}};

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TodoList { pub id: i64, pub name: String, pub is_default: bool }
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct Todo {
    pub id: i64, pub list: i64, pub title: String,
    /// `YYYY-MM-DD`, or an RFC 3339 instant from a synced provider; the first ten characters are the date.
    pub due: Option<String>, pub done: bool, pub from_mail: bool,
}
impl From<Task> for Todo {
    fn from(t: Task) -> Self { Self { id: t.id, list: t.list, title: t.title, due: t.due, done: t.done_at.is_some(), from_mail: t.mail_message_id.is_some_and(|m| !m.is_empty()) } }
}
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AgendaItem {
    /// The local day (`YYYY-MM-DD`) this row is listed under; multi-day events repeat per day.
    pub day: String, pub account: String, pub calendar: String, pub title: String,
    pub start_unix: i64, pub end_unix: i64, pub all_day: bool, pub location: Option<String>, pub join_url: Option<String>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct TodaySummary {
    pub day: String, pub events: Vec<AgendaItem>, pub tasks: Vec<Todo>, pub unread_count: u32, pub unread: Vec<MailRow>,
}
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct MailCounts { pub account: String, pub inbox: u64, pub later: u64 }

fn item(day: &str, e: AgendaEvent) -> AgendaItem {
    AgendaItem { day: day.into(), account: e.account, calendar: e.calendar, title: e.event.title, start_unix: e.event.start_utc,
        end_unix: e.event.end_utc, all_day: e.event.all_day, location: e.event.location, join_url: e.event.join_url }
}
/// The first instant of a local date; midnight can be skipped by a time zone transition.
fn day_start(date: NaiveDate) -> i64 {
    let mut time = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    loop {
        if let Some(instant) = Local.from_local_datetime(&time).earliest() { return instant.timestamp(); }
        time += chrono::Duration::seconds(1);
    }
}
fn events_on(s: &Store, date: NaiveDate) -> lagom_core::Result<Vec<AgendaItem>> {
    let next = date.succ_opt().ok_or(lagom_core::Error::Invalid("date is outside the calendar range"))?;
    let day = date.to_string();
    Ok(s.today_events(&day, day_start(date), day_start(next))?.into_iter().map(|e| item(&day, e)).collect())
}
/// Core compares due dates as text, so store the canonical zero-padded form.
fn due(value: Option<String>) -> lagom_core::Result<Option<String>> {
    value.map(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").map(|d| d.to_string())
        .map_err(|_| lagom_core::Error::Invalid("due date must be YYYY-MM-DD"))).transpose()
}

#[uniffi::export(async_runtime = "tokio")]
impl LagomCore {
    pub async fn todo_lists(&self) -> Result<Vec<TodoList>> {
        self.run(|s| {
            let default = s.default_task_list()?;
            Ok(s.task_lists()?.into_iter().map(|l: TaskList| TodoList { is_default: l.id == default, id: l.id, name: l.name }).collect())
        }).await
    }
    pub async fn add_todo_list(&self, name: String) -> Result<TodoList> {
        self.run(move |s| { let l = s.add_task_list(&name)?; Ok(TodoList { is_default: s.default_task_list()? == l.id, id: l.id, name: l.name }) }).await
    }
    /// Open tasks by position, then completed ones when asked for.
    pub async fn todos(&self, list: i64, include_done: bool) -> Result<Vec<Todo>> {
        self.run(move |s| Ok(s.tasks(list, include_done)?.into_iter().map(Into::into).collect())).await
    }
    pub async fn add_todo(&self, list: i64, title: String, due_date: Option<String>) -> Result<Todo> {
        self.run(move |s| Ok(s.add_task(list, &title, "", due(due_date)?)?.into())).await
    }
    pub async fn set_todo_done(&self, id: i64, done: bool) -> Result<()> {
        self.run(move |s| s.set_done(id, done)).await
    }
    pub async fn delete_todo(&self, id: i64) -> Result<()> {
        self.run(move |s| s.delete_task(id)).await
    }
    /// Cached events for `days` local days from `start_day`, all-day events first within each day.
    pub async fn agenda(&self, start_day: String, days: u32) -> Result<Vec<AgendaItem>> {
        self.run(move |s| {
            if days > 62 { return Err(lagom_core::Error::Invalid("agenda covers at most 62 days")); }
            let mut date = NaiveDate::parse_from_str(&start_day, "%Y-%m-%d").map_err(|_| lagom_core::Error::Invalid("start day must be YYYY-MM-DD"))?;
            let mut items = Vec::new();
            for _ in 0..days {
                items.extend(events_on(s, date)?);
                date = date.succ_opt().ok_or(lagom_core::Error::Invalid("date is outside the calendar range"))?;
            }
            Ok(items)
        }).await
    }
    /// True once any account has a fetched calendar window, so an empty agenda means a free schedule.
    pub async fn calendar_connected(&self) -> Result<bool> {
        self.run(|s| {
            for a in s.accounts()? { if s.calendar_window(&a.id)?.is_some() { return Ok(true); } }
            Ok(false)
        }).await
    }
    /// The calm Today page: today's events, due tasks (core's selection), and the newest unread Inbox mail.
    pub async fn today(&self, unread_limit: u32) -> Result<TodaySummary> {
        self.run(move |s| {
            let date = Local::now().date_naive();
            let day = date.to_string();
            Ok(TodaySummary {
                events: events_on(s, date)?,
                tasks: s.today_tasks(&day)?.into_iter().map(Into::into).collect(),
                unread_count: s.unread_inbox_count(now(), &[])? as u32,
                unread: s.unread_mail(now(), unread_limit.min(50))?.into_iter().map(Into::into).collect(),
                day,
            })
        }).await
    }
    pub async fn mail_counts(&self) -> Result<Vec<MailCounts>> {
        self.run(|s| {
            let now = now();
            s.accounts()?.into_iter().map(|a| { let c = s.counts(&a.id, now)?; Ok(MailCounts { account: a.id, inbox: c.inbox, later: c.later }) }).collect()
        }).await
    }
}

/// Fills an empty store with fixed sample mail, tasks and events for screenshots. Accounts get no
/// password, so sync fails; the app skips sync in demo mode.
/// shortcut: shipped in release builds but only called from the DEBUG `-LagomDemo` launch argument; gate behind a cargo feature if the binary size matters.
#[uniffi::export]
pub fn seed_demo(data_dir: String) -> Result<()> {
    std::fs::create_dir_all(&data_dir).map_err(lagom_core::Error::from)?;
    let mut s = Store::open(std::path::Path::new(&data_dir).join("lagom.sqlite3"))?;
    if !s.accounts()?.is_empty() { return Ok(()); }
    let today = Local::now().date_naive();
    let at = |days_ago: i64, h: u32, m: u32| day_start(today - chrono::Duration::days(days_ago)) + i64::from(h * 3600 + m * 60);
    let server = |host: &str, email: &str| ServerConfig { host: host.into(), port: 993, username: email.into(), security: Security::Tls };
    let mut accounts = Vec::new();
    for (name, email) in [("Personal", "you@home.example"), ("Work", "you@work.example")] {
        let mut a = Account::new(email.into(), server("imap.example", email), server("smtp.example", email))?;
        a.display_name = name.into();
        s.save_account(&a)?;
        accounts.push(a);
    }
    let mail = [
        (0, "Ingrid Holm", "ingrid@holm.example", (0, 9, 42), "Board meeting moved to Thursday", "Thursday at 16:00 works for everyone except the projector.", false, true),
        (1, "Johan Ek", "johan@ek.example", (0, 8, 57), "Invoice 2026-114", "Attached. Same as last month, minus the coffee machine.", false, true),
        (1, "Build bot", "ci@lagom.example", (0, 8, 15), "Nightly build passed", "All tests green. Nothing to see here.", true, false),
        (0, "Maja Lind", "maja@lind.example", (1, 12, 5), "Lunch Thursday?", "Somewhere with a lunch special. You choose, I will complain.", false, false),
        (0, "Housing co-op", "board@coop.example", (1, 10, 30), "Laundry room schedule, December", "The new schedule is on the door. It is also attached.", true, true),
        (0, "Tickets", "no-reply@tickets.example", (4, 18, 20), "Stockholm C → Göteborg C, 14 Nov", "Coach 3, seat 41. Quiet carriage.", true, false),
        (1, "Coffee roastery", "orders@roastery.example", (8, 7, 45), "Order 4411 has shipped", "Two bags of the Ethiopian, ground for filter.", true, false),
    ];
    let folders = accounts.iter().map(|a| {
        let inbox = s.save_folder(&a.id, "INBOX", None, FolderRole::Inbox)?;
        s.reset_uidvalidity(inbox, 1)?;
        Ok(inbox)
    }).collect::<lagom_core::Result<Vec<_>>>()?;
    for (uid, (account, name, address, (days, h, m), subject, snippet, seen, attachment)) in mail.into_iter().enumerate() {
        s.save_message(&lagom_core::store::Message {
            summary: MessageSummary { account: accounts[account].id.clone(), folder: folders[account], uid: uid as u32 + 1, date: at(days, h, m),
                from_name: name.into(), from_address: address.into(), to: accounts[account].email.clone(), subject: subject.into(),
                snippet: snippet.into(), seen, has_attachments: attachment, message_id: format!("demo{uid}@lagom.example"), ..Default::default() },
            body_text: format!("{snippet}\n\n{name}"), body_html: None,
        })?;
    }
    let lists = s.task_lists()?;
    let date = |offset: i64| Some((today + chrono::Duration::days(offset)).to_string());
    for (list, title, due) in [(0, "Prepare for the board meeting", date(0)), (0, "Review the project brief", date(-1)), (1, "Pick up groceries", None), (1, "Book the train home", date(3))] {
        s.add_task(lists[list.min(lists.len() - 1)].id, title, "", due)?;
    }
    let event = |id: &str, title: &str, start: i64, minutes: i64, join: Option<&str>| lagom_core::calendar::Event {
        remote_id: id.into(), title: title.into(), start: String::new(), end: String::new(), start_utc: start, end_utc: start + minutes * 60,
        all_day: false, location: None, join_url: join.map(Into::into), html_link: None };
    let mut planning = event("planning", "Planning day", at(0, 0, 0), 24 * 60, None);
    (planning.all_day, planning.start, planning.end) = (true, today.to_string(), (today + chrono::Duration::days(1)).to_string());
    let events = vec![planning, event("plan", "Plan the day", at(0, 9, 0), 30, None),
        event("review", "Design review", at(0, 11, 0), 60, Some("https://meet.example/review")), event("focus", "Focus time", at(0, 14, 0), 45, None),
        event("lunch", "Lunch with Maja", at(-2, 12, 0), 60, None), event("train", "Train to Göteborg", at(-4, 7, 10), 175, None)];
    let calendar = lagom_core::calendar::Calendar { remote_id: "primary".into(), name: "Personal".into(), color: None, shown: true, writable: false };
    Ok(s.replace_calendar_window(&accounts[0].id, at(7, 0, 0), at(-30, 0, 0), &[(calendar, events)])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, Arc<LagomCore>) {
        let dir = tempfile::tempdir().unwrap();
        let core = LagomCore::open(dir.path().to_string_lossy().into()).unwrap();
        (dir, core)
    }

    #[tokio::test]
    async fn todos_add_complete_delete_and_validate_due_dates() {
        let (_dir, core) = open();
        let lists = core.todo_lists().await.unwrap();
        assert_eq!(lists.iter().filter(|l| l.is_default).count(), 1);
        let list = lists[0].id;
        let first = core.add_todo(list, "First".into(), Some("2026-11-10".into())).await.unwrap();
        let second = core.add_todo(list, "Second".into(), None).await.unwrap();
        assert_eq!((first.due.as_deref(), first.done, first.from_mail), (Some("2026-11-10"), false, false));
        assert!(core.add_todo(list, "Bad".into(), Some("10/11/2026".into())).await.is_err());
        let padded = core.add_todo(list, "Padded".into(), Some("2026-2-3".into())).await.unwrap();
        assert_eq!(padded.due.as_deref(), Some("2026-02-03"));
        core.delete_todo(padded.id).await.unwrap();
        assert!(core.add_todo(list, "   ".into(), None).await.is_err(), "core rejects empty titles");
        assert!(core.add_todo(-1, "Nowhere".into(), None).await.is_err());
        core.set_todo_done(first.id, true).await.unwrap();
        assert_eq!(core.todos(list, false).await.unwrap(), [second.clone()]);
        let all = core.todos(list, true).await.unwrap();
        assert_eq!(all.iter().map(|t| (t.id, t.done)).collect::<Vec<_>>(), [(second.id, false), (first.id, true)]);
        core.delete_todo(second.id).await.unwrap();
        assert!(core.delete_todo(second.id).await.is_err());
        let added = core.add_todo_list("Errands".into()).await.unwrap();
        assert!(!added.is_default);
        assert_eq!(core.todo_lists().await.unwrap().last(), Some(&added));
    }

    #[tokio::test]
    async fn demo_seeds_once_and_feeds_today_agenda_and_counts() {
        let (dir, _) = open();
        let path: String = dir.path().join("demo").to_string_lossy().into();
        seed_demo(path.clone()).unwrap();
        seed_demo(path.clone()).unwrap();
        let core = LagomCore::open(path).unwrap();
        assert_eq!(core.accounts().await.unwrap().len(), 2, "a second seed leaves the store alone");
        assert!(core.calendar_connected().await.unwrap());
        let today = core.today(3).await.unwrap();
        assert_eq!(today.day, Local::now().date_naive().to_string());
        assert_eq!(today.events.first().map(|e| (e.title.as_str(), e.all_day)), Some(("Planning day", true)));
        assert_eq!(today.events.len(), 4);
        assert_eq!(today.unread_count, 3);
        assert_eq!(today.unread.len(), 3);
        assert!(today.tasks.iter().any(|t| t.title == "Review the project brief"), "overdue tasks are due today");
        let agenda = core.agenda(today.day.clone(), 7).await.unwrap();
        assert_eq!(agenda.iter().filter(|e| e.day == today.day).count(), 4);
        assert!(agenda.iter().any(|e| e.title == "Train to Göteborg" && e.day != today.day));
        assert!(core.agenda(today.day.clone(), 63).await.is_err());
        assert!(core.agenda("today".into(), 1).await.is_err());
        let counts = core.mail_counts().await.unwrap();
        assert_eq!(counts.iter().map(|c| c.inbox).sum::<u64>(), 7);
    }

    #[tokio::test]
    async fn password_accounts_have_no_calendar() {
        let (_dir, core) = open();
        {
            let s = core.stores.local.lock().unwrap();
            let server = ServerConfig { host: "127.0.0.1".into(), port: 1, username: "a@example.test".into(), security: Security::Plain };
            s.save_account(&Account::new("a@example.test".into(), server.clone(), server).unwrap()).unwrap();
        }
        assert!(!core.calendar_connected().await.unwrap());
        assert!(core.agenda(Local::now().date_naive().to_string(), 14).await.unwrap().is_empty());
        let today = core.today(3).await.unwrap();
        assert!(today.events.is_empty() && today.unread.is_empty() && today.unread_count == 0);
    }
}

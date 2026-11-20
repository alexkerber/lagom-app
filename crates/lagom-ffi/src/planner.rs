//! Today, Todo and Calendar for mobile: local tasks, cached events from core and the Mac's New event.
use super::*;
use chrono::{Local, NaiveDate, TimeZone};
use lagom_core::{calendar::{AgendaEvent, NewEvent}, tasks::{Task, TaskList}};

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TodoList { pub id: i64, pub name: String, pub is_default: bool }
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct Todo {
    pub id: i64, pub list: i64, pub title: String,
    /// `YYYY-MM-DD`, or an RFC 3339 instant from a synced provider; the first ten characters are the date.
    pub due: Option<String>, pub done: bool, pub from_mail: bool,
    /// The source mail's subject, only on a task made from mail.
    pub mail_subject: Option<String>,
}
impl From<Task> for Todo {
    fn from(t: Task) -> Self {
        let from_mail = t.mail_message_id.is_some_and(|m| !m.is_empty());
        Self { id: t.id, list: t.list, title: t.title, due: t.due, done: t.done_at.is_some(), from_mail, mail_subject: t.mail_subject.filter(|_| from_mail) }
    }
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

/// A writable Google calendar for New event, labelled as on the Mac: "Personal · you@gmail.com".
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct EventCalendar { pub account: String, pub calendar: String, pub label: String }
/// The Mac's New event form. Times come from the system pickers, so they arrive as instants.
#[derive(Clone, Debug, uniffi::Record)]
pub struct EventForm {
    pub title: String,
    pub all_day: bool,
    /// A timed event's start and end.
    pub start_unix: i64, pub end_unix: i64,
    /// An all-day event's first and last day, `YYYY-MM-DD`.
    pub first_day: String, pub last_day: String,
    /// The device's IANA zone, such as `Europe/Stockholm`, which Google needs beside a timed event.
    pub time_zone: String,
    /// Addresses as typed, separated by commas.
    pub guests: String,
    pub description: String,
    pub meet: bool,
}

/// Desktop `event_draft`, with the Mac's words for each mistake.
fn new_event(form: EventForm) -> lagom_core::Result<NewEvent> {
    use lagom_core::Error::Invalid;
    let (start, end) = if form.all_day {
        let day = |value: &str| NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| Invalid("Enter a date such as 9 Nov or YYYY-MM-DD"));
        let (first, last) = (day(&form.first_day)?, day(&form.last_day)?);
        if last < first { return Err(Invalid("Last day must be on or after the first day")); }
        (first.to_string(), last.succ_opt().ok_or(Invalid("End date is out of range"))?.to_string())
    } else {
        if form.end_unix <= form.start_unix { return Err(Invalid("End must be later than start")); }
        let at = |unix| Local.timestamp_opt(unix, 0).single().map(|t| t.to_rfc3339()).ok_or(Invalid("End date is out of range"));
        (at(form.start_unix)?, at(form.end_unix)?)
    };
    let attendees = lagom_core::contacts::split_recipients(&form.guests).iter()
        .map(|guest| lagom_core::contacts::recipient_address(guest).ok_or(Invalid("Enter valid guest email addresses")))
        .collect::<lagom_core::Result<Vec<_>>>()?;
    let draft = NewEvent {
        title: form.title.trim().into(), start, end, all_day: form.all_day,
        time_zone: if form.all_day { String::new() } else { form.time_zone },
        description: (!form.description.trim().is_empty()).then_some(form.description), attendees, meet: form.meet,
    };
    draft.event("validation".into())?;
    Ok(draft)
}

/// The account and calendar checks of desktop `Worker::create_event`, before anything reaches Google.
fn writable(s: &Store, account_id: &str, calendar: &str) -> lagom_core::Result<Account> {
    let a = account(s, account_id)?;
    if a.auth != lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google) {
        return Err(lagom_core::Error::Invalid("choose a Google calendar"));
    }
    if !lagom_core::oauth::calendar_writable(&s.account_scopes(account_id)?) { return Err(lagom_core::Error::CalendarConsent); }
    if !s.calendars(account_id)?.iter().any(|c| c.remote_id == calendar && c.writable) {
        return Err(lagom_core::Error::Invalid("choose a writable calendar"));
    }
    Ok(a)
}

/// Google's answer goes into the cache at once, so the week shows the event before the next sync fetches it.
fn create_cached(stores: &Stores, s: &Store, account_id: &str, calendar: &str,
    create: impl Future<Output = lagom_core::Result<lagom_core::google::CreatedEvent>>) -> lagom_core::Result<()> {
    let _calendar = stores.calendar.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // shortcut: a Meet link Google is still making arrives with the next sync; the Mac fetches it again after 5 s.
    let created = network(create)?;
    s.insert_calendar_event(account_id, calendar, &created.event)
}

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
    /// The stored message a task was made from, newest copy first; nil once it is no longer stored here.
    pub async fn todo_mail(&self, id: i64) -> Result<Option<i64>> {
        self.run(move |s| s.task_mail(id)).await
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
    /// True once a Google account allows calendar access, fetched or not: Calendar then waits for the next sync.
    pub async fn calendar_granted(&self) -> Result<bool> {
        self.run(|s| {
            for a in s.accounts()? { if lagom_core::oauth::calendar_allowed(&s.account_scopes(&a.id)?) { return Ok(true); } }
            Ok(false)
        }).await
    }
    /// Google accounts without calendar access, for "Show your Google Calendar". Empty when no account can add it.
    pub async fn calendar_access_needed(&self) -> Result<Vec<MailAccount>> {
        self.run(|s| {
            let mut needed = Vec::new();
            for a in s.accounts()? {
                if a.auth == lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google)
                    && !lagom_core::oauth::calendar_allowed(&s.account_scopes(&a.id)?) { needed.push(a.into()); }
            }
            Ok(needed)
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
    /// Writable calendars of Google accounts that allow calendar changes; empty hides New event.
    pub async fn event_calendars(&self) -> Result<Vec<EventCalendar>> {
        self.run(|s| {
            let mut calendars = Vec::new();
            for a in s.accounts()? {
                if a.auth != lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google)
                    || !lagom_core::oauth::calendar_writable(&s.account_scopes(&a.id)?) { continue; }
                for c in s.calendars(&a.id)?.into_iter().filter(|c| c.writable) {
                    calendars.push(EventCalendar { label: format!("{} · {}", c.name, a.email), account: a.id.clone(), calendar: c.remote_id });
                }
            }
            Ok(calendars)
        }).await
    }
    /// Creates the event in Google Calendar and caches Google's copy. After a network error the event may
    /// exist anyway; the error says to check the calendar before retrying.
    pub async fn create_event(&self, account_id: String, calendar: String, form: EventForm) -> Result<()> {
        let stores = self.stores.clone();
        self.run_own(move |s| {
            let a = writable(s, &account_id, &calendar)?;
            let draft = new_event(form)?;
            create_cached(&stores, s, &account_id, &calendar, lagom_core::google::create_event(&a, &calendar, &draft))
        }).await
    }
    /// Demo mode, as the Mac's demo calendar: the event goes into the fixture store, never to Google.
    pub async fn add_demo_event(&self, form: EventForm) -> Result<()> {
        self.run(move |s| {
            let draft = new_event(form)?;
            let a = s.accounts()?.into_iter().next().ok_or(lagom_core::Error::NotFound)?;
            s.insert_calendar_event(&a.id, "primary", &draft.event(format!("demo-created-{}", milliseconds()))?)
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
pub fn seed_demo(data_dir: String) -> Result<()> { seed(data_dir, None) }
/// The same fixtures without a fetched calendar, for screenshots of Calendar's empty states. `state`: "google"
/// makes the first account a Google account with mail access only, "granted" one that allowed calendar access
/// but has no fetch yet; anything else keeps password accounts.
#[uniffi::export]
pub fn seed_demo_without_calendar(data_dir: String, state: String) -> Result<()> {
    let scopes = match state.as_str() {
        "google" => Some("https://mail.google.com/ openid email profile".to_string()),
        "granted" => Some(format!("https://mail.google.com/ openid email profile {}", lagom_core::oauth::GOOGLE_CALENDAR_SCOPES)),
        _ => None,
    };
    seed(data_dir, Some(scopes))
}
fn seed(data_dir: String, without_calendar: Option<Option<String>>) -> Result<()> {
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
        if let (Some(Some(scopes)), true) = (&without_calendar, accounts.is_empty()) {
            a.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            s.save_authorized_account(&a, scopes)?;
        } else { s.save_account(&a)?; }
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
    let mut first_mail = None;
    for (uid, (account, name, address, (days, h, m), subject, snippet, seen, attachment)) in mail.into_iter().enumerate() {
        let summary = MessageSummary { account: accounts[account].id.clone(), folder: folders[account], uid: uid as u32 + 1, date: at(days, h, m),
            from_name: name.into(), from_address: address.into(), to: accounts[account].email.clone(), subject: subject.into(),
            snippet: snippet.into(), seen, has_attachments: attachment, message_id: format!("demo{uid}@lagom.example"), ..Default::default() };
        first_mail.get_or_insert_with(|| summary.clone());
        s.save_message(&lagom_core::store::Message { summary, body_text: format!("{snippet}\n\n{name}"), body_html: None })?;
    }
    let lists = s.task_lists()?;
    let date = |offset: i64| Some((today + chrono::Duration::days(offset)).to_string());
    for (list, title, due) in [(0, "Prepare for the board meeting", date(0)), (0, "Review the project brief", date(-1)), (1, "Pick up groceries", None), (1, "Book the train home", date(3))] {
        s.add_task(lists[list.min(lists.len() - 1)].id, title, "", due)?;
    }
    // A task made from mail keeps the subject as its second line, so it gets its own title.
    if let Some(mail) = first_mail {
        let task = s.task_from_mail(lists[0].id, &mail)?;
        s.rename_task(task.id, "Reply to Ingrid about Thursday")?;
    }
    let event = |id: &str, title: &str, start: i64, minutes: i64, join: Option<&str>| lagom_core::calendar::Event {
        remote_id: id.into(), title: title.into(), start: String::new(), end: String::new(), start_utc: start, end_utc: start + minutes * 60,
        all_day: false, location: None, join_url: join.map(Into::into), html_link: None };
    let mut planning = event("planning", "Planning day", at(0, 0, 0), 24 * 60, None);
    (planning.all_day, planning.start, planning.end) = (true, today.to_string(), (today + chrono::Duration::days(1)).to_string());
    let events = vec![planning, event("plan", "Plan the day", at(0, 9, 0), 30, None),
        event("review", "Design review", at(0, 11, 0), 60, Some("https://meet.example/review")), event("focus", "Focus time", at(0, 14, 0), 45, None),
        event("lunch", "Lunch with Maja", at(-2, 12, 0), 60, None), event("train", "Train to Göteborg", at(-4, 7, 10), 175, None)];
    if without_calendar.is_some() { return Ok(()); }
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
        let lists = core.todo_lists().await.unwrap();
        let from_mail = core.todos(lists[0].id, false).await.unwrap().into_iter().find(|t| t.from_mail).expect("a demo task from mail");
        assert_eq!(from_mail.mail_subject.as_deref(), Some("Board meeting moved to Thursday"));
        let mail = core.todo_mail(from_mail.id).await.unwrap().expect("the demo mail is stored");
        assert_eq!(core.conversation(mail).await.unwrap().iter().find(|m| m.row.id == mail).map(|m| m.row.subject.clone()).as_deref(), Some("Board meeting moved to Thursday"));
        let plain = core.todos(lists[0].id, false).await.unwrap().into_iter().find(|t| !t.from_mail).unwrap();
        assert_eq!(core.todo_mail(plain.id).await.unwrap(), None, "a task not from mail has no message");
        assert!(core.todos(lists[0].id, false).await.unwrap().iter().filter(|t| !t.from_mail).all(|t| t.mail_subject.is_none()));
        let agenda = core.agenda(today.day.clone(), 7).await.unwrap();
        assert_eq!(agenda.iter().filter(|e| e.day == today.day).count(), 4);
        assert!(agenda.iter().any(|e| e.title == "Train to Göteborg" && e.day != today.day));
        assert!(core.agenda(today.day.clone(), 63).await.is_err());
        assert!(core.agenda("today".into(), 1).await.is_err());
        let counts = core.mail_counts().await.unwrap();
        assert_eq!(counts.iter().map(|c| c.inbox).sum::<u64>(), 7);
    }

    #[tokio::test]
    async fn only_google_accounts_without_calendar_scopes_are_offered_access() {
        let (_dir, core) = open();
        let (google, granted) = {
            let s = core.stores.local.lock().unwrap();
            let server = ServerConfig { host: "127.0.0.1".into(), port: 1, username: "p@example.test".into(), security: Security::Plain };
            s.save_account(&Account::new("p@example.test".into(), server.clone(), server).unwrap()).unwrap();
            let accounts = ["mail@gmail.com", "both@gmail.com"].map(|email| {
                let (imap, smtp) = lagom_core::presets::servers(email).unwrap();
                let mut a = Account::new(email.into(), imap, smtp).unwrap();
                a.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
                a
            });
            s.save_authorized_account(&accounts[0], "https://mail.google.com/ openid email profile").unwrap();
            s.save_authorized_account(&accounts[1], &format!("https://mail.google.com/ {}", lagom_core::oauth::GOOGLE_CALENDAR_SCOPES)).unwrap();
            let [google, granted] = accounts;
            (google, granted)
        };
        assert!(core.calendar_granted().await.unwrap(), "granted before the first fetch");
        let offered = core.calendar_access_needed().await.unwrap();
        assert_eq!(offered.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), [google.id.as_str()]);
        let password = core.accounts().await.unwrap().into_iter().find(|a| a.email == "p@example.test").unwrap();
        // Settings names how each account signs in, in the Mac's words.
        assert_eq!(password.kind, "Email and password");
        assert!(core.accounts().await.unwrap().iter().any(|a| a.id == google.id && a.kind == "Google account"));
        assert!(core.begin_calendar_access(password.id).is_err(), "a password account has no Google grant");
        assert!(core.begin_calendar_access("missing".into()).is_err());
        // Google took the grant back: access is offered again and mail keeps its scope.
        core.stores.local.lock().unwrap().drop_calendar_scopes(&granted.id).unwrap();
        assert_eq!(core.calendar_access_needed().await.unwrap().len(), 2);
        assert!(!core.calendar_granted().await.unwrap());
    }

    #[tokio::test]
    async fn demo_without_calendar_offers_google_access_or_nothing() {
        for (state, offered, granted) in [("google", 1, false), ("granted", 0, true), ("none", 0, false)] {
            let dir = tempfile::tempdir().unwrap();
            let path: String = dir.path().to_string_lossy().into();
            seed_demo_without_calendar(path.clone(), state.into()).unwrap();
            let core = LagomCore::open(path).unwrap();
            assert!(!core.calendar_connected().await.unwrap());
            assert_eq!(core.calendar_access_needed().await.unwrap().len(), offered, "{state}");
            assert_eq!(core.calendar_granted().await.unwrap(), granted, "{state}");
            assert!(!core.today(3).await.unwrap().tasks.is_empty(), "the rest of the fixtures stay");
        }
    }

    fn form(title: &str, start: i64, minutes: i64) -> EventForm {
        EventForm { title: title.into(), all_day: false, start_unix: start, end_unix: start + minutes * 60, first_day: String::new(),
            last_day: String::new(), time_zone: "Europe/Stockholm".into(), guests: String::new(), description: String::new(), meet: false }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_event_offers_writable_google_calendars_checks_the_form_and_caches_the_result() {
        let (_dir, core) = open();
        let today = Local::now().date_naive();
        let nine = day_start(today) + 9 * 3600;
        let (google, password) = {
            let s = core.stores.local.lock().unwrap();
            let server = ServerConfig { host: "127.0.0.1".into(), port: 1, username: "p@example.test".into(), security: Security::Plain };
            let password = Account::new("p@example.test".into(), server.clone(), server).unwrap();
            s.save_account(&password).unwrap();
            let (imap, smtp) = lagom_core::presets::servers("you@gmail.com").unwrap();
            let mut google = Account::new("you@gmail.com".into(), imap, smtp).unwrap();
            google.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            s.save_authorized_account(&google, &format!("https://mail.google.com/ {}", lagom_core::oauth::GOOGLE_CALENDAR_SCOPES)).unwrap();
            let calendar = |id: &str, name: &str, writable| lagom_core::calendar::Calendar { remote_id: id.into(), name: name.into(), color: None, shown: true, writable };
            s.replace_calendar_window(&google.id, nine - 7 * 86_400, nine + 30 * 86_400,
                &[(calendar("primary", "Personal", true), vec![]), (calendar("holidays", "Holidays", false), vec![])]).unwrap();
            (google, password)
        };
        assert_eq!(core.event_calendars().await.unwrap(),
            [EventCalendar { account: google.id.clone(), calendar: "primary".into(), label: "Personal · you@gmail.com".into() }]);
        let refused = |result: Result<()>| match result { Err(MailError::Failed { reason }) => reason, other => panic!("{other:?}") };
        assert_eq!(refused(core.create_event(password.id, "primary".into(), form("Lunch", nine, 60)).await), "choose a Google calendar");
        assert_eq!(refused(core.create_event(google.id.clone(), "holidays".into(), form("Lunch", nine, 60)).await), "choose a writable calendar");
        assert_eq!(refused(core.create_event(google.id.clone(), "primary".into(), form("Lunch", nine, 0)).await), "End must be later than start");
        assert_eq!(refused(core.create_event(google.id.clone(), "primary".into(), form("  ", nine, 60)).await), "enter an event title");
        let mut guests = form("Lunch", nine, 60);
        guests.guests = "maja@lind.example, not an address".into();
        assert_eq!(refused(core.create_event(google.id.clone(), "primary".into(), guests).await), "Enter valid guest email addresses");
        let mut days = form("Trip", 0, 0);
        (days.all_day, days.first_day, days.last_day) = (true, "2026-11-20".into(), "2026-11-19".into());
        assert_eq!(refused(core.create_event(google.id.clone(), "primary".into(), days.clone()).await), "Last day must be on or after the first day");
        days.last_day = "2026-11-22".into();
        let draft = new_event(days).unwrap();
        assert_eq!((draft.start.as_str(), draft.end.as_str(), draft.time_zone.as_str()), ("2026-11-20", "2026-11-23", ""), "Google ends all-day events the day after");

        // Google's copy lands in the cache, so the week shows it before the next sync.
        let draft = new_event(form("Lunch with Maja", nine, 60)).unwrap();
        assert_eq!(draft.time_zone, "Europe/Stockholm");
        let event = draft.event("created-1".into()).unwrap();
        let created = lagom_core::google::CreatedEvent { event, conference_pending: false };
        let (stores, id) = (core.stores.clone(), google.id.clone());
        tokio::task::spawn_blocking(move || {
            let s = Store::open(&stores.path).unwrap();
            create_cached(&stores, &s, &id, "primary", async { Ok(created) })
        }).await.unwrap().unwrap();
        let week = core.agenda(today.to_string(), 1).await.unwrap();
        assert_eq!(week.iter().map(|e| (e.title.as_str(), e.start_unix, e.end_unix)).collect::<Vec<_>>(), [("Lunch with Maja", nine, nine + 3600)]);
    }

    #[tokio::test]
    async fn demo_events_go_into_the_fixture_calendar() {
        let dir = tempfile::tempdir().unwrap();
        let path: String = dir.path().to_string_lossy().into();
        seed_demo(path.clone()).unwrap();
        let core = LagomCore::open(path).unwrap();
        assert!(core.event_calendars().await.unwrap().is_empty(), "demo accounts have no Google calendar");
        let today = Local::now().date_naive();
        core.add_demo_event(form("Coffee with Johan", day_start(today) + 16 * 3600, 30)).await.unwrap();
        assert!(core.agenda(today.to_string(), 1).await.unwrap().iter().any(|e| e.title == "Coffee with Johan"));
        assert!(core.add_demo_event(form("", day_start(today), 30)).await.is_err());
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

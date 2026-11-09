//! Mail as the UI shows it: rows from the local store or the demo sample, list dates,
//! Later times and reply headers. Plain Rust, so tests run without a window.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone};
use lagom_core::{attachment::Attachment, store::MessageSummary};

/// One message in the list and the reading pane.
/// A folder a message can be in. `New`: one ⌘K is creating, known by its full name until the
/// server lists it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Folder(i64),
    New(String),
}

impl Default for Place {
    fn default() -> Self {
        Place::Folder(0)
    }
}

#[derive(Clone, Default)]
pub struct Mail {
    pub from: String,
    pub address: String,
    pub time: String,
    pub subject: String,
    pub snippet: String,
    /// Paragraphs. Empty for store mail until the reading pane loads the body.
    pub body: Vec<String>,
    /// The HTML part, loaded with the body. Platforms with a web view render it.
    pub html: Option<gpui::SharedString>,
    /// "Load remote images" was clicked for this message.
    pub remote: bool,
    pub attachments: Vec<Attachment>,
    pub inline: std::sync::Arc<Vec<lagom_core::inline::InlinePart>>,
    pub inline_pending: bool,
    pub inline_retry_after: Option<std::time::Instant>,
    pub unread: bool,
    /// Local store row id. `None` in demo mode.
    pub store: Option<i64>,
    /// The folder the message is in, as far as this window knows: optimistic moves set it.
    pub folder: Place,
    /// The folders it left, newest last: undo goes back to where it was when the action ran.
    pub was: Vec<Place>,
    pub snoozed_until: Option<i64>,
    pub was_snoozed: Vec<Option<i64>>,
    pub to: String,
    pub cc: String,
    pub message_id: String,
    pub thread_key: String,
    pub thread_id: String,
    pub account: String,
    pub date: i64,
}

impl Mail {
    pub fn from_summary(m: &MessageSummary, now: DateTime<Local>) -> Self {
        Self {
            from: if m.from_name.is_empty() { m.from_address.clone() } else { m.from_name.clone() },
            address: m.from_address.clone(),
            time: when(m.date, now),
            subject: if m.subject.is_empty() { "(no subject)".into() } else { m.subject.clone() },
            // Snippets stored before the core dropped invisible padding.
            snippet: lagom_core::mime::clean_snippet(&m.snippet),
            body: Vec::new(),
            html: None,
            remote: false,
            attachments: Vec::new(),
            inline: Default::default(),
            inline_pending: false,
            inline_retry_after: None,
            unread: !m.seen,
            store: Some(m.id),
            folder: Place::Folder(m.folder),
            was: Vec::new(),
            snoozed_until: m.snoozed_until,
            was_snoozed: Vec::new(),
            to: m.to.clone(),
            cc: m.cc.clone(),
            message_id: m.message_id.clone(),
            thread_key: m.thread_key.clone(),
            thread_id: m.thread_id.clone(),
            account: m.account.clone(),
            date: m.date,
        }
    }
}

/// The date column: "09:42" today, "Yesterday", a weekday this week, "3 Nov" this year, else "Nov 2024".
pub fn when(unix: i64, now: DateTime<Local>) -> String {
    let Some(t) = Local.timestamp_opt(unix, 0).single() else { return String::new() };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        ..=0 => t.format("%H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => t.format("%a").to_string(),
        _ if t.year() == now.year() => t.format("%-d %b").to_string(),
        _ => t.format("%b %Y").to_string(),
    }
}

pub fn date_group(unix: i64, today: chrono::NaiveDate) -> String {
    let Some(date) = Local.timestamp_opt(unix, 0).single().map(|t| t.date_naive()) else { return "Earlier".into() };
    let days = (today - date).num_days();
    match days {
        ..=0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if days <= today.weekday().num_days_from_monday() as i64 => date.format("%A").to_string(),
        _ => "Earlier".into(),
    }
}

pub fn reading_date(unix: i64, now: DateTime<Local>) -> String {
    let Some(t) = Local.timestamp_opt(unix, 0).single() else { return String::new() };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => format!("Today {}", t.format("%H:%M")),
        1..=6 => format!("{} {}", when(unix, now), t.format("%H:%M")),
        _ if t.year() == now.year() => t.format("%-d %b, %H:%M").to_string(),
        _ => t.format("%-d %b %Y, %H:%M").to_string(),
    }
}

pub fn full_date(unix: i64) -> String {
    Local.timestamp_opt(unix, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// Body text as paragraphs, split at blank lines.
// shortcut: caps the body at 100 000 characters, as GPUI shapes all text on screen. Render a
// virtual text view when long mail matters.
pub fn paragraphs(body: &str) -> Vec<String> {
    const CAP: usize = 100_000;
    let body = match body.char_indices().nth(CAP) {
        Some((i, _)) => &body[..i],
        None => body,
    };
    let body = body.replace("\r\n", "\n");
    let out: Vec<String> = body.split("\n\n").map(|p| p.trim_matches('\n').to_string()).filter(|p| !p.trim().is_empty()).collect();
    if out.is_empty() { vec!["(no text)".into()] } else { out }
}

/// When a message on Later comes back to the inbox. Times are local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaterWhen {
    /// 18:00 today, or tomorrow once 18:00 has passed.
    Evening,
    /// 08:00 tomorrow. L and a left swipe use this.
    Tomorrow,
    /// 08:00 on the next Monday.
    NextWeek,
}

impl LaterWhen {
    pub const fn label(self) -> &'static str {
        match self {
            LaterWhen::Evening => "Later: this evening 18:00",
            LaterWhen::Tomorrow => "Later: tomorrow 08:00",
            LaterWhen::NextWeek => "Later: next week, Monday 08:00",
        }
    }

    pub fn at(self, now: DateTime<Local>) -> DateTime<Local> {
        let today = now.date_naive();
        let (day, hour) = match self {
            LaterWhen::Evening if now.time() < NaiveTime::from_hms_opt(18, 0, 0).unwrap() => (today, 18),
            LaterWhen::Evening => (today + Duration::days(1), 18),
            LaterWhen::Tomorrow => (today + Duration::days(1), 8),
            LaterWhen::NextWeek => {
                let ahead = 7 - now.weekday().num_days_from_monday() as i64;
                (today + Duration::days(ahead), 8)
            }
        };
        let local = day.and_hms_opt(hour, 0, 0).unwrap();
        // A daylight-saving gap skips the hour: take the first valid time after it.
        Local.from_local_datetime(&local).earliest().unwrap_or_else(|| Local.from_utc_datetime(&local) + Duration::hours(1))
    }
}

/// A safe FTS5 expression for what the user typed: every word quoted, the last one as a prefix.
pub fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q.split_whitespace().map(|w| format!("\"{}\"", w.replace('"', "\"\""))).collect();
    let last = words.len().checked_sub(1)?;
    Some(words.iter().enumerate().map(|(i, w)| if i == last { format!("{w}*") } else { w.clone() }).collect::<Vec<_>>().join(" "))
}

/// "Re: subject", without stacking "Re: Re:".
pub fn reply_subject(subject: &str) -> String {
    if subject.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) { subject.into() } else { format!("Re: {subject}") }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposeKind { Reply, ReplyAll, Forward }

pub fn forward_subject(subject: &str) -> String {
    if subject.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("fwd:")) { subject.into() } else { format!("Fwd: {subject}") }
}

pub fn reply_all(sender: &str, to: &str, cc: &str, own: &[&str]) -> (String, String) {
    let mut used: std::collections::HashSet<String> = own.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let mut unique = |field: &str| recipients(field).into_iter().filter(|address| used.insert(address.to_ascii_lowercase())).collect::<Vec<_>>().join(", ");
    (unique(&format!("{sender}, {to}")), unique(cc))
}

pub fn compose_body(signature: &str, original: Option<(&Mail, &str)>, kind: ComposeKind) -> String {
    let mut body = if signature.trim().is_empty() { String::new() } else { format!("-- \n{signature}") };
    if let Some((m, text)) = original {
        if !body.is_empty() { body.push_str("\n\n"); }
        let header = if kind == ComposeKind::Forward {
            format!("Forwarded message\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\n\n", m.from, m.address, full_date(m.date), m.subject, m.to)
        } else { format!("On {}, {} wrote:\n\n", full_date(m.date), m.address) };
        body.push_str(&format!("{header}{text}").lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n"));
    }
    if !body.is_empty() { body.insert_str(0, "\n\n"); }
    body
}

/// In-Reply-To and References for a reply.
// shortcut: References holds the thread root and the parent only, as the store keeps no full
// References chain. Store the chain when threading lands.
pub fn reply_headers(message_id: &str, thread_key: &str) -> (Option<String>, Vec<String>) {
    let angle = |id: &str| if id.starts_with('<') { id.to_string() } else { format!("<{id}>") };
    if message_id.is_empty() {
        return (None, Vec::new());
    }
    let parent = angle(message_id);
    let mut refs = Vec::new();
    if !thread_key.is_empty() && thread_key != message_id && !thread_key.contains(':') {
        refs.push(angle(thread_key));
    }
    refs.push(parent.clone());
    (Some(parent), refs)
}

/// Comma-separated recipients, empty parts dropped.
pub fn recipients(field: &str) -> Vec<String> {
    lagom_core::contacts::split_recipients(field)
}

/// The list the UI shows, as the store has it now.
#[derive(Clone, Copy)]
pub enum View {
    Inbox,
    Trash,
    Folder(i64),
    Role(lagom_core::store::FolderRole),
    Later,
}

/// One read of the store for the window: the inbox, the folders and the open list.
#[derive(Default)]
pub struct Snapshot {
    pub inbox: Vec<MessageSummary>,
    pub inbox_total: u64,
    pub later_total: u64,
    pub folders: Vec<lagom_core::store::Folder>,
    /// The open folder or Later. Empty when the inbox is open.
    pub open: Vec<MessageSummary>,
    /// When the next message on Later comes back.
    pub next_due: Option<i64>,
    pub account_unread: std::collections::HashMap<String, [usize; 3]>,
    pub more: bool,
}

/// Bounded queries on indexed columns: a few ms for a 100 000-message store. Runs when the
/// worker reports a change, never per key press.
/// `pins`: selected messages and a ⌘K hit stay in their list even past `limit`.
pub fn load(store: &lagom_core::store::Store, account: &str, view: View, limit: u32, now: i64, pins: impl IntoIterator<Item = i64>, group: bool) -> lagom_core::Result<Snapshot> {
    let folders = store.folders(account, now)?;
    let counts = store.counts(account, now)?;
    let inbox_id = folders.iter().find(|f| f.role == lagom_core::store::FolderRole::Inbox).map(|f| f.id);
    let view = match view {
        View::Role(role) => folders.iter().find(|f| f.role == role).map_or(View::Role(role), |f| View::Folder(f.id)),
        View::Trash => folders.iter().find(|f| f.role == lagom_core::store::FolderRole::Trash).map_or(View::Trash, |f| View::Folder(f.id)),
        view => view,
    };
    let mut inbox = match inbox_id {
        Some(id) => if group { store.list_conversations(account, Some(id), now, limit)? } else { store.list_messages(id, now, limit, 0)? },
        None => Vec::new(),
    };
    let mut open = match view {
        View::Inbox | View::Trash | View::Role(_) => Vec::new(),
        View::Folder(id) => if group { store.list_conversations(account, Some(id), now, limit)? } else { store.list_messages(id, now, limit, 0)? },
        View::Later => if group { store.list_conversations(account, None, now, limit)? } else { store.later(account, now, limit, 0)? },
    };
    let mut loaded = match view { View::Inbox => &inbox, _ => &open }.iter().map(|m| m.id).collect::<std::collections::HashSet<_>>();
    for id in pins {
        if loaded.contains(&id) { continue; }
        let m = match store.message_summary(id) {
            Ok(m) if m.account == account => m,
            Ok(_) | Err(lagom_core::Error::NotFound) => continue,
            Err(e) => return Err(e),
        };
        let snoozed = m.snoozed_until.is_some_and(|t| t > now);
        let list = match view {
            View::Inbox if Some(m.folder) == inbox_id && !snoozed => Some(&mut inbox),
            View::Folder(id) if m.folder == id && !snoozed => Some(&mut open),
            View::Later if snoozed => Some(&mut open),
            _ => None,
        };
        // Past the page means older than every loaded row: it goes last.
        if let Some(list) = list {
            let start = list.len();
            if group {
                list.extend(store.thread_messages(account, &m.thread_id)?.into_iter().filter(|row| row.folder == m.folder && row.snoozed_until.is_some_and(|t| t > now) == snoozed));
            } else { list.push(m); }
            loaded.extend(list[start..].iter().map(|m| m.id));
        }
    }
    let next_due = store.next_snooze(account, now)?;
    Ok(Snapshot { inbox, inbox_total: counts.inbox, later_total: counts.later, folders, open, next_due, account_unread: Default::default(), more: false })
}

/// System mailboxes merge bounded pages; custom folders stay with the current account.
pub fn load_accounts(store: &lagom_core::store::Store, accounts: &[lagom_core::account::Account], current: &str, filter: Option<&str>,
    view: View, limit: u32, now: i64, pins: Vec<i64>, group: bool) -> lagom_core::Result<Snapshot> {
    use lagom_core::store::FolderRole;
    let mut result = load(store, current, view, limit.saturating_add(1), now, pins.iter().copied(), group)?;
    let role = match view {
        View::Inbox => Some(FolderRole::Inbox),
        View::Role(role) => Some(role),
        View::Folder(id) => result.folders.iter().find(|f| f.id == id)
            .map(|f| f.role).filter(|r| matches!(r, FolderRole::Archive | FolderRole::Sent)),
        _ => None,
    };
    for account in accounts {
        let folders = store.folders(&account.id, now)?;
        let unread = [FolderRole::Inbox, FolderRole::Archive, FolderRole::Sent].map(|role|
            folders.iter().filter(|f| f.role == role).map(|f| f.unread as usize).sum());
        result.account_unread.insert(account.id.clone(), unread);
        if account.id == current { continue; }
        let other_view = match role {
            Some(FolderRole::Inbox) => View::Inbox,
            Some(role) => View::Role(role),
            None => View::Inbox,
        };
        let other = load(store, &account.id, other_view, limit.saturating_add(1), now, pins.iter().copied(), group)?;
        result.next_due = result.next_due.into_iter().chain(other.next_due).min();
        result.inbox.extend(other.inbox);
        result.inbox_total += other.inbox_total;
        if role.is_some() { result.open.extend(other.open); }
    }
    let inbox_more = page(&mut result.inbox, limit, filter, &pins, group);
    let open_more = page(&mut result.open, limit, filter, &pins, group);
    result.more = if matches!(view, View::Inbox) { inbox_more } else { open_more };
    Ok(result)
}

fn page(rows: &mut Vec<MessageSummary>, limit: u32, filter: Option<&str>, pins: &[i64], group: bool) -> bool {
    rows.retain(|m| filter.is_none_or(|account| m.account == account));
    rows.sort_by_key(|m| std::cmp::Reverse((m.date, m.id)));
    let key = |m: &MessageSummary| (m.account.clone(), m.folder,
        if group && !m.thread_id.is_empty() { m.thread_id.clone() } else { m.id.to_string() });
    let mut seen = std::collections::HashSet::new();
    let mut keep = std::collections::HashSet::new();
    for m in rows.iter() {
        let key = key(m);
        if seen.insert(key.clone()) && seen.len() <= limit as usize { keep.insert(key); }
    }
    let more = seen.len() > limit as usize;
    // A pinned member retains its entire conversation, even beyond the global page.
    keep.extend(rows.iter().filter(|m| pins.contains(&m.id)).map(key));
    rows.retain(|m| keep.contains(&key(m)));
    more
}

struct Sample {
    from: &'static str,
    address: &'static str,
    time: &'static str,
    subject: &'static str,
    snippet: &'static str,
    body: &'static [&'static str],
    attachment: Option<(&'static str, u32)>,
    unread: bool,
}

macro_rules! m {
    ($from:literal, $address:literal, $time:literal, $subject:literal, $snippet:literal, [$($p:literal),*], $attachment:expr, $unread:literal) => {
        Sample { from: $from, address: $address, time: $time, subject: $subject, snippet: $snippet, body: &[$($p),*], attachment: $attachment, unread: $unread }
    };
}

/// Demo inbox length: the inbox is ids 0..INBOX_LEN, the folder contents follow (`folders::sample`).
pub const INBOX_LEN: usize = 7;
/// Messages already on Later in the demo.
pub const LATER_SAMPLE: usize = 2;

const SAMPLE: &[Sample] = &[
    m!("Ingrid Holm", "ingrid@holm.example", "09:42", "Board meeting moved to Thursday", "Thursday at 16:00 works for everyone except the projector.",
        ["Hi,", "Thursday at 16:00 works for everyone except the projector, which has asked to be replaced. I will bring a laptop instead.", "The minutes from last time are attached. Please read them before the meeting, not during it.", "Ingrid"],
        Some(("minutes-oct.pdf", 212 * 1024)), true),
    m!("Johan Ek", "johan@ek.example", "08:57", "Invoice 2026-114", "Attached. Same as last month, minus the coffee machine.",
        ["Hi,", "Invoice 2026-114 is attached. Same as last month, minus the coffee machine, which we now own.", "Thirty days, as usual.", "Johan"],
        Some(("invoice-2026-114.pdf", 48 * 1024)), true),
    m!("Build bot", "ci@lagom.example", "08:15", "Nightly build passed", "All tests green. Nothing to see here.",
        ["Nightly build 1147 passed on macOS, Linux and Windows.", "All tests green. Nothing to see here."], None, false),
    m!("Maja Lind", "maja@lind.example", "Yesterday", "Lunch Thursday?", "Somewhere with a lunch special. You choose, I will complain.",
        ["Lunch on Thursday? Somewhere with a lunch special.", "You choose, I will complain. It is tradition.", "Maja"], None, true),
    m!("Housing co-op", "board@coop.example", "Yesterday", "Laundry room schedule, December", "The new schedule is on the door. It is also attached.",
        ["Hello neighbours,", "The December laundry schedule is on the door. It is also attached, for those who never pass the door.", "The board"],
        Some(("laundry-december.pdf", 96 * 1024)), false),
    m!("Tickets", "no-reply@tickets.example", "Tue", "Stockholm C → Göteborg C, 14 Nov", "Coach 3, seat 41. Quiet carriage.",
        ["Your journey on 14 November: Stockholm C 07:10, Göteborg C 10:05.", "Coach 3, seat 41. Quiet carriage."], None, false),
    // Long on purpose: the list must end sender, subject and preview in "…", never mid-glyph.
    m!("Lagom releases and announcements for the desktop and mobile apps", "releases@lagom.example", "Mon",
        "Version 0.4 is out: faster threads, fewer settings and a calmer list for very large inboxes",
        "Threads open faster and there are fewer settings. The list keeps up with a hundred thousand messages, search finds old mail at once, and the full changelog is on the site.",
        ["Version 0.4 is out.", "Threads open faster and there are fewer settings. The full changelog is on the site."], None, false),
    m!("Coffee roastery", "orders@roastery.example", "Mon", "Order 4411 has shipped", "Two bags of the Ethiopian, ground for filter.",
        ["Order 4411 has shipped: two bags of the Ethiopian, ground for filter.", "It should arrive on Thursday."], None, true),
    m!("Bike shop", "service@bikeshop.example", "3 Nov", "Service receipt", "New chain, new brake pads, same squeak.",
        ["Thanks for your visit.", "New chain and new brake pads. We could not find the squeak."], Some(("receipt-881.pdf", 31 * 1024)), true),
    m!("Hardware store", "receipts@hardware.example", "1 Nov", "Your receipt", "One ladder, one ladder hook.",
        ["Your receipt: one ladder, one ladder hook.", "Keep it for the three-year warranty."], None, true),
    m!("Maja Lind", "maja@lind.example", "Oct", "Photos from Lisbon", "Mostly tiles. A few of us, in front of tiles.",
        ["The Lisbon photos are in the shared album.", "Mostly tiles. A few of us, in front of tiles.", "Maja"], None, false),
    m!("Ryokan Hanaya", "stay@hanaya.example", "Oct", "Booking confirmed, 3–6 April", "Dinner is served at 18:30. Slippers are provided.",
        ["Your stay from 3 to 6 April is confirmed. Two guests, garden room.", "Dinner is served at 18:30. Slippers are provided."], None, false),
    m!("Tickets", "no-reply@tickets.example", "Sun", "Göteborg C → Stockholm C, 16 Nov", "Coach 5, seat 12. Quiet carriage again.",
        ["Your journey on 16 November: Göteborg C 17:05, Stockholm C 20:00.", "Coach 5, seat 12. Quiet carriage again."], None, true),
    m!("Johan Ek", "johan@ek.example", "Sep", "Folder icons", "Stroke 1.5, like the rest. Sketches attached.",
        ["Folder icons, stroke 1.5 like the rest of the set.", "Sketches attached. The open folder is the hard one.", "Johan"],
        Some(("folder-icons.png", 640 * 1024)), false),
    m!("Seed shop", "hello@seeds.example", "Aug", "Spring catalogue", "Forty kinds of tomato. You need two.",
        ["The spring catalogue is here.", "Forty kinds of tomato. You need two."], None, false),
    m!("Housing co-op", "board@coop.example", "Oct", "Annual meeting minutes", "The bike room stays. The sauna gets a new door.",
        ["Hello neighbours,", "The minutes from the annual meeting are attached. The bike room stays. The sauna gets a new door.", "The board"],
        Some(("annual-meeting.pdf", 180 * 1024)), true),
    m!("This Week in Rust", "twir@rust.example", "Thu", "Issue 571", "Crate of the week, a few RFCs and a lot of PRs.",
        ["Issue 571: crate of the week, a few RFCs and a lot of merged PRs.", "Read it on the web, or not at all."], None, true),
    m!("Kitchen notes", "notes@kitchen.example", "Wed", "Soup season", "Six soups that freeze well, and one that does not.",
        ["Soup season. Six soups that freeze well, and one that does not.", "Start with the lentil one."], None, true),
];

/// The sample accounts for `LAGOM_DEMO=1`: the sidebar and Settings list the same two.
pub fn demo_accounts() -> Vec<lagom_core::account::Account> {
    use lagom_core::account::{Account, Auth, Security, ServerConfig};
    let server = |host: &str, port, user: &str| ServerConfig { host: host.into(), port, security: Security::Tls, username: user.into() };
    [("Personal", "you@home.example", "home.example"), ("Work", "you@work.example", "work.example")]
        .into_iter()
        .map(|(name, email, domain)| Account {
            id: name.to_lowercase(),
            auth: Auth::Password,
            display_name: name.into(),
            email: email.into(),
            imap: server(&format!("imap.{domain}"), 993, email),
            smtp: server(&format!("smtp.{domain}"), 465, email),
        })
        .collect()
}

/// An HTML newsletter for the demo: a table layout with inline styles, a `<style>` block, a
/// data: image, a remote image that stays blocked, and links.
const NEWSLETTER: &str = r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Lagom 0.4</title>
<style>@media only screen and (max-width: 600px) { .container { width: 100% !important; } } .btn:hover { opacity: .9 }</style></head>
<body style="margin:0;background:#f4f2ee">
<div style="display:none;max-height:0;overflow:hidden">Threads open faster and there are fewer settings. &#847; &zwnj; &nbsp; &#847; &zwnj; &nbsp;</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" bgcolor="#f4f2ee"><tr><td align="center" style="padding:24px 12px">
<table role="presentation" class="container" width="560" cellpadding="0" cellspacing="0" style="background:#ffffff;border-radius:12px;font-family:Helvetica,Arial,sans-serif;color:#1d1d1b">
<tr><td style="padding:28px 32px 8px"><img width="40" height="40" alt="Lagom" src="data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCA0MCA0MCI+PHJlY3Qgd2lkdGg9IjQwIiBoZWlnaHQ9IjQwIiByeD0iOSIgZmlsbD0iI0IwM0EyRSIvPjxyZWN0IHg9IjEwIiB5PSIxOCIgd2lkdGg9IjIwIiBoZWlnaHQ9IjQiIHJ4PSIyIiBmaWxsPSIjZmZmIi8+PGNpcmNsZSBjeD0iMjAiIGN5PSIyOSIgcj0iMyIgZmlsbD0iI2ZmZiIvPjwvc3ZnPg==" style="display:block;border-radius:9px"></td></tr>
<tr><td style="padding:8px 32px 0;font-size:26px;line-height:32px;font-weight:bold">Version 0.4 is out</td></tr>
<tr><td style="padding:12px 32px 0;font-size:15px;line-height:23px;color:#4a4a46">Threads open faster and there are fewer settings. The list keeps up with a hundred thousand messages, and search finds old mail at once.</td></tr>
<tr><td style="padding:20px 32px 0"><img width="496" height="160" alt="[Remote image: blocked]" src="https://images.example.com/lagom-0.4-banner.png" style="display:block;width:100%;height:auto;background:#ece9e3;border-radius:8px;color:#8a877f;font-size:13px"></td></tr>
<tr><td style="padding:20px 32px 0"><table role="presentation" cellpadding="0" cellspacing="0"><tr>
<td width="50%" valign="top" style="padding-right:12px;font-size:14px;line-height:21px"><b>Faster threads</b><br>Opening a long thread no longer waits for every message.</td>
<td width="50%" valign="top" style="padding-left:12px;font-size:14px;line-height:21px"><b>Fewer settings</b><br>Eleven settings are gone. Nobody will miss them.</td></tr></table></td></tr>
<tr><td style="padding:24px 32px 28px"><a class="btn" href="https://lagom.example/changelog" style="display:inline-block;background:#b03a2e;color:#ffffff;text-decoration:none;font-size:14px;font-weight:bold;padding:10px 18px;border-radius:8px">Read the changelog</a></td></tr>
</table>
<p style="font-family:Helvetica,Arial,sans-serif;font-size:12px;color:#8a877f;margin:16px 0 0">You get this because you use Lagom. <a href="https://lagom.example/unsubscribe" style="color:#8a877f">Unsubscribe</a> · <a href="mailto:releases@lagom.example" style="color:#8a877f">Write to us</a></p>
</td></tr></table></body></html>"##;

/// Simple HTML, as a phone or webmail writes it: no colours of its own.
const SIMPLE_HTML: &str = r#"<div dir="ltr"><div>Lunch on Thursday? Somewhere with a <b>lunch special</b>.</div><div><br></div><div>You choose, I will complain. It is <a href="https://en.wikipedia.org/wiki/Tradition">tradition</a>.</div><div><br></div><div>Maja</div></div>"#;

/// The sample mail for `LAGOM_DEMO=1`, screenshots and tests.
pub fn demo() -> Vec<Mail> {
    SAMPLE
        .iter()
        .map(|s| Mail {
            html: match s.address {
                "releases@lagom.example" => Some(NEWSLETTER.into()),
                "maja@lind.example" if s.subject.starts_with("Lunch") => Some(SIMPLE_HTML.into()),
                _ => None,
            },
            cc: String::new(),
            from: s.from.into(),
            address: s.address.into(),
            time: s.time.into(),
            subject: s.subject.into(),
            snippet: s.snippet.into(),
            body: s.body.iter().map(|p| p.to_string()).collect(),
            attachments: s.attachment.map(|(name, size)| Attachment { filename: name.into(), size, ..Default::default() }).into_iter().collect(),
            unread: s.unread,
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest().unwrap()
    }

    #[test]
    fn forward_subject_quote_and_signature() {
        assert_eq!(forward_subject("Plans"), "Fwd: Plans");
        assert_eq!(forward_subject("FWd: Plans"), "FWd: Plans");
        let m = Mail { from: "Sender".into(), address: "sender@example.com".into(), to: "recipient@example.com".into(), subject: "Plans".into(), date: 1, ..Default::default() };
        let body = compose_body("**Team**", Some((&m, "First\n\nSecond")), ComposeKind::Forward);
        assert!(body.starts_with("\n\n-- \n**Team**\n\n> Forwarded message\n> From: Sender <sender@example.com>\n> Date: "));
        assert!(body.contains("> Subject: Plans\n> To: recipient@example.com\n> \n> First\n> \n> Second"));
        assert_eq!(body.matches("-- \n**Team**").count(), 1);
        assert_eq!(compose_body("", None, ComposeKind::Reply), "");
        assert_eq!(compose_body("Team", None, ComposeKind::Reply), "\n\n-- \nTeam");
    }

    #[test]
    fn reply_all_removes_own_addresses_and_deduplicates_across_to_and_cc() {
        assert_eq!(reply_all("Sender@EXAMPLE.com", "ME@example.com, sender@example.com, Other@example.com; other@example.com", "alias@EXAMPLE.com, OTHER@example.com, Copy@example.com, copy@example.com", &["me@example.com", "Alias@example.com"]),
            ("Sender@EXAMPLE.com, Other@example.com".into(), "Copy@example.com".into()));
        assert_eq!(reply_all("me@example.com", "", "ME@example.com", &["me@example.com"]), (String::new(), String::new()));
    }

    #[test]
    fn later_times_are_local() {
        // Friday 6 November 2026, 10:12.
        let now = at(2026, 11, 6, 10, 12);
        assert_eq!(LaterWhen::Evening.at(now), at(2026, 11, 6, 18, 0));
        assert_eq!(LaterWhen::Tomorrow.at(now), at(2026, 11, 7, 8, 0));
        assert_eq!(LaterWhen::NextWeek.at(now), at(2026, 11, 9, 8, 0));
        // Past 18:00, "this evening" is tomorrow evening; on a Monday, next week is 7 days on.
        assert_eq!(LaterWhen::Evening.at(at(2026, 11, 6, 19, 0)), at(2026, 11, 7, 18, 0));
        assert_eq!(LaterWhen::NextWeek.at(at(2026, 11, 9, 7, 0)), at(2026, 11, 16, 8, 0));
        // Sunday night to Monday morning, and over a month end.
        assert_eq!(LaterWhen::NextWeek.at(at(2026, 11, 8, 23, 0)), at(2026, 11, 9, 8, 0));
        assert_eq!(LaterWhen::Tomorrow.at(at(2026, 11, 30, 23, 59)), at(2026, 12, 1, 8, 0));
        for w in [LaterWhen::Evening, LaterWhen::Tomorrow, LaterWhen::NextWeek] {
            assert!(w.at(now) > now);
        }
    }

    #[test]
    fn date_groups_follow_calendar_week_and_year_boundaries() {
        let today = at(2026, 11, 6, 10, 0).date_naive();
        for (date, label) in [(at(2026, 11, 7, 1, 0), "Today"), (at(2026, 11, 6, 1, 0), "Today"),
            (at(2026, 11, 5, 1, 0), "Yesterday"), (at(2026, 11, 4, 1, 0), "Wednesday"),
            (at(2026, 11, 2, 1, 0), "Monday"), (at(2026, 11, 1, 1, 0), "Earlier")] {
            assert_eq!(date_group(date.timestamp(), today), label);
        }
        assert_eq!(date_group(at(2026, 12, 31, 23, 59).timestamp(), at(2027, 1, 1, 0, 1).date_naive()), "Yesterday");
        assert_eq!(date_group(i64::MAX, today), "Earlier");
    }

    #[test]
    fn review_unified_snooze_wakes_for_the_earliest_account() {
        use lagom_core::store::{FolderRole, Message, Store};
        let dir = std::env::temp_dir().join(format!("lagom-due-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("mail.sqlite3")).unwrap();
        let accounts = demo_accounts();
        let mut ids = Vec::new();
        for account in &accounts {
            store.save_account(account).unwrap();
            let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
            ids.push(store.save_message(&Message { summary: MessageSummary {
                account: account.id.clone(), folder, uid: 1, ..Default::default()
            }, ..Default::default() }).unwrap());
        }
        for personal_due in [None, Some(150), Some(250)] {
            store.snooze(ids[0], personal_due).unwrap();
            store.snooze(ids[1], Some(200)).unwrap();
            let snap = load_accounts(&store, &accounts, "personal", None, View::Inbox, 200, 100, vec![], false).unwrap();
            assert_eq!(snap.next_due, Some(personal_due.unwrap_or(200).min(200)));
            assert!(!snap.inbox.iter().any(|m| m.id == ids[1]));
            let awake = load_accounts(&store, &accounts, "personal", None, View::Inbox, 200, 200, vec![], false).unwrap();
            assert!(awake.inbox.iter().any(|m| m.id == ids[1]));
        }
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unified_pages_keep_account_identity_counts_roles_and_pins() {
        use lagom_core::store::{FolderRole, Message, Store};
        let dir = std::env::temp_dir().join(format!("lagom-unified-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("mail.sqlite3")).unwrap();
        let accounts = demo_accounts();
        let mut ids = Vec::new();
        for (ix, account) in accounts.iter().enumerate() {
            store.save_account(account).unwrap();
            for (name, role) in [("INBOX", FolderRole::Inbox), ("Archive", FolderRole::Archive), ("Sent", FolderRole::Sent)] {
                // Personal has no Sent folder; the unified role must still show Work's.
                if ix == 0 && role == FolderRole::Sent { continue; }
                let folder = store.save_folder(&account.id, name, Some("/"), role).unwrap();
                for uid in 1..=4 {
                    let summary = MessageSummary { account: account.id.clone(), folder, uid, date: uid as i64 * 10 + ix as i64,
                        thread_id: format!("thread-{uid}"), seen: uid == 4, ..Default::default() };
                    let id = store.save_message(&Message { summary, body_text: "Fixture".into(), body_html: None }).unwrap();
                    if role == FolderRole::Inbox { ids.push(id); }
                }
            }
        }
        for group in [false, true] {
            let snap = load_accounts(&store, &accounts, "personal", None, View::Inbox, 2, 100, vec![ids[0], ids[4]], group).unwrap();
            assert_eq!(snap.inbox_total, 8);
            assert_eq!(snap.account_unread["personal"], [3, 3, 0]);
            assert_eq!(snap.account_unread["work"], [3, 3, 3]);
            assert_eq!(snap.inbox.len(), 4);
            assert!(snap.more);
            assert!(snap.inbox.windows(2).all(|w| w[0].date >= w[1].date));
            assert!(snap.inbox.iter().any(|m| m.id == ids[0]) && snap.inbox.iter().any(|m| m.id == ids[4]));
            for (role, len) in [(FolderRole::Archive, 2), (FolderRole::Sent, 2)] {
                let snap = load_accounts(&store, &accounts, "personal", None, View::Role(role), 2, 100, vec![], group).unwrap();
                assert_eq!(snap.open.len(), len);
                assert!(snap.open.iter().all(|m| store.folder(m.folder).unwrap().role == role));
            }
        }
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unified_page_boundary_keeps_threads_ties_pins_and_account_filters() {
        use lagom_core::store::{FolderRole, Message, Store};
        let dir = std::env::temp_dir().join(format!("lagom-page-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("mail.sqlite3")).unwrap();
        let accounts = demo_accounts();
        let mut ids = Vec::new();
        for account in &accounts {
            store.save_account(account).unwrap();
            let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
            for (uid, date, thread) in [(1, 100, "new@test"), (2, 1, "new@test"), (3, 90, "old@test"), (4, 0, "old@test")] {
                let summary = MessageSummary { account: account.id.clone(), folder, uid, date,
                    message_id: format!("{uid}@test"), thread_refs: thread.into(), ..Default::default() };
                ids.push(store.save_message(&Message { summary, ..Default::default() }).unwrap());
            }
        }
        let load = |limit, filter, pins, group| load_accounts(&store, &accounts, "personal", filter, View::Inbox, limit, 200, pins, group).unwrap();
        let row_ids = |snap: &Snapshot| snap.inbox.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(row_ids(&load(1, None, vec![], false)), [ids[4]]);
        let first = load(1, None, vec![], true);
        assert_eq!(row_ids(&first), [ids[4], ids[5]]);
        assert!(first.more);
        let pinned = load(1, None, vec![ids[3]], true);
        assert_eq!(row_ids(&pinned), [ids[4], ids[2], ids[5], ids[3]]);
        assert!(pinned.more);
        for (account, start) in [("personal", 0), ("work", 4)] {
            let first = load(1, Some(account), vec![], true);
            assert_eq!(row_ids(&first), [ids[start], ids[start + 1]]);
            assert!(first.more);
            let all = load(2, Some(account), vec![], true);
            assert_eq!(all.inbox.len(), 4);
            assert!(!all.more);
        }
        assert_eq!(load(4, None, vec![], true).inbox.len(), 8);
        assert!(!load(4, None, vec![], true).more);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn review_reading_dates_keep_relative_days_and_time() {
        let now = at(2026, 11, 9, 12, 0);
        for (date, expected) in [
            (at(2026, 11, 9, 9, 42), "Today 09:42"),
            (at(2026, 11, 8, 16, 10), "Yesterday 16:10"),
            (at(2026, 11, 7, 8, 5), "Sat 08:05"),
            (at(2026, 11, 3, 8, 5), "Tue 08:05"),
            (at(2026, 11, 2, 8, 5), "2 Nov, 08:05"),
            (at(2026, 10, 26, 14, 2), "26 Oct, 14:02"),
            (at(2025, 10, 26, 14, 2), "26 Oct 2025, 14:02"),
            (at(2026, 11, 10, 14, 2), "10 Nov, 14:02"),
        ] { assert_eq!(reading_date(date.timestamp(), now), expected); }
        assert_eq!(reading_date(at(2025, 12, 31, 23, 0).timestamp(), at(2026, 1, 1, 1, 0)), "Yesterday 23:00");
        assert_eq!(reading_date(at(2028, 2, 29, 8, 0).timestamp(), at(2028, 3, 1, 8, 0)), "Yesterday 08:00");
        assert_eq!(reading_date(i64::MAX, now), "");
    }

    #[test]
    fn list_dates() {
        let now = at(2026, 11, 6, 10, 12);
        let ts = |d: DateTime<Local>| d.timestamp();
        assert_eq!(when(ts(at(2026, 11, 6, 9, 42)), now), "09:42");
        assert_eq!(when(ts(at(2026, 11, 5, 23, 0)), now), "Yesterday");
        assert_eq!(when(ts(at(2026, 11, 3, 8, 0)), now), "Tue");
        assert_eq!(when(ts(at(2026, 10, 3, 8, 0)), now), "3 Oct");
        assert_eq!(when(ts(at(2024, 10, 3, 8, 0)), now), "Oct 2024");
        // A clock a little ahead on the server still shows a time, not a date.
        assert_eq!(when(ts(at(2026, 11, 6, 10, 20)), now), "10:20");
    }

    #[test]
    fn search_query_is_quoted() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("inv").as_deref(), Some("\"inv\"*"));
        assert_eq!(fts_query("board thur").as_deref(), Some("\"board\" \"thur\"*"));
        // FTS syntax in what the user types stays text.
        assert_eq!(fts_query("a\" OR b").as_deref(), Some("\"a\"\"\" \"OR\" \"b\"*"));
    }

    #[test]
    fn reply_fields() {
        assert_eq!(reply_subject("Hello"), "Re: Hello");
        assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
        assert_eq!(reply_headers("child@x", "root@x"), (Some("<child@x>".into()), vec!["<root@x>".into(), "<child@x>".into()]));
        assert_eq!(reply_headers("only@x", "only@x"), (Some("<only@x>".into()), vec!["<only@x>".into()]));
        // The core's fallback thread key ("folder:validity:uid") is not a Message-ID.
        assert_eq!(reply_headers("a@x", "3:1:9").1, vec!["<a@x>".to_string()]);
        assert_eq!(reply_headers("", ""), (None, vec![]));
        assert_eq!(recipients(" a@x.se, ,b@y.se;c@z.se "), ["a@x.se", "b@y.se", "c@z.se"]);
    }

    #[test]
    fn body_paragraphs() {
        assert_eq!(paragraphs("Hi,\r\n\r\nLine one\nline two\n\n\n"), ["Hi,", "Line one\nline two"]);
        assert_eq!(paragraphs(""), ["(no text)"]);
    }

    #[test]
    fn a_search_hit_past_the_page_is_loaded_into_its_list() {
        use lagom_core::store::{FolderRole, Message, Store};
        let dir = std::env::temp_dir().join(format!("lagom-pin-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("pin.sqlite3")).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("a@fastmail.com").unwrap();
        let account = lagom_core::account::Account::new("a@fastmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store.save_folder(&account.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
        let ids: Vec<i64> = (1..=6)
            .map(|uid| {
                let summary = MessageSummary { account: account.id.clone(), folder: inbox, uid, subject: format!("m{uid}"), date: uid as i64, ..Default::default() };
                store.save_message(&Message { summary, body_text: String::new(), body_html: None }).unwrap()
            })
            .collect();
        let oldest = ids[0];
        let rows = |pin, view| load(&store, &account.id, view, 3, 100, pin, false).unwrap();
        let list = |s: Snapshot| s.inbox.iter().chain(&s.open).map(|m| m.id).collect::<Vec<_>>();
        // Page of 3: the oldest is not loaded, unless pinned; then it comes last, once.
        assert!(!list(rows(None, View::Inbox)).contains(&oldest));
        assert_eq!(list(rows(Some(oldest), View::Inbox)).last(), Some(&oldest));
        assert_eq!(list(rows(Some(ids[5]), View::Inbox)).iter().filter(|&&i| i == ids[5]).count(), 1);
        // Snoozed: it belongs on Later, not in the inbox.
        store.snooze(oldest, Some(1_000)).unwrap();
        assert!(!list(rows(Some(oldest), View::Inbox)).contains(&oldest));
        assert!(list(rows(Some(oldest), View::Later)).contains(&oldest));
        let _ = std::fs::remove_dir_all(dir);
    }
}

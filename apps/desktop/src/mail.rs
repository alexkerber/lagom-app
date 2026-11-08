//! Mail as the UI shows it: rows from the local store or the demo sample, list dates,
//! Later times and reply headers. Plain Rust, so tests run without a window.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone};
use lagom_core::{attachment::Attachment, store::MessageSummary};

/// One message in the list and the reading pane.
/// A folder a message can be in. `New`: one ⌘K is creating, known by its full name until the
/// server lists it.
#[derive(Clone, Debug, PartialEq)]
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
    pub unread: bool,
    /// Local store row id. `None` in demo mode.
    pub store: Option<i64>,
    /// The folder the message is in, as far as this window knows: optimistic moves set it.
    pub folder: Place,
    /// The folders it left, newest last: undo goes back to where it was when the action ran.
    pub was: Vec<Place>,
    pub to: String,
    pub message_id: String,
    pub thread_key: String,
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
            unread: !m.seen,
            store: Some(m.id),
            folder: Place::Folder(m.folder),
            was: Vec::new(),
            to: m.to.clone(),
            message_id: m.message_id.clone(),
            thread_key: m.thread_key.clone(),
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
    field.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The list the UI shows, as the store has it now.
pub enum View {
    Inbox,
    Trash,
    Folder(i64),
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
}

/// Bounded queries on indexed columns: a few ms for a 100 000-message store. Runs when the
/// worker reports a change, never per key press.
/// `pin`: a store id that must be in the list on screen if it belongs there, even past
/// `limit` (a ⌘K hit), so the selection lands on it and not on another row.
pub fn load(store: &lagom_core::store::Store, account: &str, view: View, limit: u32, now: i64, pin: Option<i64>) -> lagom_core::Result<Snapshot> {
    let folders = store.folders(account, now)?;
    let counts = store.counts(account, now)?;
    let inbox_id = folders.iter().find(|f| f.role == lagom_core::store::FolderRole::Inbox).map(|f| f.id);
    let view = match view {
        View::Trash => folders.iter().find(|f| f.role == lagom_core::store::FolderRole::Trash).map_or(View::Trash, |f| View::Folder(f.id)),
        view => view,
    };
    let mut inbox = match inbox_id {
        Some(id) => store.list_messages(id, now, limit, 0)?,
        None => Vec::new(),
    };
    let mut open = match view {
        View::Inbox | View::Trash => Vec::new(),
        View::Folder(id) => store.list_messages(id, now, limit, 0)?,
        View::Later => store.later(account, now, limit, 0)?,
    };
    if let Some(m) = pin.and_then(|id| store.message(id).ok()).map(|m| m.summary) {
        let snoozed = m.snoozed_until.is_some_and(|t| t > now);
        let list = match view {
            View::Inbox if Some(m.folder) == inbox_id && !snoozed => Some(&mut inbox),
            View::Folder(id) if m.folder == id && !snoozed => Some(&mut open),
            View::Later if snoozed => Some(&mut open),
            _ => None,
        };
        // Past the page means older than every loaded row: it goes last.
        if let Some(list) = list.filter(|l| !l.iter().any(|r| r.id == m.id)) {
            list.push(m);
        }
    }
    let next_due = store.later(account, now, 1, 0)?.first().and_then(|m| m.snoozed_until);
    Ok(Snapshot { inbox, inbox_total: counts.inbox, later_total: counts.later, folders, open, next_due })
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
    [("Home", "you@home.example", "home.example"), ("Work", "you@work.example", "work.example")]
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
        let rows = |pin, view| load(&store, &account.id, view, 3, 100, pin).unwrap();
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

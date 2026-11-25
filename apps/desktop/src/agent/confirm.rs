use super::write::Snapshot;
use lagom_core::{
    account::Account,
    calendar::NewEvent,
    agent::{
        protocol::{Code, Request},
        token::AgentRow,
    },
};
use tokio::sync::oneshot;

pub(crate) struct Sheet {
    peer: std::sync::Weak<std::os::unix::net::UnixStream>,
    pub withdrawn_at: std::sync::Arc<std::sync::OnceLock<std::time::Instant>>,
    pub expires: tokio::time::Instant,
    pub serial: usize,
    pub closed: Option<oneshot::Receiver<()>>,
    pub agent: String,
    pub heading: &'static str,
    pub approve_label: &'static str,
    pub rows: Vec<Row>,
    pub previous_description: Option<String>,
    pub body: String,
    pub expanded: bool,
    pub answer: oneshot::Sender<Result<(), Code>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    Added,
    Removed,
}

/// One fixed label and its escaped value. `old`: the current value when a change replaces it.
pub(crate) struct Row {
    pub label: &'static str,
    pub value: String,
    pub old: Option<String>,
    pub tag: Option<Tag>,
}

fn line(value: &str) -> String {
    super::read::sheet_text(value).replace('\n', "\\n")
}

fn address(value: &str) -> Result<String, Code> {
    let mailbox = value
        .parse::<lettre::message::Mailbox>()
        .map_err(|_| Code::InvalidRequest)?;
    let domain = mailbox.email.domain();
    let ascii = idna::domain_to_ascii(domain).map_err(|_| Code::InvalidRequest)?;
    let warn = !domain.is_ascii()
        || ascii
            .split('.')
            .any(|s| s.to_ascii_lowercase().starts_with("xn--"));
    let mut result = format!(
        "{}@{}{}",
        mailbox.email.user(),
        ascii,
        if warn { " ⚠" } else { "" }
    );
    if let Some(name) = &mailbox.name {
        let mut chars = name.chars();
        let shown: String = chars.by_ref().take(64).collect();
        result.push_str(&format!(" ({shown})"));
        let remaining = chars.count();
        if remaining > 0 { result.push_str(&format!(" ({remaining} more characters)")); }
    }
    Ok(result)
}

/// The address a calendar compares: punycode domain, any case, no display name.
pub(super) fn normal(value: &str) -> Result<String, Code> {
    let mailbox = value
        .parse::<lettre::message::Mailbox>()
        .map_err(|_| Code::InvalidRequest)?;
    if !mailbox.email.user().is_ascii() || mailbox.email.user().contains('"') || value.chars().any(char::is_control) {
        return Err(Code::InvalidRequest);
    }
    let domain = idna::domain_to_ascii(mailbox.email.domain()).map_err(|_| Code::InvalidRequest)?;
    Ok(format!("{}@{domain}", mailbox.email.user()).to_lowercase())
}

/// Each guest once, as the event will have them. On a change (`old` and `new`), a guest that
/// only `new` has is Added and one that only `old` has is Removed.
fn guests(old: Option<&[String]>, new: Option<&[String]>) -> Result<Vec<(String, Option<Tag>)>, Code> {
    let parse = |list: &[String]| list.iter().map(|g| Ok((normal(g)?, address(g)?))).collect::<Result<Vec<_>, Code>>();
    let before = old.map(parse).transpose()?.unwrap_or_default();
    let after = new.map(parse).transpose()?;
    let compare = old.is_some() && after.is_some();
    let has = |list: &[(String, String)], key: &str| list.iter().any(|(k, _)| k == key);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (key, shown) in after.as_deref().unwrap_or(&before) {
        if seen.insert(key.clone()) {
            out.push((shown.clone(), (compare && !has(&before, key)).then_some(Tag::Added)));
        }
    }
    if compare {
        for (key, shown) in &before {
            if seen.insert(key.clone()) {
                out.push((shown.clone(), Some(Tag::Removed)));
            }
        }
    }
    Ok(out)
}

fn local_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| {
            use chrono::Timelike;
            let t = t.with_timezone(&chrono::Local);
            // Seconds only when set, so an odd time is never rounded away.
            t.format(if t.second() == 0 { "%a %Y-%m-%d %H:%M %:z" } else { "%a %Y-%m-%d %H:%M:%S %:z" })
                .to_string()
        })
        .unwrap_or_else(|_| format!("{value} (all day)"))
}

impl Sheet {
    fn withdrawal_time(&self) -> std::time::Instant {
        self.withdrawn_at.get().copied().unwrap_or_else(std::time::Instant::now)
    }

    pub fn decide(self, approve: bool) -> bool {
        use std::io::{ErrorKind, Read};
        // Check the socket at the click even if the listener has not handled EOF yet.
        let connected = approve
            && tokio::time::Instant::now() < self.expires
            && self
                .peer
                .upgrade()
                .is_some_and(|s| matches!((&*s).read(&mut [0]),Err(e) if e.kind()==ErrorKind::WouldBlock));
        self.answer.send(if connected { Ok(()) } else { Err(Code::Denied) }).is_ok() && connected
    }

    pub fn new(
        agent: AgentRow,
        account: &Account,
        req: &Request,
        snapshot: &Snapshot,
        closed: oneshot::Receiver<()>,
        peer: std::sync::Weak<std::os::unix::net::UnixStream>,
    ) -> Result<(Self, oneshot::Receiver<Result<(), Code>>), Code> {
        let (answer, rx) = oneshot::channel();
        let mut rows = Vec::new();
        let mut push = |label, value: &str, old: Option<&str>, tag| rows.push(Row { label, value: line(value), old: old.map(line), tag });
        let mut row = |label, value: &str| push(label, value, None, None);
        row("Agent", &format!("{} ({})", agent.name, agent.host));
        let from = if matches!(snapshot, Snapshot::Mail(..)) { "From" } else { "Account" };
        row(from, &address(&account.email)?);
        let mut previous_description = None;
        let body;
        match snapshot {
            Snapshot::Mail(_, m) => {
                for (label, group) in [("To", &m.to), ("Cc", &m.cc), ("Bcc", &m.bcc)] {
                    for value in group {
                        row(label, &address(value)?);
                    }
                }
                row("Subject", &m.subject);
                for p in &m.attachments {
                    row("Attachment", &format!("{} ({} bytes)", p.filename, p.data.len()));
                }
                body = m.text.clone();
            }
            Snapshot::Event { name, old, new, .. } => {
                let current = old.as_ref().map(|s| s.attendees.as_slice());
                // A change without guests keeps the current ones.
                let next = new.as_ref().map(|e| if e.attendees.is_empty() { current.unwrap_or_default() } else { &e.attendees[..] });
                for (guest, tag) in guests(current, next)? {
                    push("Guest", &guest, None, tag);
                }
                let was = old.as_ref().map(|s| &s.event);
                let or_none = |v: Option<&str>| v.filter(|v| !v.is_empty()).unwrap_or("None").to_owned();
                let meet = |e: &NewEvent| if e.meet { "New link".into() } else { or_none(was.and_then(|w| w.join_url.as_deref())) };
                let fields = [
                    ("Title", was.map(|e| e.title.clone()), new.as_ref().map(|e| e.title.clone())),
                    ("Calendar", None, Some(name.clone())),
                    ("Start", was.map(|e| local_time(&e.start)), new.as_ref().map(|e| local_time(&e.start))),
                    ("End", was.map(|e| local_time(&e.end)), new.as_ref().map(|e| local_time(&e.end))),
                    ("Location", was.map(|e| or_none(e.location.as_deref())),
                        new.as_ref().map(|e| or_none(e.location.as_deref().or_else(|| was?.location.as_deref())))),
                    ("Meet", was.map(|e| or_none(e.join_url.as_deref())), new.as_ref().map(meet)),
                ];
                // A changed field is one row: the current value, then the new one.
                for (label, before, after) in fields {
                    match (before, after) {
                        (Some(b), Some(a)) if b != a => push(label, &a, Some(&b), None),
                        (b, a) => if let Some(v) = a.or(b) { push(label, &v, None, None) },
                    }
                }
                let current = old.as_ref().and_then(|s| s.description.as_deref()).unwrap_or("");
                body = match new {
                    Some(e) => e.description.as_deref().unwrap_or(current).to_owned(),
                    None => current.to_owned(),
                };
                if old.is_some() && new.is_some() && current != body {
                    previous_description = Some(super::read::sheet_text(current));
                }
            }
            Snapshot::None => unreachable!(),
        }
        let (heading, approve_label) = match req.cmd.as_str() {
            "draft.send" => ("Send this message?", "Send"),
            "event.create" => ("Create this event?", "Create event"),
            "event.update" => ("Update this event?", "Update event"),
            "event.delete" => ("Delete this event?", "Delete event"),
            _ => unreachable!(),
        };
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        Ok((
            Self {
                peer,
                withdrawn_at: Default::default(),
                expires: tokio::time::Instant::now() + std::time::Duration::from_secs(120),
                serial: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                closed: Some(closed),
                agent: agent.id,
                heading,
                approve_label,
                rows,
                previous_description,
                body: super::read::sheet_text(&body),
                expanded: false,
                answer,
            },
            rx,
        ))
    }
}

/// `LAGOM_SCREEN=agent-confirm-send`, `agent-confirm-event` and `agent-confirm-delete`: a sample sheet for screenshots.
pub(crate) fn demo(screen: &str) -> Option<Sheet> {
    use lagom_core::{calendar::Event, google::EventSnapshot, smtp::Outgoing};
    let (cmd, snapshot) = match screen {
        "agent-confirm-send" => ("draft.send", Snapshot::Mail(String::new(), Outgoing {
            to: vec!["Mia Berg <mia@family.example>".into()],
            cc: vec!["Ola Berg <ola@family.example>".into(), "noor@friends.example".into()],
            subject: "Re: Saturday dinner at ours, and who brings dessert this time".into(),
            text: "Hi Mia,\n\nSaturday works for us. Come at six, dinner is at seven.\n\nWe make the soup. \
Can you bring bread? Ola said he would bring dessert, but last time that was a bag of crisps.\n\n\
Noor, you are welcome too. Bring the card game.\n\n"
                .repeat(14) + "See you then,\nAlex",
            ..Default::default()
        })),
        "agent-confirm-event" | "agent-confirm-delete" => {
            let old = EventSnapshot {
                event: Event {
                    remote_id: String::new(),
                    title: "Planning: spring trip".into(),
                    start: "2026-11-26T14:00:00+01:00".into(),
                    end: "2026-11-26T15:00:00+01:00".into(),
                    start_utc: 0,
                    end_utc: 0,
                    all_day: false,
                    location: Some("Kitchen table".into()),
                    join_url: None,
                    html_link: None,
                },
                description: Some("Dates, budget and who books the cabin.".into()),
                attendees: vec!["you@home.example".into(), "mia@family.example".into()],
                etag: String::new(),
            };
            let new = NewEvent {
                title: "Planning: spring trip".into(),
                start: "2026-11-26T15:00:00+01:00".into(),
                end: "2026-11-26T16:00:00+01:00".into(),
                time_zone: "Europe/Stockholm".into(),
                location: None,
                all_day: false,
                description: Some("Dates, budget and who books the cabin. Noor joins for the budget part.".into()),
                attendees: vec!["you@home.example".into(), "mia@family.example".into(), "Noor <noor@friends.example>".into()],
                meet: false,
            };
            match screen {
                "agent-confirm-delete" => ("event.delete", Snapshot::Event { calendar: String::new(), name: "Family".into(), old: Some(old), new: None }),
                _ => ("event.update", Snapshot::Event { calendar: String::new(), name: "Family".into(), old: Some(old), new: Some(new) }),
            }
        }
        _ => return None,
    };
    let agent = AgentRow { id: "3f9a1c0b77de".into(), kind: "ClaudeCode".into(), name: "Claude Code".into(), host: "studio".into(), remote: false, active: true, expires: None, last_used: None };
    let req = Request { v: 1, token: None, cmd: cmd.into(), args: Default::default(), source_ip: None };
    let (done, closed) = oneshot::channel();
    let account = &crate::mail::demo_accounts()[0];
    let (sheet, answer) = Sheet::new(agent, account, &req, &snapshot, closed, std::sync::Weak::new()).ok()?;
    // The sheet stays up while its answer and close channels are open; a demo never answers.
    std::mem::forget((done, answer));
    Some(sheet)
}

#[derive(Default)]
pub(crate) struct Waiting {
    pub sheets: std::collections::VecDeque<Sheet>,
    pub pressed: Option<(usize, bool)>,
    pub withdrawn: bool,
    pending_withdrawal: Option<std::time::Instant>,
    pub scroll: [gpui::ScrollHandle; 3],
    pub front_since: Option<(usize, std::time::Instant)>,
}

#[derive(Default)]
pub(crate) struct Denials(std::collections::HashMap<String, std::time::Instant>);
impl gpui::Global for Denials {}
impl Denials {
    pub fn deny(&mut self, agent: String) {
        self.0.insert(agent, std::time::Instant::now());
    }
    fn cooling_down(&mut self, agent: &str) -> bool {
        self.0
            .retain(|_, at| at.elapsed() < std::time::Duration::from_secs(30));
        self.0.contains_key(agent)
    }
}
impl gpui::Global for Waiting {}

impl Waiting {
    pub fn prune(&mut self, denials: &mut Denials) {
        if self.sheets.front().is_some_and(|s| s.answer.is_closed()) {
            self.scroll = Default::default();
            self.pending_withdrawal = Some(self.sheets[0].withdrawal_time());
            self.withdrawn = false;
            self.pressed = None;
        }
        self.sheets.retain(|s| {
            if !s.answer.is_closed() { return true; }
            denials.0.insert(s.agent.clone(), s.withdrawal_time());
            false
        });
        if !self.sheets.is_empty() {
            self.show_replacement();
        }
    }

    pub fn answered(&mut self, withdrawn: bool) {
        self.withdrawn = false;
        self.pending_withdrawal = withdrawn.then(std::time::Instant::now);
        if !self.sheets.is_empty() { self.show_replacement(); }
    }

    fn show_replacement(&mut self) {
        if let Some(at) = self.pending_withdrawal.take() {
            self.withdrawn = at.elapsed() < std::time::Duration::from_secs(30);
        }
    }

    pub fn delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(if self.withdrawn { 1500 } else { 700 })
    }

    pub fn armed(&self, serial: usize, keyboard: bool, activating: bool, active: bool) -> bool {
        !keyboard
            && !activating
            && active
            && self
                .front_since
                .is_some_and(|(id, at)| id == serial && at.elapsed() >= self.delay())
    }
    pub fn push(&mut self, sheet: Sheet, denials: &mut Denials) -> bool {
        self.prune(denials);
        if sheet.answer.is_closed() {
            let at = sheet.withdrawal_time();
            denials.0.insert(sheet.agent, at);
            return false;
        }
        if denials.cooling_down(&sheet.agent)
            || self.sheets.len() >= 3
            || self.sheets.iter().any(|s| s.agent == sheet.agent)
        {
            let _ = sheet.answer.send(Err(Code::Busy));
            return false;
        }
        self.sheets.push_back(sheet);
        self.show_replacement();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_preserve_text_shaping_but_escape_direction_and_tags() {
        let text = "\u{200d}\u{fe0f}\u{ad}";
        assert_eq!(super::super::read::clean(text), text);
        assert_eq!(line(text), r"\u{200d}\u{fe0f}\u{ad}");
        let hidden = "\t\u{61c}\u{202e}\u{e0000}\u{e007f}\u{2028}\u{2029}";
        assert_eq!(super::super::read::clean(hidden), r"\t\u{61c}\u{202e}\u{e0000}\u{e007f}\u{2028}\u{2029}");
        assert_ne!(line(r"\u{202e}"), line("\u{202e}"));
    }

    #[test]
    fn identity_fields_escape_invisible_text_and_show_ascii_domain_first() {
        assert_eq!(
            line("a\n\t\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}\u{2028}\u{2029}\u{202e}"),
            r"a\n\t\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}\u{2028}\u{2029}\u{202e}"
        );
        for c in ['\u{ad}', '\u{3164}', '\u{115f}', '\u{1160}', '\u{2061}', '\u{2064}', '\u{e0000}', '\u{e007f}', '\u{600}'] {
            assert_eq!(line(&c.to_string()), c.escape_unicode().to_string());
        }
        for bad in [r#""quoted"@example.org"#, "å@example.org"] {
            assert_eq!(normal(bad), Err(Code::InvalidRequest));
        }
        let name = "a".repeat(70);
        assert_eq!(
            address(&format!("{name} <me@bücher.example>")).unwrap(),
            format!("me@xn--bcher-kva.example ⚠ ({}) (6 more characters)", "a".repeat(64))
        );
        let time = |v| local_time(v).split(' ').nth(2).unwrap().to_owned();
        assert!(time("2026-11-26T14:00:30+00:00").ends_with(":30"));
        assert_eq!(time("2026-11-26T14:00:00+00:00").len(), 5);
    }

    #[test]
    fn guest_tags_compare_the_full_address() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let old = list(&["Mia <mia@family.example>", "ola@family.example", "you@home.example"]);
        let new = list(&["Mia Berg <mia@family.example>", "OLA@Family.Example", "you@home.example.org", "you@home.example"]);
        assert_eq!(guests(Some(&old), Some(&new)).unwrap(), [
            ("mia@family.example (Mia Berg)".into(), None),
            ("OLA@family.example".into(), None),
            ("you@home.example.org".into(), Some(Tag::Added)),
            ("you@home.example".into(), None),
        ]);
        let gone = list(&["mia@family.example"]);
        assert_eq!(guests(Some(&old), Some(&gone)).unwrap(), [
            ("mia@family.example".into(), None),
            ("ola@family.example".into(), Some(Tag::Removed)),
            ("you@home.example".into(), Some(Tag::Removed)),
        ]);
        // A create or a delete lists guests without tags, each once.
        assert_eq!(guests(None, Some(&list(&["a@x.example", "A@X.example"]))).unwrap(), [("a@x.example".into(), None)]);
        assert!(guests(Some(&old), None).unwrap().iter().all(|(_, tag)| tag.is_none()));
    }

    #[test]
    fn arming_belongs_to_the_front_sheet_and_active_mouse_click() {
        use std::time::{Duration, Instant};
        let mut waiting = Waiting::default();
        waiting.front_since = Some((1, Instant::now()));
        assert!(!waiting.armed(1, false, false, true));
        waiting.front_since = Some((1, Instant::now() - Duration::from_millis(701)));
        assert!(waiting.armed(1, false, false, true));
        for (id, key, first, active) in [
            (2, false, false, true),
            (1, true, false, true),
            (1, false, true, true),
            (1, false, false, false),
        ] {
            assert!(!waiting.armed(id, key, first, active));
        }
        let mut denials = Denials::default();
        denials.deny("agent".into());
        assert!(denials.cooling_down("agent"));
        assert!(!denials.cooling_down("another"));
        denials
            .0
            .insert("agent".into(), Instant::now() - Duration::from_secs(30));
        assert!(!denials.cooling_down("agent"));
    }
}

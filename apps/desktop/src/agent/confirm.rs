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
    pub expires: tokio::time::Instant,
    pub serial: usize,
    pub closed: Option<oneshot::Receiver<()>>,
    pub agent: String,
    pub heading: &'static str,
    pub approve_label: &'static str,
    pub rows: Vec<(&'static str, String)>,
    pub previous_description: Option<String>,
    pub body: String,
    pub expanded: bool,
    pub answer: oneshot::Sender<Result<(), Code>>,
}

fn line(value: &str) -> String {
    super::read::clean(value).replace('\n', "\\n")
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
        let mut short: String = name.chars().take(64).collect();
        if short.len() < name.len() {
            short.pop();
            short.push('…');
        }
        result.push_str(&format!(" ({short})"));
    }
    Ok(result)
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
    pub fn decide(self, approve: bool) {
        use std::io::{ErrorKind, Read};
        // Check the socket at the click even if the listener has not handled EOF yet.
        let connected = approve
            && tokio::time::Instant::now() < self.expires
            && self
                .peer
                .upgrade()
                .is_some_and(|s| matches!((&*s).read(&mut [0]),Err(e) if e.kind()==ErrorKind::WouldBlock));
        let _ = self
            .answer
            .send(if connected { Ok(()) } else { Err(Code::Denied) });
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
        let mut row = |label, value: &str| rows.push((label, line(value)));
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
                if let Some(s) = old {
                    for guest in &s.attendees {
                        row("Current guest", &address(guest)?);
                    }
                }
                if let Some(e) = new {
                    let guests = if e.attendees.is_empty() {
                        old.as_ref().map(|s| &s.attendees).unwrap_or(&e.attendees)
                    } else {
                        &e.attendees
                    };
                    for guest in guests {
                        row("Guest", &address(guest)?);
                    }
                }
                let was = old.as_ref().map(|s| &s.event);
                let location = |e: &NewEvent| e.location.as_deref().or_else(|| was?.location.as_deref()).unwrap_or("").to_owned();
                // Each current value sits right above its new one, so a change reads as a pair.
                let fields = [
                    ("Current title", "Title", was.map(|e| e.title.clone()), new.as_ref().map(|e| e.title.clone())),
                    ("Current start", "Start", was.map(|e| local_time(&e.start)), new.as_ref().map(|e| local_time(&e.start))),
                    ("Current end", "End", was.map(|e| local_time(&e.end)), new.as_ref().map(|e| local_time(&e.end))),
                    ("Current location", "Location", was.map(|e| e.location.clone().unwrap_or_default()), new.as_ref().map(location)),
                    ("Current Meet", "Create Meet link", was.map(|e| e.join_url.clone().unwrap_or_else(|| "None".into())),
                        new.as_ref().map(|e| if e.meet { "Yes" } else { "No" }.into())),
                ];
                for (ix, (current, next, before, after)) in fields.into_iter().enumerate() {
                    if let Some(v) = before {
                        row(current, &v);
                    }
                    if let Some(v) = after {
                        row(next, &v);
                    }
                    if ix == 0 {
                        row("Calendar", name);
                    }
                }
                previous_description = old.as_ref().and_then(|s| s.description.as_deref()).map(super::read::clean);
                body = match new {
                    Some(e) => e.description.as_deref().or_else(|| old.as_ref()?.description.as_deref()).unwrap_or("").to_owned(),
                    None => String::new(),
                };
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
                expires: tokio::time::Instant::now() + std::time::Duration::from_secs(120),
                serial: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                closed: Some(closed),
                agent: agent.id,
                heading,
                approve_label,
                rows,
                previous_description,
                body: super::read::clean(&body),
                expanded: false,
                answer,
            },
            rx,
        ))
    }
}

/// `LAGOM_SCREEN=agent-confirm-send` and `agent-confirm-event`: a sample sheet for screenshots.
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
        "agent-confirm-event" => {
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
            ("event.update", Snapshot::Event { calendar: String::new(), name: "Family".into(), old: Some(old), new: Some(new) })
        }
        _ => return None,
    };
    let agent = AgentRow { id: "3f9a1c0b77de".into(), kind: "ClaudeCode".into(), name: "Claude Code".into(), host: "studio".into(), remote: false, active: true };
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
    pub pressed: Option<usize>,
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
    pub fn armed(&self, serial: usize, keyboard: bool, activating: bool, active: bool) -> bool {
        !keyboard
            && !activating
            && active
            && self
                .front_since
                .is_some_and(|(id, at)| id == serial && at.elapsed() >= std::time::Duration::from_millis(700))
    }
    pub fn push(&mut self, sheet: Sheet, denials: &mut Denials) -> bool {
        self.sheets.retain(|s| !s.answer.is_closed());
        if sheet.answer.is_closed() {
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_fields_escape_invisible_text_and_show_ascii_domain_first() {
        assert_eq!(
            line("a\n\t\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}\u{2028}\u{2029}\u{202e}"),
            r"a\n\t\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}\u{2028}\u{2029}\u{202e}"
        );
        let name = "a".repeat(70);
        assert_eq!(
            address(&format!("{name} <me@bücher.example>")).unwrap(),
            format!("me@xn--bcher-kva.example ⚠ ({}…)", "a".repeat(63))
        );
        let time = |v| local_time(v).split(' ').nth(2).unwrap().to_owned();
        assert!(time("2026-11-26T14:00:30+00:00").ends_with(":30"));
        assert_eq!(time("2026-11-26T14:00:00+00:00").len(), 5);
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

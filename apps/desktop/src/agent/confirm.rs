use super::write::Snapshot;
use lagom_core::{
    account::Account,
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
    pub heading: String,
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
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
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
        row("Account", &address(&account.email)?);
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
                if let Some(s) = old {
                    row("Current title", &s.event.title);
                }
                if let Some(e) = new {
                    row("Title", &e.title);
                }
                row("Calendar", name);
                if let Some(s) = old {
                    row("Current start", &local_time(&s.event.start));
                    row("Current end", &local_time(&s.event.end));
                    row("Current location", s.event.location.as_deref().unwrap_or(""));
                    row("Current Meet", s.event.join_url.as_deref().unwrap_or("None"));
                    previous_description = s.description.as_deref().map(super::read::clean);
                }
                body = if let Some(e) = new {
                    row("Start", &local_time(&e.start));
                    row("End", &local_time(&e.end));
                    row(
                        "Location",
                        e.location
                            .as_deref()
                            .or_else(|| old.as_ref()?.event.location.as_deref())
                            .unwrap_or(""),
                    );
                    row("Create Meet link", if e.meet { "Yes" } else { "No" });
                    e.description
                        .as_deref()
                        .or_else(|| old.as_ref()?.description.as_deref())
                        .unwrap_or("")
                        .to_owned()
                } else {
                    String::new()
                };
            }
            Snapshot::None => unreachable!(),
        }
        let (purpose, approve_label) = match req.cmd.as_str() {
            "draft.send" => ("send this message", "Send"),
            "event.create" => ("create this event", "Create event"),
            "event.update" => ("update this event", "Update event"),
            "event.delete" => ("delete this event", "Delete event"),
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
                heading: format!("Request to {purpose}"),
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

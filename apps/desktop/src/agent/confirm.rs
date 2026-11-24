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
    pub details: String,
    pub body: String,
    pub expanded: bool,
    pub answer: oneshot::Sender<Result<(), Code>>,
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
        let _ = self.answer.send(if connected { Ok(()) } else { Err(Code::Denied) });
    }

    pub fn new(
        agent: AgentRow,
        account: &Account,
        req: &Request,
        snapshot: &Snapshot,
        closed: oneshot::Receiver<()>,
        peer: std::sync::Weak<std::os::unix::net::UnixStream>,
    ) -> (Self, oneshot::Receiver<Result<(), Code>>) {
        let (answer, rx) = oneshot::channel();
        let mut details = format!("Account  {}\n", account.email);
        let mut body = String::new();
        match snapshot {
            Snapshot::Mail(_, m) => {
                details += &format!(
                    "To  {}\nCc  {}\nBcc  {}\nSubject  {}\n",
                    m.to.join(", "),
                    m.cc.join(", "),
                    m.bcc.join(", "),
                    m.subject
                );
                body = m.text.clone();
                if let Some(quote) = &m.quoted {
                    body.push_str("\n\n");
                    body.push_str(&quote.text);
                    for part in lagom_core::html::sanitize_parts(&quote.html).1 {
                        details += &format!("Inline image  {} ({} bytes)\n", part.mime_type, part.data.len());
                    }
                }
                for p in &m.attachments {
                    details += &format!("Attachment  {} ({} bytes)\n", p.filename, p.data.len());
                }
            }
            Snapshot::Event { calendar, old, new } => {
                details += &format!("Calendar  {calendar}\n");
                if let Some(s) = old {
                    details += &format!("Current location  {}\n", s.event.location.as_deref().unwrap_or(""));
                    details += &format!(
                        "Current event  {}\n{} – {}\n{}\nGuests  {}\nMeet  {}\n",
                        s.event.title,
                        s.event.start,
                        s.event.end,
                        s.description.as_deref().unwrap_or(""),
                        s.attendees.join(", "),
                        s.event.join_url.as_deref().unwrap_or("None")
                    );
                }
                if let Some(e) = new {
                    details += &format!(
                        "Location  {}\n",
                        e.location
                            .as_deref()
                            .or(old.as_ref().and_then(|s| s.event.location.as_deref()))
                            .unwrap_or("")
                    );
                    details += &format!(
                        "Proposed event  {}\n{} – {}\n{}\nGuests  {}\nCreate Meet link  {}",
                        e.title,
                        e.start,
                        e.end,
                        e.description
                            .as_deref()
                            .or(old.as_ref().and_then(|o| o.description.as_deref()))
                            .unwrap_or(""),
                        if e.attendees.is_empty() {
                            old.as_ref().map(|o| o.attendees.join(", ")).unwrap_or_default()
                        } else {
                            e.attendees.join(", ")
                        },
                        e.meet
                    );
                }
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
        (
            Self {
                peer,
                expires: tokio::time::Instant::now() + std::time::Duration::from_secs(120),
                serial: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                closed: Some(closed),
                agent: agent.id,
                heading: super::read::clean(&format!("{} ({}) wants to {purpose}", agent.name, agent.host)),
                approve_label,
                details: super::read::clean(&details),
                body: super::read::clean(&body),
                expanded: false,
                answer,
            },
            rx,
        )
    }
}

#[derive(Default)]
pub(crate) struct Waiting(pub std::collections::VecDeque<Sheet>);
impl gpui::Global for Waiting {}

impl Waiting {
    pub fn push(&mut self, sheet: Sheet) -> bool {
        self.0.retain(|s| !s.answer.is_closed());
        if sheet.answer.is_closed() {
            return false;
        }
        if self.0.len() >= 3 || self.0.iter().any(|s| s.agent == sheet.agent) {
            let _ = sheet.answer.send(Err(Code::Busy));
            return false;
        }
        self.0.push_back(sheet);
        true
    }
}

//! New-mail selection is independent of the notification service and window state.
use lagom_core::store::MessageSummary;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[cfg(all(target_os = "macos", not(test)))]
mod macos;
#[cfg(all(target_os = "macos", not(test)))]
use macos as platform;
// Compiled in tests too, for its click parsing test.
#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
mod linux;
#[cfg(all(target_os = "linux", not(test)))]
use linux as platform;
#[cfg(all(target_os = "windows", not(test)))]
mod toast;
#[cfg(all(target_os = "windows", not(test)))]
use toast as platform;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub account: String,
    pub message: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
struct Notice {
    title: String,
    body: String,
    target: Target,
}

fn notice(messages: &[MessageSummary]) -> Option<Notice> {
    let first = messages.first()?;
    Some(Notice {
        title: if messages.len() == 1 {
            if first.from_name.is_empty() { first.from_address.clone() } else { first.from_name.clone() }
        } else { "lagom".into() },
        body: if messages.len() == 1 { first.subject.clone() } else { format!("{} new messages", messages.len()) },
        target: Target { account: first.account.clone(), message: (messages.len() == 1).then_some(first.id) },
    })
}

/// A Windows toast: sender and subject as plain text. XML 1.0 has no control characters.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn toast_xml(notice: &Notice) -> String {
    let text = |s: &str| s.chars().filter(|c| !c.is_control() || matches!(c, '\t' | '\n')).collect::<String>()
        .replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        text(&notice.title), text(&notice.body))
}

#[derive(Debug)]
pub enum Event {
    Authorized(bool),
    Clicked(Target),
    Failed(&'static str),
}

// The macOS center holds its delegate weakly; retain the service even without a window.
pub struct State {
    pub service: Service,
    pub _events: gpui::Task<()>,
}

impl gpui::Global for State {}

pub fn init(cx: &mut gpui::App, reopen: impl Fn(&mut gpui::App) -> anyhow::Result<gpui::WindowHandle<crate::MainWindow>> + 'static) {
    let (service, mut events) = Service::new();
    let task = cx.spawn(async move |cx| {
        while let Some(event) = events.recv().await {
            if cx.update(|cx| {
                let window = if matches!(event, Event::Clicked(_)) {
                    match reopen(cx) {
                        Ok(window) => Some(window),
                        Err(error) => { eprintln!("Could not open notification: {error}"); None }
                    }
                } else {
                    cx.windows().into_iter().find_map(|w| w.downcast::<crate::MainWindow>())
                };
                if let Some(window) = window {
                    window.update(cx, |view, window, cx| view.on_notification(event, window, cx)).ok();
                } else if let Event::Authorized(allowed) = event {
                    let pending = cx.global_mut::<State>().service.authorized(allowed);
                    crate::main_window::notify_without_window(pending, cx);
                } else if let Event::Failed(error) = event {
                    eprintln!("{error}");
                }
            }).is_err() { return; }
        }
    });
    cx.set_global(State { service, _events: task });
}

pub struct Service {
    platform: platform::Service,
    requested: bool,
    authorized: bool,
    pending: Vec<MessageSummary>,
}

impl Service {
    pub fn new() -> (Self, UnboundedReceiver<Event>) {
        let (tx, rx) = unbounded_channel();
        (Self { platform: platform::Service::new(tx), requested: false, authorized: false, pending: Vec::new() }, rx)
    }

    pub fn show(&mut self, messages: Vec<MessageSummary>) {
        if messages.is_empty() { return; }
        if self.authorized {
            let mut accounts = std::collections::BTreeMap::<_, Vec<_>>::new();
            for message in messages {
                accounts.entry(message.account.clone()).or_default().push(message);
            }
            for messages in accounts.values() {
                if let Some(notice) = notice(messages) { self.platform.show(notice); }
            }
        } else if !self.requested || !self.pending.is_empty() {
            self.pending.extend(messages);
            if !self.requested {
                self.requested = true;
                self.platform.authorize();
            }
        }
    }

    #[cfg(test)]
    pub fn event_sender(&self) -> UnboundedSender<Event> { self.platform.events.clone() }

    #[cfg(test)]
    pub fn posted_count(&self) -> usize { self.platform.notices.borrow().len() }

    pub fn authorized(&mut self, allowed: bool) -> Vec<MessageSummary> {
        self.authorized = allowed;
        let pending = std::mem::take(&mut self.pending);
        if allowed { pending } else { Vec::new() }
    }
}

#[cfg(all(not(any(target_os = "macos", target_os = "linux", target_os = "windows")), not(test)))]
mod platform {
    use super::*;

    pub struct Service(UnboundedSender<Event>);
    impl Service {
        pub fn new(events: UnboundedSender<Event>) -> Self { Self(events) }
        pub fn authorize(&self) {
            let _ = self.0.send(Event::Authorized(false));
            let _ = self.0.send(Event::Failed("Notifications are not available on this platform yet"));
        }
        pub fn show(&self, _: Notice) {}
    }
}

#[cfg(test)]
mod platform {
    use super::*;
    pub struct Service {
        pub(super) events: UnboundedSender<Event>,
        pub notices: std::cell::RefCell<Vec<Notice>>,
    }
    impl Service {
        pub fn new(events: UnboundedSender<Event>) -> Self { Self { events, notices: Default::default() } }
        pub fn authorize(&self) { let _ = self.events.send(Event::Authorized(true)); }
        pub fn show(&self, notice: Notice) { self.notices.borrow_mut().push(notice); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_is_lazy_once_and_pending_mail_is_batched() {
        let (mut service, mut events) = Service::new();
        assert!(events.try_recv().is_err());
        service.show(Vec::new());
        assert!(events.try_recv().is_err());
        service.show(vec![message(11)]);
        service.show(vec![message(12)]);
        assert!(matches!(events.try_recv(), Ok(Event::Authorized(true))));
        assert!(events.try_recv().is_err());
        assert!(service.platform.notices.borrow().is_empty());
        let pending = service.authorized(true);
        service.show(pending);
        assert_eq!(service.platform.notices.borrow()[0].body, "2 new messages");
        service.show(vec![message(13)]);
        assert!(events.try_recv().is_err());
        assert_eq!(service.platform.notices.borrow().len(), 2);
    }
    #[test]
    fn pending_permission_groups_notifications_by_account() {
        let (mut service, _) = Service::new();
        service.show(vec![message(11)]);
        let mut other = message(12);
        other.account = "other".into();
        let mut third = other.clone();
        third.id = 13;
        service.show(vec![other, third]);
        let pending = service.authorized(true);
        service.show(pending);
        let notices = service.platform.notices.borrow();
        assert_eq!(notices.len(), 2);
        let first = notices.iter().find(|n| n.target.account == "fixture").unwrap();
        assert_eq!(first.body, "Subject");
        assert_eq!(first.target.message, Some(11));
        let other = notices.iter().find(|n| n.target.account == "other").unwrap();
        assert_eq!(other.title, "lagom");
        assert_eq!(other.body, "2 new messages");
        assert_eq!(other.target.message, None);
    }

    #[test]
    fn denied_permission_discards_pending_mail_without_prompting_again() {
        let (mut service, mut events) = Service::new();
        service.show(vec![message(11)]);
        events.try_recv().unwrap();
        assert!(service.authorized(false).is_empty());
        service.show(vec![message(12)]);
        assert!(events.try_recv().is_err());
        assert!(service.platform.notices.borrow().is_empty());
        assert!(service.pending.is_empty());
    }

    fn message(uid: u32) -> MessageSummary {
        MessageSummary { id: uid as i64, account: "fixture".into(), folder: 1, uid,
            from_address: "sender@example.test".into(), subject: "Subject".into(), ..Default::default() }
    }
    #[test]
    fn toast_xml_is_escaped_plain_text() {
        let notice = Notice { title: "A & B <a@b>".into(), body: "Re: \u{1}5 > 4".into(), target: Target { account: "a".into(), message: None } };
        assert_eq!(toast_xml(&notice), r#"<toast><visual><binding template="ToastGeneric"><text>A &amp; B &lt;a@b&gt;</text><text>Re: 5 &gt; 4</text></binding></visual></toast>"#);
    }

    #[test]
    fn single_sender_subject_and_batch_summary_have_correct_targets() {
        assert!(notice(&[]).is_none());
        let mut m = message(11);
        let single = notice(&[m.clone()]).unwrap();
        assert_eq!((single.title.as_str(), single.body.as_str()), ("sender@example.test", "Subject"));
        assert_eq!(single.target.message, Some(11));
        m.from_name = "Sender".into();
        assert_eq!(notice(&[m.clone()]).unwrap().title, "Sender");
        let batch = notice(&[m, message(12), message(13)]).unwrap();
        assert_eq!(batch.body, "3 new messages");
        assert_eq!(batch.target, Target { account: "fixture".into(), message: None });
        let bytes = serde_json::to_string(&single.target).unwrap();
        assert_eq!(serde_json::from_str::<Target>(&bytes).unwrap(), single.target);
    }
}

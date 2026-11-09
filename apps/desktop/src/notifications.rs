//! New-mail selection is independent of the notification service and window state.
use lagom_core::store::{Folder, FolderRole, MessageSummary};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[cfg(all(target_os = "macos", not(test)))]
mod macos;
#[cfg(all(target_os = "macos", not(test)))]
use macos as platform;

pub fn new_mail(before: &Folder, after: &Folder, messages: Vec<MessageSummary>) -> Vec<MessageSummary> {
    if before.role != FolderRole::Inbox || after.role != FolderRole::Inbox
        || before.id != after.id || before.account != after.account
        || before.uidvalidity.is_none() || before.uidvalidity != after.uidvalidity
        || (before.last_uid == 0 && before.uidnext.is_none())
    {
        return Vec::new();
    }
    messages.into_iter().filter(|m| {
        m.account == after.account && m.folder == after.id && m.uid > before.last_uid
            && m.uid <= after.last_uid && !m.seen && m.snoozed_until.is_none()
    }).collect()
}

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
        } else { "Lagom".into() },
        body: if messages.len() == 1 { first.subject.clone() } else { format!("{} new messages", messages.len()) },
        target: Target { account: first.account.clone(), message: (messages.len() == 1).then_some(first.id) },
    })
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

#[cfg(all(target_os = "linux", not(test)))]
mod platform {
    use super::*;

    pub struct Service(UnboundedSender<Event>);
    impl Service {
        pub fn new(events: UnboundedSender<Event>) -> Self { Self(events) }
        pub fn authorize(&self) { let _ = self.0.send(Event::Authorized(true)); }
        pub fn show(&self, notice: Notice) {
            // shortcut: clicks need a running process; add desktop-entry activation with Linux packaging.
            let events = self.0.clone();
            let failed = events.clone();
            if std::thread::Builder::new().name("lagom-notification".into()).spawn(move || {
                // Escape server-supported body markup: mail subjects are plain text.
                let body = notice.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                match notify_rust::Notification::new().appname("Lagom").summary(&notice.title)
                    .body(&body).action("default", "Open").timeout(10_000).show()
                {
                    Ok(handle) => handle.wait_for_action(|action| {
                        if action == "default" { let _ = events.send(Event::Clicked(notice.target)); }
                    }),
                    Err(_) => { let _ = events.send(Event::Failed("Could not post a notification")); },
                }
            }).is_err() { let _ = failed.send(Event::Failed("Could not start notifications")); }
        }
    }
}

#[cfg(all(not(any(target_os = "macos", target_os = "linux")), not(test)))]
mod platform {
    use super::*;

    // shortcut: Windows needs a registered app identity for toast activation; add it with the installer.
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

    fn folder() -> Folder {
        Folder { id: 1, account: "fixture".into(), name: "INBOX".into(), delimiter: None,
            role: FolderRole::Inbox, uidvalidity: Some(4), uidnext: Some(11), last_uid: 10,
            highest_modseq: None, unread: 0 }
    }
    fn message(uid: u32) -> MessageSummary {
        MessageSummary { id: uid as i64, account: "fixture".into(), folder: 1, uid,
            from_address: "sender@example.test".into(), subject: "Subject".into(), ..Default::default() }
    }
    #[test]
    fn initial_sync_partial_initial_retry_and_uidvalidity_reset_are_silent() {
        let after = folder();
        let mut before = after.clone();
        before.uidvalidity = None;
        before.uidnext = None;
        before.last_uid = 0;
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
        before.uidvalidity = after.uidvalidity;
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
        before.uidnext = Some(1);
        before.uidvalidity = Some(3);
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
    }
    #[test]
    fn empty_synced_inbox_and_cached_restart_accept_new_uids() {
        let after = folder();
        let mut before = after.clone();
        before.last_uid = 0;
        before.uidnext = Some(1);
        assert_eq!(new_mail(&before, &after, vec![message(10)]), vec![message(10)]);
        before.last_uid = 9;
        assert_eq!(new_mail(&before, &after, vec![message(10)]), vec![message(10)]);
        assert!(new_mail(&after, &after, vec![message(10)]).is_empty());
    }
    #[test]
    fn only_new_unread_unsnoozed_inbox_mail_counts() {
        let before = folder();
        let mut after = before.clone();
        after.last_uid = 20;
        let mut messages: Vec<_> = (10..=17).map(message).collect();
        messages[2].seen = true;
        messages[3].snoozed_until = Some(i64::MAX);
        messages[4].snoozed_until = Some(1);
        messages[5].folder = 2;
        messages[6].account = "other".into();
        messages.push(message(21));
        assert_eq!(new_mail(&before, &after, messages), vec![message(11), message(17)]);
        after.role = FolderRole::Archive;
        assert!(new_mail(&before, &after, vec![message(11)]).is_empty());
        let mut before = before;
        before.role = FolderRole::Archive;
        assert!(new_mail(&before, &after, vec![message(11)]).is_empty());
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

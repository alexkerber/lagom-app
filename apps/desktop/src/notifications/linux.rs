//! Linux notifications, with the backend choice GLib's GNotification makes. GNOME's
//! org.gtk.Notifications calls back through org.freedesktop.Application.ActivateAction on our bus
//! name, and D-Bus starts lagom first when it is not running (the .deb installs
//! packaging/linux/app.lagommail.desktop.service). Other servers get freedesktop notifications,
//! whose clicks reach only the running process that posted them.
use super::{Event, Notice, Target, UnboundedSender};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::wire::{OwnedVariant, Variant};

/// Desktop file, icon, D-Bus name and window app_id; main.rs repeats it for the window.
pub const APP_ID: &str = "app.lagommail.desktop";
const PATH: &str = "/app/lagommail/desktop";
const OPEN: &str = "open-message";

struct Application(UnboundedSender<Event>);

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    // Starting the process already opened the main window.
    fn activate(&self, _platform_data: HashMap<String, OwnedVariant>) {}
    fn open(&self, _uris: Vec<String>, _platform_data: HashMap<String, OwnedVariant>) {}
    fn activate_action(&self, name: String, parameter: Vec<OwnedVariant>, _platform_data: HashMap<String, OwnedVariant>) {
        if let Some(target) = clicked(&name, &parameter) { let _ = self.0.send(Event::Clicked(target)); }
    }
}

/// The target of a notification click; the parameter crosses the session bus, so parse strictly.
fn clicked(name: &str, parameter: &[OwnedVariant]) -> Option<Target> {
    if name != OPEN { return None; }
    let [json] = parameter else { return None };
    serde_json::from_str(<&str>::try_from(json).ok()?).ok()
}

pub struct Service {
    events: UnboundedSender<Event>,
    // None without a session bus or when another lagom owns the name: freedesktop only.
    bus: Option<zbus::Connection>,
    next: AtomicU64,
}

impl Service {
    pub fn new(events: UnboundedSender<Event>) -> Self {
        let bus = zbus::block_on(zbus::connection::Builder::session()
            .name(APP_ID).serve_at(PATH, Application(events.clone())).build()).ok();
        Self { events, bus, next: AtomicU64::new(0) }
    }

    pub fn authorize(&self) { let _ = self.events.send(Event::Authorized(true)); }

    pub fn show(&self, notice: Notice) {
        let (events, bus) = (self.events.clone(), self.bus.clone());
        // GNOME replaces a notification with the same ID, also one left from an earlier run.
        let id = format!("mail-{}-{}", std::process::id(), self.next.fetch_add(1, Ordering::Relaxed));
        let failed = events.clone();
        if std::thread::Builder::new().name("lagom-notification".into()).spawn(move || {
            if bus.is_some_and(|bus| gtk(&bus, &id, &notice).is_some()) { return; }
            fdo(notice, events);
        }).is_err() { let _ = failed.send(Event::Failed("Could not start notifications")); }
    }
}

/// GNOME only. It fails without the installed desktop file (cargo run, a bare AppImage).
fn gtk(bus: &zbus::Connection, id: &str, notice: &Notice) -> Option<()> {
    let target = serde_json::to_string(&notice.target).ok()?;
    let action = format!("app.{OPEN}");
    let content = HashMap::from([
        ("title", Variant::from(notice.title.as_str())),
        ("body", Variant::from(notice.body.as_str())),
        ("default-action", Variant::from(action.as_str())),
        ("default-action-target", Variant::from(target.as_str())),
    ]);
    zbus::block_on(bus.call_method(Some("org.gtk.Notifications"), "/org/gtk/Notifications",
        Some("org.gtk.Notifications"), "AddNotification", &(APP_ID, id, content))).ok().map(drop)
}

fn fdo(notice: Notice, events: UnboundedSender<Event>) {
    // Escape server-supported body markup: mail subjects are plain text.
    let body = notice.body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    match notify_rust::Notification::new().appname("lagom").icon(APP_ID)
        .hint(notify_rust::Hint::DesktopEntry(APP_ID.into()))
        .summary(&notice.title).body(&body).action("default", "Open").timeout(10_000).show()
    {
        Ok(handle) => handle.wait_for_action(|action| {
            if action == "default" { let _ = events.send(Event::Clicked(notice.target)); }
        }),
        Err(_) => { let _ = events.send(Event::Failed("Could not post a notification")); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_open_action_with_one_target_string_clicks() {
        let target = Target { account: "a@example.test".into(), message: Some(7) };
        let json = serde_json::to_string(&target).unwrap();
        let param = |v: Variant<'_>| OwnedVariant::try_from(v).unwrap();
        assert_eq!(clicked(OPEN, &[param(Variant::from(json.as_str()))]), Some(target));
        assert_eq!(clicked("quit", &[param(Variant::from(json.as_str()))]), None);
        assert_eq!(clicked(OPEN, &[]), None);
        assert_eq!(clicked(OPEN, &[param(Variant::from(7u32))]), None);
        assert_eq!(clicked(OPEN, &[param(Variant::from("not json"))]), None);
    }
}

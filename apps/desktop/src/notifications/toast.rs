use super::*;
use std::collections::VecDeque;
use windows::{
    core::{w, IInspectable, HSTRING},
    Data::Xml::Dom::XmlDocument,
    Foundation::TypedEventHandler,
    UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier},
    Win32::{
        System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ},
        UI::Shell::SetCurrentProcessExplicitAppUserModelID,
    },
};

/// The AppUserModelID the installer gives the Start menu shortcut and registers for toasts.
const APP_ID: &str = "app.lagommail.desktop";

pub struct Service {
    notifier: Option<ToastNotifier>,
    // A toast's Activated handler fires only while its object lives; keep what Action Center keeps.
    // shortcut: clicks need a running lagom; register a COM activator when cold-start activation matters.
    shown: VecDeque<ToastNotification>,
    events: UnboundedSender<Event>,
}

impl Service {
    pub fn new(events: UnboundedSender<Event>) -> Self {
        // The same identity as the shortcut, so the taskbar groups the window with its pinned icon.
        let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(w!("app.lagommail.desktop")) };
        // Toasts for an identity nobody registered never appear, so a bare executable reports that.
        let registered = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Classes\AppUserModelId\app.lagommail.desktop"), w!("DisplayName"), RRF_RT_REG_SZ, None, None, None)
        }.is_ok();
        let notifier = registered.then(|| ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)).ok()).flatten();
        Self { notifier, shown: VecDeque::new(), events }
    }

    pub fn authorize(&self) {
        let allowed = self.notifier.is_some();
        let _ = self.events.send(Event::Authorized(allowed));
        if !allowed { let _ = self.events.send(Event::Failed("Notifications require the installed lagom")); }
    }

    pub fn show(&mut self, notice: Notice) {
        if self.post(notice).is_err() { let _ = self.events.send(Event::Failed("Could not post a notification")); }
    }

    fn post(&mut self, notice: Notice) -> windows::core::Result<()> {
        let Some(notifier) = &self.notifier else { return Ok(()) };
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(toast_xml(&notice)))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        let (events, target) = (self.events.clone(), notice.target);
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
            let _ = events.send(Event::Clicked(target.clone()));
            Ok(())
        }))?;
        notifier.Show(&toast)?;
        if self.shown.len() == 20 { self.shown.pop_front(); }
        self.shown.push_back(toast);
        Ok(())
    }
}

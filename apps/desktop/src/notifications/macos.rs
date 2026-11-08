use super::*;
use block2::{DynBlock, RcBlock};
use objc2::{define_class, msg_send, rc::Retained, runtime::{Bool, ProtocolObject}, AnyThread, DefinedClass};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationDefaultActionIdentifier,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LagomNotificationDelegate"]
    #[ivars = UnboundedSender<Event>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn clicked(&self, _: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &DynBlock<dyn Fn()>) {
            if response.actionIdentifier().isEqualToString(unsafe { UNNotificationDefaultActionIdentifier }) {
                let id = response.notification().request().identifier().to_string();
                if let Ok(target) = serde_json::from_str(&id) {
                    let _ = self.ivars().send(Event::Clicked(target));
                }
            }
            done.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn present(&self, _: &UNUserNotificationCenter, _: &UNNotification, done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
            // The main window may be inactive while another app window is in front.
            done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
        }
    }
);

pub struct Service {
    center: Option<Retained<UNUserNotificationCenter>>,
    _delegate: Retained<Delegate>,
    events: UnboundedSender<Event>,
}

impl Service {
    pub fn new(events: UnboundedSender<Event>) -> Self {
        let delegate: Retained<Delegate> = unsafe { msg_send![super(Delegate::alloc().set_ivars(events.clone())), init] };
        // UNUserNotificationCenter raises an exception for an unbundled executable.
        let center = NSBundle::mainBundle().bundleIdentifier().map(|_| UNUserNotificationCenter::currentNotificationCenter());
        if let Some(center) = &center { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))); }
        Self { center, _delegate: delegate, events }
    }

    pub fn authorize(&self) {
        let Some(center) = &self.center else {
            let _ = self.events.send(Event::Authorized(false));
            let _ = self.events.send(Event::Failed("Notifications require the Lagom app bundle"));
            return;
        };
        let events = self.events.clone();
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &RcBlock::new(move |allowed: Bool, error: *mut NSError| {
            let _ = events.send(Event::Authorized(allowed.as_bool() && error.is_null()));
            if !error.is_null() { let _ = events.send(Event::Failed("Could not request notification permission")); }
        }));
    }

    pub fn show(&self, notice: Notice) {
        let Some(center) = &self.center else { return };
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        content.setBody(&NSString::from_str(&notice.body));
        let id = NSString::from_str(&serde_json::to_string(&notice.target).expect("notification target"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        let events = self.events.clone();
        center.addNotificationRequest_withCompletionHandler(&request, Some(&RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() { let _ = events.send(Event::Failed("Could not post a notification")); }
        })));
    }
}

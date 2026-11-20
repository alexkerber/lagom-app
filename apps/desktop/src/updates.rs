//! Sparkle 2 updates for the notarized dmg (cfg `lagom_sparkle`, see build.rs). The App Store
//! build has none. Info.plist holds the feed, the EdDSA public key and the policy: check daily,
//! download in the background, install on quit. Sparkle draws its own windows.
//!
//! The framework is loaded from Contents/Frameworks at launch instead of linked, so `cargo build`
//! needs no copy of it. The hardened runtime still only loads it with our Team ID signature.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSBundle, NSString};

thread_local! {
    /// The SPUStandardUpdaterController. `None` when Sparkle did not load. Main thread only.
    static CONTROLLER: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
}

/// Starts the updater. Call once on the main thread after NSApplication exists.
pub fn start() {
    CONTROLLER.with(|c| { c.get_or_init(load); });
}

fn load() -> Option<Retained<AnyObject>> {
    let frameworks = NSBundle::mainBundle().privateFrameworksPath()?;
    let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
    // SAFETY: loads the framework we ship and signed; dyld applies library validation.
    let loaded = NSBundle::bundleWithPath(&path).is_some_and(|b| unsafe { b.load() });
    let Some(class) = AnyClass::get(c"SPUStandardUpdaterController").filter(|_| loaded) else {
        eprintln!("Sparkle did not load from {path}; updates are off");
        return None;
    };
    #[cfg(debug_assertions)]
    test_feed();
    // SAFETY: Sparkle's designated initializer; nil delegates give the standard behaviour.
    unsafe {
        let controller: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![controller, initWithStartingUpdater: true, updaterDelegate: None::<&AnyObject>, userDriverDelegate: None::<&AnyObject>]
    }
}

/// Debug builds only: `LAGOM_SPARKLE_FEED` points Sparkle at a local test appcast. The
/// registration domain is not saved, so the next launch uses Info.plist again.
#[cfg(debug_assertions)]
fn test_feed() {
    use objc2_foundation::{NSDictionary, NSUserDefaults};
    let Ok(url) = std::env::var("LAGOM_SPARKLE_FEED") else { return };
    let value = NSString::from_str(&url);
    let defaults = NSDictionary::<NSString, AnyObject>::from_slices(&[&*NSString::from_str("SUFeedURL")], &[&*value as &AnyObject]);
    // SAFETY: a dictionary of property-list values, as the method requires.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

fn with<R>(f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
    CONTROLLER.with(|c| c.get().and_then(Option::as_deref).map(f))
}

fn updater(controller: &AnyObject) -> Retained<AnyObject> {
    // SAFETY: `updater` is a non-null property of SPUStandardUpdaterController.
    unsafe { msg_send![controller, updater] }
}

/// The menu's Check for Updates…: Sparkle's window with the result.
pub fn check() {
    // SAFETY: the standard IBAction; the sender is unused.
    with(|c| unsafe { let _: () = msg_send![c, checkForUpdates: None::<&AnyObject>]; });
}

/// Whether Sparkle checks on its own. `None` when Sparkle did not load, so Settings hides the row.
pub fn automatic() -> Option<bool> {
    // SAFETY: a BOOL property of SPUUpdater.
    with(|c| unsafe { msg_send![&*updater(c), automaticallyChecksForUpdates] })
}

/// Sparkle saves the choice in its own user defaults.
pub fn set_automatic(on: bool) {
    // SAFETY: the BOOL setter of the same property.
    with(|c| unsafe { let _: () = msg_send![&*updater(c), setAutomaticallyChecksForUpdates: on]; });
}

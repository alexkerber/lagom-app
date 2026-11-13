//! macOS: whether lagom opens mailto links, and making it so. The bundle's Info.plist declares the scheme.
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSString};

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    // CFStringRef in and out; NSString is toll-free bridged.
    fn LSCopyDefaultHandlerForURLScheme(scheme: *const c_void) -> *mut c_void;
    // shortcut: deprecated since macOS 12 but still honoured for mailto; move to NSWorkspace
    // setDefaultApplicationAtURL:toOpenURLsWithScheme: when Apple removes it.
    fn LSSetDefaultHandlerForURLScheme(scheme: *const c_void, handler: *const c_void) -> i32;
}

/// `None` outside an app bundle (`cargo run`): there is nothing to register.
pub fn is_default() -> Option<bool> {
    let id = NSBundle::mainBundle().bundleIdentifier()?;
    let scheme = NSString::from_str("mailto");
    // SAFETY: a valid CFString in; the Copy rule hands us one reference, which Retained releases.
    let current = unsafe { Retained::from_raw(LSCopyDefaultHandlerForURLScheme(Retained::as_ptr(&scheme).cast()).cast::<NSString>()) };
    Some(current.is_some_and(|current| current.to_string().eq_ignore_ascii_case(&id.to_string())))
}

/// True when Launch Services took the change.
pub fn make_default() -> bool {
    let Some(id) = NSBundle::mainBundle().bundleIdentifier() else { return false };
    let scheme = NSString::from_str("mailto");
    // SAFETY: both arguments are valid CFStrings for the call's duration.
    unsafe { LSSetDefaultHandlerForURLScheme(Retained::as_ptr(&scheme).cast(), Retained::as_ptr(&id).cast()) == 0 }
}

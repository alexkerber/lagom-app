use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{NSData, NSString};

/// The standard About panel: the icon set by `install`, the name, and the bundle's version.
pub fn about() {
    let Some(main_thread) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(main_thread).orderFrontStandardAboutPanel(None);
}

pub fn install() {
    let Some(main_thread) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(include_bytes!("../../../brand/app-icon/export/lagom-512.png"));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // Called on the main thread after GPUI has created NSApplication.
        unsafe { NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image)) };
    }
}

/// The Dock icon shows the unread count of the unified Inbox, as the Mail row does; none at zero.
pub fn set_badge(count: usize) {
    static SHOWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let Some(main_thread) = MainThreadMarker::new() else { return };
    if SHOWN.swap(count, std::sync::atomic::Ordering::Relaxed) == count { return; }
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(main_thread).dockTile().setBadgeLabel(label.as_deref());
}

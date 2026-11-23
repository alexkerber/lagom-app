use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSImage};
use objc2_foundation::{NSBundle, NSData, NSString};

/// The standard About panel: the icon set by `install`, the name, and the bundle's version.
pub fn about() {
    let Some(main_thread) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(main_thread).orderFrontStandardAboutPanel(None);
}

/// Gives `cargo run` an icon. A bundled lagom keeps its own, so macOS can show the dark and tinted ones.
pub fn install() {
    let Some(main_thread) = MainThreadMarker::new() else { return };
    if NSBundle::mainBundle().bundleIdentifier().is_some() { return }
    let data = NSData::with_bytes(include_bytes!("../../../brand/app-icon/export/lagom-512.png"));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // Called on the main thread after GPUI has created NSApplication.
        unsafe { NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image)) };
    }
}

/// The Dock icon shows the unread count of the unified Inbox, as the Mail row does; none at zero.
pub fn set_badge(count: usize) {
    // Set on every count read, not only on change: a label set while the app still launches was
    // measured not to show (0.1.17, 307 unread, no badge).
    let Some(main_thread) = MainThreadMarker::new() else { return };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(main_thread).dockTile().setBadgeLabel(label.as_deref());
}

/// `LAGOM_THEME=light|dark` also sets the window material (sidebar), so captures match a Mac in that mode.
pub fn force_appearance() {
    let Some(main_thread) = MainThreadMarker::new() else { return };
    let name = match std::env::var("LAGOM_THEME").as_deref() {
        Ok("light") => unsafe { NSAppearanceNameAqua },
        Ok("dark") => unsafe { NSAppearanceNameDarkAqua },
        _ => return,
    };
    NSApplication::sharedApplication(main_thread).setAppearance(NSAppearance::appearanceNamed(name).as_deref());
}

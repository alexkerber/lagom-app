//! Liquid Glass behind the sidebar on macOS 26 and later.
//!
//! GPUI draws everything in one Metal layer, so the glass is an AppKit `NSGlassEffectView`
//! placed under GPUI's view. It only shows where GPUI paints translucent pixels.
//! Older macOS and other platforms keep GPUI's window blur.

use gpui::Window;

/// Puts the glass under the sidebar. Returns true when the glass is in place.
#[cfg(target_os = "macos")]
pub fn install(window: &Window, sidebar_width: f64) -> bool {
    use objc2::{runtime::AnyClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSGlassEffectView, NSView, NSWindowOrderingMode,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Runtime check: the class exists from macOS 26. objc2 would panic on a missing class.
    if AnyClass::get(c"NSGlassEffectView").is_none() {
        return false;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    // gpui::Window has its own `window_handle()`, so name the raw-window-handle trait.
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return false;
    };
    // SAFETY: GPUI 0.2.2 returns its live content NSView, owned by the window we borrow.
    let gpui_view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    // GPUI's view sits inside the window's contentView, above GPUI's own blur view.
    let Some(content) = (unsafe { gpui_view.superview() }) else {
        return false;
    };

    let frame = NSRect::new(
        NSPoint::new(0., 0.),
        NSSize::new(sidebar_width, content.bounds().size.height),
    );
    let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), frame);
    glass.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewHeightSizable | NSAutoresizingMaskOptions::ViewMaxXMargin,
    );
    // The glass follows the window's appearance. LAGOM_THEME overrides only GPUI's theme,
    // so mirror that override here or a light sidebar gets dark glass.
    let forced = match std::env::var("LAGOM_THEME").as_deref() {
        Ok("light") => Some(unsafe { NSAppearanceNameAqua }),
        Ok("dark") => Some(unsafe { NSAppearanceNameDarkAqua }),
        _ => None,
    };
    if let Some(name) = forced {
        glass.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }
    content.addSubview_positioned_relativeTo(&glass, NSWindowOrderingMode::Below, Some(gpui_view));
    true
}

#[cfg(not(target_os = "macos"))]
pub fn install(_window: &Window, _sidebar_width: f64) -> bool {
    false
}

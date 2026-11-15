//! The app icon shows the unread count of the unified Inbox, as the sidebar Mail row does; none at
//! zero. macOS: the Dock badge. Windows: a taskbar overlay icon. Linux: the Unity LauncherEntry
//! count, which GNOME's Dash to Dock, KDE Plasma and Plank read.
// Only Windows draws the badge itself; tests call no platform code.
#![cfg_attr(any(test, not(target_os = "windows")), allow(dead_code))]

#[cfg(all(target_os = "windows", not(test)))]
pub use taskbar::attach;

pub fn set(count: usize) {
    #[cfg(target_os = "macos")]
    crate::icon::set_badge(count);
    #[cfg(target_os = "windows")]
    taskbar::set(count);
    #[cfg(target_os = "linux")]
    launcher::set(count);
}

/// The text in the badge: none at zero, three characters at most.
fn label(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".into()),
    }
}

/// 3×5 glyphs for the digits and '+', three bits a row from the top, the left pixel high.
const GLYPHS: [u16; 11] = [
    0b111_101_101_101_111, 0b010_110_010_010_111, 0b111_001_111_100_111, 0b111_001_111_001_111,
    0b101_101_111_001_001, 0b111_100_111_001_111, 0b111_100_111_101_111, 0b111_001_001_001_001,
    0b111_101_111_101_111, 0b111_101_111_001_111, 0b000_010_111_010_000,
];

fn glyph(c: u8) -> u16 {
    if c.is_ascii_digit() { GLYPHS[(c - b'0') as usize] } else { GLYPHS[10] }
}

/// A `size`×`size` accent disc with `text` in white: BGRA rows from the top, straight alpha,
/// black where transparent.
/// shortcut: the default light accent, not the accent chosen in Settings; pass it in when the badge must follow it.
fn pixels(text: &str, size: usize) -> Vec<u8> {
    let accent = crate::theme::Theme::light(crate::theme::DEFAULT_ACCENT).accent;
    let fill = [accent.b, accent.g, accent.r].map(|c| (c * 255.).round() as u8);
    let text = text.as_bytes();
    // A glyph is 3 pixels wide with 1 pixel between glyphs; whole-pixel scale keeps them sharp.
    let width = text.len() * 4 - 1;
    let radius = size as f32 / 2.;
    // The text box, with a pixel to spare, stays inside the opaque part of the disc.
    let fits = |s: usize| width * s <= size * 4 / 5 && 5 * s <= size * 7 / 10
        && ((width * s) as f32 / 2. + 1.).hypot((5 * s) as f32 / 2. + 1.) <= radius;
    let scale = (2..=size).take_while(|&s| fits(s)).last().unwrap_or(1);
    let (left, top) = (size.saturating_sub(width * scale) / 2, size.saturating_sub(5 * scale) / 2);
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let distance = (x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius);
            let alpha = ((radius - distance + 0.5).clamp(0., 1.) * 255.).round() as u8;
            let ink = x >= left && y >= top && {
                let (gx, gy) = ((x - left) / scale, (y - top) / scale);
                gx < width && gy < 5 && gx % 4 != 3 && glyph(text[gx / 4]) >> (14 - gy * 3 - gx % 4) & 1 == 1
            };
            let [b, g, r] = if ink { [255; 3] } else if alpha == 0 { [0; 3] } else { fill };
            out.extend([b, g, r, alpha]);
        }
    }
    out
}

#[cfg(target_os = "windows")]
mod taskbar {
    use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicUsize, Ordering::Relaxed};
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, GetSystemMetrics, RegisterWindowMessageW, HICON, ICONINFO, SM_CXSMICON, WM_NCDESTROY,
    };

    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    static BUTTON_CREATED: AtomicU32 = AtomicU32::new(0);

    /// The main window's taskbar button carries the overlay. Called on the UI thread for each new main window.
    pub fn attach(window: &gpui::Window) {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
        WINDOW.store(handle.hwnd.get(), Relaxed);
        // Explorer drops overlays when it recreates the button (first show, Explorer restart) and
        // then sends TaskbarButtonCreated; set the overlay again on it.
        unsafe {
            BUTTON_CREATED.store(RegisterWindowMessageW(w!("TaskbarButtonCreated")), Relaxed);
            let _ = SetWindowSubclass(HWND(handle.hwnd.get() as _), Some(button_created), 1, 0);
        }
        apply();
    }

    pub fn set(count: usize) {
        if COUNT.swap(count, Relaxed) != count { apply(); }
    }

    unsafe extern "system" fn button_created(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == BUTTON_CREATED.load(Relaxed) { apply(); }
        if msg == WM_NCDESTROY {
            // A closed main window: forget its handle, which Windows may give to another window.
            let _ = WINDOW.compare_exchange(hwnd.0 as isize, 0, Relaxed, Relaxed);
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(button_created), 1) };
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    fn apply() {
        let hwnd = HWND(WINDOW.load(Relaxed) as _);
        if hwnd.is_invalid() { return; }
        let count = COUNT.load(Relaxed);
        unsafe {
            // GPUI initialises OLE on this thread.
            let Ok(taskbar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else { return };
            if taskbar.HrInit().is_err() { return; }
            let Some(text) = super::label(count) else {
                let _ = taskbar.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null());
                return;
            };
            let Some(icon) = icon(&text, GetSystemMetrics(SM_CXSMICON).max(16) as usize) else { return };
            let _ = taskbar.SetOverlayIcon(hwnd, icon, &HSTRING::from(format!("{count} unread")));
            // The taskbar keeps its own copy.
            let _ = DestroyIcon(icon);
        }
    }

    fn icon(text: &str, size: usize) -> Option<HICON> {
        let pixels = super::pixels(text, size);
        // Set where transparent, for drawing that reads the mask; monochrome rows are 16-bit aligned.
        let stride = size.div_ceil(16) * 2;
        let mut mask = vec![0u8; stride * size];
        for (i, pixel) in pixels.chunks(4).enumerate() {
            if pixel[3] == 0 { mask[i / size * stride + i % size / 8] |= 0x80 >> (i % size % 8); }
        }
        let side = size as i32;
        unsafe {
            let color = CreateBitmap(side, side, 1, 32, Some(pixels.as_ptr().cast()));
            let mask = CreateBitmap(side, side, 1, 1, Some(mask.as_ptr().cast()));
            let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() });
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon.ok()
        }
    }
}

#[cfg(target_os = "linux")]
mod launcher {
    use std::collections::HashMap;
    use std::sync::{mpsc, OnceLock};
    use zbus::zvariant::Value as Variant;

    /// The installed desktop file, packaging/linux/app.lagommail.desktop.desktop.
    const APP_URI: &str = "application://app.lagommail.desktop.desktop";
    const PATH: &str = "/app/lagommail/desktop/launcher";

    /// Sends on a thread of its own: D-Bus never blocks the UI.
    /// shortcut: no LauncherEntry.Query, so a dock started after lagom shows the count from the next change; serve Query if that matters.
    pub fn set(count: usize) {
        static UPDATES: OnceLock<mpsc::Sender<usize>> = OnceLock::new();
        let updates = UPDATES.get_or_init(|| {
            let (updates, counts) = mpsc::channel::<usize>();
            let _ = std::thread::Builder::new().name("lagom-badge".into()).spawn(move || {
                // Docks drop the count when its connection closes, so this one lives as long as lagom.
                let Ok(bus) = zbus::block_on(zbus::Connection::session()) else { return };
                let mut shown = 0;
                for count in counts {
                    if count == shown { continue; }
                    shown = count;
                    let _ = zbus::block_on(bus.emit_signal(None::<&str>, PATH, "com.canonical.Unity.LauncherEntry", "Update",
                        &(APP_URI, properties(count))));
                }
            });
            updates
        });
        let _ = updates.send(count);
    }

    fn properties(count: usize) -> HashMap<&'static str, Variant<'static>> {
        HashMap::from([("count", Variant::from(count as i64)), ("count-visible", Variant::from(count > 0))])
    }

    #[cfg(test)]
    #[test]
    fn the_count_is_an_int64_and_hidden_at_zero() {
        let shown = properties(3);
        assert_eq!((shown.len(), &shown["count"], &shown["count-visible"]), (2, &Variant::from(3i64), &Variant::from(true)));
        assert_eq!(properties(0)["count-visible"], Variant::from(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_is_empty_at_zero_and_stops_at_99_plus() {
        assert_eq!([0, 1, 99, 100, 12_345].map(label), [None, Some("1".into()), Some("99".into()), Some("99+".into()), Some("99+".into())]);
    }

    #[test]
    fn the_badge_is_a_disc_with_white_text_inside_it() {
        for (text, size) in ["1", "8", "20", "99", "99+"].into_iter().flat_map(|t| (16..=64).map(move |s| (t, s))) {
            let px = pixels(text, size);
            assert_eq!(px.len(), size * size * 4, "{text} {size}");
            let at = |x: usize, y: usize| &px[(y * size + x) * 4..][..4];
            assert_eq!(at(0, 0)[3], 0, "corners are transparent");
            assert!(at(size / 2, 0)[3] > 200, "the disc reaches the edge");
            let white: Vec<_> = px.chunks(4).filter(|p| p[..3] == [255; 3]).collect();
            assert!(!white.is_empty() && white.iter().all(|p| p[3] == 255), "{text} {size}: text sits inside the disc");
        }
        // At 16 px one digit is drawn at twice the scale of three; "8" lights 13 glyph pixels.
        let ink = |text| pixels(text, 16).chunks(4).filter(|p| p[..3] == [255; 3]).count();
        assert_eq!((ink("8"), ink("888")), (13 * 4, 13 * 3));
    }
}

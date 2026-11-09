//! Design tokens for Concept D "Butik". Source: Claude design canvas, ConceptD and MainWindowD.
// The full token set is defined here even where no screen uses a token yet.
#![allow(dead_code)]

use gpui::{rgb, Rgba, WindowAppearance};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
/// The Dusk profile, from the app icon: Swedish blue leads, gold marks Later.
pub enum Accent {
    Dusk,
    PineGreen,
    Gold,
}

impl Accent {
    /// (light, dark)
    fn hex(self) -> (u32, u32) {
        match self {
            Accent::Dusk => (0x2A5BB8, 0x8AB0F5),
            Accent::PineGreen => (0x1E6B4E, 0x4CC38A),
            Accent::Gold => (0x94640A, 0xF5C22E),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub bg: Rgba,
    pub subtle: Rgba,
    pub raised: Rgba,
    pub ink: Rgba,
    pub ink2: Rgba,
    pub line: Rgba,
    /// Selection when the list does not have the keys, or the window is inactive: neutral grey, ink text.
    pub sel: Rgba,
    /// Selection in the focused list of the active window: the accent as a fill, white text on it.
    /// Darker than the accent itself where needed, so white text keeps 4.5:1 or more.
    pub sel_focus: Rgba,
    /// The active sidebar row's icon: the selection colour, lifted halfway to white in dark mode
    /// so it keeps 3:1 or more on the grey sidebar selection.
    pub sel_icon: Rgba,
    pub accent: Rgba,
    pub accent_ink: Rgba,
    // Behind a swiped row, by the move it commits to. `accent_ink` sits on all three:
    // 5.1–7.0:1 in light (white), 6.0–10:1 in dark (near black).
    pub swipe_done: Rgba,
    pub swipe_later: Rgba,
    /// Delete. The only red in the app.
    pub danger: Rgba,
    /// While pressed. Translucent ink, so one token works on every surface. Native controls show
    /// no hover state, so there is no hover token.
    pub press: Rgba,
}

impl Theme {
    pub fn light(accent: Accent) -> Self {
        Self {
            dark: false,
            bg: rgb(0xFFFFFF),
            subtle: rgb(0xF5F5F3),
            raised: rgb(0xFFFFFF),
            ink: rgb(0x111110),
            ink2: rgb(0x5F5F5A),
            line: rgb(0xE6E6E2),
            sel: rgb(0xDCDCDC),
            sel_focus: rgb(accent.selection()),
            sel_icon: rgb(accent.selection()),
            accent: rgb(accent.hex().0),
            accent_ink: rgb(0xFFFFFF),
            swipe_done: rgb(Accent::PineGreen.hex().0),
            swipe_later: rgb(Accent::Gold.hex().0),
            danger: rgb(0xC62828),
            press: Rgba { a: 0.12, ..rgb(0x111110) },
        }
    }

    pub fn dark(accent: Accent) -> Self {
        Self {
            dark: true,
            // The system dark appearance: a mid-dark window, sheets and fields a step lighter.
            bg: rgb(0x1E1E1E),
            subtle: rgb(0x262626),
            raised: rgb(0x2C2C2C),
            ink: rgb(0xF2F2EE),
            ink2: rgb(0x9B9B95),
            line: rgb(0x3A3A3A),
            sel: rgb(0x464646),
            sel_focus: rgb(accent.selection()),
            sel_icon: rgb(accent.selection()),
            accent: rgb(accent.hex().1),
            accent_ink: rgb(0x1E1E1E),
            swipe_done: rgb(Accent::PineGreen.hex().1),
            swipe_later: rgb(Accent::Gold.hex().1),
            danger: rgb(0xF87171),
            press: Rgba { a: 0.14, ..rgb(0xF2F2EE) },
        }
    }

    /// `LAGOM_THEME=light|dark` overrides the system appearance.
    pub fn for_appearance(appearance: WindowAppearance, accent: Accent) -> Self {
        let dark = match std::env::var("LAGOM_THEME").as_deref() {
            Ok("dark") => true,
            Ok("light") => false,
            _ => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        let mut t = if dark { Self::dark(accent) } else { Self::light(accent) };
        if let Some(fill) = system_selection() {
            t.sel_focus = rgb(fill);
        }
        t.sel_icon = if dark { lift(t.sel_focus) } else { t.sel_focus };
        t
    }
}

/// Halfway to white.
fn lift(c: Rgba) -> Rgba {
    Rgba { r: (c.r + 1.) / 2., g: (c.g + 1.) / 2., b: (c.b + 1.) / 2., a: c.a }
}

impl Accent {
    /// The focused-selection fill for this accent, the same in light and dark.
    fn selection(self) -> u32 {
        self.hex().0
    }
}

/// The selection fill for the user's macOS accent colour, as AppKit lists use it. `None` for
/// Multicolor (no `AppleAccentColor` key), where the app's own accent leads, and off macOS.
/// Read once: the value is cached for the life of the process.
fn system_selection() -> Option<u32> {
    static FILL: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *FILL.get_or_init(|| {
        // Demo mode shows the app's own accent, so screenshots look the same on every Mac.
        if std::env::var_os("LAGOM_DEMO").is_some_and(|v| v == "1") {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            // shortcut: a change in System Settings shows after a relaunch; observe
            // NSSystemColorsDidChangeNotification to follow it live.
            let out = std::process::Command::new("defaults").args(["read", "-g", "AppleAccentColor"]).output().ok()?;
            let value = String::from_utf8_lossy(&out.stdout).trim().parse::<i32>().ok()?;
            out.status.success().then(|| accent_fill(value)).flatten()
        }
        #[cfg(not(target_os = "macos"))]
        None
    })
}

/// `AppleAccentColor` value to a fill that keeps white text at 4.5:1 or more. AppKit's accents
/// (orange, yellow, green) are too light for white text, so these are their darker variants.
fn accent_fill(value: i32) -> Option<u32> {
    Some(match value {
        -1 => 0x6E6E73, // graphite
        0 => 0xD70015,  // red
        1 => 0xC93400,  // orange
        2 => 0xA05A00,  // yellow
        3 => 0x1F7A36,  // green
        4 => 0x0064E1,  // blue
        5 => 0x8944AB,  // purple
        6 => 0xD30F45,  // pink
        _ => return None,
    })
}

pub const DEFAULT_ACCENT: Accent = Accent::Dusk;

pub mod font {
    pub const MONO: &str = "Geist Mono"; // key caps in the command palette

    /// All text: the platform's system font (SF Pro on macOS), so the app reads as native.
    /// GPUI maps ".SystemUIFont" per platform.
    /// `LAGOM_UI_FONT=inter` switches back to Inter for comparison.
    pub fn ui() -> &'static str {
        static UI: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        UI.get_or_init(|| match std::env::var("LAGOM_UI_FONT").as_deref() {
            Ok("inter") => "Inter",
            _ => ".SystemUIFont",
        })
    }
}

/// Type scale in pt, after the macOS text styles: body 13, callout 12, subheadline 11.
pub mod size {
    pub const MONO: f32 = 11.0;
    pub const SMALL: f32 = 11.0;
    pub const SNIPPET: f32 = 12.0;
    pub const BASE: f32 = 13.0;
    /// Message text in the reading pane and the compose body.
    pub const BODY: f32 = 14.0;
    pub const TITLE: f32 = 15.0;
    pub const SUBJECT: f32 = 20.0;
    pub const DISPLAY: f32 = 26.0;
}

/// Line height as a multiple of the font size. GPUI's default is 1.618, looser than AppKit text.
pub const LEADING: f32 = 1.25;
/// Running text (message bodies) reads better a little looser.
pub const READING_LEADING: f32 = 1.4;

/// Corner radii in px. Buttons use `CONTROL`, not pills.
pub mod radius {
    pub const KEY: f32 = 4.0;
    pub const CONTROL: f32 = 8.0;
    pub const ATTACHMENT: f32 = 10.0;
    pub const PANEL: f32 = 12.0;
    pub const CARD: f32 = 16.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_match_the_dusk_profile() {
        assert_eq!(Accent::Dusk.hex(), (0x2A5BB8, 0x8AB0F5));
        assert_eq!(Theme::dark(DEFAULT_ACCENT).accent, rgb(0x8AB0F5));
        assert_eq!(Theme::light(DEFAULT_ACCENT).accent, rgb(0x2A5BB8));
        assert!(contrast(0x111110, Accent::Gold.hex().1) >= 4.5 && contrast(0xFFFFFF, Accent::Gold.hex().0) >= 4.5);
    }

    fn luminance(c: u32) -> f32 {
        let ch = |v: u32| {
            let v = v as f32 / 255.;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c >> 16 & 0xFF) + 0.7152 * ch(c >> 8 & 0xFF) + 0.0722 * ch(c & 0xFF)
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn white_text_reads_on_every_selection_fill() {
        let fills = (-1..=6).filter_map(accent_fill).chain([Accent::Dusk, Accent::PineGreen, Accent::Gold].map(Accent::selection));
        for fill in fills {
            assert!(contrast(0xFFFFFF, fill) >= 4.5, "white on {fill:06X}: {}", contrast(0xFFFFFF, fill));
        }
        assert_eq!(accent_fill(7), None);
    }

    #[test]
    fn dark_sidebar_icon_reads_on_the_grey_selection() {
        let hex = |c: Rgba| ((c.r * 255.).round() as u32) << 16 | ((c.g * 255.).round() as u32) << 8 | (c.b * 255.).round() as u32;
        for fill in (-1..=6).filter_map(accent_fill).chain([DEFAULT_ACCENT.selection()]) {
            let icon = hex(lift(rgb(fill)));
            assert!(contrast(icon, 0x464646) >= 3.0, "{icon:06X} on the dark selection: {}", contrast(icon, 0x464646));
        }
    }

    #[test]
    fn ink_reads_on_the_inactive_selection() {
        assert!(contrast(0x111110, 0xDCDCDC) >= 4.5);
        assert!(contrast(0xF2F2EE, 0x464646) >= 4.5);
        assert!(contrast(0x9B9B95, 0x1E1E1E) >= 4.5, "secondary text on the dark window");
    }
}

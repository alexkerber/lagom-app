//! Design tokens for Concept D "Butik". Source: Claude design canvas, ConceptD and MainWindowD.
// The full token set is defined here even where no screen uses a token yet.
#![allow(dead_code)]

use gpui::{rgb, Rgba, WindowAppearance};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accent {
    FaluRed,
    PineGreen,
    CatalogueBlue,
}

impl Accent {
    /// (light, dark)
    fn hex(self) -> (u32, u32) {
        match self {
            Accent::FaluRed => (0xB03A2E, 0xEB6A55),
            Accent::PineGreen => (0x1E6B4E, 0x4CC38A),
            Accent::CatalogueBlue => (0x2F56A8, 0x86A8F0),
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
    pub sel: Rgba,
    pub accent: Rgba,
    pub accent_ink: Rgba,
    // Behind a swiped row, by the move it commits to. `accent_ink` sits on all three:
    // 5.6–7.0:1 in light (white), 7.1–8.9:1 in dark (near black).
    pub swipe_done: Rgba,
    pub swipe_later: Rgba,
    /// Delete. A purer red than Falu red, so Delete never reads as the accent.
    pub danger: Rgba,
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
            sel: rgb(0xEFEFEC),
            accent: rgb(accent.hex().0),
            accent_ink: rgb(0xFFFFFF),
            swipe_done: rgb(Accent::PineGreen.hex().0),
            swipe_later: rgb(Accent::CatalogueBlue.hex().0),
            danger: rgb(0xC62828),
        }
    }

    pub fn dark(accent: Accent) -> Self {
        Self {
            dark: true,
            bg: rgb(0x0B0B0A),
            subtle: rgb(0x0B0B0A),
            raised: rgb(0x151514),
            ink: rgb(0xF2F2EE),
            ink2: rgb(0x9B9B95),
            line: rgb(0x262624),
            sel: rgb(0x1F1F1D),
            accent: rgb(accent.hex().1),
            accent_ink: rgb(0x0B0B0A),
            swipe_done: rgb(Accent::PineGreen.hex().1),
            swipe_later: rgb(Accent::CatalogueBlue.hex().1),
            danger: rgb(0xF87171),
        }
    }

    /// `LAGOM_THEME=light|dark` overrides the system appearance.
    pub fn for_appearance(appearance: WindowAppearance, accent: Accent) -> Self {
        let dark = match std::env::var("LAGOM_THEME").as_deref() {
            Ok("dark") => true,
            Ok("light") => false,
            _ => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        if dark { Self::dark(accent) } else { Self::light(accent) }
    }
}

pub const DEFAULT_ACCENT: Accent = Accent::FaluRed;

pub mod font {
    pub const DISPLAY: &str = "Inter Tight"; // 700–800, bundled
    pub const UI: &str = "Inter"; // 400–600
    pub const MONO: &str = "Geist Mono"; // keys and counts
}

/// Type scale in px, as used in ConceptD and MainWindowD.
pub mod size {
    pub const MONO: f32 = 11.0;
    pub const SMALL: f32 = 12.0;
    pub const SNIPPET: f32 = 13.0;
    pub const BASE: f32 = 14.0;
    pub const BODY: f32 = 16.0;
    pub const TITLE: f32 = 18.0;
    pub const SUBJECT: f32 = 30.0;
    pub const DISPLAY: f32 = 34.0;
}

/// Corner radii in px. Buttons use `CONTROL`, not pills (Alex 2026-11-06).
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
    fn accents_match_concept_d() {
        assert_eq!(Accent::FaluRed.hex(), (0xB03A2E, 0xEB6A55));
        assert_eq!(Theme::dark(DEFAULT_ACCENT).accent, rgb(0xEB6A55));
        assert_eq!(Theme::light(DEFAULT_ACCENT).accent, rgb(0xB03A2E));
    }
}

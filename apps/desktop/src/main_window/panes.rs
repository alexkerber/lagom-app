use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

pub(super) const SIDEBAR_DEFAULT: f32 = 220.;
pub(super) const SIDEBAR_MIN: f32 = 180.;
const SIDEBAR_MAX: f32 = 320.;
const COLLAPSE_AT: f32 = 120.;
pub(super) const LIST_MIN: f32 = 280.;
const LIST_MAX: f32 = 560.;
pub(super) const READING_MIN: f32 = 440.;
// The panel's right margin and its two borders.
pub(super) const PANEL_INSET: f32 = 10.;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(super) enum StartPage {
    Home,
    Calendar,
    Todo,
    #[default]
    #[serde(other)]
    Mail,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct Panes {
    pub sidebar: f32,
    pub list: f32,
    pub sidebar_hidden: bool,
    #[serde(default = "group_default")]
    pub group_conversations: bool,
    #[serde(default = "notifications_default")]
    pub notifications: bool,
    #[serde(default)]
    pub start_page: StartPage,
}

fn group_default() -> bool { true }
fn notifications_default() -> bool { true }

impl Panes {
    pub fn new(viewport: f32) -> Self {
        Self {
            sidebar: SIDEBAR_DEFAULT,
            list: default_list(viewport, SIDEBAR_DEFAULT),
            sidebar_hidden: false,
            group_conversations: true,
            notifications: true,
            start_page: StartPage::Mail,
        }
    }

    /// Keep the preferred widths on window resize, giving spare space to the reading pane.
    pub fn fit(self, viewport: f32) -> (f32, f32) {
        let available = viewport - PANEL_INSET - READING_MIN;
        // At the minimum window size even the minimum list may leave less than the preferred
        // sidebar width. Temporarily narrow the sidebar too, restoring it when space returns.
        let sidebar = if self.sidebar_hidden {
            0.
        } else {
            bounded(self.sidebar, SIDEBAR_DEFAULT, SIDEBAR_MIN, SIDEBAR_MAX)
                .min((available - LIST_MIN).max(SIDEBAR_MIN))
        };
        let list =
            bounded(self.list, 420., LIST_MIN, LIST_MAX).min((available - sidebar).max(LIST_MIN));
        (sidebar, list)
    }

    pub fn resize_sidebar(&mut self, width: f32, viewport: f32) {
        let (_, list) = self.fit(viewport);
        self.sidebar_hidden = width < COLLAPSE_AT;
        if !self.sidebar_hidden {
            self.sidebar = width
                .clamp(SIDEBAR_MIN, SIDEBAR_MAX)
                .min((viewport - PANEL_INSET - READING_MIN - LIST_MIN).max(SIDEBAR_MIN));
        }
        self.list = list.min((viewport - PANEL_INSET - READING_MIN - self.fit(viewport).0).max(LIST_MIN));
    }

    pub fn resize_list(&mut self, width: f32, viewport: f32) {
        self.list = width
            .clamp(LIST_MIN, LIST_MAX)
            .min((viewport - PANEL_INSET - READING_MIN - self.fit(viewport).0).max(LIST_MIN));
    }

    pub fn load(path: &Path, viewport: f32) -> anyhow::Result<Self> {
        let bytes = match retry(|| fs::read(path)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new(viewport)),
            Err(e) => return Err(e.into()),
        };
        let mut panes: Self = serde_json::from_slice(&bytes)?;
        panes.sidebar = bounded(panes.sidebar, SIDEBAR_DEFAULT, SIDEBAR_MIN, SIDEBAR_MAX);
        panes.list = bounded(panes.list, 420., LIST_MIN, LIST_MAX);
        Ok(panes)
    }

    pub fn save(self, path: &Path) -> anyhow::Result<()> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let bytes = serde_json::to_vec(&self)?;
        let (temp, mut file) = loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let temp = path.with_extension(format!("json.{}.{id}.tmp", std::process::id()));
            match fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(file) => break (temp, file),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        };
        let result = file.write_all(&bytes).and_then(|_| {
            drop(file);
            retry(|| fs::rename(&temp, path))
        });
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.map_err(Into::into)
    }
}

/// Windows denies opening or replacing a file while another handle replaces it or holds it (antivirus),
/// so retry briefly: error 5 is access denied, 32 a sharing violation.
fn retry<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = 0;
    loop {
        match op() {
            Err(e) if cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32)) && tries < 50 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            result => return result,
        }
    }
}

fn bounded(value: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

pub(super) fn default_list(viewport: f32, sidebar: f32) -> f32 {
    (0.36 * (viewport - sidebar - PANEL_INSET)).clamp(300., 420.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_window_resizing_keeps_valid_pane_preferences() {
        for width in [720., 800., 899.] {
            let mut panes = Panes::new(width);
            panes.resize_sidebar(220., width);
            panes.resize_list(320., width);
            let (sidebar, list) = panes.fit(width);
            assert!(panes.sidebar >= SIDEBAR_MIN && panes.list >= LIST_MIN);
            assert!(sidebar + list + PANEL_INSET < width);
        }
    }

    #[test]
    fn old_preferences_group_by_default_and_off_roundtrips() {
        let mut panes: Panes = serde_json::from_str(r#"{"sidebar":220,"list":420,"sidebar_hidden":false}"#).unwrap();
        assert!(panes.group_conversations);
        panes.group_conversations = false;
        let restored: Panes = serde_json::from_str(&serde_json::to_string(&panes).unwrap()).unwrap();
        assert!(!restored.group_conversations);
    }

    #[test]
    fn preferences_from_either_branch_preserve_disabled_features() {
        for (json, group, notifications) in [
            (r#"{"sidebar":220,"list":420,"sidebar_hidden":false,"group_conversations":false}"#, false, true),
            (r#"{"sidebar":220,"list":420,"sidebar_hidden":false,"notifications":false}"#, true, false),
        ] {
            let panes: Panes = serde_json::from_str(json).unwrap();
            assert_eq!(panes.group_conversations, group);
            assert_eq!(panes.notifications, notifications);
            let restored: Panes = serde_json::from_str(&serde_json::to_string(&panes).unwrap()).unwrap();
            assert_eq!(restored, panes);
        }
    }

    #[test]
    fn defaults_match_the_original_layout() {
        assert_eq!(Panes::new(1440.).fit(1440.), (220., 420.));
        assert_eq!(Panes::new(970.).fit(970.), (220., 300.));
    }

    #[test]
    fn window_resize_shrinks_list_first_and_restores_preferences() {
        let panes = Panes {
            sidebar: 320.,
            list: 560.,
            sidebar_hidden: false,
            group_conversations: true,
            notifications: true,
            start_page: StartPage::Mail,
        };
        assert_eq!(panes.fit(1440.), (320., 560.));
        assert_eq!(panes.fit(1100.), (320., 330.));
        assert_eq!(panes.fit(1050.), (320., 280.));
        assert_eq!(panes.fit(970.), (240., 280.));
        assert_eq!(panes.fit(1440.), (320., 560.));
    }

    #[test]
    fn sidebar_drag_shrinks_list_then_stops_at_reading_minimum() {
        let mut panes = Panes::new(970.);
        panes.resize_sidebar(320., 970.);
        assert_eq!(panes.fit(970.), (240., 280.));
        panes.resize_sidebar(140., 970.);
        assert_eq!(panes.fit(970.), (180., 280.));
        panes.resize_sidebar(1000., 1440.);
        assert_eq!(panes.sidebar, 320.);
    }

    #[test]
    fn list_drag_clamps_to_limits_and_remaining_space() {
        let mut panes = Panes::new(1440.);
        panes.resize_list(1000., 1440.);
        assert_eq!(panes.list, 560.);
        panes.resize_list(100., 1440.);
        assert_eq!(panes.list, 280.);
        panes.resize_list(560., 970.);
        assert_eq!(panes.list, 300.);
    }

    #[test]
    fn collapse_and_show_preserve_sidebar_width() {
        let mut panes = Panes::new(1440.);
        panes.resize_sidebar(119., 1440.);
        assert_eq!(panes.fit(1440.), (0., 420.));
        assert_eq!(panes.sidebar, 220.);
        panes.sidebar_hidden = false;
        assert_eq!(panes.fit(1440.), (220., 420.));
        panes.resize_sidebar(120., 1440.);
        assert_eq!(panes.fit(1440.), (180., 420.));
    }

    #[test]
    fn supported_window_sizes_always_leave_reading_minimum() {
        for viewport in [970., 1000., 1100., 1440., 2000.] {
            for sidebar in [180., 220., 320., f32::NAN, f32::INFINITY] {
                for list in [280., 420., 560., f32::NAN, -100.] {
                    for sidebar_hidden in [false, true] {
                        let panes = Panes {
                            sidebar,
                            list,
                            sidebar_hidden,
                            group_conversations: true,
                            notifications: true,
                            start_page: StartPage::Mail,
                        };
                        let (sidebar, list) = panes.fit(viewport);
                        assert!(sidebar == 0. || (180. ..=320.).contains(&sidebar));
                        assert!((280. ..=560.).contains(&list));
                        assert!(viewport - PANEL_INSET - sidebar - list >= READING_MIN);
                    }
                }
            }
        }
    }

    #[test]
    fn concurrent_saves_use_private_temp_files() {
        let dir = std::env::temp_dir().join(format!("lagom-panes-writers-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window.json");
        let shared_temp = path.with_extension("json.tmp");
        fs::write(&shared_temp, "another writer").unwrap();
        std::thread::scope(|scope| {
            for i in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    let panes = Panes { sidebar: 180. + i as f32, list: 560., sidebar_hidden: i % 2 == 0, group_conversations: true, notifications: true, start_page: StartPage::Mail };
                    for _ in 0..100 {
                        panes.save(path).unwrap();
                        let saved = Panes::load(path, 1440.).unwrap();
                        assert!((180. ..=187.).contains(&saved.sidebar));
                        assert_eq!(saved.list, 560.);
                        assert_eq!(saved.sidebar_hidden, (saved.sidebar as u32 - 180).is_multiple_of(2));
                    }
                });
            }
        });
        assert_eq!(fs::read_to_string(&shared_temp).unwrap(), "another writer");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_file(path).unwrap();
        fs::remove_file(shared_temp).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn preferences_roundtrip_and_reject_invalid_json() {
        let path = std::env::temp_dir().join(format!("lagom-panes-{}.json", std::process::id()));
        let mut panes = Panes::new(1440.);
        panes.resize_sidebar(300., 1440.);
        panes.resize_list(500., 1440.);
        panes.sidebar_hidden = true;
        panes.notifications = false;
        panes.save(&path).unwrap();
        assert_eq!(Panes::load(&path, 970.).unwrap(), panes);
        fs::write(&path, r#"{"sidebar":900,"list":-5,"sidebar_hidden":false}"#).unwrap();
        assert!(Panes::load(&path, 1440.).unwrap().notifications);
        assert_eq!(Panes::load(&path, 1440.).unwrap().fit(1440.), (320., 280.));
        fs::write(&path, "broken").unwrap();
        assert!(Panes::load(&path, 1440.).is_err());
        fs::remove_file(&path).unwrap();
        assert_eq!(Panes::load(&path, 1440.).unwrap(), Panes::new(1440.));
    }
}

use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

#[derive(Default)]
pub(super) struct Writes {
    pending: std::collections::HashMap<std::path::PathBuf, (Panes, usize)>,
    pub tail: Option<gpui::Task<()>>,
}

impl gpui::Global for Writes {}

impl Writes {
    pub fn load(path: &Path, viewport: f32, cx: &gpui::App) -> anyhow::Result<Panes> {
        // Reopening and notifications must see changes still waiting for disk.
        match cx.try_global::<Self>().and_then(|writes| writes.pending.get(path)) {
            Some((panes, _)) => Ok(*panes),
            None => Panes::load(path, viewport),
        }
    }

    pub fn save(panes: Panes, path: std::path::PathBuf, cx: &mut gpui::Context<super::MainWindow>) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
            gpui::App::on_app_quit(cx, |cx| {
                let tail = cx.global_mut::<Self>().tail.take();
                // shortcut: GPUI allows 100 ms at quit, extend shutdown handling if slow storage exceeds it.
                async move { if let Some(tail) = tail { tail.await; } }
            }).detach();
        }
        let writes = cx.global_mut::<Self>();
        let pending = writes.pending.entry(path.clone()).or_insert((panes, 0));
        *pending = (panes, pending.1 + 1);
        let previous = writes.tail.take();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let target = path.clone();
        let tail = cx.background_executor().spawn(async move {
            if let Some(previous) = previous { previous.await; }
            let result = panes.save(&target).map_err(|e| format!("Could not save window preferences: {e}"));
            if let Err(error) = &result { eprintln!("{error}"); }
            let _ = tx.send(result);
        });
        cx.global_mut::<Self>().tail = Some(tail);
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            cx.update(|cx| {
                let writes = cx.global_mut::<Self>();
                if let Some((_, remaining)) = writes.pending.get_mut(&path) {
                    *remaining -= 1;
                    if *remaining == 0 { writes.pending.remove(&path); }
                }
                if let Err(error) = result {
                    this.update(cx, |this, cx| this.notify(error, cx)).ok();
                }
            }).ok();
        }).detach();
    }
}

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
    /// Where the main window was, to open there next time.
    #[serde(default)]
    pub window: Option<SavedWindow>,
}

/// The main window's frame in screen points; `maximized` keeps the restore frame too.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl SavedWindow {
    pub fn from_bounds(bounds: gpui::WindowBounds) -> Option<Self> {
        let b = bounds.get_bounds();
        let (x, y, width, height) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        // Full screen reopens as a window: a new Space at launch surprises more than it helps.
        let saved = Self { x, y, width, height, maximized: matches!(bounds, gpui::WindowBounds::Maximized(_)) };
        [x, y, width, height].iter().all(|v| v.is_finite()).then_some(saved).filter(|s| s.width > 0. && s.height > 0.)
    }

    /// The saved frame, clamped to the minimum size, if most of it is still on one of `displays`.
    pub fn bounds(self, displays: &[gpui::Bounds<gpui::Pixels>], min: (f32, f32)) -> Option<gpui::WindowBounds> {
        use gpui::{point, px, size, Bounds};
        let frame = Bounds::new(point(px(self.x), px(self.y)), size(px(self.width.max(min.0)), px(self.height.max(min.1))));
        // A display that was unplugged would leave the window out of reach.
        let visible = displays.iter().any(|d| {
            let shared = d.intersect(&frame);
            // Disjoint frames intersect with negative sizes, so clamp before multiplying.
            f32::from(shared.size.width).max(0.) * f32::from(shared.size.height).max(0.) >= 0.5 * f32::from(frame.size.width) * f32::from(frame.size.height)
        });
        visible.then_some(if self.maximized { gpui::WindowBounds::Maximized(frame) } else { gpui::WindowBounds::Windowed(frame) })
    }
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
            window: None,
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
    fn saved_window_reopens_only_where_it_can_be_reached() {
        use gpui::{point, px, size, Bounds, WindowBounds};
        let display = Bounds::new(point(px(0.), px(0.)), size(px(1512.), px(982.)));
        let frame = |x: f32, y: f32, w: f32, h: f32| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        let saved = SavedWindow::from_bounds(WindowBounds::Windowed(frame(100., 50., 1200., 800.))).unwrap();
        assert_eq!(saved.bounds(&[display], (720., 600.)), Some(WindowBounds::Windowed(frame(100., 50., 1200., 800.))));
        // Old files without the field still load, and the frame survives a round trip.
        let old: Panes = serde_json::from_str(r#"{"sidebar":220,"list":420,"sidebar_hidden":false}"#).unwrap();
        assert_eq!(old.window, None);
        let panes = Panes { window: Some(saved), ..old };
        assert_eq!(serde_json::from_str::<Panes>(&serde_json::to_string(&panes).unwrap()).unwrap(), panes);
        // Too small grows to the minimum; maximized stays maximized.
        let tiny = SavedWindow { width: 10., height: 10., maximized: true, ..saved };
        assert_eq!(tiny.bounds(&[display], (720., 600.)), Some(WindowBounds::Maximized(frame(100., 50., 720., 600.))));
        // A frame on an unplugged display, or mostly off screen, opens centred instead.
        for (x, y) in [(3000., 50.), (-2000., -2000.), (1000., 50.)] {
            assert_eq!(SavedWindow { x, y, ..saved }.bounds(&[display], (720., 600.)), None, "{x},{y}");
        }
        assert_eq!(SavedWindow::from_bounds(WindowBounds::Windowed(frame(f32::NAN, 0., 800., 600.))), None);
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
            window: None,
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
                            window: None,
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
                    let panes = Panes { sidebar: 180. + i as f32, list: 560., sidebar_hidden: i % 2 == 0, group_conversations: true, notifications: true, start_page: StartPage::Mail, window: None };
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

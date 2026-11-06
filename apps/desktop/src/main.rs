mod command_palette;
mod main_window;
mod theme;

use std::borrow::Cow;

use gpui::{
    point, prelude::*, px, size, App, Application, AssetSource, Bounds, SharedString,
    TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions,
};

use main_window::MainWindow;

/// Icons and fonts compiled into the binary.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/inbox.svg" => include_bytes!("../assets/icons/inbox.svg"),
            "icons/later.svg" => include_bytes!("../assets/icons/later.svg"),
            "icons/sent.svg" => include_bytes!("../assets/icons/sent.svg"),
            "icons/done.svg" => include_bytes!("../assets/icons/done.svg"),
            "icons/search.svg" => include_bytes!("../assets/icons/search.svg"),
            "icons/chevron.svg" => include_bytes!("../assets/icons/chevron.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        // Inter Tight is bundled. Inter and Geist Mono come from the system for now.
        // shortcut: relies on installed Inter and Geist Mono, bundle them before the first release.
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!("../assets/fonts/InterTight-Bold.ttf")),
                Cow::Borrowed(include_bytes!("../assets/fonts/InterTight-ExtraBold.ttf")),
            ])
            .expect("load bundled fonts");

        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // macOS: no visible title bar, traffic lights inside the sidebar. The invisible
                // system title bar still moves the window. Windows and Linux keep their native bar.
                titlebar: Some(TitlebarOptions {
                    title: Some("lagom".into()),
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: cfg!(target_os = "macos").then(|| point(px(18.), px(18.))),
                }),
                window_background: WindowBackgroundAppearance::Blurred,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| MainWindow::new(window, cx)),
        )
        .expect("open main window");
        cx.activate(true);
    });
}

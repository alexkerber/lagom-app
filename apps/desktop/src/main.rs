#[cfg(target_os = "macos")]
mod icon;
mod accounts;
mod command_palette;
mod compose;
mod empty_inbox;
mod folders;
mod mail;
mod main_window;
mod markdown;
mod motion;
mod settings;
mod notifications;
mod sync;
#[cfg(target_os = "macos")]
mod menus;
mod text_input;
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
            "icons/new-task.svg" => include_bytes!("../assets/icons/new-task.svg"),
            "icons/new-list.svg" => include_bytes!("../assets/icons/new-list.svg"),
            "icons/rename.svg" => include_bytes!("../assets/icons/rename.svg"),
            "icons/delete-list.svg" => include_bytes!("../assets/icons/delete-list.svg"),
            "icons/linked-mail.svg" => include_bytes!("../assets/icons/linked-mail.svg"),
            "icons/complete-task.svg" => include_bytes!("../assets/icons/complete-task.svg"),
            "icons/open-task.svg" => include_bytes!("../assets/icons/open-task.svg"),
            "icons/previous.svg" => include_bytes!("../assets/icons/previous.svg"),
            "icons/next.svg" => include_bytes!("../assets/icons/next.svg"),
            "icons/camera.svg" => include_bytes!("../assets/icons/camera.svg"),
            "icons/sun.svg" => include_bytes!("../assets/icons/sun.svg"),
            "icons/external.svg" => include_bytes!("../assets/icons/external.svg"),
            "icons/close.svg" => include_bytes!("../assets/icons/close.svg"),
            "icons/account-add.svg" => include_bytes!("../assets/icons/account-add.svg"),
            "icons/account-remove.svg" => include_bytes!("../assets/icons/account-remove.svg"),
            "icons/access.svg" => include_bytes!("../assets/icons/access.svg"),
            "icons/send.svg" => include_bytes!("../assets/icons/send.svg"),
            "icons/compose.svg" => include_bytes!("../assets/icons/compose.svg"),
            "icons/settings.svg" => include_bytes!("../assets/icons/settings.svg"),
            "icons/insert-file.svg" => include_bytes!("../assets/icons/insert-file.svg"),
            "icons/bold.svg" => include_bytes!("../assets/icons/bold.svg"),
            "icons/italic.svg" => include_bytes!("../assets/icons/italic.svg"),
            "icons/cc.svg" => include_bytes!("../assets/icons/cc.svg"),
            "icons/connect.svg" => include_bytes!("../assets/icons/connect.svg"),
            "icons/filter-all.svg" => include_bytes!("../assets/icons/filter-all.svg"),
            "icons/home.svg" => include_bytes!("../assets/icons/home.svg"),
            "icons/cloud-synced.svg" => include_bytes!("../assets/icons/cloud-synced.svg"),
            "icons/cloud-failed.svg" => include_bytes!("../assets/icons/cloud-failed.svg"),
            "icons/cloud-syncing.svg" => include_bytes!("../assets/icons/cloud-syncing.svg"),
            "icons/reply.svg" => include_bytes!("../assets/icons/reply.svg"),
            "icons/reply-all.svg" => include_bytes!("../assets/icons/reply-all.svg"),
            "icons/forward.svg" => include_bytes!("../assets/icons/forward.svg"),
            "icons/more.svg" => include_bytes!("../assets/icons/more.svg"),
            "icons/unread.svg" => include_bytes!("../assets/icons/unread.svg"),
            "icons/read.svg" => include_bytes!("../assets/icons/read.svg"),
            "icons/move.svg" => include_bytes!("../assets/icons/move.svg"),
            "icons/copy.svg" => include_bytes!("../assets/icons/copy.svg"),
            "icons/restore.svg" => include_bytes!("../assets/icons/restore.svg"),
            "icons/image.svg" => include_bytes!("../assets/icons/image.svg"),
            "icons/load-more.svg" => include_bytes!("../assets/icons/load-more.svg"),
            "icons/attachment.svg" => include_bytes!("../assets/icons/attachment.svg"),
            "icons/cancel.svg" => include_bytes!("../assets/icons/cancel.svg"),
            "icons/undo.svg" => include_bytes!("../assets/icons/undo.svg"),
            "icons/retry-send.svg" => include_bytes!("../assets/icons/retry-send.svg"),
            "icons/edit-draft.svg" => include_bytes!("../assets/icons/edit-draft.svg"),
            "icons/empty-trash.svg" => include_bytes!("../assets/icons/empty-trash.svg"),
            "icons/mail.svg" => include_bytes!("../assets/icons/mail.svg"),
            "icons/calendar.svg" => include_bytes!("../assets/icons/calendar.svg"),
            "icons/todo.svg" => include_bytes!("../assets/icons/todo.svg"),
            "icons/inbox.svg" => include_bytes!("../assets/icons/inbox.svg"),
            "icons/later.svg" => include_bytes!("../assets/icons/later.svg"),
            "icons/sent.svg" => include_bytes!("../assets/icons/sent.svg"),
            "icons/done.svg" => include_bytes!("../assets/icons/done.svg"),
            "icons/search.svg" => include_bytes!("../assets/icons/search.svg"),
            "icons/chevron.svg" => include_bytes!("../assets/icons/chevron.svg"),
            "icons/chevron-right.svg" => include_bytes!("../assets/icons/chevron-right.svg"),
            "icons/folder.svg" => include_bytes!("../assets/icons/folder.svg"),
            "icons/plus.svg" => include_bytes!("../assets/icons/plus.svg"),
            "icons/trash.svg" => include_bytes!("../assets/icons/trash.svg"),
            "icons/link.svg" => include_bytes!("../assets/icons/link.svg"),
            "icons/pop-out.svg" => include_bytes!("../assets/icons/pop-out.svg"),
            "app-icon.png" => include_bytes!("../../../brand/app-icon/export/lagom-512.png"),
            "icons/list-bullets.svg" => include_bytes!("../assets/icons/list-bullets.svg"),
            "icons/list-numbers.svg" => include_bytes!("../assets/icons/list-numbers.svg"),
            "illustrations/empty-tray.svg" => include_bytes!("../assets/illustrations/empty-tray.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

fn open_main_window(demo: bool, cx: &mut App) -> anyhow::Result<gpui::WindowHandle<MainWindow>> {
    cx.activate(true);
    if let Some(handle) = cx.windows().into_iter().find_map(|w| w.downcast::<MainWindow>()) {
        handle.update(cx, |_, window, _| window.activate_window())?;
        return Ok(handle);
    }
    // Demo captures can exercise narrow layouts without reading saved window preferences.
    let width = if demo {
        std::env::var("LAGOM_DEMO_WIDTH").ok().and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite()).unwrap_or(1440.).max(main_window::MIN_WINDOW.0)
    } else { 1440. };
    let bounds = Bounds::centered(None, size(px(width), px(900.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(main_window::MIN_WINDOW.0), px(main_window::MIN_WINDOW.1))),
            // macOS: no visible title bar, traffic lights inside the sidebar. The invisible
            // system title bar still moves the window. Windows and Linux keep their native bar.
            titlebar: Some(TitlebarOptions {
                // The list on screen; MainWindow keeps it current.
                title: Some("Inbox".into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: cfg!(target_os = "macos").then(|| point(px(18.), px(18.))),
            }),
            window_background: WindowBackgroundAppearance::Blurred,
            ..Default::default()
        },
        // `LAGOM_DEMO=1`: the sample mail, for screenshots and design work. No store, no network.
        move |window, cx| cx.new(|cx| MainWindow::new(demo, window, cx)),
    )
}

fn main() {
    let demo = std::env::var_os("LAGOM_DEMO").is_some_and(|v| v == "1");
    let app = Application::new().with_assets(Assets);
    app.on_reopen(move |cx| {
        if let Err(error) = open_main_window(demo, cx) { eprintln!("Could not reopen mail: {error}"); }
    });
    app.run(move |cx: &mut App| {
        // All text is the system font. Geist Mono (palette key caps) comes from the system if
        // installed, else GPUI falls back. shortcut: bundle Geist Mono or drop it before release.
        main_window::bind_keys(cx);
        text_input::bind_keys(cx);
        // After bind_keys: the macOS menu reads its shortcuts from the keymap when it is built.
        #[cfg(target_os = "macos")]
        menus::init(cx);
        if !demo { notifications::init(cx, move |cx| open_main_window(false, cx)); }
        open_main_window(demo, cx).expect("open main window");
        #[cfg(target_os = "macos")]
        icon::install();
    });
}

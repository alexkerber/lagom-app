//! The native macOS menu bar with standard shortcuts. Only items that work today.
//! macOS only: GPUI's `hide_other_apps` panics on Windows.

use gpui::{actions, App, KeyBinding, Menu, MenuItem, SystemMenuType, Window};

actions!(
    lagom,
    [Quit, Hide, HideOthers, ShowAll, CloseWindow, Minimize, Zoom, ToggleFullScreen, OpenRepository]
);

const REPOSITORY: &str = "https://github.com/alexkerber/lagom-app";

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &OpenRepository, cx| cx.open_url(REPOSITORY));
    cx.on_action(|_: &CloseWindow, cx| with_active_window(cx, |window| window.remove_window()));
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window| window.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| with_active_window(cx, |window| window.toggle_fullscreen()));

    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);

    cx.set_menus(vec![
        Menu {
            name: "lagom".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide lagom", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit lagom", Quit),
            ],
        },
        Menu { name: "File".into(), items: vec![MenuItem::action("Close Window", CloseWindow)] },
        // Done and Later stay off the menu: a key equivalent without cmd could catch typed letters.
        Menu {
            name: "Edit".into(),
            items: vec![MenuItem::action("Undo", crate::main_window::Undo)],
        },
        Menu {
            name: "Message".into(),
            items: vec![
                MenuItem::action("Reply", crate::main_window::Reply),
                MenuItem::action("Delete", crate::main_window::DeleteMessage),
                MenuItem::separator(),
                MenuItem::action("Command Palette", crate::main_window::OpenPalette),
            ],
        },
        Menu { name: "View".into(), items: vec![MenuItem::action("Enter Full Screen", ToggleFullScreen)] },
        Menu {
            name: "Window".into(),
            items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
        },
        Menu { name: "Help".into(), items: vec![MenuItem::action("lagom on GitHub", OpenRepository)] },
    ]);
}

/// Deferred: a menu action runs while its window is being updated, so a direct update fails.
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window) + 'static) {
    cx.defer(move |cx| {
        if let Some(handle) = cx.active_window() {
            handle.update(cx, |_, window, _| f(window)).ok();
        }
    });
}

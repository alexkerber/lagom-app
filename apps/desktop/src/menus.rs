//! The native macOS menu bar with standard shortcuts. Only items that work today.
//! macOS only: GPUI's `hide_other_apps` panics on Windows.

use gpui::{actions, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, Window};

use crate::text_input;
use crate::main_window::CloseWindow;

actions!(
    lagom,
    [Quit, Hide, HideOthers, ShowAll, Minimize, Zoom, ToggleFullScreen, OpenRepository, About]
);

const REPOSITORY: &str = "https://github.com/alexkerber/lagom-app";

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| {
        // A hidden main window has no focused action path to guard Quit.
        if cx.active_window().is_none() {
            for main in cx.windows().into_iter().filter_map(|w| w.downcast::<crate::main_window::MainWindow>()) {
                let held = main.update(cx, |main, window, cx| {
                    let held = main.hold_exit(cx);
                    if held { window.activate_window(); }
                    held
                });
                if held.unwrap_or(true) { return; }
            }
        }
        cx.quit();
    });
    cx.on_action(|_: &About, _| crate::icon::about());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &OpenRepository, cx| cx.open_url(REPOSITORY));
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window| window.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| with_active_window(cx, |window| window.toggle_fullscreen()));

    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);

    cx.set_menus(vec![
        Menu {
            name: "lagom".into(),
            items: vec![
                MenuItem::action("About lagom", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", crate::main_window::OpenSettings),
                MenuItem::separator(),
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
            // The focused text field handles these. With none focused, Undo undoes in the inbox and
            // the rest are greyed out. OsAction gives the standard selectors, so system panels work.
            items: vec![
                MenuItem::os_action("Undo", crate::main_window::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", text_input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", text_input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", text_input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", text_input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", text_input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Message".into(),
            items: vec![
                MenuItem::action("New Message", crate::main_window::NewMessage),
                MenuItem::action("Open in New Window", crate::main_window::OpenInWindow),
                MenuItem::action("Reply", crate::main_window::Reply),
                MenuItem::action("Mark Read or Unread", crate::main_window::ToggleRead),
                MenuItem::action("Delete", crate::main_window::DeleteMessage),
                MenuItem::separator(),
                MenuItem::action("Command Palette", crate::main_window::OpenPalette),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Today", crate::main_window::ShowHome),
                MenuItem::action("Mail", crate::main_window::ShowMail),
                MenuItem::action("Calendar", crate::main_window::ShowCalendar),
                MenuItem::action("Todo", crate::main_window::ShowTodo),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", crate::main_window::ToggleSidebar),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", ToggleFullScreen),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Keyboard Shortcuts", crate::main_window::ShowShortcuts),
                MenuItem::action("lagom on GitHub", OpenRepository),
            ],
        },
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

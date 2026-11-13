//! Settings: accounts first, as a list that opens a page per account (name, signature,
//! sign-in, remove), then general switches and the default task list. Remove asks inside the
//! app, not in a system dialog, and deletes the saved password and the local mail; the server
//! keeps its mail.

use std::time::Duration;

use std::collections::HashMap;

use gpui::{div, point, prelude::*, px, relative, Animation, AnimationExt, BoxShadow, Context, Div, Entity, FontWeight, Rgba, SharedString, Stateful};
use lagom_core::account::{Account, Auth};
use lagom_core::oauth::Provider;

use crate::main_window::{button, icon, icon_button, pressable, MainWindow, OneLine};
use crate::text_input::TextInput;
use crate::motion::spring;
use crate::theme::{radius, size, Theme};

/// What the Settings page shows.
pub struct Data<'a> {
    pub accounts: &'a [Account],
    /// Each account's colour, as in the sidebar and the calendar.
    pub colors: Vec<Rgba>,
    /// The account whose page is open; `None` shows the list.
    pub open: Option<&'a str>,
    /// Remove waits for a yes on the open account.
    pub confirm: bool,
    /// Accounts whose browser sign-in runs now.
    pub signing_in: Vec<String>,
    pub group: bool,
    pub notifications: bool,
    pub task_lists: &'a [lagom_core::tasks::TaskList],
    pub default_list: Option<i64>,
    pub names: &'a HashMap<String, Entity<TextInput>>,
    pub signatures: &'a HashMap<String, Entity<TextInput>>,
    pub password: Option<&'a NewPassword>,
    /// Keyboard shortcuts scrolls it to the end.
    pub scroll: &'a gpui::ScrollHandle,
}

/// A new password on its way to the provider.
pub struct NewPassword {
    pub id: String,
    pub input: Entity<TextInput>,
    /// The worker tries it with the servers now.
    pub checking: bool,
    /// The attempt this form waits for; a reply to any other attempt only shows a notice.
    pub attempt: u64,
    pub error: Option<String>,
}

/// How the account signs in, in the words people use.
fn sign_in_words(a: &Account) -> (&'static str, &'static str) {
    match a.auth {
        Auth::OAuth(Provider::Google) => ("Google account", "Signed in with Google."),
        Auth::OAuth(Provider::Microsoft) => ("Microsoft account", "Signed in with Microsoft."),
        Auth::Password => ("Email and password", "Signed in with your email address and password."),
    }
}

/// A round mark with the account's initial in its colour.
fn avatar(a: &Account, color: Rgba, s: f32) -> Div {
    let name = if a.display_name.trim().is_empty() { &a.email } else { &a.display_name };
    let initial = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    div().flex_shrink_0().size(px(s)).rounded_full().flex().items_center().justify_center()
        .bg(Rgba { a: 0.18, ..color }).text_color(color).font_weight(FontWeight::SEMIBOLD).text_size(px(s * 0.42))
        .child(initial)
}

/// A titled group: heading, an optional line under it, then its content.
fn section(t: &Theme, title: &'static str, note: Option<&'static str>) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().text_size(px(size::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
        .children(note.map(|n| div().text_size(px(size::SNIPPET)).text_color(t.ink2).line_height(relative(1.4)).child(n)))
}

/// Rows on one raised plate, as in System Settings.
/// Tab reaches it; Enter or Space presses it.
fn stop<E: StatefulInteractiveElement>(t: &Theme, el: E) -> E {
    let ring = t.accent;
    el.focusable().tab_stop(true).focus(move |s| s.border_1().border_color(ring))
}

fn plate(t: &Theme) -> Div {
    div().flex().flex_col().rounded(px(radius::PANEL)).bg(t.subtle).border_1().border_color(t.line).overflow_hidden()
}

pub fn view(t: &Theme, d: Data, cx: &mut Context<MainWindow>) -> Div {
    let content = match d.open.and_then(|id| d.accounts.iter().position(|a| a.id == id)) {
        Some(ix) => account_page(t, &d, ix, cx),
        None => main_page(t, &d, cx),
    };
    div().flex_1().min_w(px(0.)).flex().child(
        div()
            .id("settings-page")
            .track_scroll(d.scroll)
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            // Content keeps its own height, so a long page scrolls instead of overflowing.
            .items_start()
            .px(px(24.))
            .py(px(32.))
            .rounded(px(radius::PANEL))
            .child(content),
    )
}

fn main_page(t: &Theme, d: &Data, cx: &mut Context<MainWindow>) -> Div {
    let mut rows = plate(t);
    for (ix, a) in d.accounts.iter().enumerate() {
        let id = a.id.clone();
        let named = a.display_name.trim() != a.email && !a.display_name.trim().is_empty();
        let (kind, _) = sign_in_words(a);
        rows = rows.child(
            stop(t, pressable(div().id(("account", ix)), t, false))
                .debug_selector({ let id = a.id.clone(); move || format!("account-{id}") })
                .flex().items_center().gap(px(12.)).px(px(14.)).py(px(10.))
                .when(ix > 0, |r| r.border_t_1().border_color(t.line))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.open_account_settings(id.clone(), window, cx)))
                .child(avatar(a, d.colors[ix], 32.))
                .child(div().flex_1().min_w(px(0.)).flex().flex_col()
                    .child(div().font_weight(FontWeight::MEDIUM).one_line().child(a.email.clone()))
                    .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).one_line()
                        .child(if named { format!("{} · {kind}", a.display_name.trim()) } else { kind.to_string() })))
                .child(icon("icons/chevron-right.svg", t.ink2, 16.)),
        );
    }
    let accounts = div().flex().flex_col().gap(px(12.))
        .child(div().flex().items_start().justify_between().gap(px(16.))
            .child(section(t, "Accounts", Some("The email addresses in lagom. Only you see them.")))
            .child(stop(t, button(t, "Add account", None)).on_click(cx.listener(|this, _, window, cx| this.settings_add(window, cx)))))
        .when(d.accounts.is_empty(), |s| s.child(div().text_color(t.ink2).child("No accounts yet.")))
        .when(!d.accounts.is_empty(), |s| s.child(rows));
    let general = div().flex().flex_col().gap(px(12.))
        .child(section(t, "General", None))
        .child(plate(t).px(px(14.)).py(px(6.))
            .child(stop(t, toggle_row(t, "Group by conversation", d.group)).on_click(cx.listener(|this, _, _, cx| this.toggle_conversations(cx))))
            .child(stop(t, toggle_row(t, "Notifications for new mail", d.notifications)).on_click(cx.listener(|this, _, _, cx| this.toggle_notifications(cx)))));
    let lists = (!d.task_lists.is_empty()).then(|| div().debug_selector(|| "default-task-lists".into()).flex().flex_col().gap(px(12.))
        .child(section(t, "Default task list", Some("New tasks go here.")))
        .child(plate(t).p(px(4.)).children(d.task_lists.iter().map(|list| {
            let id = list.id;
            let on = d.default_list == Some(id);
            div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(6.)).rounded(px(radius::CONTROL))
                .when(on, |d| d.bg(t.sel))
                .child(div().flex_1().min_w(px(0.)).one_line().child(list.name.clone()))
                .child(stop(t, icon_button(t, "default-task-list", if on { "icons/complete-task.svg" } else { "icons/open-task.svg" },
                    if on { "Default task list" } else { "Set as default task list" })
                    .id(("default-task-list", id as usize)).debug_selector(move || format!("default-task-list-{id}")))
                    .on_click(cx.listener(move |this, _, _, cx| { this.save_task(crate::main_window::TaskChange::DefaultList(id), cx); })))
        }))));
    div()
        .w_full()
        .max_w(px(600.))
        .flex()
        .flex_col()
        .gap(px(32.))
        // A window-title-sized heading, as in System Settings panes.
        .child(div().text_size(px(size::SUBJECT)).font_weight(FontWeight::BOLD).child("Settings"))
        .child(accounts)
        .child(general)
        .children(lists)
        .child(shortcuts(t))
        // The About equivalent on every platform; macOS also has About lagom in the menu bar.
        .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).child(format!("lagom {}", option_env!("LAGOM_VERSION").unwrap_or("development build"))))
}

/// Every shortcut, grouped, with the keys this platform uses.
fn shortcuts(t: &Theme) -> Div {
    div().debug_selector(|| "keyboard-shortcuts".into()).flex().flex_col().gap(px(12.))
        .child(section(t, "Keyboard shortcuts", Some("Single keys work when you are not typing in a field.")))
        .children(crate::shortcuts::LIST.iter().map(|(group, rows)| {
            plate(t).px(px(14.)).py(px(6.))
                .child(div().py(px(4.)).text_size(px(size::SNIPPET)).font_weight(FontWeight::MEDIUM).text_color(t.ink2).child(*group))
                .children(rows.iter().map(|(label, chords)| {
                    div().flex().items_center().justify_between().gap(px(12.)).min_h(px(28.))
                        .child(div().min_w(px(0.)).one_line().child(*label))
                        .child(div().flex().flex_shrink_0().gap(px(4.))
                            .children(chords.iter().map(|c| crate::command_palette::key_cap(t, crate::shortcuts::label(c), false))))
                }))
        }))
}

fn account_page(t: &Theme, d: &Data, ix: usize, cx: &mut Context<MainWindow>) -> Div {
    let a = &d.accounts[ix];
    let id = a.id.clone();
    let (kind, signed) = sign_in_words(a);
    let field = |label: &'static str, note: &'static str, input: Option<&Entity<TextInput>>, h: Option<f32>| {
        div().flex().flex_col().gap(px(6.))
            .child(div().text_size(px(size::SNIPPET)).font_weight(FontWeight::MEDIUM).child(label))
            .children(input.map(|i| div().when_some(h, |d, h| d.h(px(h))).child(i.clone())))
            .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).line_height(relative(1.4)).child(note))
    };
    let signing_in = d.signing_in.contains(&a.id);
    let oauth = matches!(a.auth, Auth::OAuth(_));
    let remove = if d.confirm {
        let yes = id.clone();
        div().flex().flex_col().gap(px(12.))
            .child(div().text_color(t.danger).line_height(relative(1.4))
                .child(format!("Remove {} from lagom? Your mail stays with your provider.", a.email)))
            .child(div().flex().gap(px(8.))
                .child(stop(t, button(t, "Remove", Some(t.danger))).on_click(cx.listener(move |this, _, window, cx| this.remove_account(yes.clone(), window, cx))))
                .child(stop(t, button(t, "Keep", None)).on_click(cx.listener(|this, _, window, cx| this.confirm_remove(None, window, cx)))))
    } else {
        let ask = id.clone();
        div().flex().flex_col().gap(px(12.))
            .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).line_height(relative(1.4))
                .child("Removes the account and its saved mail from this Mac. Your mail stays with your provider."))
            .child(div().flex().child(stop(t, button(t, "Remove account", None)).text_color(t.danger)
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_remove(Some(ask.clone()), window, cx)))))
    };
    let again = id.clone();
    div()
        .w_full()
        .max_w(px(600.))
        .flex()
        .flex_col()
        .gap(px(28.))
        .child(div().flex().items_center().gap(px(4.)).ml(px(-6.))
            .child(stop(t, icon_button(t, "back", "icons/previous.svg", "Back to Settings · Esc"))
                .on_click(cx.listener(|this, _, window, cx| this.close_account_settings(window, cx))))
            .child(div().text_color(t.ink2).child("Settings")))
        .child(div().flex().items_center().gap(px(14.))
            .child(avatar(a, d.colors[ix], 48.))
            .child(div().min_w(px(0.)).flex().flex_col().gap(px(2.))
                .child(div().text_size(px(size::SUBJECT)).font_weight(FontWeight::BOLD).one_line().child(a.email.clone()))
                .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).child(kind))))
        .child(plate(t).p(px(16.)).gap(px(20.))
            .child(field("Your name", "Shown to the people you write to, and in the sidebar.", d.names.get(&a.id), None))
            .child(field("Signature", "Added to new mail, replies and forwards.", d.signatures.get(&a.id), Some(88.))
                .debug_selector({ let id = a.id.clone(); move || format!("signature-{id}") })))
        .child(plate(t).p(px(16.)).gap(px(12.))
            .child(div().text_size(px(size::SNIPPET)).font_weight(FontWeight::MEDIUM).child("Sign-in"))
            .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).child(signed))
            .when(oauth, |p| p.child(div().flex().child(
                // The label changes while the browser is open; the stable wrapper keeps the keys.
                stop(t, div().id("sign-in-again").rounded(px(radius::CONTROL)).child(button(t, if signing_in { "Cancel sign-in" } else { "Sign in again" }, None)))
                    .on_click(cx.listener(move |this, _, _, cx| this.sign_in_again(&again, cx))))))
            .when(!oauth, |p| p.child(password_form(t, &id, d.password.filter(|p| p.id == id), cx))))
        .child(plate(t).p(px(16.)).child(remove))
}

/// A setting with a macOS-style switch. The whole row toggles, so the target is large.
/// "Change password", then the field: lagom saves the password only after the provider accepts it.
fn password_form(t: &Theme, id: &str, form: Option<&NewPassword>, cx: &mut Context<MainWindow>) -> Div {
    let Some(form) = form else {
        let id = id.to_string();
        return div().flex().child(stop(t, button(t, "Change password", None))
            .on_click(cx.listener(move |this, _, window, cx| this.start_password_change(id.clone(), window, cx))));
    };
    let note = match &form.error {
        Some(e) => div().text_color(t.danger).child(e.clone()),
        None => div().text_color(t.ink2).child("lagom checks it with your provider before it saves it."),
    };
    let save = if form.checking { "Checking…" } else { "Save password" };
    div().flex().flex_col().gap(px(8.))
        .child(div().text_size(px(size::SNIPPET)).font_weight(FontWeight::MEDIUM).child("New password"))
        .child(form.input.clone())
        .child(note.text_size(px(size::SNIPPET)).line_height(relative(1.4)))
        .child(div().flex().gap(px(8.))
            .child(stop(t, div().id("save-password").rounded(px(radius::CONTROL)).child(button(t, save, Some(t.accent))))
                .on_click(cx.listener(|this, _, window, cx| this.save_password(window, cx))))
            .child(stop(t, button(t, "Cancel", None)).on_click(cx.listener(|this, _, window, cx| this.cancel_password_change(window, cx)))))
}

pub(crate) fn toggle_row(t: &Theme, label: &'static str, on: bool) -> Stateful<Div> {
    div().id(label).debug_selector(move || label.into()).flex().items_center().justify_between().min_h(px(32.))
        .child(label)
        .child(switch(t, label, on))
}

/// Track 38×22 with a white knob that springs across. On takes the selection colour, as the
/// system switch takes the accent; off is the neutral selection grey.
fn switch(t: &Theme, id: &'static str, on: bool) -> impl IntoElement {
    const TRAVEL: f32 = 16.;
    let knob = div()
        .absolute()
        .top(px(2.))
        .size(px(18.))
        .rounded_full()
        .bg(gpui::rgb(0xFFFFFF))
        .shadow(vec![BoxShadow {
            color: Rgba { r: 0., g: 0., b: 0., a: 0.28 }.into(),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.5),
            spread_radius: px(0.),
        }])
        // A new id per state replays the slide on every toggle.
        // shortcut: the knob also slides once when Settings opens; keep the last toggle in state to skip that.
        .with_animation(SharedString::from(format!("{id}-{on}")), Animation::new(Duration::from_millis(220)), move |knob, v| {
            let x = if on { spring(v) } else { 1. - spring(v) };
            knob.left(px(2. + TRAVEL * x))
        });
    div()
        .relative()
        .flex_shrink_0()
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .bg(if on { t.sel_focus } else { t.sel })
        .child(knob)
}

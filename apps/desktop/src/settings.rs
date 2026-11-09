//! Notification preference and accounts. Remove asks inside the app, not in a
//! system dialog, and deletes the saved password and the local mail; the server keeps its mail.

use std::time::Duration;

use gpui::{div, point, prelude::*, px, Animation, AnimationExt, BoxShadow, Context, Div, FontWeight, Rgba, SharedString, Stateful};
use lagom_core::account::Account;

use crate::main_window::{button, mono, MainWindow, OneLine};
use crate::motion::spring;
use crate::theme::{radius, size, Theme};

/// `confirm`: the id of the account whose removal waits for a yes.
pub fn view(t: &Theme, accounts: &[Account], confirm: Option<&str>, group: bool, notifications: bool, task_lists: &[lagom_core::tasks::TaskList], default_list: Option<i64>, signatures: &std::collections::HashMap<String, gpui::Entity<crate::text_input::TextInput>>, cx: &mut Context<MainWindow>) -> Div {
    let mut list = div().flex().flex_col().pt(px(20.)).border_t_1().border_color(t.line);
    for a in accounts {
        let id = a.id.clone();
        let asking = confirm == Some(a.id.as_str());
        let row = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(48.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::MEDIUM).one_line().child(a.email.clone()))
                    .child(mono(format!("{} · {}", a.imap.host, a.smtp.host), t.ink2).one_line()),
            );
        let row = if asking {
            row.child(div().text_size(px(size::SNIPPET)).text_color(t.danger).child("Remove the password and local mail?"))
                .child(button(t, "Remove", Some(t.danger)).on_click(cx.listener(move |this, _, _, cx| this.remove_account(id.clone(), cx))))
                .child(button(t, "Keep", None).on_click(cx.listener(|this, _, _, cx| this.confirm_remove(None, cx))))
        } else {
            row.child(button(t, "Remove", None).on_click(cx.listener(move |this, _, _, cx| this.confirm_remove(Some(id.clone()), cx))))
        };
        list = list.child(row).children(signatures.get(&a.id).map(|input|
            div().debug_selector({ let id = a.id.clone(); move || format!("signature-{id}") })
                .flex().flex_col().gap(px(6.)).py(px(12.)).child(mono("Signature", t.ink2))
                .child(div().h(px(72.)).child(input.clone()))
        ));
    }
    let content = div()
        .w_full()
        .max_w(px(640.))
        .flex()
        .flex_col()
        .gap(px(16.))
        // A window-title-sized heading, as in System Settings panes.
        .child(div().font_weight(FontWeight::SEMIBOLD).child("Settings"))
        .child(div().text_color(t.ink2).child(SharedString::from(match accounts.len() {
            0 => "No accounts.".to_string(),
            1 => "One account.".to_string(),
            n => format!("{n} accounts."),
        })))
        .child(toggle_row(t, "Group by conversation", group).on_click(cx.listener(|this, _, _, cx| this.toggle_conversations(cx))))
        .child(toggle_row(t, "Notifications for new mail", notifications).on_click(cx.listener(|this, _, _, cx| this.toggle_notifications(cx))))
        .when(!task_lists.is_empty(), |d| d.child(div().debug_selector(|| "default-task-lists".into()).flex().flex_col().gap(px(8.)).child("Default task list")
            .children(task_lists.iter().map(|list| {
                let id = list.id;
                crate::main_window::pressable(div().id(("default-task-list", id as usize)), t, default_list == Some(id))
                    .debug_selector(move || format!("default-task-list-{id}"))
                    .px(px(10.)).py(px(6.)).rounded(px(radius::CONTROL))
                    .when(default_list == Some(id), |d| d.bg(t.sel))
                    .child(format!("{}{}", if default_list == Some(id) { "✓ " } else { "" }, list.name))
                    .on_click(cx.listener(move |this, _, _, cx| { this.save_task(crate::main_window::TaskChange::DefaultList(id), cx); }))
            }))))
        .child(list)
        .child(div().flex().child(button(t, "Add account", None).on_click(cx.listener(|this, _, window, cx| this.settings_add(window, cx)))))
        .child(mono("Removing an account keeps its mail on the server.", t.ink2));
    div().flex_1().min_w(px(0.)).flex().child(
        div()
            .id("settings-page")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(px(24.))
            .py(px(24.))
            .rounded(px(radius::PANEL))
            .child(content),
    )
}

/// A setting with a macOS-style switch. The whole row toggles, so the target is large.
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

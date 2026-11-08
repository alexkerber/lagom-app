//! Settings, minimal: the accounts, with Add and Remove. Remove asks inside the app, not in a
//! system dialog, and deletes the saved password and the local mail; the server keeps its mail.

use gpui::{div, prelude::*, px, Context, Div, FontWeight, SharedString};
use lagom_core::account::Account;

use crate::main_window::{button, mono, MainWindow, OneLine};
use crate::theme::{radius, size, Theme};

/// `confirm`: the id of the account whose removal waits for a yes.
pub fn view(t: &Theme, accounts: &[Account], confirm: Option<&str>, group: bool, cx: &mut Context<MainWindow>) -> Div {
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
        list = list.child(row);
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
        .child(div().flex().items_center().gap(px(12.)).child(div().flex_1().child("Group by conversation"))
            .child(button(t, if group { "On" } else { "Off" }, None).on_click(cx.listener(|this, _, _, cx| this.toggle_conversations(cx)))))
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

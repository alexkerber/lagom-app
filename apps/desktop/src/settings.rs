//! Settings, minimal: the accounts, with Add and Remove. Remove asks inside the app, not in a
//! system dialog, and deletes the saved password and the local mail; the server keeps its mail.

use gpui::{div, prelude::*, px, relative, Context, Div, FontWeight, SharedString};
use lagom_core::account::Account;

use crate::main_window::{key_button, mono, MainWindow};
use crate::theme::{font, radius, size, Theme};

/// `confirm`: the id of the account whose removal waits for a yes.
pub fn view(t: &Theme, accounts: &[Account], confirm: Option<&str>, cx: &mut Context<MainWindow>) -> Div {
    let mut list = div().flex().flex_col().pt(px(20.)).border_t_1().border_color(t.line);
    for a in accounts {
        let id = a.id.clone();
        let asking = confirm == Some(a.id.as_str());
        let row = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(56.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::MEDIUM).truncate().child(a.email.clone()))
                    .child(mono(format!("{} · {}", a.imap.host, a.smtp.host), t.ink2).truncate()),
            );
        let row = if asking {
            row.child(div().text_size(px(size::SNIPPET)).text_color(t.danger).child("Remove the password and local mail?"))
                .child(key_button(t, "Remove", "", Some(t.danger)).on_click(cx.listener(move |this, _, _, cx| this.remove_account(id.clone(), cx))))
                .child(key_button(t, "Keep", "", None).on_click(cx.listener(|this, _, _, cx| this.confirm_remove(None, cx))))
        } else {
            row.child(key_button(t, "Remove", "", None).on_click(cx.listener(move |this, _, _, cx| this.confirm_remove(Some(id.clone()), cx))))
        };
        list = list.child(row);
    }
    let content = div()
        .w_full()
        .max_w(px(640.))
        .flex()
        .flex_col()
        .gap(px(20.))
        .child(div().font_family(font::DISPLAY).text_size(px(size::DISPLAY)).font_weight(FontWeight::BOLD).line_height(relative(1.05)).child("Settings"))
        .child(div().text_size(px(size::TITLE)).text_color(t.ink2).child(SharedString::from(match accounts.len() {
            0 => "No accounts.".to_string(),
            1 => "One account.".to_string(),
            n => format!("{n} accounts."),
        })))
        .child(list)
        .child(div().flex().child(key_button(t, "Add account", "", None).on_click(cx.listener(|this, _, window, cx| this.settings_add(window, cx)))))
        .child(mono("Removing an account keeps its mail on the server.", t.ink2));
    div().flex_1().min_w(px(0.)).flex().child(
        div()
            .id("settings-page")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(px(24.))
            .py(px(56.))
            .rounded(px(radius::PANEL))
            .child(content),
    )
}

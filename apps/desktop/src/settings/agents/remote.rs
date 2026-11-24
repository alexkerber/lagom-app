use super::*;
use crate::text_input::TextInput;
use gpui::{ClipboardItem, Entity, Focusable};

pub(crate) struct RemoteForm {
    pub fields: [Entity<TextInput>; 4],
    pub accounts: Vec<String>,
    pub level: Level,
}

pub(super) fn view(
    t: &Theme,
    a: &Agents,
    accounts: &[Account],
    cx: &mut Context<MainWindow>,
) -> Div {
    let mut panel = plate(t).p(px(14.)).flex().flex_col().gap(px(8.));
    if let Some((token, line)) = &a.issued {
        for (label, copy, value) in [
            ("Token · shown once", "Copy token", token.as_str()),
            ("SSH authorized_keys line", "Copy line", line.as_str()),
        ] {
            panel = panel
                .child(note(t, label))
                .child(mono(value.to_string(), t.ink))
                .child(stop(t, button(t, copy, None)).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some((token, line)) = &this.agents.issued {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                if copy == "Copy token" {
                                    token.to_string()
                                } else {
                                    line.clone()
                                },
                            ));
                        }
                    },
                )));
        }
        return panel.child(stop(t, button(t, "Done", None)).on_click(cx.listener(
            |this, _, _, cx| {
                this.agents.issued = None;
                cx.notify();
            },
        )));
    }
    let Some(form) = &a.remote else {
        return div().child(
            stop(t, button(t, "Add remote agent", None)).on_click(cx.listener(
                |this, _, window, cx| {
                    let expires =
                        (chrono::Utc::now().date_naive() + chrono::Days::new(30)).to_string();
                    let fields = [
                        "Host name",
                        "203.0.113.0/24, 2001:db8::1",
                        "YYYY-MM-DD",
                        "ssh-ed25519 …",
                    ]
                    .map(|hint| {
                        cx.new(|cx| {
                            TextInput::new(window, cx)
                                .bordered()
                                .placeholder(hint)
                                .with_text(if hint == "YYYY-MM-DD" { &expires } else { "" })
                        })
                    });
                    window.focus(&fields[0].focus_handle(cx));
                    this.agents.remote = Some(RemoteForm {
                        fields,
                        accounts: Vec::new(),
                        level: Level::Read,
                    });
                    cx.notify();
                },
            )),
        );
    };
    for (label, field) in [
        "Remote host",
        "Source IP addresses or ranges",
        "Expires at midnight UTC",
        "SSH public key",
    ]
    .into_iter()
    .zip(&form.fields)
    {
        panel = panel.child(note(t, label)).child(field.clone());
    }
    panel = panel.child(note(t, "Allowed accounts"));
    for (ix, account) in accounts.iter().enumerate() {
        let id = account.id.clone();
        let on = form.accounts.contains(&id);
        panel = panel.child(
            stop(
                t,
                button(
                    t,
                    if on { "Selected" } else { "Select" },
                    on.then_some(t.accent),
                )
                .id(("remote-account", ix))
                .child(account.email.clone()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = &mut this.agents.remote {
                    if on {
                        f.accounts.retain(|a| a != &id);
                    } else {
                        f.accounts.push(id.clone());
                    }
                    cx.notify();
                }
            })),
        );
    }
    let mut levels = div().flex().gap(px(4.));
    for level in [Level::Read, Level::Triage, Level::Send] {
        levels = levels.child(
            stop(
                t,
                button(t, word(level), (form.level == level).then_some(t.accent)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = &mut this.agents.remote {
                    f.level = level;
                    cx.notify();
                }
            })),
        );
    }
    panel.child(note(t, "Maximum access")).child(levels).child(
        div()
            .flex()
            .gap(px(8.))
            .child(
                stop(t, button(t, "Create agent", Some(t.accent)))
                    .on_click(cx.listener(|this, _, _, cx| this.create_remote(cx))),
            )
            .child(
                stop(t, button(t, "Cancel", None)).on_click(cx.listener(|this, _, _, cx| {
                    this.agents.remote = None;
                    cx.notify();
                })),
            ),
    )
}

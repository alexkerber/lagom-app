use super::*;
use crate::text_input::TextInput;
use gpui::{ClipboardItem, Entity, Focusable};

pub(crate) struct RemoteForm {
    pub fields: [Entity<TextInput>; 4],
    _sources_changed: gpui::Subscription,
    pub accounts: Vec<String>,
    pub level: Level,
}

pub(crate) fn source_ranges(input: &str) -> anyhow::Result<Vec<ipnet::IpNet>> {
    use anyhow::{ensure, Context as _};
    use ipnet::IpNet;
    input
        .split(',')
        .map(|source| {
            let source = source.trim();
            let net = source
                .parse::<IpNet>()
                .or_else(|_| source.parse::<std::net::IpAddr>().map(IpNet::from))
                .context("Enter IP addresses or CIDR ranges separated by commas")?;
            ensure!(
                net == net.trunc(),
                "Enter a network address without host bits"
            );
            let minimum = if net.addr().is_ipv4() { 16 } else { 48 };
            ensure!(
                net.prefix_len() >= minimum,
                "Use IPv4 prefixes of /16 or longer and IPv6 prefixes of /48 or longer"
            );
            Ok(net)
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn copy_token(board: &objc2_app_kit::NSPasteboard, token: &str) -> bool {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSPasteboardItem;
    use objc2_foundation::{ns_string, NSArray, NSMutableData, NSRange};

    let item = NSPasteboardItem::new();
    if !item.setString_forType(ns_string!(""), ns_string!("org.nspasteboard.ConcealedType")) {
        return false;
    }
    let bytes = NSMutableData::with_bytes(token.as_bytes());
    let copied = item.setData_forType(&bytes, ns_string!("public.utf8-plain-text")) && {
        board.clearContents();
        board.writeObjects(&NSArray::from_slice(&[ProtocolObject::from_ref(&*item)]))
    };
    bytes.resetBytesInRange(NSRange::new(0, token.len()));
    copied
}

pub(super) fn view(
    t: &Theme,
    a: &Agents,
    accounts: &[Account],
    cx: &mut Context<MainWindow>,
) -> Div {
    let mut panel = plate(t).p(px(14.)).flex().flex_col().gap(px(8.));
    if let Some((token, line)) = &a.issued {
        panel = panel
            .child(note(t, "SSH authorized_keys line"))
            .child(mono(line.clone(), t.ink))
            .child(
                stop(t, button(t, "Copy line", None)).on_click(cx.listener(|this, _, _, cx| {
                    if let Some((_, line)) = &this.agents.issued {
                        cx.write_to_clipboard(ClipboardItem::new_string(line.clone()));
                    }
                })),
            )
            .child(
                stop(
                    t,
                    button(
                        t,
                        if a.show_token {
                            "Hide token"
                        } else {
                            "Show token"
                        },
                        None,
                    ),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.agents.show_token = !this.agents.show_token;
                    cx.notify();
                })),
            );
        if a.show_token {
            // GPUI caches shaped strings. Keep the complete token in its zeroizing buffer.
            let glyphs = token.chars().map(|ch| mono(ch.to_string(), t.ink));
            panel = panel
                .child(div().flex().flex_wrap().children(glyphs))
                .child(stop(t, button(t, "Copy token", None)).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some((token, _)) = &this.agents.issued {
                            #[cfg(target_os = "macos")]
                            if !copy_token(&objc2_app_kit::NSPasteboard::generalPasteboard(), token)
                            {
                                this.notify("Could not copy token", cx);
                            }
                            #[cfg(not(target_os = "macos"))]
                            cx.write_to_clipboard(ClipboardItem::new_string(token.to_string()));
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
                        _sources_changed: cx.observe(&fields[1], |_, _, cx| cx.notify()),
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
    let sources = form.fields[1].read(cx).text();
    if !sources.is_empty() {
        let preview = match source_ranges(sources) {
            Ok(nets) => format!(
                "Allowed sources: {}",
                nets.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(error) => error.to_string(),
        };
        panel = panel.child(note(t, preview));
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
    panel.child(note(t, "Maximum access")).child(levels)
        .child(div().text_size(px(size::SNIPPET)).text_color(t.ink).line_height(relative(1.4)).child(super::CONSENT))
        .child(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_preview_only_canonical_narrow_ranges() {
        for source in [
            "10.0.0.1/16",
            "2001:db8::1/48",
            "10.0.0.0/15",
            "2001:db8::/47",
            "0.0.0.0/0",
            "::/0",
            "",
        ] {
            assert!(source_ranges(source).is_err(), "{source}");
        }
        let ranges = source_ranges("10.0.0.0/16, 2001:db8::/48, 203.0.113.1, ::1").unwrap();
        assert_eq!(
            ranges.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["10.0.0.0/16", "2001:db8::/48", "203.0.113.1/32", "::1/128"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copied_token_and_concealment_marker_share_one_item() {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::ns_string;
        let board = NSPasteboard::pasteboardWithUniqueName();
        assert!(copy_token(&board, "fixture"));
        let items = board.pasteboardItems().unwrap();
        assert_eq!(items.count(), 1);
        let item = items.objectAtIndex(0);
        assert_eq!(
            item.stringForType(ns_string!("public.utf8-plain-text"))
                .unwrap()
                .to_string(),
            "fixture"
        );
        assert!(item
            .stringForType(ns_string!("org.nspasteboard.ConcealedType"))
            .is_some());
        board.clearContents();
    }
}

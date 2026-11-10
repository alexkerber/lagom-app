use super::*;
use crate::text_input::{Backspace, Enter, Escape, FocusNext, Paste};
use lagom_core::contacts::{Contact, Contacts, split_recipients, valid_recipient};

#[derive(Default)]
pub(super) struct Addresses {
    pub tokens: [Vec<String>; 2],
    pub contacts: Contacts,
    pub matches: Vec<Contact>,
    pub active: Option<usize>,
    pub selected: usize,
}

impl Compose {
    pub(super) fn load_contacts(&mut self, cx: &mut Context<Self>) {
        let Some((store, account)) = &self.store else { return; };
        let (path, account) = (store.path().to_owned(), account.id.clone());
        const ERROR: &str = "Could not load contacts. Enter an address to continue.";
        let load = cx.background_executor().spawn(async move {
            Store::open(path)?.contacts(&account, chrono::Utc::now().timestamp())
        });
        self._contacts = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(contacts) => {
                        this.addresses.contacts = contacts;
                        if this.import_error == Some(ERROR) { this.import_error = None; }
                        if let Some(field) = this.addresses.active { this.suggest(field, cx); }
                    }
                    Err(_) => this.import_error = Some(ERROR),
                }
                cx.notify();
            }).ok();
        }));
    }

    pub(super) fn address_tokens(&self, field: usize, cx: &gpui::App) -> Vec<String> {
        let mut parts = self.addresses.tokens[field].clone();
        parts.extend(split_recipients(self.fields()[field].read(cx).text()));
        parts
    }

    pub(super) fn suggest(&mut self, field: usize, cx: &mut Context<Self>) {
        self.addresses.matches = self.addresses.contacts.suggest(self.fields()[field].read(cx).text());
        self.addresses.active = Some(field);
        self.addresses.selected = 0;
        cx.notify();
    }

    pub(super) fn address_changed(&mut self, field: usize, cx: &mut Context<Self>) {
        let text = self.fields()[field].read(cx).text();
        let parts = split_recipients(text);
        if parts.len() > 1 || parts.first().is_some_and(|part| part != text.trim()) {
            self.commit_address(field, cx);
        }
        self.suggest(field, cx);
    }

    pub(super) fn commit_address(&mut self, field: usize, cx: &mut Context<Self>) {
        let input = self.fields()[field].clone();
        let text = input.read(cx).text();
        if !text.is_empty() {
            self.addresses.tokens[field].extend(split_recipients(text));
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        self.addresses.matches.clear();
        self.save_later(cx);
        cx.notify();
    }

    fn accept_address(&mut self, field: usize, at: usize, cx: &mut Context<Self>) {
        let Some(contact) = self.addresses.matches.get(at) else { return; };
        self.addresses.tokens[field].push(contact.recipient());
        self.fields()[field].clone().update(cx, |input, cx| input.set_text("", cx));
        self.addresses.matches.clear();
        self.save_later(cx);
        cx.notify();
    }

    fn address_enter(&mut self, field: usize, tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Tab must not slip in an address the user never wrote to; Enter on a shown row is a choice.
        let pick = self.addresses.matches.get(self.addresses.selected).is_some_and(|c| !tab || c.written);
        if self.addresses.active == Some(field) && pick {
            self.accept_address(field, self.addresses.selected, cx);
        } else {
            self.commit_address(field, cx);
            window.focus_next();
        }
        cx.stop_propagation();
    }

    pub(super) fn address_field(&self, field: usize, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let input = self.fields()[field].clone();
        let open = input.focus_handle(cx).is_focused(window) && self.addresses.active == Some(field) && !self.addresses.matches.is_empty();
        let tokens = self.addresses.tokens[field].iter().enumerate().map(|(at, text)| {
            let invalid = !valid_recipient(text);
            pressable(div().id(("recipient", at)), &t, false)
                .debug_selector(move || format!("recipient-{field}-{at}{}", if invalid { "-invalid" } else { "" }))
                .px(px(6.)).py(px(3.)).rounded(px(radius::CONTROL)).bg(t.subtle)
                .text_color(if invalid { t.danger } else { t.ink })
                .on_click(cx.listener(move |this, _, window, cx| {
                    // Keep the unfinished address before reopening a token for correction.
                    this.commit_address(field, cx);
                    let text = this.addresses.tokens[field].remove(at);
                    let input = this.fields()[field].clone();
                    input.update(cx, |input, cx| input.set_text(&text, cx));
                    window.focus(&input.focus_handle(cx));
                    this.save_later(cx);
                    cx.notify();
                }))
                .child(format!("{text},"))
        }).collect::<Vec<_>>();
        div().flex_1().min_w(px(0.)).relative()
            .capture_action(cx.listener(move |this, _: &Enter, window, cx| this.address_enter(field, false, window, cx)))
            .capture_action(cx.listener(move |this, _: &FocusNext, window, cx| this.address_enter(field, true, window, cx)))
            .capture_action(cx.listener(move |this, _: &Escape, _, cx| {
                if this.addresses.active == Some(field) && !this.addresses.matches.is_empty() {
                    this.addresses.matches.clear();
                    cx.stop_propagation();
                    cx.notify();
                } else { cx.propagate(); }
            }))
            .capture_action(cx.listener(move |this, _: &Backspace, _, cx| {
                if this.fields()[field].read(cx).text().is_empty() && !this.addresses.tokens[field].is_empty() {
                    this.addresses.tokens[field].pop();
                    this.save_later(cx);
                    cx.stop_propagation();
                    cx.notify();
                } else { cx.propagate(); }
            }))
            .capture_action(cx.listener(move |this, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    // Single-line TextInput replaces newlines with spaces; keep address boundaries first.
                    let text = text.replace(['\r', '\n'], ",");
                    this.fields()[field].clone().update(cx, |input, cx| input.insert(&text, false, cx));
                    this.commit_address(field, cx);
                    cx.stop_propagation();
                } else { cx.propagate(); }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let key = &event.keystroke;
                if this.addresses.active != Some(field) || this.addresses.matches.is_empty() || key.modifiers != gpui::Modifiers::none() { return; }
                let n = this.addresses.matches.len();
                match key.key.as_str() {
                    "down" => this.addresses.selected = (this.addresses.selected + 1) % n,
                    "up" => this.addresses.selected = (this.addresses.selected + n - 1) % n,
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().id(("recipient-tokens", field)).flex().flex_wrap().items_center().gap(px(4.)).py(px(6.))
                .max_h(px(96.)).overflow_y_scroll().children(tokens)
                .child(div().flex_1().min_w(px(140.)).child(input)))
            .when(open, |d| d.child(gpui::deferred(div().absolute().top_full().left_0().right_0()
                .child(div().id(("address-suggestions", field)).occlude().bg(t.raised).border_1().border_color(t.line)
                    .rounded(px(radius::CONTROL)).shadow_lg().py(px(4.))
                    .children(self.addresses.matches.iter().enumerate().map(|(at, contact)| {
                        pressable(div().id(("suggestion", at)), &t, at == self.addresses.selected)
                            .debug_selector(move || format!("suggestion-{field}-{at}"))
                            .px(px(10.)).py(px(6.)).text_color(t.ink)
                            .when(at == self.addresses.selected, |d| d.bg(t.sel))
                            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| { window.prevent_default(); cx.stop_propagation(); })
                            .on_mouse_up(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| { this.accept_address(field, at, cx); cx.stop_propagation(); }))
                            .child(contact.recipient())
                    }))
                    .with_animation(("contacts-fade", field), Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()), |d, v| d.opacity(v))))
                .with_priority(2)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use lagom_core::store::{FolderRole, Message, MessageSummary};

    struct Host(Entity<Compose>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    fn fixture() -> (Store, Account, i64) {
        let path = std::env::temp_dir().join(format!("lagom-contacts-{}-{:?}-{}.db", std::process::id(), std::thread::current().id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let store = Store::open(&path).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("self@gmail.com").unwrap();
        let account = Account::new("self@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        (store, account, folder)
    }

    fn open(cx: &mut TestAppContext) -> (Entity<Compose>, &mut VisualTestContext) {
        let (store, account, inbox) = fixture();
        let sent = store.save_folder(&account.id, "Sent", None, FolderRole::Sent).unwrap();
        for uid in 1..=8 {
            store.save_message(&Message { summary: MessageSummary {
                account: account.id.clone(), folder: sent, uid, date: uid as i64,
                from_address: account.email.clone(), to: format!("Équipe {uid} <team{uid}@example.com>"),
                ..Default::default()
            }, ..Default::default() }).unwrap();
        }
        store.save_message(&Message { summary: MessageSummary {
            account: account.id.clone(), folder: inbox, uid: 1, date: 9,
            from_name: "Anna Berg".into(), from_address: "anna@lookalike.example".into(),
            ..Default::default()
        }, ..Default::default() }).unwrap();
        let contacts = store.contacts(&account.id, 10).unwrap();
        let path = store.path().to_owned();
        drop(store);
        std::fs::remove_file(path).unwrap();
        cx.update(crate::text_input::bind_keys);
        let (host, cx) = cx.add_window_view(|window, cx| Host(cx.new(|cx| {
            let mut compose = Compose::new("self@gmail.com", "", "", None, window, cx);
            compose.addresses.contacts = contacts;
            compose
        })));
        let view = host.read_with(cx, |host, _| host.0.clone());
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn keyboard_accepts_six_suggestions_in_to_and_cc(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("e");
        assert_eq!(view.read_with(cx, |v, _| v.addresses.matches.len()), 6);
        cx.simulate_keystrokes("down up down enter");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["Équipe 7 <team7@example.com>"]);
        assert_eq!(view.read_with(cx, |v, window_cx| v.to.read(window_cx).text().to_string()), "");
        cx.simulate_input("t");
        cx.simulate_keystrokes("tab");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to.len()), 2);
        cx.simulate_keystrokes("backspace");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to.len()), 1);
        view.update_in(cx, |v, window, cx| v.add_cc(window, cx));
        cx.simulate_input("eq");
        cx.simulate_keystrokes("up tab");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).cc), ["Équipe 3 <team3@example.com>"]);
        cx.simulate_input("new@example.com");
        cx.simulate_keystrokes("enter");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).cc.len()), 2);
        assert!(cx.update(|window, cx| view.read(cx).subject.focus_handle(cx).is_focused(window)));
    }

    #[gpui::test]
    fn tab_never_picks_an_address_the_user_never_wrote_to(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("anna");
        assert_eq!(view.read_with(cx, |v, _| v.addresses.matches.iter().map(|c| c.written).collect::<Vec<_>>()), [false]);
        cx.simulate_keystrokes("tab");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["anna"]);
        view.update_in(cx, |v, window, cx| v.add_cc(window, cx));
        cx.simulate_input("anna");
        cx.simulate_keystrokes("enter");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).cc), ["Anna Berg <anna@lookalike.example>"]);
    }

    #[gpui::test]
    fn escape_dismisses_suggestions_before_preserving_the_draft(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("e");
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.addresses.matches.is_empty() && v.is_modal()));
        cx.simulate_keystrokes("escape");
        assert!(view.read_with(cx, |v, _| v.keeps_draft()));
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    }

    #[gpui::test]
    fn click_accepts_and_paste_marks_invalid_tokens_and_blocks_send(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("e");
        click(cx, "suggestion-0-1");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["Équipe 7 <team7@example.com>"]);
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("\"Team, West\" <west@example.com>; east@example.com\r\nbad@@example.com".into())));
        cx.simulate_keystrokes("secondary-v");
        assert_eq!(view.read_with(cx, |v, _| v.addresses.tokens[0].len()), 4);
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to[1].clone()), "\"Team, West\" <west@example.com>");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(cx.debug_bounds("recipient-0-3-invalid").is_some());
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| v.is_modal() && v.problem == Some("A recipient is not an email address")));
        click(cx, "recipient-0-3-invalid");
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("fixed@example.com");
        cx.simulate_keystrokes("tab");
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| v.undoable()));
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to.last().cloned()), Some("fixed@example.com".into()));
    }

    #[gpui::test]
    fn background_contacts_and_recipient_tokens_survive_draft_restore(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let (store, account, folder) = fixture();
        let store = Rc::new(store);
        store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid: 1,
            from_name: "Team West".into(), from_address: "west@example.com".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
        cx.simulate_input("west");
        view.update(cx, |v, cx| v.persist(store.clone(), account.clone(), cx));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.addresses.matches[0].address.clone()), "west@example.com");
        cx.simulate_keystrokes("enter");
        view.update_in(cx, |v, window, cx| v.add_cc(window, cx));
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("\"Team, East\" <east@example.com>; invalid".into())));
        cx.simulate_keystrokes("secondary-v");
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        let draft = store.drafts().unwrap().remove(0);
        assert_eq!(draft.to, "Team West <west@example.com>");
        assert_eq!(draft.cc, "\"Team, East\" <east@example.com>, invalid");
        for draft in [Draft { recipient_tokens: None, ..draft.clone() }, draft] {
            view.update_in(cx, |v, window, cx| {
                *v = Compose::restore(draft, account.clone(), store.clone(), window, cx);
                v.reopen(window, cx);
            });
            cx.run_until_parked();
            assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["Team West <west@example.com>"]);
            assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).cc), ["\"Team, East\" <east@example.com>", "invalid"]);
            cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
            assert!(view.read_with(cx, |v, _| v.is_modal() && v.problem.is_some()));
        }
        view.update(cx, |v, _| { v.store = None; v._save_delay = None; v._contacts = None; });
        let path = store.path().to_owned();
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    fn paste_unbalanced_tokens(cx: &mut VisualTestContext) {
        for token in ["\"Team", "alice@example.com", "West\" <west@example.com>"] {
            cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(token.into())));
            cx.simulate_keystrokes("secondary-v");
        }
    }

    #[gpui::test]
    fn separate_invalid_tokens_cannot_combine_to_bypass_send_validation(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        for field in 0..2 {
            if field == 1 {
                view.update_in(cx, |v, window, cx| {
                    v.addresses.tokens[0].clear();
                    v.add_cc(window, cx);
                });
            }
            paste_unbalanced_tokens(cx);
            assert_eq!(view.read_with(cx, |v, _| v.addresses.tokens[field].clone()), ["\"Team", "alice@example.com", "West\" <west@example.com>"]);
            cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
            assert!(view.read_with(cx, |v, _| v.is_modal() && v.problem == Some("A recipient is not an email address")));
            let mail = view.read_with(cx, |v, cx| v.outgoing(cx));
            assert_eq!(if field == 0 { mail.to } else { mail.cc }, ["\"Team", "alice@example.com", "West\" <west@example.com>"]);
        }
    }

    #[gpui::test]
    fn draft_restore_preserves_invalid_token_boundaries_and_uncommitted_addresses(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let (store, account, _) = fixture();
        let store = Rc::new(store);
        view.update(cx, |v, cx| v.persist(store.clone(), account.clone(), cx));
        paste_unbalanced_tokens(cx);
        cx.simulate_input("pending@example.com");
        view.update_in(cx, |v, window, cx| v.add_cc(window, cx));
        paste_unbalanced_tokens(cx);
        cx.simulate_keystrokes("escape");
        let draft = store.drafts().unwrap().remove(0);
        view.update_in(cx, |v, window, cx| {
            *v = Compose::restore(draft, account, store.clone(), window, cx);
            v.reopen(window, cx);
        });
        cx.run_until_parked();
        let expected = ["\"Team", "alice@example.com", "West\" <west@example.com>"];
        let mail = view.read_with(cx, |v, cx| v.outgoing(cx));
        assert_eq!(mail.to, expected.into_iter().chain(["pending@example.com"]).collect::<Vec<_>>());
        assert_eq!(mail.cc, expected);
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| v.is_modal() && v.problem == Some("A recipient is not an email address")));
        let account = view.read_with(cx, |v, _| v.account_id().unwrap().to_owned());
        assert!(store.due_outbox(&account, i64::MAX).unwrap().is_empty());
        view.update(cx, |v, _| {
            for tokens in &mut v.addresses.tokens { tokens.retain(|token| valid_recipient(token)); }
        });
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| v.undoable()));
        let queued = store.due_outbox(&account, i64::MAX).unwrap();
        assert_eq!(queued.len(), 1);
        let mail = store.pending(&queued[0]).unwrap().mail;
        assert_eq!(mail.to, ["alice@example.com", "pending@example.com"]);
        assert_eq!(mail.cc, ["alice@example.com"]);
        view.update(cx, |v, _| { v.store = None; v._save_delay = None; v._contacts = None; });
        let path = store.path().to_owned();
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn typed_commas_and_quoted_names_keep_recipient_boundaries(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_input("one@example.com,");
        cx.simulate_input("\"Team, West\"");
        assert_eq!(view.read_with(cx, |v, _| v.addresses.tokens[0].len()), 1);
        cx.simulate_input(" <west@example.com>,");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["one@example.com", "\"Team, West\" <west@example.com>"]);
        cx.simulate_keystrokes("backspace");
        assert_eq!(view.read_with(cx, |v, cx| v.outgoing(cx).to), ["one@example.com"]);
    }
}

use super::*;

pub(super) const GROUP_H: f32 = 26.;
const HINT_DELAY: Duration = Duration::from_millis(400);

#[derive(Default)]
pub(super) struct Hints {
    pub(super) over: bool,
    modifiers: gpui::Modifiers,
    pub(super) visible: bool,
    pub(super) timer: Option<Task<()>>,
}

impl MainWindow {
    pub(super) fn accounts(&self) -> Vec<Account> {
        self.live.as_ref().map_or_else(mail::demo_accounts, |live| live.accounts.clone())
    }

    pub(super) fn open_mailbox(&mut self, kind: Option<Kind>, account: Option<usize>, cx: &mut Context<Self>) {
        if let Some(ix) = account {
            self.switch_account(ix, cx);
            self.inbox.account_filter = self.accounts().get(ix).map(|a| a.id.clone());
        } else {
            self.inbox.account_filter = None;
        }
        let folder = kind.map(|kind| self.inbox.find_kind(kind).unwrap_or_else(|| {
            let name = if kind == Kind::Done { "Done" } else { "Sent" };
            self.inbox.folders.push(Folder { kind, ..Folder::new(name, None, &[]) });
            self.inbox.folders.len() - 1
        }));
        self.open_box(folder, cx);
    }

    pub(super) fn mailbox_row(&self, slot: usize, cx: &mut Context<Self>) -> Div {
        let (label, path, kind) = [("Inbox", "icons/inbox.svg", None), ("Done", "icons/done.svg", Some(Kind::Done)),
            ("Sent", "icons/sent.svg", Some(Kind::Sent))][slot];
        let t = &self.theme;
        let accounts = self.accounts();
        let multiple = accounts.len() > 1;
        let active = matches!(self.page, Page::Mail) && self.inbox.open_kind() == kind;
        let expanded = self.mailbox_expanded[slot];
        let count = if slot == 0 { self.inbox.count() } else { accounts.iter().map(|a| self.mailbox_unread(&a.id, slot)).sum() };
        let row = pressable(div().id(SharedString::from(format!("nav-{label}"))), t, active && self.inbox.account_filter.is_none())
            .debug_selector(move || format!("nav-{label}"))
            .flex().items_center().gap(px(8.)).h(px(SIDEBAR_ROW)).px(px(10.)).rounded(px(radius::CONTROL))
            .when(active && self.inbox.account_filter.is_none(), |d| d.bg(t.sel))
            .child(icon(path, if active && self.inbox.account_filter.is_none() { t.sel_icon } else { t.ink2 }, 16.))
            .child(div().flex_1().child(label))
            .when(count > 0, |d| d.child(roll(mono(count.to_string(), t.ink2), format!("{label}-{count}"))))
            .when(multiple && slot != 0, |d| d.child(pressable(div().id(("disclosure", slot)), t, false)
                .debug_selector(move || format!("disclosure-{label}"))
                .group("mailbox-disclosure").w(px(20.)).h(px(24.)).flex().items_center().justify_center()
                .child(icon(if expanded { "icons/chevron.svg" } else { "icons/chevron-right.svg" }, t.ink2, 16.)
                    .group_hover("mailbox-disclosure", { let ink = t.ink; move |s| s.text_color(ink) }))
                .tooltip({ let theme = *t; move |_, cx| cx.new(|_| Tip(theme, format!("{} {label} accounts", if expanded { "Hide" } else { "Show" }).into())).into() })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.mailbox_expanded[slot] = !this.mailbox_expanded[slot];
                    cx.notify();
                }))))
            .on_click(cx.listener(move |this, _, _, cx| this.open_mailbox(kind, None, cx)));
        div().flex().flex_col().child(row)
            .when(multiple && expanded, |d| d.children(accounts.iter().enumerate().map(|(ix, account)| {
                let selected = active && self.inbox.account_filter.as_deref() == Some(&account.id);
                let unread = self.mailbox_unread(&account.id, slot);
                pressable(div().id((SharedString::from(format!("{label}-account")), ix)), t, selected)
                    .debug_selector(move || format!("{label}-account-{ix}"))
                    .h(px(SIDEBAR_ROW)).pl(px(34.)).pr(px(10.)).flex().items_center().gap(px(8.))
                    .rounded(px(radius::CONTROL)).when(selected, |d| d.bg(t.sel))
                    .child(div().size(px(7.)).rounded_full().bg(self.account_accent(ix)).flex_shrink_0())
                    .child(div().flex_1().min_w(px(0.)).one_line().child(live::short_name(account)))
                    .when(unread > 0, |d| d.child(roll(mono(unread.to_string(), t.ink2), format!("{label}-{ix}-{unread}"))))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_mailbox(kind, Some(ix), cx)))
            })))
    }

    fn mailbox_unread(&self, account: &str, slot: usize) -> usize {
        if let Some(counts) = self.inbox.account_unread.get(account) { return counts[slot]; }
        let list = if slot == 0 { Some(&self.inbox.items) } else {
            self.inbox.find_kind(if slot == 1 { Kind::Done } else { Kind::Sent }).map(|f| &self.inbox.folders[f].items)
        };
        list.into_iter().flatten().filter(|&&id| self.inbox.mail[id].account == account && self.inbox.mail[id].unread).count()
    }

    pub(super) fn update_hints(&mut self, over: bool, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let held = over && (modifiers.platform || modifiers.shift) && matches!(self.page, Page::Mail)
            && self.palette.is_none() && self.compose.is_none();
        let changed = self.hints.over != over || self.hints.modifiers != modifiers;
        self.hints.over = over;
        self.hints.modifiers = modifiers;
        if !held {
            self.hints.timer = None;
            self.hints.visible = false;
        } else if changed || self.hints.timer.is_none() {
            self.hints.visible = false;
            self.hints.timer = Some(cx.spawn(async |this, cx| {
                cx.background_executor().timer(HINT_DELAY).await;
                this.update(cx, |this, cx| { this.hints.visible = true; cx.notify(); }).ok();
            }));
        }
        cx.notify();
    }

    pub(super) fn hint_chip(&self) -> Div {
        let keys = match (self.hints.modifiers.platform, self.hints.modifiers.shift) {
            (true, true) => "⌘ Shift", (true, false) => "⌘", _ => "Shift",
        };
        div().absolute().top(px(8.)).left(px(12.)).px(px(8.)).py(px(4.))
            .rounded(px(radius::CONTROL)).bg(self.theme.raised).border_1().border_color(self.theme.line)
            .text_size(px(size::SMALL)).text_color(self.theme.ink2).debug_selector(|| "modifier-hint".into())
            .child(format!("{keys} · {} selected", self.inbox.selected_members().len()))
    }

    pub(super) fn message_action(&mut self, id: usize, action: MessageAction, window: &mut Window, cx: &mut Context<Self>) {
        self.message_menu = None;
        match action {
            MessageAction::Menu => self.message_menu = Some(id),
            MessageAction::Close => {},
            MessageAction::ReplyAll => self.compose_kind(Some(id), ComposeKind::ReplyAll, window, cx),
            MessageAction::Forward => self.compose_kind(Some(id), ComposeKind::Forward, window, cx),
            MessageAction::Unread => self.act_on(id, Move::Read(false), window, cx),
            MessageAction::Copy => cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.inbox.mail[id].address.clone())),
            MessageAction::Move => {
                if let Some(store) = self.inbox.mail[id].store { return self.move_message(store, window, cx); }
                else if let Some(ix) = self.inbox.list().iter().position(|&rep| self.inbox.members(rep).contains(&id)) { self.inbox.select(ix); }
                self.open_palette("move ", window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn reading_menu(&self, id: usize, cx: &mut Context<Self>) -> Div {
        reading_menu(&self.theme, id, self.message_menu == Some(id), cx.listener(move |this, action: &MessageAction, window, cx| {
            this.message_action(id, *action, window, cx);
        }))
    }
}

#[derive(Clone, Copy)]
pub(super) enum MessageAction { Menu, Close, ReplyAll, Forward, Unread, Move, Copy }

pub(super) fn reading_menu(t: &Theme, id: usize, open: bool,
    action: impl Fn(&MessageAction, &mut Window, &mut App) + 'static) -> Div {
    let theme = *t;
    let action = Rc::new(action);
    let toggle = action.clone();
    div().relative().flex_shrink_0()
        .child(icon_button(t, "message-menu", "icons/more.svg", "Message actions").debug_selector(move || format!("message-menu-{id}"))
            .on_click(move |_, window, cx| toggle(&if open { MessageAction::Close } else { MessageAction::Menu }, window, cx)))
        .when(open, |d| d.child(gpui::deferred(div().absolute().right_0().top(px(32.)).w(px(180.)).p(px(4.))
            .bg(t.raised).border_1().border_color(t.line).rounded(px(radius::CONTROL)).flex().flex_col()
            .occlude()
            .children([("Reply all", "Reply all · Shift+R", MessageAction::ReplyAll), ("Forward", "Forward · F", MessageAction::Forward),
                ("Mark unread", "Mark unread · Shift+U", MessageAction::Unread), ("Move", "Move · M", MessageAction::Move),
                ("Copy address", "Copy address", MessageAction::Copy)]
                .into_iter().map(|(label, tip, run)| {
                    let action = action.clone();
                    pressable(div().id(label).group(label), t, false).debug_selector(move || format!("menu-{label}"))
                        .h(px(28.)).px(px(8.)).flex().items_center().gap(px(8.)).rounded(px(radius::KEY))
                        .child(icon(action_icon(label).unwrap(), t.ink2, 16.).group_hover(label, |s| s.text_color(theme.ink)))
                        .child(label).tooltip(move |_, cx| cx.new(|_| Tip(theme, tip.into())).into())
                        .on_click(move |_, window, cx| action(&run, window, cx))
                }))).priority(1)))
}

pub(super) fn sync_icon(state: &crate::sync::SyncState) -> &'static str {
    use crate::sync::SyncState;
    match state {
        SyncState::Synced(_) => "icons/cloud-synced.svg",
        SyncState::Failed(_) | SyncState::ReauthRequired => "icons/cloud-failed.svg",
        _ => "icons/cloud-syncing.svg",
    }
}

pub(super) fn action_icon(label: &str) -> Option<&'static str> {
    Some(match label {
        "Reply" => "icons/reply.svg", "Reply all" => "icons/reply-all.svg", "Forward" => "icons/forward.svg",
        "Later" => "icons/later.svg", "Done" => "icons/done.svg", "Move to Inbox" => "icons/restore.svg",
        "Move" => "icons/move.svg", "Mark unread" => "icons/unread.svg", "Mark read" => "icons/read.svg",
        "Copy address" => "icons/copy.svg", "Load remote images" => "icons/image.svg", "Load more" => "icons/load-more.svg",
        "Delete" => "icons/trash.svg", "Empty Trash" | "Delete all" => "icons/empty-trash.svg", "Cancel" => "icons/close.svg",
        "Undo" => "icons/undo.svg", "Send again" => "icons/retry-send.svg", "Edit draft" => "icons/edit-draft.svg",
        "New task" | "Add task" => "icons/plus.svg", "New list" | "Add list" => "icons/new-list.svg",
        "Rename" => "icons/rename.svg", "Delete list" => "icons/delete-list.svg", "Open mail" => "icons/linked-mail.svg",
        "New event" => "icons/new-event.svg", "Today" => "icons/calendar.svg", "Join" | "Join meeting" => "icons/camera.svg",
        "Open in Google Calendar" => "icons/external.svg", "Close" => "icons/close.svg",
        "Add account" => "icons/account-add.svg", "Remove" => "icons/account-remove.svg", "Keep" | "Cancel sign-in" => "icons/close.svg",
        "Show your Google Calendar" | "Allow Google Tasks access" => "icons/access.svg",
        "Send" => "icons/send.svg", "New message" => "icons/compose.svg", "Insert file…" => "icons/insert-file.svg",
        "Sync now" => "icons/cloud-syncing.svg", "Settings" => "icons/settings.svg", "Connect" => "icons/connect.svg",
        "Connecting" => "icons/cloud-syncing.svg",
        _ => return None,
    })
}

pub(super) fn shortcut_button(t: &Theme, label: &'static str, fill: Option<Rgba>, hints: bool) -> Stateful<Div> {
    let shortcut = match label { "Reply" => Some(0), "Forward" => Some(0), "Later" => Some(0), "Done" => Some(3),
        "Move" | "Move to Inbox" => Some(0), "Mark unread" => Some(5), _ => None };
    button_label(t, label, fill, if hints { shortcut } else { None })
}

impl Inbox {
    pub(super) fn adjust_account_unread(&mut self, id: usize, kind: Option<Kind>, up: bool) {
        let slot = match kind { None => 0, Some(Kind::Done) => 1, Some(Kind::Sent) => 2, _ => return };
        if let Some(counts) = self.account_unread.get_mut(&self.mail[id].account) {
            counts[slot] = if up { counts[slot] + 1 } else { counts[slot].saturating_sub(1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn mail_action_icons_are_embedded_distinct_and_use_house_strokes() {
        use gpui::AssetSource;
        let mut paths = HashSet::new();
        let mut drawings = HashSet::new();
        for label in ["Reply", "Reply all", "Forward", "Later", "Done", "Move to Inbox", "Move", "Mark unread", "Mark read",
            "Copy address", "Load remote images", "Load more", "Delete", "Empty Trash", "Undo", "Send again", "Edit draft",
            "New task", "Add list", "Rename", "Delete list", "Open mail", "Today", "Join", "Open in Google Calendar", "Close",
            "Add account", "Remove", "Show your Google Calendar", "Send", "New message", "Insert file…", "Sync now", "Settings", "Connect"] {
            let path = action_icon(label).unwrap();
            assert!(paths.insert(path), "{label} must have its own meaning");
            let bytes = crate::Assets.load(path).unwrap().unwrap();
            let svg = std::str::from_utf8(&bytes).unwrap();
            assert!(svg.contains("viewBox=\"0 0 16 16\"") && svg.contains("stroke-width=\"1.5\""));
            assert!(drawings.insert(svg.to_string()), "{label} must have its own drawing");
        }
        assert_eq!(action_icon("Cancel"), action_icon("Close"));
        for path in ["icons/more.svg", "icons/attachment.svg"] {
            assert!(crate::Assets.load(path).unwrap().is_some());
        }
    }

    #[test]
    fn stroke_icons_set_their_own_ink_in_both_themes() {
        for t in [Theme::light(DEFAULT_ACCENT), Theme::dark(DEFAULT_ACCENT)] {
            let mut glyph = icon("icons/linked-mail.svg", t.ink2, 16.);
            assert_eq!(glyph.style().text.as_ref().unwrap().color, Some(t.ink2.into()));
        }
    }

    #[test]
    fn sync_states_have_distinct_icons_without_relying_on_color() {
        use crate::sync::SyncState;
        assert_eq!(sync_icon(&SyncState::Synced(1)), "icons/cloud-synced.svg");
        assert_eq!(sync_icon(&SyncState::Failed("fixture".into())), "icons/cloud-failed.svg");
        assert_eq!(sync_icon(&SyncState::ReauthRequired), "icons/cloud-failed.svg");
        assert_eq!(sync_icon(&SyncState::Syncing), "icons/cloud-syncing.svg");
        assert_eq!(sync_icon(&SyncState::SigningIn), "icons/cloud-syncing.svg");
    }

    #[gpui::test]
    fn review_task_and_palette_actions_show_sixteen_pixel_icons(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        view.update_in(cx, |v, window, cx| v.show_section(3, window, cx));
        cx.run_until_parked();
        for path in ["plus", "more", "mail", "settings"] {
            let bounds = cx.debug_bounds(format!("icon-icons/{path}.svg").leak()).unwrap();
            assert_eq!(bounds.size, gpui::size(px(16.), px(16.)), "{path}");
        }
        view.update_in(cx, |v, window, cx| v.open_palette("New task", window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("icon-icons/plus.svg").is_some());
    }

    #[gpui::test]
    fn review_sender_address_is_visible_in_main_popout_and_expanded_message(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        view.update(cx, |v, cx| {
            v.inbox.mail[0].from = "Bank".into();
            v.inbox.mail[0].address = "attacker@example.test".into();
            cx.notify();
        });
        cx.run_until_parked();
        let address = cx.debug_bounds("sender-address-attacker@example.test").expect("show the actual sender below the display name");
        assert!(address.size.width > px(0.) && address.size.height > px(0.));
        cx.simulate_keystrokes("secondary-o");
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        let mut popup = gpui::VisualTestContext::from_window(popout.into(), cx);
        popup.simulate_resize(gpui::size(px(420.), px(800.)));
        popup.run_until_parked();
        let address = popup.debug_bounds("sender-address-attacker@example.test").unwrap();
        assert!(address.left() >= px(0.) && address.right() <= px(420.));
        view.update(cx, |v, cx| {
            v.inbox.mail[0].thread_id = "fixture-thread".into();
            v.inbox.mail[1].thread_id = "fixture-thread".into();
            v.inbox.mail[1].from = "Bank".into();
            v.inbox.mail[1].address = "other@example.test".into();
            v.inbox.regroup();
            v.reading_thread = None;
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            if !v.expanded.contains_key(&1) { v.toggle_message(1, true, cx); }
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("sender-address-other@example.test").is_some(), "expanded messages retain their own address");
    }

    #[gpui::test]
    fn mail_icons_fit_a_narrow_popout_and_move_shortcut_targets_its_message(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.simulate_keystrokes("secondary-o j");
        let popout = view.read_with(cx, |v, _| v.popouts[0].1);
        let mut popup = gpui::VisualTestContext::from_window(popout.into(), cx);
        popup.simulate_resize(gpui::size(px(420.), px(800.)));
        popup.run_until_parked();
        for label in ["Reply", "reply-all", "Forward", "Later", "Done"] {
            let bounds = popup.debug_bounds(label).unwrap();
            assert!(bounds.left() >= px(0.) && bounds.right() <= px(420.), "{label} must fit the toolbar");
        }
        for path in ["reply", "reply-all", "forward", "later", "done", "more"] {
            let bounds = popup.debug_bounds(format!("icon-icons/{path}.svg").leak()).unwrap();
            assert_eq!(bounds.size, gpui::size(px(16.), px(16.)));
        }
        popup.simulate_keystrokes("m");
        view.read_with(cx, |v, cx| {
            assert_eq!(v.inbox.current(), Some(0));
            assert_eq!(v.palette.as_ref().unwrap().read(cx).query(cx), "move ");
        });
    }

    #[gpui::test]
    fn unified_launch_uses_two_demo_accounts_without_a_hint(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "unified", window, cx));
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(v.live.is_none() && v.preferences_path.is_none());
            assert_eq!(v.mailbox_expanded, [true, true, true]);
            assert_eq!(v.accounts().iter().map(|a| a.display_name.as_str()).collect::<Vec<_>>(), ["Personal", "Work"]);
            assert!(!v.hints.visible && v.hints.timer.is_none());
        });
        for label in ["Inbox-account-1", "Done-account-1", "Sent-account-1", "message-card"] {
            assert!(cx.debug_bounds(label).is_some());
        }
    }

    #[gpui::test]
    fn unified_demo_filters_accounts_and_disclosures_keep_selection(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("Inbox-account-0").is_some());
        assert!(cx.debug_bounds("Done-account-0").is_none());
        assert!(cx.debug_bounds("Sent-account-0").is_none());
        for (label, kind) in [("Inbox", None), ("Done", Some(Kind::Done)), ("Sent", Some(Kind::Sent))] {
            if kind.is_some() {
                let at = cx.debug_bounds(format!("disclosure-{label}").leak()).unwrap().center();
                let before = view.read_with(cx, |v, _| v.inbox.current());
                cx.simulate_click(at, Default::default());
                assert_eq!(view.read_with(cx, |v, _| v.inbox.current()), before);
                cx.run_until_parked();
            }
            for ix in 0..2 {
                let at = cx.debug_bounds(format!("{label}-account-{ix}").leak()).unwrap().center();
                cx.simulate_click(at, Default::default());
                view.read_with(cx, |v, _| {
                    assert_eq!(v.inbox.open_kind(), kind);
                    assert!(!v.inbox.list().is_empty());
                    assert!(v.inbox.list().iter().all(|&id| v.inbox.mail[id].account == v.accounts()[ix].id));
                });
            }
            let at = cx.debug_bounds(format!("nav-{label}").leak()).unwrap().center();
            cx.simulate_click(at, Default::default());
            view.read_with(cx, |v, _| {
                assert!(v.inbox.account_filter.is_none());
                assert_eq!(v.inbox.list().iter().map(|&id| &v.inbox.mail[id].account).collect::<HashSet<_>>().len(), 2);
            });
        }
    }

    #[gpui::test]
    fn modifier_hint_waits_and_release_cancels_it(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert!(cx.debug_bounds("modifier-hint").is_none());
        for modifiers in [gpui::Modifiers { platform: true, ..Default::default() },
            gpui::Modifiers { shift: true, ..Default::default() }, gpui::Modifiers { platform: true, shift: true, ..Default::default() }] {
            view.update(cx, |v, cx| v.update_hints(true, modifiers, cx));
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(399));
            cx.run_until_parked();
            assert!(!view.read_with(cx, |v, _| v.hints.visible));
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            assert!(cx.debug_bounds("modifier-hint").is_some());
            cx.simulate_modifiers_change(Default::default());
            assert!(!view.read_with(cx, |v, _| v.hints.visible));
            cx.run_until_parked();
            // GPUI retains debug bounds after an element disappears; assert the rendering gate and cancelled timer.
            assert!(view.read_with(cx, |v, _| !v.hints.visible && v.hints.timer.is_none()));
        }
        view.update(cx, |v, cx| v.update_hints(true, gpui::Modifiers { shift: true, ..Default::default() }, cx));
        cx.run_until_parked();
        view.update(cx, |v, cx| v.update_hints(false, Default::default(), cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| !v.hints.visible && v.hints.timer.is_none()));
    }

    #[gpui::test]
    fn reading_card_menu_and_keyboard_skip_date_headers(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.run_until_parked();
        let card = cx.debug_bounds("message-card").unwrap();
        assert!(cx.debug_bounds("reading-subject").unwrap().bottom() <= card.top());
        assert!(cx.debug_bounds("date-group-0").is_some());
        assert!(cx.debug_bounds("account-bar-0").is_some());
        cx.simulate_keystrokes("j j j");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.current()), Some(3));
        cx.run_until_parked();
        let at = cx.debug_bounds("message-menu-3").unwrap().center();
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        for label in ["Reply all", "Forward", "Mark unread", "Move", "Copy address"] {
            assert!(cx.debug_bounds(format!("menu-{label}").leak()).is_some());
        }
        let at = cx.debug_bounds("menu-Copy address").unwrap().center();
        cx.simulate_click(at, Default::default());
        let address = view.read_with(cx, |v, _| v.inbox.mail[3].address.clone());
        cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), address));
        assert!(view.read_with(cx, |v, _| v.message_menu.is_none()));
        view.update(cx, |v, cx| { v.inbox.mail[3].unread = false; cx.notify(); });
        view.update_in(cx, |v, window, cx| v.message_action(3, MessageAction::Unread, window, cx));
        assert!(view.read_with(cx, |v, _| v.inbox.mail[3].unread));
        assert!(include_str!("../../../../scripts/smoke.sh").contains("inbox unified"));
    }

    #[test]
    fn date_header_layout_includes_headers_without_selectable_rows() {
        let layout = RowLayout { len: 4, headers: vec![0, 2], changes: vec![] };
        assert_eq!(layout.y(0), LIST_PAD + GROUP_H);
        assert_eq!(layout.y(2), row_y(2) + 2. * GROUP_H);
        assert_eq!(layout.bottom(), row_y(4) + 2. * GROUP_H);
        assert!(layout.visible(layout.y(2), ROW_H).contains(&2));
    }
}

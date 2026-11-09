use super::*;

impl Inbox {
    pub(super) fn is_selected(&self, id: usize) -> bool {
        self.chosen.as_ref().map_or(self.current() == Some(id), |ids| ids.contains(&id))
    }

    pub(super) fn selected_rows(&self) -> Vec<usize> {
        self.list().iter().copied().filter(|&id| self.is_selected(id)).collect()
    }

    pub(super) fn selected_members(&self) -> Vec<usize> {
        self.selected_rows().into_iter().flat_map(|id| self.members(id)).collect()
    }

    pub(super) fn single(&self) -> Option<usize> {
        match &self.chosen {
            Some(ids) if ids.len() == 1 => ids.iter().next().copied(),
            Some(_) => None,
            None => self.current(),
        }
    }

    pub(super) fn selection_unread(&self) -> bool {
        self.selected_members().iter().any(|&id| self.mail[id].unread)
    }

    pub(super) fn multi(&self) -> bool {
        self.chosen.as_ref().is_some_and(|ids| ids.len() > 1)
    }

    pub(super) fn click(&mut self, ix: usize, shift: bool, toggle: bool) {
        let Some(&id) = self.list().get(ix) else { return };
        if shift {
            let current = self.current().unwrap_or(id);
            let anchor = *self.anchor.get_or_insert(current);
            let start = self.list().iter().position(|&rep| self.members(rep).contains(&anchor)).unwrap_or(self.selected);
            self.chosen = Some(self.list()[start.min(ix)..=start.max(ix)].iter().copied().collect());
            self.selected = ix;
        } else if toggle {
            let mut chosen = self.selected_rows().into_iter().collect::<HashSet<_>>();
            if !chosen.remove(&id) { chosen.insert(id); }
            self.chosen = Some(chosen);
            self.selected = ix;
            self.anchor = Some(id);
        } else {
            self.select(ix);
        }
    }

    pub(super) fn extend(&mut self, delta: isize) {
        let ix = self.selected.saturating_add_signed(delta).min(self.list().len().saturating_sub(1));
        self.click(ix, true, false);
    }

    pub(super) fn select_all(&mut self) {
        self.anchor = self.current();
        self.chosen = Some(self.list().iter().copied().collect());
    }

    pub(super) fn act_rows(&mut self, rows: &[usize], what: Move) -> Option<Vec<usize>> {
        let keep = self.current();
        let selected = self.selected;
        let members = rows.iter().flat_map(|&id| self.members(id)).collect::<Vec<_>>();
        let mut moved = Vec::new();
        for member in members {
            let Some(at) = self.raw_list().iter().position(|&id| id == member) else { continue };
            if self.act_one(at, what).is_some() { moved.push(member); }
        }
        if moved.is_empty() { return None; }
        self.batches.push(moved.clone());
        self.regroup();
        self.selected = keep.and_then(|keep| self.list().iter().position(|&rep| self.members(rep).contains(&keep)))
            .unwrap_or(selected).min(self.list().len().saturating_sub(1));
        if self.chosen.as_ref().is_some_and(|ids| ids.is_empty()) { self.chosen = None; }
        Some(moved)
    }

    pub(super) fn set_unread(&mut self, id: usize, unread: bool) {
        let before = self.mail[id].unread;
        self.mail[id].unread = unread;
        if before == unread { return; }
        if self.items.contains(&id) { self.adjust_account_unread(id, None, unread); }
        else if let Some(kind) = self.folders.iter().find(|f| f.items.contains(&id)
            || f.store.is_some_and(|s| self.mail[id].folder == mail::Place::Folder(s))).map(|f| f.kind) {
            self.adjust_account_unread(id, Some(kind), unread);
        }
        for folder in &mut self.folders {
            if folder.items.contains(&id) || folder.store.is_some_and(|store| self.mail[id].folder == mail::Place::Folder(store)) {
                folder.unread = if unread { folder.unread + 1 } else { folder.unread.saturating_sub(1) };
            }
        }
    }
}

impl MainWindow {
    pub(super) fn act_selected(&mut self, what: Move, when: LaterWhen, cx: &mut Context<Self>) {
        let rows = self.inbox.selected_rows();
        if rows.len() <= 1 {
            if let Some(ix) = rows.first().and_then(|id| self.inbox.list().iter().position(|rep| rep == id)) {
                self.act_with(ix, what, 0., when, cx);
            }
            return;
        }
        if self.emptying_trash() { return; }
        let Some(members) = self.inbox.act_rows(&rows, what) else { return };
        self.clear_motion();
        let mut destinations = HashMap::new();
        let mut sent = true;
        for member in members {
            sent &= self.server(what, member, when, &mut destinations, cx);
            if !matches!(what, Move::Read(_)) { self.close_popouts(Some(member), cx); }
        }
        if !sent { return; }
        self.notify(match what {
            Move::Read(true) => "Marked read",
            Move::Read(false) => "Marked unread",
            Move::Delete => "Deleted",
            Move::Later => "Moved to Later",
            _ => "Moved",
        }, cx);
        if let Some(toast) = &mut self.toast { toast.1 = true; }
        cx.notify();
    }

    pub(super) fn toggle_read(&mut self, cx: &mut Context<Self>) {
        let seen = self.inbox.selection_unread();
        self.act(Move::Read(seen), cx);
    }

    pub(super) fn filter_unread(&mut self, unread: bool, cx: &mut Context<Self>) {
        self.release(false, cx);
        self.clear_motion();
        // shortcut: filter loaded pages; use indexed unread queries when large filtered mailboxes need direct paging.
        self.inbox.unread_only = unread;
        self.inbox.regroup();
        self.inbox.select(self.inbox.selected);
        self.scroll.set_offset(Point::default());
        cx.notify();
    }

    pub(super) fn selection_pane(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let members = self.inbox.selected_members();
        let seen = self.inbox.selection_unread();
        let trash = self.inbox.open_kind() == Some(Kind::Trash);
        // A small stack of cards, one per selected row up to three: the front card names the first message.
        let rows = self.inbox.selected_rows();
        let depth = rows.len().clamp(1, 3);
        let mut stack = div().relative().w(px(300.)).h(px(84. + 12. * (depth - 1) as f32));
        for d in (0..depth).rev() {
            let inset = 18. * d as f32;
            let mut card = div().absolute().top(px(12. * (depth - 1 - d) as f32)).left(px(inset)).right(px(inset)).h(px(84.))
                .rounded(px(radius::CARD)).border_1().border_color(t.line).bg(t.raised).shadow_md();
            if d == 0 {
                if let Some(m) = rows.first().map(|&id| &self.inbox.mail[id]) {
                    card = card.flex().flex_col().justify_center().gap(px(2.)).px(px(16.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.ink).one_line().child(m.from.clone()))
                        .child(div().text_color(t.ink2).one_line().child(m.subject.clone()));
                }
            } else {
                card = card.opacity(1. - 0.25 * d as f32);
            }
            stack = stack.child(card);
        }
        let summary = div().id("selection-summary").flex_1().flex().flex_col().items_center().justify_center().gap(px(16.))
            .child(stack.with_animation(("selection-stack", depth), Animation::new(Duration::from_millis(180)), |el, v| {
                el.top(px(6. * (1. - spring(v))))
            }))
            .child(div().font_weight(FontWeight::MEDIUM).text_color(t.ink).child(format!("{} messages selected", members.len())))
            .child(mono("E done  ·  L later  ·  ⌫ delete", t.ink2));
        #[cfg(test)]
        let summary = summary.debug_selector(move || format!("{} messages selected", members.len()));
        div().flex_1().min_w(px(0.)).h_full().flex().flex_col()
            .child(toolbar(t).flex_wrap().h_auto().min_h(px(HEADER_H)).py(px(8.)).px(px(16.))
                .when(!trash, |d| d
                    .child(polish::shortcut_button(t, "Later", None, self.hints.visible).on_click(cx.listener(|this, _, _, cx| this.act(Move::Later, cx))))
                    .child(button(t, "Delete", None).on_click(cx.listener(|this, _, _, cx| this.act(Move::Delete, cx)))))
                .child(polish::shortcut_button(t, "Move", None, self.hints.visible).on_click(cx.listener(|this, _, window, cx| this.open_palette("move ", window, cx))))
                .child(polish::shortcut_button(t, if seen { "Mark read" } else { "Mark unread" }, None, self.hints.visible)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_read(cx))))
                .child(div().flex_1())
                .child(polish::shortcut_button(t, if trash { "Move to Inbox" } else { "Done" }, Some(t.accent), self.hints.visible)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(if trash { Move::Inbox } else { Move::Done }, cx)))))
            .child(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn review_thread_read_shortcut_matches_aggregate_action(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        view.update(cx, |v, cx| {
            v.inbox.mail[0].thread_id = "thread".into();
            v.inbox.mail[1].thread_id = "thread".into();
            v.inbox.mail[0].unread = true;
            v.inbox.mail[1].unread = false;
            v.inbox.mail[1].date = v.inbox.mail[0].date + 1;
            v.inbox.group = true;
            v.inbox.regroup();
            v.inbox.select(0);
            assert_eq!(v.inbox.current(), Some(1));
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(px(MIN_WINDOW.0), px(MIN_WINDOW.1)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("Mark read").is_none());
        cx.simulate_keystrokes("shift-u");
        assert!(view.read_with(cx, |v, _| v.inbox.selected_members().iter().all(|&id| !v.inbox.mail[id].unread)));
    }

    #[test]
    fn shift_arrows_extend_shrink_and_cross_anchor() {
        let mut inbox = Inbox::new(6, 0);
        inbox.select(2);
        inbox.extend(1);
        inbox.extend(1);
        assert_eq!(inbox.selected_rows(), [2, 3, 4]);
        inbox.extend(-1);
        assert_eq!(inbox.selected_rows(), [2, 3]);
        inbox.extend(-1);
        inbox.extend(-1);
        assert_eq!(inbox.selected_rows(), [1, 2]);
        assert_eq!(inbox.current(), Some(1));
        inbox.prev();
        assert_eq!(inbox.selected_rows(), [0]);
        inbox.select_all();
        inbox.prev();
        assert_eq!(inbox.selected_rows(), [0]);
    }

    #[test]
    fn shift_click_uses_anchor_and_plain_click_collapses() {
        let mut inbox = Inbox::new(6, 0);
        inbox.click(1, false, false);
        inbox.click(4, true, false);
        assert_eq!(inbox.selected_rows(), [1, 2, 3, 4]);
        inbox.click(0, true, false);
        assert_eq!(inbox.selected_rows(), [0, 1]);
        inbox.click(1, false, false);
        assert_eq!(inbox.selected_rows(), [1]);
        assert_eq!(inbox.anchor, Some(1));
    }

    #[test]
    fn cmd_click_toggles_without_losing_cursor() {
        let mut inbox = Inbox::new(5, 0);
        inbox.click(3, false, true);
        assert_eq!(inbox.selected_rows(), [0, 3]);
        inbox.click(0, false, true);
        assert_eq!(inbox.selected_rows(), [3]);
        assert_eq!(inbox.current(), Some(0));
        inbox.click(3, false, true);
        assert!(inbox.selected_rows().is_empty());
        inbox.extend(-1);
        assert_eq!(inbox.selected_rows(), [2, 3]);
    }

    #[test]
    fn selection_and_anchor_survive_new_rows_and_new_thread_representatives() {
        use lagom_core::store::MessageSummary;
        let row = |id, thread: &str| MessageSummary { id, date: id, thread_id: thread.into(), ..Default::default() };
        let snap = |inbox| mail::Snapshot { inbox, ..Default::default() };
        for group in [false, true] {
            let mut inbox = Inbox::live();
            inbox.group = group;
            inbox.apply(snap(vec![row(3, "c"), row(2, "b"), row(1, "a")]), &HashSet::new(), chrono::Local::now());
            inbox.select(1);
            inbox.extend(1);
            let anchor = inbox.anchor;
            inbox.apply(snap(vec![row(5, "b"), row(4, "d"), row(3, "c"), row(2, "b"), row(1, "a")]), &HashSet::new(), chrono::Local::now());
            assert_eq!(inbox.mail[inbox.current().unwrap()].store, Some(1));
            assert_eq!(inbox.anchor, anchor);
            let selected = inbox.selected_members().into_iter().map(|id| inbox.mail[id].store.unwrap()).collect::<HashSet<_>>();
            assert_eq!(selected, if group { HashSet::from([1, 2, 5]) } else { HashSet::from([1, 2]) });
            inbox.extend(-1);
            assert!(inbox.selected_members().iter().any(|&id| inbox.mail[id].store == Some(2)));
            assert!(!inbox.selected_members().iter().any(|&id| inbox.mail[id].store == Some(1)));
        }
    }

    #[test]
    fn multi_actions_and_one_undo_restore_every_member_in_order() {
        for group in [false, true] {
            for what in [Move::Done, Move::Later, Move::Delete, Move::Folder(0)] {
                let mut inbox = Inbox::demo(5);
                inbox.mail[0].thread_id = "thread".into();
                inbox.mail[1].thread_id = "thread".into();
                inbox.group = group;
                inbox.regroup();
                let before = inbox.items.clone();
                inbox.select(0);
                inbox.extend(1);
                let members = inbox.selected_members();
                let rows = inbox.selected_rows();
                inbox.act_rows(&rows, what).unwrap();
                assert!(members.iter().all(|id| !inbox.items.contains(id)));
                assert_eq!(inbox.batches.len(), 1);
                inbox.undo().unwrap();
                assert_eq!(inbox.items, before);
                assert!(inbox.batches.is_empty() && inbox.history.is_empty());
                assert_eq!(inbox.selected_members().into_iter().collect::<HashSet<_>>(), members.into_iter().collect());
            }
        }
    }

    #[test]
    fn unread_filter_uses_all_thread_members_and_select_all_uses_shown_rows() {
        let mut inbox = Inbox::new(5, 0);
        for m in &mut inbox.mail { m.unread = false; }
        inbox.mail[0].unread = true;
        inbox.mail[3].unread = true;
        inbox.mail[0].thread_id = "thread".into();
        inbox.mail[1].thread_id = "thread".into();
        inbox.mail[1].date = 1;
        inbox.unread_only = true;
        inbox.regroup();
        assert_eq!(inbox.list(), &[0, 3]);
        inbox.select_all();
        assert_eq!(inbox.selected_members(), [0, 3]);
        inbox.group = true;
        inbox.regroup();
        assert_eq!(inbox.list(), &[1, 3]);
        inbox.select_all();
        assert_eq!(inbox.selected_members().into_iter().collect::<HashSet<_>>(), HashSet::from([0, 1, 3]));
        inbox.act_rows(&inbox.selected_rows(), Move::Read(true));
        assert!(inbox.list().is_empty());
        inbox.undo().unwrap();
        assert_eq!(inbox.list(), &[1, 3]);
        assert!(!inbox.mail[1].unread);
    }

    #[test]
    fn read_undo_preserves_mixed_flags_and_failed_member_leaves_batch() {
        let mut inbox = Inbox::new(4, 0);
        inbox.mail[0].unread = true;
        inbox.mail[1].unread = false;
        inbox.mail[2].unread = true;
        inbox.select_all();
        let before = inbox.mail.iter().map(|m| m.unread).collect::<Vec<_>>();
        inbox.act_rows(&inbox.selected_rows(), Move::Read(false));
        assert!(inbox.selected_members().iter().all(|&id| inbox.mail[id].unread));
        inbox.undo().unwrap();
        assert_eq!(inbox.mail.iter().map(|m| m.unread).collect::<Vec<_>>(), before);
        inbox.select_all();
        inbox.act_rows(&inbox.selected_rows(), Move::Read(true));
        inbox.revert(0, None);
        assert!(inbox.mail[0].unread);
        assert!(!inbox.batches.last().unwrap().contains(&0));
        inbox.undo().unwrap();
        assert_eq!(inbox.mail.iter().map(|m| m.unread).collect::<Vec<_>>(), before);
    }

    #[gpui::test]
    fn keys_select_done_and_undo_as_one_step(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        let before = view.read_with(cx, |v, _| v.inbox.items.clone());
        cx.simulate_keystrokes("shift-down shift-down");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [0, 1, 2]);
        cx.simulate_keystrokes("e");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.items.len()), before.len() - 3);
        cx.simulate_keystrokes("secondary-z");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.items.clone()), before);
        assert!(view.read_with(cx, |v, _| v.inbox.batches.is_empty()));
        cx.simulate_keystrokes("secondary-a");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows().len()), before.len());
        let cursor = view.read_with(cx, |v, _| v.inbox.current().unwrap());
        cx.simulate_keystrokes("escape");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [cursor]);
        cx.simulate_keystrokes("shift-down up");
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [cursor]);
    }

    #[gpui::test]
    fn mouse_modifiers_and_swipe_keep_their_scope(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        cx.run_until_parked();
        let click = |cx: &mut gpui::VisualTestContext, selector, modifiers| {
            let at = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_click(at, modifiers);
        };
        click(cx, "row-1", Default::default());
        click(cx, "row-3", gpui::Modifiers { shift: true, ..Default::default() });
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [1, 2, 3]);
        click(cx, "row-2", gpui::Modifiers::secondary_key());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [1, 3]);
        view.update(cx, |v, cx| {
            v.swipe = Swipe::Row { id: 0, dx: THRESHOLD + 1. };
            v.release(true, cx);
            assert!(!v.inbox.items.contains(&0));
            assert_eq!(v.inbox.selected_rows(), [1, 3]);
            v.undo(cx);
            v.clear_motion();
        });
        click(cx, "row-3", Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.selected_rows()), [3]);
    }
    #[gpui::test]
    fn unread_header_stays_available_when_filter_is_empty(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        view.update(cx, |v, cx| {
            for m in &mut v.inbox.mail { m.unread = false; }
            cx.notify();
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("filter-unread").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(view.read_with(cx, |v, _| v.inbox.unread_only && v.inbox.list().is_empty()));
        let at = cx.debug_bounds("filter-all").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(view.read_with(cx, |v, _| !v.inbox.unread_only && !v.inbox.list().is_empty()));
    }

    #[gpui::test]
    fn summary_counts_thread_members_and_toolbar_buttons_act_on_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = super::super::dispatch::open(cx);
        view.update(cx, |v, cx| {
            v.inbox.mail[0].thread_id = "thread".into();
            v.inbox.mail[1].thread_id = "thread".into();
            v.inbox.regroup();
            v.inbox.select(0);
            v.inbox.extend(1);
            assert_eq!(v.inbox.selected_members().len(), 3);
            cx.notify();
        });
        cx.simulate_resize(gpui::size(px(MIN_WINDOW.0), px(MIN_WINDOW.1)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("3 messages selected").is_some());
        let done = cx.debug_bounds("Done").unwrap();
        for button in ["Done", "Later", "Delete", "Move", "Mark read"] {
            let bounds = cx.debug_bounds(button).unwrap();
            let summary = cx.debug_bounds("3 messages selected").unwrap();
            assert!(bounds.left() >= summary.left() && bounds.right() <= summary.right());
            assert!(bounds.top() < done.top() || bounds.right() <= done.right(), "Done must sit last");
        }
        let before = view.read_with(cx, |v, _| v.inbox.items.len());
        let at = cx.debug_bounds("Done").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.inbox.items.len()), before - 3);
    }

}

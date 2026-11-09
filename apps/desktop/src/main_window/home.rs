use super::*;

impl MainWindow {
    pub(super) fn show_section(&mut self, section: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        match section {
            0 => self.show_home(window, cx),
            1 => self.open_box(None, cx),
            2 | 3 => {
                self.release(false, cx);
                self.clear_motion();
                self.page = if section == 2 { Page::Calendar } else { Page::Todo };
                self.reload_tasks(cx);
            }
            _ => return,
        }
        self.sidebar_scroll.scroll_to_item(0);
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn reload_unread(&mut self, cx: &mut Context<Self>) {
        if let Some(live) = &self.live {
            let pending = self.pending_moves();
            match live.store.unread_mail(chrono::Local::now().timestamp(), 5u32.saturating_add(pending.len() as u32)) {
                Ok(messages) => self.today.unread = messages.iter().filter(|m| !pending.contains(&m.id)).take(5)
                    .map(|m| Mail::from_summary(m, chrono::Local::now())).collect(),
                Err(e) => self.notify(format!("Could not read unread mail: {e}"), cx),
            }
        } else {
            let mut unread: Vec<_> = self.inbox.items.iter().map(|&id| &self.inbox.mail[id]).filter(|m| m.unread).cloned().collect();
            unread.sort_by_key(|m| std::cmp::Reverse(m.date));
            unread.truncate(5);
            self.today.unread = unread;
        }
    }

    pub(super) fn unread_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        div().id("home-unread").debug_selector(|| "home-unread".into()).flex().flex_col().flex_shrink_0().gap(px(4.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Unread mail"))
            .when(self.today.unread.is_empty(), |d| d.child(mono("All caught up", t.ink2)))
            .children(self.today.unread.iter().enumerate().map(|(ix, mail)| {
                let mail = mail.clone();
                pressable(div().id(("home-unread", ix)), &t, false).debug_selector(move || format!("unread-mail-{ix}"))
                    .flex().items_center().gap(px(8.)).px(px(8.)).h(px(30.)).rounded(px(radius::CONTROL))
                    .child(div().size(px(5.)).rounded_full().bg(t.accent).flex_shrink_0())
                    .child(div().w(px(120.)).one_line().child(mail.from.clone()))
                    .child(div().flex_1().min_w(px(0.)).one_line().child(mail.subject.clone()))
                    .child(mono(mail.time.clone(), t.ink2))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = mail.store {
                            this.open_notification(crate::notifications::Target { account: mail.account.clone(), message: Some(id) }, window, cx);
                        } else {
                            let target = this.inbox.items.iter().position(|&id| this.inbox.mail[id].subject == mail.subject);
                            this.open_box(None, cx);
                            if let Some(ix) = target { this.select(|inbox| inbox.select(ix), cx); }
                        }
                        window.focus(&this.focus);
                    }))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn review_section_icons_are_embedded_and_distinct() {
        use gpui::AssetSource;
        let icons: HashSet<_> = SECTIONS.iter().map(|(_, path, _)| {
            let bytes = crate::Assets.load(path).unwrap().expect("sidebar icon must be embedded");
            let svg = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(svg.contains("viewBox=\"0 0 16 16\""));
            assert!(svg.contains("stroke-width=\"1.5\""));
            svg
        }).collect();
        assert_eq!(icons.len(), 4);
    }

    #[gpui::test]
    fn review_mailboxes_have_spacing_without_a_duplicate_heading(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        cx.run_until_parked();
        let todo = cx.debug_bounds("section-3").unwrap();
        let inbox = cx.debug_bounds("nav-Inbox").unwrap();
        let gap = inbox.top() - todo.bottom();
        assert!(gap >= px(12.) && gap <= px(16.), "mailboxes need a small gap, not a heading: {gap:?}");
    }

    #[gpui::test]
    fn review_demo_home_has_events_without_consent_or_task_controls(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("Show your Google Calendar").is_none());
        assert!(cx.debug_bounds("Add list").is_none());
        assert!(cx.debug_bounds("task-hint").is_some());
        assert!(cx.debug_bounds("agenda-event-0").is_some());
        view.read_with(cx, |v, _| {
            assert!(v.live.is_none());
            assert!(v.preferences_path.is_none());
            assert_eq!(v.today.events.len(), 3);
            let day = chrono::Local::now().date_naive();
            for entry in &v.today.events {
                let event = &entry.event;
                assert_eq!(chrono::DateTime::from_timestamp(event.start_utc, 0).unwrap().with_timezone(&chrono::Local).date_naive(), day);
                assert!(event.end_utc > event.start_utc);
                assert!(event.join_url.is_none() && event.html_link.is_none());
            }
        });
    }

    #[gpui::test]
    fn review_launch_screens_match_the_smoke_list(cx: &mut TestAppContext) {
        let smoke = include_str!("../../../../scripts/smoke.sh");
        let screens = smoke.lines().find_map(|line| line.strip_prefix("screens='")).unwrap().trim_end_matches('\'');
        for screen in ["home", "inbox", "calendar", "todo"] {
            assert!(screens.split_whitespace().any(|s| s == screen));
            let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, screen, window, cx));
            cx.run_until_parked();
            view.read_with(cx, |v, _| {
                assert!(v.live.is_none() && v.preferences_path.is_none());
                assert!(matches!((&v.page, screen), (Page::Home, "home") | (Page::Mail, "inbox") | (Page::Calendar, "calendar") | (Page::Todo, "todo")));
            });
            assert_eq!(cx.debug_bounds("Add list").is_some(), screen == "todo");
            assert_eq!(cx.debug_bounds("task-hint").is_some(), matches!(screen, "home" | "todo"));
            assert!(cx.debug_bounds("Show your Google Calendar").is_none());
        }
    }

    #[gpui::test]
    fn home_start_sidebar_order_icons_shortcuts_and_mail_opens_inbox(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Home)));
        assert_eq!(SECTIONS.map(|s| s.0), ["Home", "Mail", "Calendar", "Todo"]);
        assert_eq!(SECTIONS.map(|s| s.2), ["⌘1", "⌘2", "⌘3", "⌘4"]);
        let paths: HashSet<_> = SECTIONS.iter().map(|s| s.1).chain(["icons/inbox.svg", "icons/later.svg", "icons/sent.svg", "icons/done.svg", "icons/trash.svg"]).collect();
        assert_eq!(paths.len(), 9);
        cx.run_until_parked();
        let brand = cx.debug_bounds("sidebar-brand").expect("static Lagom header");
        assert!(cx.debug_bounds("account-switcher").is_none());
        cx.simulate_click(brand.center(), Default::default());
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Home)));
        let rows: Vec<_> = (0..4).map(|i| cx.debug_bounds(format!("section-{i}").leak()).unwrap()).collect();
        assert!(rows.windows(2).all(|r| r[0].bottom() <= r[1].top()));
        for (key, section) in [("secondary-4", 3), ("secondary-3", 2), ("secondary-1", 0)] {
            cx.simulate_keystrokes(key);
            assert!(view.read_with(cx, |v, _| matches!((&v.page, section), (Page::Home, 0) | (Page::Calendar, 2) | (Page::Todo, 3))));
        }
        view.update(cx, |v, cx| v.open_kind(Kind::Done, cx));
        cx.simulate_keystrokes("secondary-2");
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Mail) && v.inbox.open.is_none()));
        cx.simulate_keystrokes("secondary-1");
        cx.run_until_parked();
        let unread = view.read_with(cx, |v, _| v.today.unread[0].subject.clone());
        let at = cx.debug_bounds("unread-mail-0").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Mail) && v.inbox.mail[v.inbox.current().unwrap()].subject == unread));
        assert_eq!(mail::demo_accounts().iter().map(|a| a.display_name.as_str()).collect::<Vec<_>>(), ["Personal", "Work"]);
    }
}

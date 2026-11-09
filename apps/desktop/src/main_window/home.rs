use super::*;

impl MainWindow {
    pub(super) fn show_section(&mut self, section: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        self.reset_task_focus();
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
            match live.store.unread_inbox_count(chrono::Local::now().timestamp(), &pending.into_iter().collect::<Vec<_>>()) {
                Ok(count) => self.today.unread = count,
                Err(e) => self.notify(format!("Could not read unread count: {e}"), cx),
            }
        } else {
            self.today.unread = self.inbox.items.iter().filter(|&&id| self.inbox.mail[id].unread).count();
        }
    }

    pub(super) fn unread_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        pressable(div().id("home-unread"), &t, false).debug_selector(|| "home-unread".into())
            .flex().items_center().gap(px(8.)).h(px(32.)).rounded(px(radius::CONTROL)).text_color(t.ink2)
            .child(icon("icons/mail.svg", t.ink2, 16.))
            .child(format!("{} unread in Inbox", self.today.unread))
            .child(icon("icons/chevron-right.svg", t.ink2, 16.))
            .on_click(cx.listener(|this, _, window, cx| { this.open_box(None, cx); window.focus(&this.focus); }))
    }

    pub(super) fn home_view(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let stacked = window.viewport_size().width < px(900.);
        div().flex_1().min_w(px(0.)).p(px(24.)).flex().flex_col().gap(px(24.))
            .child(div().flex().items_center().justify_between().gap(px(16.))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(size::TITLE)).child("Today"))
                .child(mono(chrono::Local::now().format("%A %-d %B").to_string(), t.ink2)))
            .child(div().id("today-groups").debug_selector(|| "today-groups".into()).flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.today.scroll)
                .child(div().flex().gap(px(32.)).when(stacked, |d| d.flex_col())
                    .child(div().min_w(px(0.)).when(!stacked, |d| d.flex_1()).child(self.agenda_view(cx)))
                    .child(div().min_w(px(0.)).when(!stacked, |d| d.flex_1()).child(self.home_tasks(cx)))))
            .children(self.task_hint(window))
            .child(self.unread_view(cx))
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

    #[test]
    fn action_icons_are_embedded_at_house_stroke_and_have_distinct_meanings() {
        use gpui::AssetSource;
        let actions = ["Reply", "Forward", "Later", "Done", "New event", "New task", "New list", "Rename", "Delete", "Move", "Join", "Undo", "Connect", "Add account", "Load remote images", "Load more", "Mark read", "Mark unread"];
        let mut icons = HashSet::new();
        for label in actions {
            let path = action_icon(label).expect("action icon");
            let bytes = crate::Assets.load(path).unwrap().expect("embedded icon");
            let svg = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(svg.contains("viewBox=\"0 0 16 16\""));
            assert!(svg.contains("stroke-width=\"1.5\""));
            assert!(icons.insert(svg), "distinct meaning: {label}");
        }
    }

    #[gpui::test]
    fn today_columns_stack_below_900_and_events_open_calendar(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        for dark in [false, true] {
            let (view, cx) = cx.add_window_view(|window, cx| {
                let mut v = MainWindow::new(true, window, cx);
                v.theme = if dark { Theme::dark(DEFAULT_ACCENT) } else { Theme::light(DEFAULT_ACCENT) }; v
            });
            for width in [1100., 900., 899., 720.] {
                cx.simulate_resize(gpui::size(px(width), px(900.)));
                cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
                let schedule = cx.debug_bounds("today-agenda").unwrap();
                let tasks = cx.debug_bounds("home-tasks").unwrap();
                if width < 900. { assert!(schedule.bottom() <= tasks.top()); }
                else { assert!(schedule.right() <= tasks.left()); assert_eq!(schedule.top(), tasks.top()); }
                assert!(schedule.size.width > px(180.) && tasks.size.width > px(180.));
                assert!(cx.debug_bounds("home-unread").is_some());
                assert!(cx.debug_bounds("unread-mail-0").is_none());
                assert!(cx.debug_bounds("nav-Inbox").is_none());
                assert!(cx.debug_bounds("today-now").is_some());
            }
            let at = cx.debug_bounds("agenda-event-1").unwrap().center();
            cx.simulate_click(at, Default::default());
            assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Calendar) && v.week.detail.is_some()));
        }
    }

    #[gpui::test]
    fn sidebar_mail_group_is_conditional_and_top_rows_stay_put(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        cx.run_until_parked();
        let top = cx.debug_bounds("section-0").unwrap();
        assert!(cx.debug_bounds("mail-sidebar").is_none());
        for section in [1, 2, 3, 0] {
            view.update_in(cx, |v, window, cx| v.show_section(section, window, cx));
            cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
            view.update(cx, |v, cx| { v.sidebar_fade = Some(Instant::now() - MOTION); cx.notify(); });
            cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
            assert_eq!(cx.debug_bounds("section-0").unwrap(), top);
            assert_eq!(view.read_with(cx, |v, _| v.sidebar_mail), section == 1);
            assert!(view.read_with(cx, |v, _| v.sidebar_fade.is_none()));
        }
    }

    #[gpui::test]
    fn review_mailboxes_have_spacing_without_a_duplicate_heading(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
        cx.run_until_parked();
        let todo = cx.debug_bounds("section-3").unwrap();
        let inbox = cx.debug_bounds("nav-Inbox").unwrap();
        let gap = inbox.top() - todo.bottom();
        assert!(gap >= px(12.) && gap <= px(20.), "mailboxes need a small gap, not a heading: {gap:?}");
    }

    #[gpui::test]
    fn review_demo_home_has_events_without_consent_or_task_controls(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("Show your Google Calendar").is_none());
        assert!(cx.debug_bounds("new-list").is_none());
        assert!(cx.debug_bounds("task-hint").is_none());
        assert!(cx.debug_bounds("agenda-event-0").is_some());
        view.read_with(cx, |v, _| {
            assert!(v.live.is_none());
            assert!(v.preferences_path.is_none());
            assert_eq!(v.today.events.len(), 4);
            let day = chrono::Local::now().date_naive();
            for entry in &v.today.events {
                let event = &entry.event;
                assert_eq!(chrono::DateTime::from_timestamp(event.start_utc, 0).unwrap().with_timezone(&chrono::Local).date_naive(), day);
                assert!(event.end_utc > event.start_utc);
                assert!(event.html_link.is_none());
                assert!(event.join_url.as_ref().is_none_or(|url| url == "https://meet.example.test/demo"));
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
            assert_eq!(cx.debug_bounds("new-list").is_some(), screen == "todo");
            assert!(cx.debug_bounds("task-hint").is_none());
            assert!(cx.debug_bounds("Show your Google Calendar").is_none());
            assert_eq!(cx.debug_bounds("mail-sidebar").is_some(), screen == "inbox");
            assert_eq!(cx.debug_bounds("nav-Inbox").is_some(), screen == "inbox");
        }
    }

    #[gpui::test]
    fn home_start_sidebar_order_icons_shortcuts_and_mail_opens_inbox(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Home)));
        assert_eq!(SECTIONS.map(|s| s.0), ["Today", "Mail", "Calendar", "Todo"]);
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
        let at = cx.debug_bounds("home-unread").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Mail) && v.inbox.open.is_none()));
        assert_eq!(mail::demo_accounts().iter().map(|a| a.display_name.as_str()).collect::<Vec<_>>(), ["Personal", "Work"]);
    }
}

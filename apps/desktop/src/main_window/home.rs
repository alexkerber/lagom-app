use super::*;

impl Page {
    pub(super) fn restore(page: StartPage) -> Self {
        match page {
            StartPage::Home => Self::Home,
            StartPage::Mail => Self::Mail,
            StartPage::Calendar => Self::Calendar,
            StartPage::Todo => Self::Todo,
        }
    }
}

impl MainWindow {
    pub(super) fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.draft_open = None;
        self.page = page;
        if self.demo { return; }
        let saved = match self.page {
            Page::Home => StartPage::Home,
            Page::Calendar => StartPage::Calendar,
            Page::Todo => StartPage::Todo,
            Page::Mail | Page::Drafts | Page::Settings(_) | Page::AddAccount(_) => StartPage::Mail,
        };
        if self.panes.start_page != saved {
            self.panes.start_page = saved;
            self.save_panes(cx);
        }
    }

    pub(super) fn show_section(&mut self, section: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        self.reset_task_focus();
        match section {
            0 => self.show_home(window, cx),
            1 => self.open_box(None, cx),
            2 | 3 => {
                self.release(false, cx);
                self.clear_motion();
                if section == 2 && !matches!(self.page, Page::Calendar) { self.week.scroll_to_now = true; }
                self.set_page(if section == 2 { Page::Calendar } else { Page::Todo }, cx);
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
    fn first_start_and_legacy_preferences_open_mail() {
        let path = std::env::temp_dir().join(format!("lagom-first-start-{}.json", std::process::id()));
        assert!(!path.exists());
        assert!(matches!(Page::restore(Panes::load(&path, 1440.).unwrap().start_page), Page::Mail));
        let panes: Panes = serde_json::from_str(r#"{"sidebar":220,"list":420,"sidebar_hidden":false}"#).unwrap();
        assert!(matches!(Page::restore(panes.start_page), Page::Mail));
    }

    #[test]
    fn saved_settings_and_unknown_pages_open_mail() {
        for page in ["Settings", "AddAccount", "Unknown"] {
            let json = format!(r#"{{"sidebar":220,"list":420,"sidebar_hidden":false,"start_page":"{page}"}}"#);
            let panes: Panes = serde_json::from_str(&json).unwrap();
            assert!(matches!(Page::restore(panes.start_page), Page::Mail));
        }
    }

    #[gpui::test]
    fn review_navigation_defers_preference_io(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("lagom-deferred-page-{}.json", std::process::id()));
        let original = Panes::new(1440.);
        original.save(&path).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update_in(cx, |v, window, cx| {
            v.demo = false;
            v.preferences_path = Some(path.clone());
            for section in [2, 3, 0] { v.show_section(section, window, cx); }
            assert!(matches!(v.page, Page::Home));
            assert_eq!(Panes::load(&path, 1440.).unwrap(), original, "navigation must return before file I/O");
        });
        cx.run_until_parked();
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Home);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn review_blocked_preference_writes_keep_navigation_responsive_and_ordered(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("lagom-ordered-page-{}.json", std::process::id()));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update(cx, |v, cx| {
            v.demo = false;
            v.preferences_path = Some(path.clone());
            v.save_panes(cx);
        });
        cx.run_until_parked();
        let original = Panes::load(&path, 1440.).unwrap();
        let (resume, wait) = tokio::sync::oneshot::channel();
        view.update(cx, |_, cx| {
            let tail = cx.background_executor().spawn(async move { wait.await.unwrap(); });
            cx.global_mut::<panes::Writes>().tail = Some(tail);
        });
        for section in [2, 3, 2] {
            view.update_in(cx, |v, window, cx| {
                v.show_section(section, window, cx);
                assert!(matches!((&v.page, section), (Page::Calendar, 2) | (Page::Todo, 3)));
            });
            cx.run_until_parked();
            assert_eq!(Panes::load(&path, 1440.).unwrap(), original);
        }
        view.update(cx, |v, cx| {
            v.toggle_notifications(cx);
            assert_eq!(panes::Writes::load(&path, 1440., cx).unwrap(), v.panes);
        });
        view.update_in(cx, |_, window, _| window.remove_window());
        drop(view);
        resume.send(()).unwrap();
        cx.run_until_parked();
        let saved = Panes::load(&path, 1440.).unwrap();
        assert_eq!(saved.start_page, StartPage::Calendar);
        assert!(!saved.notifications);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn review_quit_flushes_queued_preferences(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("lagom-quit-page-{}.json", std::process::id()));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update(cx, |v, cx| {
            v.demo = false;
            v.preferences_path = Some(path.clone());
            v.set_page(Page::Calendar, cx);
            v.set_page(Page::Todo, cx);
        });
        cx.cx.update(|cx| cx.shutdown());
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Todo);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn review_preference_errors_are_reported_and_later_writes_continue(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("lagom-page-error-{}.json", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window.json");
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update(cx, |v, cx| {
            v.demo = false;
            v.preferences_path = Some(dir.clone());
            v.set_page(Page::Calendar, cx);
            v.preferences_path = Some(path.clone());
            v.set_page(Page::Todo, cx);
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.toast.as_ref().unwrap().0.starts_with("Could not save window preferences:")));
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Todo);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[gpui::test]
    fn navigation_saves_before_close_and_settings_falls_back_to_mail(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("lagom-navigation-page-{}.json", std::process::id()));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update(cx, |v, _| {
            v.demo = false;
            v.preferences_path = Some(path.clone());
            v.panes.group_conversations = false;
        });
        for (section, page) in [(2, StartPage::Calendar), (3, StartPage::Todo), (0, StartPage::Home), (1, StartPage::Mail), (2, StartPage::Calendar)] {
            view.update_in(cx, |v, window, cx| v.show_section(section, window, cx));
            cx.run_until_parked();
            let saved = Panes::load(&path, 1440.).unwrap();
            assert_eq!(saved.start_page, page);
            assert!(!saved.group_conversations);
        }
        view.update_in(cx, |v, window, cx| v.open_settings(window, cx));
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| matches!(v.page, Page::Settings(_))));
        assert!(matches!(Page::restore(Panes::load(&path, 1440.).unwrap().start_page), Page::Mail));
        view.update_in(cx, |v, window, cx| {
            v.resize = Some(Resize { divider: Divider::Sidebar, start_x: px(220.), start_width: 220., start_panes: v.panes, moved: false });
            v.show_section(2, window, cx);
            v.resize_to(px(240.), window, cx);
            v.finish_resize(cx);
        });
        cx.run_until_parked();
        assert_eq!(Panes::load(&path, 1440.).unwrap().start_page, StartPage::Calendar);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn demo_starts_on_mail_and_never_saves_page_changes(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("lagom-demo-page-{}.json", std::process::id()));
        let mut panes = Panes::new(1440.);
        panes.start_page = StartPage::Calendar;
        panes.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "", window, cx));
        view.update(cx, |v, _| {
            assert!(matches!(v.page, Page::Mail));
            assert!(v.preferences_path.is_none());
            v.preferences_path = Some(path.clone());
        });
        for section in [0, 2, 3, 1] {
            view.update_in(cx, |v, window, cx| v.show_section(section, window, cx));
        }
        view.update_in(cx, |v, window, cx| v.open_settings(window, cx));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(&path).unwrap();
        view.update_in(cx, |v, window, cx| v.show_section(2, window, cx));
        assert!(!path.exists());
    }

    #[test]
    fn review_task_creation_uses_a_plain_plus() {
        use gpui::AssetSource;
        for label in ["New task", "Add task"] {
            assert_eq!(action_icon(label), Some("icons/plus.svg"));
        }
        let bytes = crate::Assets.load("icons/plus.svg").unwrap().unwrap();
        let svg = std::str::from_utf8(&bytes).unwrap();
        assert!(svg.contains("viewBox=\"0 0 16 16\""));
        assert!(svg.contains("stroke-width=\"1.5\""));
        assert!(!svg.contains("<circle"));
    }

    #[test]
    fn review_icon_buttons_give_the_svg_its_own_visible_color() {
        for theme in [Theme::light(DEFAULT_ACCENT), Theme::dark(DEFAULT_ACCENT)] {
            for path in ["icons/mail.svg", "icons/plus.svg", "icons/more.svg"] {
                let mut glyph = action_glyph(&theme, path, None);
                assert_eq!(glyph.style().text.as_ref().and_then(|text| text.color), Some(theme.ink2.into()), "{path}");
            }
        }
    }

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
        let actions = ["Reply", "Forward", "Later", "Done", "New event", "New task", "New list", "Rename", "Delete", "Move", "Join", "Undo", "Sign in", "Add account", "Load remote images", "Load more", "Mark read", "Mark unread"];
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
                let mut v = MainWindow::with_screen(true, "home", window, cx);
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
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "home", window, cx));
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
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "home", window, cx));
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
            let (view, cx) = cx.add_window_view(|window, cx| {
                let mut view = MainWindow::with_screen(true, screen, window, cx);
                view.mailbox_expanded = [true; 3];
                view
            });
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
            for mailbox in ["Inbox", "Sent", "Done"] {
                for account in 0..2 {
                    assert_eq!(cx.debug_bounds(format!("{mailbox}-account-{account}").leak()).is_some(), screen == "inbox");
                }
            }
            assert!(cx.debug_bounds("mail-unread-badge").is_some());
        }
    }

    #[gpui::test]
    fn home_start_sidebar_order_icons_shortcuts_and_mail_opens_inbox(cx: &mut TestAppContext) {
        cx.update(|cx| bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "home", window, cx));
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

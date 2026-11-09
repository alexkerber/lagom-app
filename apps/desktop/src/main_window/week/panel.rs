use super::*;
use lagom_core::contacts::{Contact, Contacts, split_recipients, valid_recipient};

impl MainWindow {
    pub(in super::super) fn calendar_guests(&self, cx: &App) -> Vec<String> {
        let mut guests = self.week.guests.clone();
        if let Some(input) = &self.week.guest_input { guests.extend(split_recipients(input.read(cx).text())); }
        guests
    }

    pub(super) fn prepare_calendar_guests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.week.guest_input.is_none() {
            let input = cx.new(|cx| TextInput::new(window, cx).bordered().placeholder("Name or email"));
            cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, _, cx| {
                match event {
                    TextInputEvent::Changed => {
                        this.week.suggestions = this.week.contacts.suggest(this.week.guest_input.as_ref().unwrap().read(cx).text());
                        this.week.suggestion = 0;
                        this.week.guest_error = None;
                    }
                    TextInputEvent::Enter => this.add_calendar_guest(cx),
                    TextInputEvent::Escape => this.week.suggestions.clear(),
                }
                cx.notify();
            }).detach();
            self.week.guest_input = Some(input);
        }
        let account = self.live.as_ref().and_then(|l| l.account()).map(|a| a.id.clone())
            .or_else(|| self.demo.then(|| "demo".to_owned()));
        if self.week.contact_account == account { return; }
        self.week.contact_account = account.clone();
        self.week.contacts = Contacts::default();
        self.week.suggestions.clear();
        self.week.suggestion = 0;
        self.week.contact_load = None;
        if self.demo {
            self.week.contacts = Contacts::new(vec![Contact { name: "Planning team".into(), address: "planning@example.com".into(), score: 1. }]);
        } else if let (Some(live), Some(account)) = (&self.live, account) {
            let path = live.store.path().to_owned();
            let load = cx.background_executor().spawn(async move { Store::open(path)?.contacts(&account, chrono::Utc::now().timestamp()) });
            self.week.contact_load = Some(cx.spawn(async move |this, cx| {
                let result = load.await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(contacts) => {
                            this.week.contacts = contacts;
                            this.week.suggestion = 0;
                            this.week.suggestions = this.week.contacts.suggest(this.week.guest_input.as_ref().unwrap().read(cx).text());
                        }
                        Err(_) => this.week.guest_error = Some("Could not load contacts. Enter an email address.".into()),
                    }
                    cx.notify();
                }).ok();
            }));
        }
    }

    pub(super) fn add_calendar_guest(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.week.guest_input.clone() else { return };
        let guests = match self.week.suggestions.get(self.week.suggestion) {
            Some(contact) => vec![contact.recipient()],
            None => split_recipients(input.read(cx).text()),
        };
        if guests.iter().any(|g| !valid_recipient(g)) {
            self.week.guest_error = Some("Enter a valid email address".into());
        } else {
            for guest in guests { if !self.week.guests.contains(&guest) { self.week.guests.push(guest); } }
            input.update(cx, |input, cx| input.set_text("", cx));
            self.week.suggestions.clear();
            self.week.guest_error = None;
        }
        cx.notify();
    }

    pub(super) fn calendar_panel(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        let today = chrono::Local::now().date_naive();
        let month = self.week.mini_month;
        let first = monday(month);
        let selected = monday(self.week.anchor);
        let mut mini = div().flex().flex_col().gap(px(4.))
            .child(div().flex().items_center()
                .child(icon_button(&t, "mini-previous", "icons/chevron-left.svg", "Previous month").on_click(cx.listener(|this, _, _, cx| {
                    this.week.mini_month = shift_month(this.week.mini_month, -1); cx.notify();
                })))
                .child(div().flex_1().text_center().font_weight(FontWeight::MEDIUM).child(month.format("%B %Y").to_string()))
                .child(icon_button(&t, "mini-next", "icons/chevron-right.svg", "Next month").on_click(cx.listener(|this, _, _, cx| {
                    this.week.mini_month = shift_month(this.week.mini_month, 1); cx.notify();
                }))))
            .child(div().flex().children(["M", "T", "W", "T", "F", "S", "S"].map(|name| mono(name, t.ink2).flex_1().text_center())));
        for row in 0..6 {
            let day = first + chrono::Duration::days(row * 7);
            mini = mini.child(div().flex().rounded(px(radius::CONTROL))
                .when(day == selected, |d| d.bg(t.sel))
                .children((0..7).map(|col| {
                    let date = day + chrono::Duration::days(col);
                    pressable(div().id(("mini-date", (row * 7 + col) as usize)), &t, day == selected)
                        .debug_selector(move || format!("mini-date-{date}"))
                        .flex_1().h(px(28.)).flex().items_center().justify_center().rounded_full()
                        .text_color(if date.month() == month.month() { t.ink } else { t.ink2 })
                        .when(date == today, |d| d.bg(Theme::light(DEFAULT_ACCENT).accent).text_color(gpui::rgb(0xFFFFFF)))
                        .child(date.day().to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.calendar_date(date, View::Week, cx); window.focus(&this.focus);
                        }))
                })));
        }
        let input = self.week.guest_input.as_ref().unwrap();
        let focused = input.focus_handle(cx).is_focused(window);
        div().id("calendar-panel").debug_selector(|| "calendar-panel".into()).w(px(224.)).flex_shrink_0().min_h(px(0.))
            .overflow_y_scroll().pl(px(16.)).border_l_1().border_color(t.line).flex().flex_col().gap(px(16.))
            .child(mini)
            .child(div().flex().items_center().gap(px(6.)).child(icon("icons/guests.svg", t.ink2, 16.)).child("Meet with"))
            .child(div().debug_selector(|| "meet-with".into())
                .capture_action(cx.listener(|this, _: &crate::text_input::Paste, _, cx| {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        this.week.guest_input.clone().unwrap().update(cx, |input, cx| input.insert(&text.replace(['\r', '\n'], ","), false, cx));
                        this.week.suggestions.clear();
                        this.add_calendar_guest(cx);
                        cx.stop_propagation();
                    } else { cx.propagate(); }
                }))
                .capture_action(cx.listener(|this, _: &crate::text_input::FocusNext, window, cx| {
                    this.add_calendar_guest(cx); window.focus_next(); cx.stop_propagation();
                }))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    let n = this.week.suggestions.len();
                    if n == 0 || event.keystroke.modifiers != gpui::Modifiers::none() { return; }
                    match event.keystroke.key.as_str() {
                        "down" => this.week.suggestion = (this.week.suggestion + 1) % n,
                        "up" => this.week.suggestion = (this.week.suggestion + n - 1) % n,
                        _ => return,
                    }
                    cx.stop_propagation(); cx.notify();
                }))
                .child(input.clone()))
            .when(focused, |d| d.children(self.week.suggestions.iter().enumerate().map(|(index, contact)|
                pressable(div().id(("meet-suggestion", index)), &t, index == self.week.suggestion)
                    .debug_selector(move || format!("meet-suggestion-{index}"))
                    .p(px(6.)).rounded(px(radius::CONTROL)).when(index == self.week.suggestion, |d| d.bg(t.sel))
                    .child(contact.recipient())
                    .on_mouse_down(MouseButton::Left, |_, window, cx| { window.prevent_default(); cx.stop_propagation(); })
                    .on_click(cx.listener(move |this, _, _, cx| { this.week.suggestion = index; this.add_calendar_guest(cx); })))))
            .children(self.week.guests.iter().enumerate().map(|(index, guest)| div().flex().items_center().gap(px(4.))
                .child(div().flex_1().min_w(px(0.)).one_line().child(guest.clone()))
                .child(icon_button(&t, "remove-guest", "icons/remove-guest.svg", "Remove guest").id(("remove-guest", index))
                    .on_click(cx.listener(move |this, _, _, cx| { this.week.guests.remove(index); cx.notify(); })))))
            .when_some(self.week.guest_error.clone(), |d, error| d.child(mono(error, t.danger)))
            // shortcut: guest availability is unavailable; add busy overlays when provider free/busy is implemented.
            .child(mono("Guests for your next event. Availability is not shown.", t.ink2))
    }

    pub(super) fn calendar_event_line(&self, entry: &AgendaEvent, day: usize, ix: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        let index = self.live.as_ref().and_then(|l| l.accounts.iter().position(|a| a.id == entry.account)).unwrap_or(0);
        let entry = entry.clone();
        pressable(div().id(("week-event", day * 100_000 + ix)), &t, false)
            .debug_selector(move || format!("week-event-{day}-{ix}"))
            .flex().items_center().gap(px(5.)).min_h(px(22.)).min_w(px(0.)).px(px(4.)).rounded(px(radius::KEY))
            .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(self.account_accent(index)))
            .child(div().flex_1().min_w(px(0.)).text_size(px(size::SMALL)).one_line().child(entry.event.title.clone()))
            .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.week.detail = Some(entry.clone()); window.focus(&this.focus); cx.notify(); }))
    }

    pub(super) fn calendar_month(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let today = chrono::Local::now().date_naive();
        let mut month = div().debug_selector(|| "calendar-month".into()).flex().flex_col().flex_1().min_h(px(0.))
            .child(div().flex().children(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map(|day| mono(day, t.ink2).flex_1().p(px(6.)))));
        for row in 0..6 {
            month = month.child(div().flex().flex_1().min_h(px(0.)).children((0..7).map(|col| {
                let index = row * 7 + col;
                let date = self.week.start + chrono::Duration::days(index as i64);
                let events = &self.week.days[index];
                pressable(div().id(("month-cell", index)), &t, false)
                    .debug_selector(move || format!("month-cell-{index}"))
                    .flex_1().min_w(px(0.)).overflow_hidden().border_1().border_color(t.line).p(px(4.)).flex().flex_col()
                    .when(col >= 5, |d| d.bg(t.subtle))
                    .on_click(cx.listener(move |this, _, window, cx| { this.calendar_date(date, View::Day, cx); window.focus(&this.focus); }))
                    .child(div().size(px(24.)).flex_shrink_0().flex().items_center().justify_center().rounded_full()
                        .text_color(if date.month() == self.week.anchor.month() { t.ink } else { t.ink2 })
                        .when(date == today, |d| d.bg(Theme::light(DEFAULT_ACCENT).accent).text_color(gpui::rgb(0xFFFFFF)))
                        .child(date.day().to_string()))
                    .children(events.iter().take(3).enumerate().map(|(ix, e)| self.calendar_event_line(e, index, ix, cx).flex_shrink_0()))
                    .when(events.len() > 3, |d| d.child(mono(format!("+{} more", events.len() - 3), t.ink2).debug_selector(move || format!("month-more-{index}"))))
            })));
        }
        if self.week.days.iter().enumerate().any(|(i, _)| !synced_day(self.week.start + chrono::Duration::days(i as i64), &self.week.windows)) {
            month = month.child(mono("Some dates are outside the synced calendar window", t.ink2));
        }
        month
    }
}

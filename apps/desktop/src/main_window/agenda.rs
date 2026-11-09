use super::*;
use crate::sync::{Cmd, SyncState};
use lagom_core::{account::Auth, oauth::{self, Provider}};

impl MainWindow {
    pub(super) fn reload_agenda(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else {
            if self.demo { self.today.events = demo_events(chrono::Local::now().date_naive()); }
            return;
        };
        let result = (|| -> lagom_core::Result<_> {
            let day = chrono::Local::now().date_naive();
            let events = live.store.today_events(&day.to_string(), day_start(day, &chrono::Local), day_start(day.succ_opt().unwrap(), &chrono::Local))?;
            let accounts = live.store.accounts()?.into_iter().filter(|a| a.auth == Auth::OAuth(Provider::Google))
                .map(|account| live.store.account_scopes(&account.id).map(|scopes| (account, oauth::calendar_allowed(&scopes))))
                .collect::<lagom_core::Result<Vec<_>>>()?;
            Ok((events, accounts))
        })();
        match result {
            Ok((events, accounts)) => {
                self.today.events = events;
                self.today.calendar_sync.retain(|id, _| accounts.iter().any(|(a, _)| a.id == *id));
                self.today.calendar_accounts = accounts;
            }
            Err(e) => self.notify(format!("Could not read calendar: {e}"), cx),
        }
    }

    pub(super) fn calendar_controls(&self, cx: &mut Context<Self>) -> Div {
        if self.demo && self.live.is_none() { return div(); }
        let t = self.theme;
        let mut controls = div().id("calendar-accounts").flex_shrink_0().max_h(px(140.)).overflow_y_scroll().flex().flex_col().gap(px(8.));
        if self.today.calendar_accounts.is_empty() && self.today.events.is_empty() {
            controls = controls.child(button(&t, "Show your Google Calendar", None).on_click(cx.listener(|this, _, window, cx| this.add_account(window, cx))));
        }
        for (ix, (account, allowed)) in self.today.calendar_accounts.iter().enumerate() {
            let id = account.id.clone();
            let state = self.today.calendar_sync.get(&id);
            let signing_in = state == Some(&SyncState::SigningIn);
            controls = controls.child(div().flex().items_center().gap(px(8.))
                .when(*allowed, |d| d.child(icon(if matches!(state, Some(SyncState::Failed(_))) { "icons/cloud-off.svg" } else { "icons/cloud.svg" }, t.ink2, 16.)))
                .child(mono(account.email.clone(), t.ink2))
                .when(!allowed || signing_in, |d| d.child(button(&t, if signing_in { "Cancel sign-in" } else { "Show your Google Calendar" }, None)
                    .debug_selector(move || format!("allow-calendar-{ix}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let cmd = if signing_in { Cmd::CancelSignIn } else { Cmd::AllowCalendar(id.clone()) };
                        if let Some(live) = &this.live {
                            if live.bridge.send(cmd) {
                                this.today.calendar_sync.insert(id.clone(), if signing_in { SyncState::Synced(0) } else { SyncState::SigningIn });
                            } else { this.worker_gone(cx); }
                        }
                        cx.notify();
                    }))))
                .when_some(state.and_then(|s| match s { SyncState::Failed(e) => Some(e.clone()), _ => None }),
                    |d, error| d.child(mono(error, t.ink2))));
        }
        div().flex_shrink_0().child(controls)
    }

    pub(super) fn agenda_view(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let now = chrono::Local::now();
        let has_calendar = (self.demo && self.live.is_none()) || self.today.calendar_accounts.iter().any(|(_, allowed)| *allowed);
        let mut agenda = div().debug_selector(|| "today-agenda".into()).w_full().flex().flex_col().gap(px(12.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Schedule"));
        if !has_calendar {
            let account = self.today.calendar_accounts.first().map(|(a, _)| a.id.clone());
            return agenda.child(pressable(div().id("home-calendar-consent"), &t, false)
                .debug_selector(|| "Show your Google Calendar".into()).flex().items_center().gap(px(8.)).min_h(px(32.)).text_color(t.ink2)
                .child(icon("icons/calendar.svg", t.ink2, 16.)).child("Show your Google Calendar")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(id) = &account {
                        if let Some(live) = &this.live {
                            if !live.bridge.send(Cmd::AllowCalendar(id.clone())) { this.worker_gone(cx); }
                        }
                    } else { this.add_account(window, cx); }
                })));
        }
        if self.today.events.is_empty() {
            return agenda.child(div().py(px(16.)).flex().flex_col().gap(px(8.))
                .child(icon("icons/calendar.svg", t.ink2, 32.)).child(mono("Nothing scheduled today", t.ink2)));
        }
        let now_line = || div().debug_selector(|| "today-now".into()).flex().items_center().gap(px(10.)).min_h(px(24.))
            .child(mono(now.format("%H:%M").to_string(), t.swipe_later).w(px(48.)).flex_shrink_0())
            .child(div().h(px(2.)).flex_1().bg(t.swipe_later));
        let mut entries: Vec<_> = self.today.events.iter().enumerate().collect();
        entries.sort_by_key(|(ix, entry)| (!entry.event.all_day, entry.event.start_utc, *ix));
        let mut marked_now = false;
        for (ix, entry) in entries {
            let event = &entry.event;
            if !event.all_day && !marked_now && event.start_utc > now.timestamp() {
                agenda = agenda.child(now_line()); marked_now = true;
            }
            let when = chrono::DateTime::from_timestamp(event.start_utc, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
            let past = !event.all_day && event.end_utc <= now.timestamp();
            let index = self.live.as_ref().and_then(|live| live.accounts.iter().position(|a| a.id == entry.account)).unwrap_or(0);
            let accent = self.account_accent(index);
            let entry = entry.clone();
            agenda = agenda.child(pressable(div().id(("agenda-event", ix)), &t, false).debug_selector(move || format!("agenda-event-{ix}"))
                .flex().items_center().gap(px(10.)).min_h(px(36.)).rounded(px(radius::KEY))
                .when(!event.all_day, |d| d.child(mono(when, t.ink2).w(px(48.)).flex_shrink_0())
                    .child(div().w(px(2.)).h(px(24.)).flex_shrink_0().bg(accent)))
                .when(event.all_day, |d| d.child(div().w(px(48.)).flex_shrink_0().flex().justify_end().child(icon("icons/sun.svg", t.ink2, 16.)))
                    .child(div().size(px(4.)).rounded_full().bg(accent)))
                .child(div().flex_1().min_w(px(0.)).one_line().text_color(if past { t.ink2 } else { t.ink }).child(event.title.clone()))
                .when(event.join_url.is_some(), |d| d.child(icon("icons/video.svg", t.ink2, 16.)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_section(2, window, cx);
                    this.move_week(0, cx);
                    this.week.detail = Some(entry.clone());
                    cx.notify();
                })));
        }
        if !marked_now { agenda = agenda.child(now_line()); }
        agenda
    }
}

pub(super) fn demo_events(day: chrono::NaiveDate) -> Vec<lagom_core::calendar::AgendaEvent> {
    [(0, "Planning day"), (9, "Plan the day"), (11, "Design review"), (14, "Focus time")].into_iter().filter_map(|(hour, title)| {
        let start = day.and_hms_opt(hour, 0, 0)?.and_local_timezone(chrono::Local).earliest()?;
        let end = start + if hour == 0 { chrono::Duration::days(1) } else { chrono::Duration::minutes(45) };
        Some(lagom_core::calendar::AgendaEvent { account: "demo".into(), calendar: "Demo calendar".into(),
            event: lagom_core::calendar::Event {
                remote_id: format!("demo-{hour}"), title: title.into(), start: if hour == 0 { day.to_string() } else { start.to_rfc3339() }, end: if hour == 0 { day.succ_opt().unwrap().to_string() } else { end.to_rfc3339() },
                start_utc: start.timestamp(), end_utc: end.timestamp(), all_day: hour == 0,
                location: None, join_url: (hour == 11).then(|| "https://meet.example.test/demo".into()), html_link: None,
            },
        })
    }).collect()
}

pub(super) fn day_start<T: chrono::TimeZone>(date: chrono::NaiveDate, zone: &T) -> i64 {
    let mut time = date.and_hms_opt(0, 0, 0).unwrap();
    loop {
        if let Some(instant) = zone.from_local_datetime(&time).earliest() { return instant.timestamp(); }
        // Midnight can be skipped by a timezone transition, including a whole local date.
        time += chrono::Duration::seconds(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Santiago's September 2026 transition, independent of the host's timezone database.
    #[derive(Clone)]
    struct Santiago;
    impl chrono::TimeZone for Santiago {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &Self::Offset) -> Self { Self }
        fn offset_from_local_date(&self, date: &chrono::NaiveDate) -> chrono::LocalResult<Self::Offset> {
            self.offset_from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, time: &chrono::NaiveDateTime) -> chrono::LocalResult<Self::Offset> {
            let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
            if *time < day.and_hms_opt(0, 0, 0).unwrap() {
                chrono::LocalResult::Single(chrono::FixedOffset::west_opt(4 * 3600).unwrap())
            } else if *time < day.and_hms_opt(1, 0, 0).unwrap() {
                chrono::LocalResult::None
            } else {
                chrono::LocalResult::Single(chrono::FixedOffset::west_opt(3 * 3600).unwrap())
            }
        }
        fn offset_from_utc_date(&self, date: &chrono::NaiveDate) -> Self::Offset {
            self.offset_from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, time: &chrono::NaiveDateTime) -> Self::Offset {
            let transition = chrono::NaiveDate::from_ymd_opt(2026, 9, 6).unwrap().and_hms_opt(4, 0, 0).unwrap();
            chrono::FixedOffset::west_opt(if *time < transition { 4 * 3600 } else { 3 * 3600 }).unwrap()
        }
    }

    #[test]
    fn missing_midnight_uses_local_date_at_both_boundaries() {
        let path = std::env::temp_dir().join(format!("lagom-midnight-{}.db", std::process::id()));
        let store = lagom_core::store::Store::open(&path).unwrap();
        let (imap, smtp) = lagom_core::presets::servers("fixture@gmail.com").unwrap();
        let account = lagom_core::account::Account::new("fixture@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let events = [
            ("previous", "2026-09-05T22:00:00-04:00", "2026-09-05T23:00:00-04:00"),
            ("ends-at-start", "2026-09-05T23:00:00-04:00", "2026-09-06T01:00:00-03:00"),
            ("first", "2026-09-06T01:00:00-03:00", "2026-09-06T02:00:00-03:00"),
            ("last", "2026-09-06T23:00:00-03:00", "2026-09-07T00:00:00-03:00"),
            ("next", "2026-09-07T00:00:00-03:00", "2026-09-07T01:00:00-03:00"),
        ].into_iter().map(|(id, start, end)| lagom_core::calendar::Event {
            remote_id: id.into(), title: id.into(), start: start.into(), end: end.into(),
            start_utc: chrono::DateTime::parse_from_rfc3339(start).unwrap().timestamp(),
            end_utc: chrono::DateTime::parse_from_rfc3339(end).unwrap().timestamp(),
            all_day: false, location: None, join_url: None, html_link: None,
        }).collect();
        store.replace_calendar_window(&account.id, i64::MIN, i64::MAX, &[(lagom_core::calendar::Calendar {
            remote_id: "primary".into(), name: "Fixture".into(), color: None, shown: true, writable: true,
        }, events)]).unwrap();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        for (date, expected) in [(day.pred_opt().unwrap(), vec!["previous", "ends-at-start"]), (day, vec!["first", "last"])] {
            let events = store.today_events(&date.to_string(), day_start(date, &Santiago), day_start(date.succ_opt().unwrap(), &Santiago)).unwrap();
            assert_eq!(events.iter().map(|e| e.event.remote_id.as_str()).collect::<Vec<_>>(), expected);
        }
        assert_eq!(day_start(day.succ_opt().unwrap(), &Santiago) - day_start(day, &Santiago), 23 * 3600);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}

use super::*;
use crate::sync::{Cmd, SyncState};
use lagom_core::{account::Auth, oauth::{self, Provider}};

impl MainWindow {
    pub(super) fn reload_agenda(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
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

    pub(super) fn agenda_view(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let has_calendar = self.today.calendar_accounts.iter().any(|(_, allowed)| *allowed) || !self.today.events.is_empty();
        let mut agenda = div().id("today-agenda").w_full().flex_shrink_0().max_h(px(280.)).overflow_y_scroll()
            .flex().flex_col().gap(px(12.))
            .when(has_calendar, |d| d.child(div().debug_selector(|| "calendar-heading".into()).font_weight(FontWeight::SEMIBOLD).child("Calendar")));
        for (ix, (account, allowed)) in self.today.calendar_accounts.iter().enumerate() {
            let id = account.id.clone();
            let state = self.today.calendar_sync.get(&id);
            let signing_in = state == Some(&SyncState::SigningIn);
            agenda = agenda.child(div().flex().items_center().gap(px(8.))
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
        if has_calendar && self.today.events.is_empty() {
            agenda = agenda.child(mono("No cached events for today", t.ink2));
        }
        // shortcut: Today lays out every event, virtualize when large agendas need bounded layout.
        for (ix, entry) in self.today.events.iter().enumerate() {
            let event = &entry.event;
            let when = if event.all_day { "All day".into() } else {
                chrono::DateTime::from_timestamp(event.start_utc, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default()
            };
            agenda = agenda.child(div().id(("agenda-event", ix)).debug_selector(move || format!("agenda-event-{ix}"))
                .flex().flex_col().gap(px(4.)).p(px(10.)).rounded(px(radius::CONTROL)).bg(t.subtle)
                .child(mono(format!("{when} · {}", entry.calendar), t.ink2))
                .child(event.title.clone())
                .children(event.location.clone().map(|location| mono(location, t.ink2)))
                .children(event.join_url.clone().filter(|url| lagom_core::google::safe_join_url(url)).map(|url|
                    button(&t, "Join meeting", None).on_click(move |_, _, cx| cx.open_url(&url)))));
        }
        div().w_full().flex_shrink_0().child(agenda)
    }
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
            all_day: false, location: None, join_url: None,
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

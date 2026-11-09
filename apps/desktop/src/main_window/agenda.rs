use super::*;
use crate::sync::{Cmd, SyncState};
use lagom_core::{account::Auth, oauth::{self, Provider}};

impl MainWindow {
    pub(super) fn reload_agenda(&mut self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else { return };
        let result = (|| -> lagom_core::Result<_> {
            use chrono::TimeZone;
            let day = chrono::Local::now().date_naive();
            let midnight = |date: chrono::NaiveDate| {
                let time = date.and_hms_opt(0, 0, 0).unwrap();
                chrono::Local.from_local_datetime(&time).earliest()
                    .unwrap_or_else(|| chrono::Local.from_utc_datetime(&time)).timestamp()
            };
            let events = live.store.today_events(&day.to_string(), midnight(day), midnight(day.succ_opt().unwrap()))?;
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
        let mut agenda = div().id("today-agenda").w(px(280.)).flex_shrink_0().min_h(px(0.)).overflow_y_scroll()
            .flex().flex_col().gap(px(12.)).child(div().font_weight(FontWeight::SEMIBOLD).child("Calendar"));
        for (ix, (account, allowed)) in self.today.calendar_accounts.iter().enumerate() {
            let id = account.id.clone();
            let state = self.today.calendar_sync.get(&id);
            let signing_in = state == Some(&SyncState::SigningIn);
            agenda = agenda.child(div().flex().flex_col().gap(px(4.))
                .child(mono(account.email.clone(), t.ink2))
                .when(!allowed || signing_in, |d| d.child(button(&t, if signing_in { "Cancel sign-in" } else { "Allow calendar access" }, None)
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
        if self.today.events.is_empty() {
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
        div().flex().min_h(px(0.)).child(agenda)
    }
}

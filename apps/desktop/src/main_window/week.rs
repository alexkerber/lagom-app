use super::*;
use chrono::{Datelike, NaiveDate, Timelike};
use lagom_core::calendar::{AgendaEvent, Event as CalendarEvent};

const HOUR: f32 = 60.;
const MIN_EVENT: f32 = 36.;

pub(super) struct Week {
    start: NaiveDate,
    days: Vec<Vec<AgendaEvent>>,
    windows: Vec<Option<(i64, i64)>>,
    detail: Option<AgendaEvent>,
    scroll: ScrollHandle,
    tick: Option<Task<()>>,
    loaded_day: Option<NaiveDate>,
}

impl Default for Week {
    fn default() -> Self {
        let scroll = ScrollHandle::new();
        scroll.set_offset(gpui::point(px(0.), px(-8. * HOUR)));
        Self { start: monday(chrono::Local::now().date_naive()), days: vec![vec![]; 7], windows: vec![], detail: None, scroll, tick: None, loaded_day: None }
    }
}

fn monday(day: NaiveDate) -> NaiveDate { day - chrono::Duration::days(day.weekday().num_days_from_monday() as i64) }
fn synced_day(day: NaiveDate, windows: &[Option<(i64, i64)>]) -> bool {
    let start = agenda::day_start(day, &chrono::Local);
    let end = agenda::day_start(day.succ_opt().unwrap(), &chrono::Local);
    !windows.is_empty() && windows.iter().all(|bounds| bounds.is_some_and(|(from, to)| from <= start && to >= end))
}

#[derive(Debug)]
struct Block { event: usize, start: f32, end: f32, lane: usize, lanes: usize }

fn layout<T: chrono::TimeZone>(events: &[AgendaEvent], day: NaiveDate, zone: &T) -> Vec<Block> {
    let start = agenda::day_start(day, zone);
    let end = agenda::day_start(day.succ_opt().unwrap(), zone);
    let minute = |stamp| chrono::DateTime::from_timestamp(stamp, 0).map(|t| {
        let local = t.with_timezone(zone);
        local.hour() as f32 * 60. + local.minute() as f32 + local.second() as f32 / 60.
    }).unwrap_or(0.);
    let mut blocks: Vec<_> = events.iter().enumerate().filter(|(_, e)| !e.event.all_day && e.event.start_utc < end && e.event.end_utc > start)
        .map(|(ix, e)| {
            let top = if e.event.start_utc <= start { 0. } else { minute(e.event.start_utc) };
            let bottom = if e.event.end_utc >= end { 1440. } else { minute(e.event.end_utc) };
            // shortcut: repeated DST hours share a wall-clock row; add a repeated-hour axis when timezone controls land.
            Block { event: ix, start: top, end: bottom.max(top + MIN_EVENT).min(1440.), lane: 0, lanes: 1 }
        }).collect();
    blocks.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)).then(a.event.cmp(&b.event)));
    let mut group = 0;
    let mut until = 0.;
    let mut lanes: Vec<f32> = Vec::new();
    for i in 0..blocks.len() {
        if blocks[i].start >= until {
            for b in &mut blocks[group..i] { b.lanes = lanes.len(); }
            group = i;
            lanes.clear();
        }
        let lane = lanes.iter().position(|&end| end <= blocks[i].start).unwrap_or(lanes.len());
        if lane == lanes.len() { lanes.push(blocks[i].end); } else { lanes[lane] = blocks[i].end; }
        blocks[i].lane = lane;
        until = if group == i { blocks[i].end } else { until.max(blocks[i].end) };
    }
    for b in &mut blocks[group..] { b.lanes = lanes.len(); }
    blocks
}

fn event_time(event: &CalendarEvent) -> String {
    if event.all_day {
        let last = NaiveDate::parse_from_str(&event.end, "%Y-%m-%d").ok().and_then(|d| d.pred_opt()).map(|d| d.to_string()).unwrap_or_else(|| event.end.clone());
        if event.start == last { format!("All day · {}", event.start) } else { format!("All day · {} – {last}", event.start) }
    }
    else { format!("{} – {}", mail::full_date(event.start_utc), mail::full_date(event.end_utc)) }
}
fn short_time(event: &CalendarEvent) -> String {
    let time = |stamp| chrono::DateTime::from_timestamp(stamp, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
    format!("{}–{}", time(event.start_utc), time(event.end_utc))
}

actions!(calendar_week, [PreviousWeek, NextWeek, CurrentWeek, CloseEvent]);
pub(super) fn bind_keys(cx: &mut App) {
    let keys = Some("Calendar && !TextInput");
    cx.bind_keys([
        KeyBinding::new("left", PreviousWeek, keys), KeyBinding::new("right", NextWeek, keys),
        KeyBinding::new("t", CurrentWeek, keys), KeyBinding::new("shift-t", CurrentWeek, keys),
        KeyBinding::new("escape", CloseEvent, keys),
    ]);
}

impl MainWindow {
    pub(super) fn account_accent(&self, index: usize) -> Rgba {
        let accent = [crate::theme::Accent::Dusk, crate::theme::Accent::PineGreen, crate::theme::Accent::Gold][index % 3];
        if self.theme.dark { Theme::dark(accent).accent } else { Theme::light(accent).accent }
    }

    pub(super) fn reload_week(&mut self, cx: &mut Context<Self>) {
        let today = chrono::Local::now().date_naive();
        self.week.loaded_day = Some(today);
        let Some(live) = &self.live else {
            if self.demo {
                self.week.days = vec![vec![]; 7];
                let index = (today - self.week.start).num_days();
                if (0..7).contains(&index) { self.week.days[index as usize] = agenda::demo_events(today); }
                let start = monday(today);
                self.week.windows = vec![Some((agenda::day_start(start, &chrono::Local), agenda::day_start(start + chrono::Duration::weeks(1), &chrono::Local)))];
            }
            return;
        };
        let result = (|| -> lagom_core::Result<_> {
            let windows = self.today.calendar_accounts.iter().map(|(account, _)| live.store.calendar_window(&account.id))
                .collect::<lagom_core::Result<Vec<_>>>()?;
            let days = (0..7).map(|i| {
                let day = self.week.start + chrono::Duration::days(i);
                live.store.today_events(&day.to_string(), agenda::day_start(day, &chrono::Local), agenda::day_start(day.succ_opt().unwrap(), &chrono::Local))
            }).collect::<lagom_core::Result<Vec<_>>>()?;
            Ok((days, windows))
        })();
        match result {
            Ok((days, windows)) => { self.week.days = days; self.week.windows = windows; }
            Err(e) => {
                self.week.days = vec![vec![]; 7];
                self.week.windows.clear();
                self.notify(format!("Could not read calendar: {e}"), cx);
            }
        }
    }

    fn move_week(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        self.week.start = if delta == 0 { monday(chrono::Local::now().date_naive()) }
            else { self.week.start.checked_add_signed(chrono::Duration::weeks(delta)).unwrap_or(self.week.start) };
        self.week.detail = None;
        self.reload_week(cx);
        cx.notify();
    }

    pub(super) fn week_keys<E: InteractiveElement>(element: E, cx: &mut Context<Self>) -> E {
        element.on_action(cx.listener(|this, _: &PreviousWeek, _, cx| this.move_week(-1, cx)))
            .on_action(cx.listener(|this, _: &NextWeek, _, cx| this.move_week(1, cx)))
            .on_action(cx.listener(|this, _: &CurrentWeek, _, cx| this.move_week(0, cx)))
            .on_action(cx.listener(|this, _: &CloseEvent, _, cx| { this.week.detail = None; cx.notify(); }))
    }

    fn event_block(&self, entry: &AgendaEvent, day: usize, ix: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let index = self.live.as_ref().and_then(|l| l.accounts.iter().position(|a| a.id == entry.account)).unwrap_or(0);
        let accent = self.account_accent(index);
        let t = self.theme;
        let entry = entry.clone();
        pressable(div().id(("week-event", day * 100_000 + ix)), &t, false)
            .debug_selector(move || format!("week-event-{day}-{ix}"))
            .px(px(5.)).py(px(2.)).rounded(px(radius::CONTROL)).overflow_hidden()
            .bg(Rgba { a: if t.dark { 0.22 } else { 0.12 }, ..accent }).border_l_2().border_color(accent)
            .child(div().text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM).one_line().child(entry.event.title.clone()))
            .when(!entry.event.all_day, |d| d.child(mono(short_time(&entry.event), t.ink2)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.week.detail = Some(entry.clone());
                window.focus(&this.focus);
                cx.notify();
            }))
    }

    pub(super) fn week_view(&mut self, cx: &mut Context<Self>) -> Div {
        let now = chrono::Local::now();
        let today = now.date_naive();
        if self.week.loaded_day != Some(today) { self.reload_agenda(cx); self.reload_week(cx); }
        if self.week.tick.is_none() {
            self.week.tick = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(60)).await;
                this.update(cx, |this, cx| { this.week.tick = None; if matches!(this.page, Page::Calendar) { cx.notify(); } }).ok();
            }));
        }
        let t = self.theme;
        let start = self.week.start;
        let allowed = self.today.calendar_accounts.iter().any(|(_, allowed)| *allowed) || self.week.days.iter().any(|d| !d.is_empty());
        let mut page = div().relative().flex_1().min_w(px(0.)).p(px(24.)).flex().flex_col().gap(px(16.))
            .child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().text_size(px(size::TITLE)).font_weight(FontWeight::SEMIBOLD).child("Calendar"))
                .child(mono(format!("{} – {}", start.format("%-d %b %Y"), (start + chrono::Duration::days(6)).format("%-d %b %Y")), t.ink2))
                .child(button(&t, "‹", None).debug_selector(|| "previous-week".into()).on_click(cx.listener(|this, _, _, cx| this.move_week(-1, cx))))
                .child(button(&t, "Today", None).on_click(cx.listener(|this, _, _, cx| this.move_week(0, cx))))
                .child(button(&t, "›", None).debug_selector(|| "next-week".into()).on_click(cx.listener(|this, _, _, cx| this.move_week(1, cx)))));
        page = page.child(self.calendar_controls(cx));
        if !allowed { return page; }
        let mut headers = div().flex().flex_shrink_0().pl(px(48.));
        let mut all_day = div().id("week-all-day").debug_selector(|| "week-all-day".into()).flex().flex_shrink_0().min_h(px(32.)).max_h(px(150.)).overflow_y_scroll()
            .child(mono("All day", t.ink2).w(px(48.)).flex_shrink_0());
        let mut hours = div().relative().w(px(48.)).flex_shrink_0().h(px(24. * HOUR));
        for hour in 0..24 {
            hours = hours.child(mono(format!("{hour:02}:00"), t.ink2).absolute().top(px(hour as f32 * HOUR)));
        }
        let mut grid = div().flex().h(px(24. * HOUR)).child(hours);
        for day in 0..7 {
            let date = start + chrono::Duration::days(day as i64);
            let synced = synced_day(date, &self.week.windows);
            headers = headers.child(div().flex_1().min_w(px(0.)).text_color(if date == today { t.accent } else { t.ink })
                .font_weight(FontWeight::MEDIUM).child(date.format("%a %-d").to_string()));
            let entries = &self.week.days[day];
            all_day = all_day.child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(3.)).px(px(2.))
                .children(entries.iter().enumerate().filter(|(_, e)| e.event.all_day)
                    .map(|(ix, e)| self.event_block(e, day, ix, cx).min_h(px(26.)))));
            let mut column = div().relative().flex_1().min_w(px(0.)).h_full().border_l_1().border_color(t.line);
            if !synced {
                column = column.child(mono("Not synced this far", t.ink2).p(px(8.)).debug_selector(move || format!("unsynced-day-{day}")));
            }
            for hour in 0..24 {
                column = column.child(div().absolute().top(px(hour as f32 * HOUR)).w_full().h(px(1.)).bg(t.line));
            }
            for block in layout(entries, date, &chrono::Local) {
                column = column.child(self.event_block(&entries[block.event], day, block.event, cx).absolute()
                    .top(px(block.start * HOUR / 60.)).h(px((block.end - block.start) * HOUR / 60.))
                    .left(relative(block.lane as f32 / block.lanes as f32)).w(relative(1. / block.lanes as f32)));
            }
            if date == today {
                column = column.child(div().absolute().top(px((now.hour() as f32 + now.minute() as f32 / 60.) * HOUR))
                    .w_full().h(px(2.)).bg(t.accent).debug_selector(|| "current-time".into()));
            }
            grid = grid.child(column);
        }
        page = page.child(headers).child(all_day)
            .when((0..7).any(|i| !synced_day(start + chrono::Duration::days(i), &self.week.windows)), |d|
                d.child(mono("Not synced this far", t.ink2).debug_selector(|| "week-unsynced".into())))
            .when(self.week.days.iter().any(|d| !d.is_empty()) || (0..7).any(|i| synced_day(start + chrono::Duration::days(i), &self.week.windows)), |d|
                d.child(div().id("week-grid").debug_selector(|| "week-grid".into()).flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.week.scroll).child(grid)))
            .child(mono("← → week · T today", t.ink2));
        if let Some(entry) = &self.week.detail {
            page = page.child(div().id("event-detail").debug_selector(|| "event-detail".into()).absolute().top(px(64.)).right(px(24.)).w(px(320.))
                .occlude().max_h(px(400.)).overflow_y_scroll().p(px(16.)).rounded(px(radius::CONTROL)).border_1().border_color(t.line).bg(t.raised).shadow_md()
                .flex().flex_col().gap(px(12.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| { this.week.detail = None; cx.notify(); }))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(entry.event.title.clone()))
                .child(mono(event_time(&entry.event), t.ink2))
                .children(entry.event.location.clone().map(|location| div().child(location)))
                .child(mono(entry.calendar.clone(), t.ink2))
                .children(entry.event.join_url.clone().filter(|s| lagom_core::google::safe_join_url(s)).map(|url|
                    button(&t, "Join", None).on_click(move |_, _, cx| cx.open_url(&url))))
                .children(entry.event.html_link.clone().filter(|s| lagom_core::google::safe_calendar_url(s)).map(|url|
                    button(&t, "Open in Google Calendar", None).on_click(move |_, _, cx| cx.open_url(&url))))
                .child(button(&t, "Close", None).on_click(cx.listener(|this, _, _, cx| { this.week.detail = None; cx.notify(); }))));
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn entry(day: NaiveDate, start: u32, end: u32, all_day: bool) -> AgendaEvent {
        let midnight = day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        AgendaEvent { account: "fixture".into(), calendar: "Fixture calendar".into(), event: CalendarEvent {
            remote_id: format!("{start}-{end}"), title: "Fixture event".into(), start: day.to_string(), end: day.succ_opt().unwrap().to_string(),
            start_utc: midnight + start as i64 * 60, end_utc: midnight + end as i64 * 60,
            all_day, location: Some("Fixture room".into()), join_url: Some("https://meet.example.test/fixture".into()),
            html_link: Some("https://www.google.com/calendar/event?eid=fixture".into()),
        } }
    }

    #[gpui::test]
    fn review_demo_calendar_shares_today_events_and_clears_other_weeks(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
        cx.run_until_parked();
        let today = chrono::Local::now().date_naive();
        let day = today.weekday().num_days_from_monday() as usize;
        assert!(cx.debug_bounds(format!("week-event-{day}-0").leak()).is_some());
        assert!(cx.debug_bounds("Show your Google Calendar").is_none());
        assert!(cx.debug_bounds("week-unsynced").is_none());
        view.update(cx, |v, cx| {
            assert!(v.live.is_none());
            assert_eq!(v.week.days[day], v.today.events);
            assert_eq!(v.week.days.iter().map(Vec::len).sum::<usize>(), 3);
            v.move_week(1, cx);
            assert!(v.week.days.iter().all(Vec::is_empty));
            v.move_week(-1, cx);
            assert_eq!(v.week.days[day], v.today.events);
            assert_eq!(v.week.days.iter().map(Vec::len).sum::<usize>(), 3);
        });
    }

    #[test]
    fn overlap_chains_share_lanes_and_touching_events_reuse_full_width() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let entries = vec![entry(day, 540, 660, false), entry(day, 570, 610, false), entry(day, 610, 690, false),
            entry(day, 690, 750, false), entry(day, 0, 1440, true)];
        let blocks = layout(&entries, day, &chrono::Utc);
        assert_eq!(blocks.iter().map(|b| (b.event, b.lane, b.lanes)).collect::<Vec<_>>(), [(0, 0, 2), (1, 1, 2), (2, 1, 2), (3, 0, 1)]);
        for a in &blocks { for b in &blocks {
            if a.event != b.event && a.start < b.end && b.start < a.end { assert_ne!(a.lane, b.lane); }
        } }
    }

    #[test]
    fn overnight_clipping_and_short_meetings_do_not_paint_on_each_other() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let entries = vec![entry(day.pred_opt().unwrap(), 1380, 1500, false), entry(day, 5, 10, false),
            entry(day, 12, 17, false), entry(day, 1430, 1500, false)];
        let blocks = layout(&entries, day, &chrono::Utc);
        assert_eq!((blocks[0].start, blocks[0].end, blocks[0].lanes), (0., 60., 3));
        assert_eq!(blocks.last().unwrap().end, 1440.);
    }

    #[test]
    fn monday_crosses_year_boundary_and_coverage_requires_full_days_for_every_account() {
        let day = NaiveDate::from_ymd_opt(2027, 1, 3).unwrap();
        assert_eq!(monday(day).to_string(), "2026-12-28");
        let start = agenda::day_start(day, &chrono::Local);
        let end = agenda::day_start(day.succ_opt().unwrap(), &chrono::Local);
        for (windows, valid) in [(vec![], false), (vec![None], false), (vec![Some((start, end))], true),
            (vec![Some((start + 1, end))], false), (vec![Some((start, end - 1))], false),
            (vec![Some((start, end)), None], false)] {
            assert_eq!(synced_day(day, &windows), valid);
        }
    }

    #[gpui::test]
    fn navigation_all_day_strip_detail_and_unsynced_note(cx: &mut TestAppContext) {
        cx.update(|cx| super::super::bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        let start = view.read_with(cx, |v, _| v.week.start);
        view.update(cx, |v, _| {
            v.demo = false;
            v.week.days[0] = vec![entry(start, 0, 1440, true), entry(start, 540, 600, false)];
            v.week.windows = vec![Some((agenda::day_start(start, &chrono::Local), agenda::day_start(start + chrono::Duration::weeks(1), &chrono::Local)))];
        });
        cx.simulate_keystrokes("secondary-3");
        cx.run_until_parked();
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("current-time").is_some());
        let strip = cx.debug_bounds("week-all-day").unwrap();
        let event = cx.debug_bounds("week-event-0-0").unwrap();
        assert!(event.top() >= strip.top() && event.bottom() <= strip.bottom());
        cx.simulate_click(event.center(), Default::default());
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.week.detail.is_some()));
        assert!(cx.debug_bounds("event-detail").is_some());
        assert!(cx.debug_bounds("Join").is_some());
        assert!(cx.debug_bounds("Open in Google Calendar").is_some());
        cx.simulate_keystrokes("escape right");
        assert_eq!(view.read_with(cx, |v, _| v.week.start), start + chrono::Duration::days(7));
        assert!(view.read_with(cx, |v, _| v.week.detail.is_none()));
        cx.simulate_keystrokes("left left t");
        assert_eq!(view.read_with(cx, |v, _| v.week.start), start);
        let at = cx.debug_bounds("next-week").unwrap().center();
        cx.simulate_click(at, Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.week.start), start + chrono::Duration::days(7));
        cx.simulate_keystrokes("shift-t");
        assert_eq!(view.read_with(cx, |v, _| v.week.start), start);
        view.update(cx, |v, cx| v.move_week(10, cx));
        cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(); });
        assert!(cx.debug_bounds("week-unsynced").is_some());
        assert!(view.read_with(cx, |v, _| (0..7).all(|i| !synced_day(v.week.start + chrono::Duration::days(i), &v.week.windows))));
    }
    #[test]
    fn all_day_detail_uses_inclusive_end_date() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let mut e = entry(day, 0, 1440, true).event;
        assert_eq!(event_time(&e), "All day · 2026-11-09");
        e.end = "2026-11-12".into();
        assert_eq!(event_time(&e), "All day · 2026-11-09 – 2026-11-11");
    }

    #[gpui::test]
    fn far_week_shows_note_without_grid_and_calendar_without_access_offers_consent(cx: &mut TestAppContext) {
        cx.update(|cx| super::super::bind_keys(cx));
        let (view, cx) = cx.add_window_view(|window, cx| MainWindow::new(true, window, cx));
        view.update_in(cx, |v, window, cx| {
            v.demo = false;
            let mut account = mail::demo_accounts().remove(0);
            account.auth = lagom_core::account::Auth::OAuth(lagom_core::oauth::Provider::Google);
            v.today.calendar_accounts = vec![(account, true)];
            v.move_week(-10, cx);
            v.show_section(2, window, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("week-unsynced").is_some());
        assert!(cx.debug_bounds("week-grid").is_none());
        view.update(cx, |v, cx| { v.today.calendar_accounts[0].1 = false; cx.notify(); });
        cx.run_until_parked();
        assert!(cx.debug_bounds("allow-calendar-0").is_some());
    }

}

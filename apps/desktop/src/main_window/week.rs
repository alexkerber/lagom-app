use super::*;
use chrono::{Datelike, NaiveDate, Timelike};
use lagom_core::calendar::{AgendaEvent, Event as CalendarEvent};

const HOUR: f32 = 60.;
const MIN_EVENT: f32 = 36.;
const GUTTER: f32 = 48.;
mod panel;
#[cfg(test)]
mod polish_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View { Day, Week, Month }

struct PanelPreference(bool);
impl gpui::Global for PanelPreference {}

impl View {
    fn start(self, day: NaiveDate) -> NaiveDate {
        match self { Self::Day => day, Self::Week => monday(day), Self::Month => monday(day.with_day(1).unwrap()) }
    }
    fn days(self) -> usize { match self { Self::Day => 1, Self::Week => 7, Self::Month => 42 } }
}

fn shift_month(day: NaiveDate, delta: i64) -> NaiveDate {
    let first = day.with_day(1).unwrap();
    if delta < 0 { first.checked_sub_months(chrono::Months::new((-delta) as u32)) }
    else { first.checked_add_months(chrono::Months::new(delta as u32)) }.unwrap_or(first)
}

fn column_fill(t: &Theme, date: NaiveDate, today: NaiveDate) -> Rgba {
    if date == today { Rgba { a: if t.dark { 0.10 } else { 0.06 }, ..t.accent } }
    else if date.weekday().num_days_from_monday() >= 5 { if t.dark { Rgba { a: 0.08, ..gpui::rgb(0) } } else { t.subtle } }
    else { t.bg }
}

fn now_scroll(minute: f32, height: f32) -> f32 {
    (minute * HOUR / 60. - height / 3.).clamp(0., (24. * HOUR - height).max(0.))
}

pub(super) struct Week {
    pub(super) start: NaiveDate,
    pub(super) anchor: NaiveDate,
    pub(super) view: View,
    mini_month: NaiveDate,
    panel: Option<bool>,
    pub(super) guests: Vec<String>,
    guest_input: Option<Entity<TextInput>>,
    contacts: lagom_core::contacts::Contacts,
    contact_account: Option<String>,
    contact_load: Option<Task<()>>,
    suggestions: Vec<lagom_core::contacts::Contact>,
    suggestion: usize,
    guest_error: Option<String>,
    pub(super) scroll_to_now: bool,
    days: Vec<Vec<AgendaEvent>>,
    windows: Vec<Option<(i64, i64)>>,
    pub(super) detail: Option<AgendaEvent>,
    pub(super) editor: Option<new_event::Editor>,
    pub(super) created: Vec<AgendaEvent>,
    drag: Option<(usize, u32, u32)>,
    pub(super) scroll: ScrollHandle,
    tick: Option<Task<()>>,
    loaded_day: Option<NaiveDate>,
}

impl Default for Week {
    fn default() -> Self {
        let scroll = ScrollHandle::new();
        let today = chrono::Local::now().date_naive();
        Self { start: monday(today), anchor: today, view: View::Week, mini_month: today.with_day(1).unwrap(), panel: None,
            guests: vec![], guest_input: None, contacts: Default::default(), contact_account: None, contact_load: None,
            suggestions: vec![], suggestion: 0, guest_error: None, scroll_to_now: true, days: vec![vec![]; 7], windows: vec![], detail: None, editor: None, created: vec![], drag: None, scroll, tick: None, loaded_day: None }
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

fn compact(event: &CalendarEvent) -> bool { !event.all_day && event.end_utc - event.start_utc < 30 * 60 }

fn event_title(event: &CalendarEvent) -> gpui::StyledText {
    gpui::StyledText::new(if compact(event) { format!("{} · {}", event.title, short_time(event)) } else { event.title.clone() })
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

actions!(calendar_week, [PreviousWeek, NextWeek, CurrentWeek, CloseEvent, NewEvent, ToggleCalendarPanel]);
pub(super) fn bind_keys(cx: &mut App) {
    let keys = Some("Calendar && !TextInput && !NewEvent");
    cx.bind_keys([
        KeyBinding::new("enter", crate::text_input::Enter, Some("NewEvent && !TextInput")),
        KeyBinding::new("escape", crate::text_input::Escape, Some("NewEvent && !TextInput")),
        KeyBinding::new("tab", crate::text_input::FocusNext, Some("NewEvent && !TextInput")),
        KeyBinding::new("shift-tab", crate::text_input::FocusPrev, Some("NewEvent && !TextInput")),
        KeyBinding::new("left", PreviousWeek, keys), KeyBinding::new("right", NextWeek, keys),
        KeyBinding::new("t", CurrentWeek, keys), KeyBinding::new("shift-t", CurrentWeek, keys),
        KeyBinding::new("escape", CloseEvent, keys),
        KeyBinding::new("alt-secondary-p", ToggleCalendarPanel, keys),
        KeyBinding::new("n", NewEvent, keys), KeyBinding::new("shift-n", NewEvent, keys),
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
                self.week.days = vec![vec![]; self.week.view.days()];
                let index = (today - self.week.start).num_days();
                if (0..self.week.view.days() as i64).contains(&index) { self.week.days[index as usize] = agenda::demo_events(today); }
                for i in 0..self.week.view.days() {
                    let day = self.week.start + chrono::Duration::days(i as i64);
                    let created = self.demo_created_on(day);
                    self.week.days[i].extend(created);
                }
                let start = monday(today);
                self.week.windows = vec![Some((agenda::day_start(start, &chrono::Local), agenda::day_start(start + chrono::Duration::weeks(1), &chrono::Local)))];
            }
            return;
        };
        let result = (|| -> lagom_core::Result<_> {
            let windows = self.today.calendar_accounts.iter().map(|(account, _)| live.store.calendar_window(&account.id))
                .collect::<lagom_core::Result<Vec<_>>>()?;
            let days = (0..self.week.view.days() as i64).map(|i| {
                let day = self.week.start + chrono::Duration::days(i);
                live.store.today_events(&day.to_string(), agenda::day_start(day, &chrono::Local), agenda::day_start(day.succ_opt().unwrap(), &chrono::Local))
            }).collect::<lagom_core::Result<Vec<_>>>()?;
            Ok((days, windows))
        })();
        match result {
            Ok((days, windows)) => {
                self.week.detail = self.week.detail.as_ref().and_then(|selected| days.iter().flatten().find(|entry|
                    entry.account == selected.account && entry.calendar_id == selected.calendar_id && entry.event.remote_id == selected.event.remote_id).cloned());
                self.week.days = days;
                self.week.windows = windows;
            }
            Err(e) => {
                self.week.days = vec![vec![]; self.week.view.days()];
                self.week.windows.clear();
                self.notify(format!("Could not read calendar: {e}"), cx);
            }
        }
    }

    fn move_week(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.week.editor.is_some() || self.palette.is_some() || self.compose.as_ref().is_some_and(|c| c.read(cx).is_modal()) { return; }
        let today = chrono::Local::now().date_naive();
        let day = if delta == 0 { today } else if self.week.view == View::Month { shift_month(self.week.anchor, delta) }
            else { self.week.anchor.checked_add_signed(chrono::Duration::days(delta * if self.week.view == View::Week { 7 } else { 1 })).unwrap_or(self.week.anchor) };
        self.calendar_date(day, self.week.view, cx);
        if delta == 0 { self.week.scroll_to_now = true; }
        cx.notify();
    }

    pub(super) fn calendar_date(&mut self, day: NaiveDate, view: View, cx: &mut Context<Self>) {
        self.week.anchor = day;
        self.week.view = view;
        self.week.start = view.start(day);
        self.week.mini_month = day.with_day(1).unwrap();
        self.week.detail = None;
        self.week.drag = None;
        self.reload_week(cx);
        cx.notify();
    }

    fn toggle_calendar_panel(&mut self, cx: &mut Context<Self>) {
        let show = !self.week.panel.unwrap_or(false);
        self.week.panel = Some(show);
        cx.set_global(PanelPreference(show));
        cx.notify();
    }

    pub(super) fn week_keys<E: InteractiveElement>(element: E, cx: &mut Context<Self>) -> E {
        element.on_action(cx.listener(|this, _: &PreviousWeek, _, cx| this.move_week(-1, cx)))
            .on_action(cx.listener(|this, _: &NextWeek, _, cx| this.move_week(1, cx)))
            .on_action(cx.listener(|this, _: &CurrentWeek, _, cx| this.move_week(0, cx)))
            .on_action(cx.listener(|this, _: &ToggleCalendarPanel, _, cx| { this.toggle_calendar_panel(cx); }))
            .on_action(cx.listener(|this, _: &NewEvent, window, cx| this.new_event_default(window, cx)))
            .on_action(cx.listener(|this, _: &CloseEvent, window, cx| { this.week.drag = None; this.week.detail = None; this.cancel_new_event(window, cx); }))
    }

    fn event_block(&self, entry: &AgendaEvent, title: gpui::StyledText, day: usize, ix: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let index = self.live.as_ref().and_then(|l| l.accounts.iter().position(|a| a.id == entry.account)).unwrap_or(0);
        let accent = self.account_accent(index);
        let t = self.theme;
        let entry = entry.clone();
        pressable(div().id(("week-event", day * 100_000 + ix)), &t, false)
            .debug_selector(move || format!("week-event-{day}-{ix}"))
            .flex().flex_col().px(px(5.)).py(px(2.)).rounded(px(radius::CONTROL)).overflow_hidden()
            .bg(Rgba { a: if t.dark { 0.22 } else { 0.12 }, ..accent }).border_l_2().border_color(accent)
            .child(div().debug_selector(|| "week-title".into()).w_full().flex_shrink_0().text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM).whitespace_nowrap().text_ellipsis().child(title))
            .when(!entry.event.all_day && !compact(&entry.event), |d| d.child(div().flex().items_center().gap(px(4.)).when(entry.event.join_url.is_some(), |d| d.child(icon("icons/meet.svg", t.ink2, 16.))).child(mono(short_time(&entry.event), t.ink2).flex_1().min_w(px(0.)).flex_shrink_0().whitespace_nowrap().text_ellipsis().debug_selector(|| "week-time".into()))))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.week.detail = Some(entry.clone());
                window.focus(&this.focus);
                cx.notify();
            }))
    }

    pub(super) fn week_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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
        let count = self.week.view.days();
        let show_panel = *self.week.panel.get_or_insert_with(|| cx.try_global::<PanelPreference>().map_or(f32::from(window.viewport_size().width) > 1200., |p| p.0));
        if show_panel { self.prepare_calendar_guests(window, cx); }
        let allowed = self.demo || self.today.calendar_accounts.iter().any(|(_, allowed)| *allowed) || self.week.days.iter().any(|d| !d.is_empty());
        let mut page = div().relative()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let minute = this.grid_minute(event.position.y);
                if let Some((_, _, to)) = &mut this.week.drag { *to = minute; cx.notify(); }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if let Some((day, from, _)) = this.week.drag.take() {
                    let (start, end) = new_event::drag_interval(from, this.grid_minute(event.position.y));
                    this.open_new_event(this.week.start + chrono::Duration::days(day as i64), start, end, window, cx);
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| { this.week.drag = None; cx.notify(); })).flex_1().min_w(px(0.)).p(px(24.)).flex().flex_col().gap(px(16.))
            .child(div().flex().flex_wrap().items_center().gap(px(8.))
                .child(div().flex_1().flex().gap(px(6.)).text_size(px(size::SUBJECT)).font_weight(FontWeight::SEMIBOLD)
                    .min_w(px(170.)).child(div().debug_selector(|| "calendar-title-month".into()).child(self.week.anchor.format("%B").to_string())).child(div().debug_selector(|| "calendar-title-year".into()).text_color(t.ink2).child(self.week.anchor.format("%Y").to_string())))
                .child(div().flex().items_center().border_1().border_color(t.line).rounded(px(radius::CONTROL))
                    .child(icon_button(&t, "previous-week", "icons/chevron-left.svg", "Previous period · ←").on_click(cx.listener(|this, _, window, cx| { this.move_week(-1, cx); window.focus(&this.focus); })))
                    .child(button(&t, "Today", None).on_click(cx.listener(|this, _, window, cx| { this.move_week(0, cx); window.focus(&this.focus); })))
                    .child(icon_button(&t, "next-week", "icons/chevron-right.svg", "Next period · →").on_click(cx.listener(|this, _, window, cx| { this.move_week(1, cx); window.focus(&this.focus); }))))
                .child(div().flex().border_1().border_color(t.line).rounded(px(radius::CONTROL))
                    .children([(View::Day, "Day"), (View::Week, "Week"), (View::Month, "Month")].into_iter().map(|(view, label)|
                        button(&t, label, None).when(view == self.week.view, |d| d.bg(t.sel))
                            .on_click(cx.listener(move |this, _, window, cx| { this.calendar_date(this.week.anchor, view, cx); window.focus(&this.focus); })))))
                .child(button(&t, "New event", Some(t.accent)).on_click(cx.listener(|this, _, window, cx| this.new_event_default(window, cx))))
                .child(icon_button(&t, "calendar-panel-toggle", "icons/calendar-panel.svg", "Toggle calendar panel · ⌘⌥P")
                    .when(show_panel, |d| d.bg(t.sel)).on_click(cx.listener(|this, _, window, cx| { this.toggle_calendar_panel(cx); window.focus(&this.focus); }))));
        page = page.child(self.calendar_controls(cx));
        if !allowed {
            let mut empty = div().flex().flex_1().min_h(px(0.)).gap(px(16.))
                .child(div().flex_1().flex().flex_col().items_center().justify_center().gap(px(8.))
                    .child(icon("icons/calendar.svg", t.ink2, 32.)).child(mono("Connect a calendar to see your events", t.ink2)));
            if show_panel { empty = empty.child(self.calendar_panel(window, cx)); }
            return page.child(empty).child(self.new_event_sheet(cx));
        }
        let mut body = div().flex().flex_1().min_h(px(0.)).gap(px(16.));
        let mut calendar = div().flex().flex_col().flex_1().min_w(px(0.)).gap(px(8.));
        let mut headers = div().flex().flex_shrink_0().pl(px(GUTTER));
        let mut all_day = div().id("week-all-day").debug_selector(|| "week-all-day".into()).flex().flex_shrink_0().min_h(px(32.)).max_h(px(150.)).overflow_y_scroll()
            .child(mono("All day", t.ink2).w(px(GUTTER)).flex_shrink_0());
        let mut hours = div().relative().w(px(GUTTER)).flex_shrink_0().h(px(24. * HOUR));
        for hour in 0..24 {
            hours = hours.child(div().absolute().top(px(hour as f32 * HOUR)).w_full().pr(px(8.)).flex().justify_end().items_start()
                .child(mono(format!("{hour:02}"), t.ink).text_size(px(size::BASE)).debug_selector(move || format!("hour-{hour}")))
                .child(mono("00", t.ink2).text_size(px(size::SMALL)).debug_selector(move || format!("hour-minutes-{hour}"))));
        }
        let mut grid = div().relative().flex().h(px(24. * HOUR)).child(hours);
        for day in 0..if self.week.view == View::Month { 0 } else { count } {
            let date = start + chrono::Duration::days(day as i64);
            let synced = synced_day(date, &self.week.windows);
            headers = headers.child(div().flex_1().min_w(px(0.)).flex().items_center().gap(px(6.)).text_color(t.ink2)
                .child(div().debug_selector(move || format!("weekday-{day}")).child(date.format("%a").to_string()))
                .child(div().debug_selector(move || format!("day-number-{day}")).size(px(30.)).flex().items_center().justify_center().rounded_full().text_size(px(size::SUBJECT))
                    .text_color(if date.weekday().num_days_from_monday() >= 5 { t.ink2 } else { t.ink })
                    .when(date == today, |d| d.bg(Theme::light(DEFAULT_ACCENT).accent).text_color(gpui::rgb(0xFFFFFF)))
                    .child(date.day().to_string())));
            let entries = &self.week.days[day];
            all_day = all_day.child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(3.)).px(px(2.))
                .children(entries.iter().enumerate().filter(|(_, e)| e.event.all_day)
                    .map(|(ix, e)| self.calendar_event_line(e, day, ix, cx))));
            let mut column = div().id(("week-day", day)).debug_selector(move || format!("week-day-{day}"))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if this.week.editor.is_some() { return; }
                    let minute = this.grid_minute(event.position.y);
                    this.week.drag = Some((day, minute.min(1425), minute));
                    cx.notify();
                }))
                .relative().flex_1().min_w(px(0.)).h_full().border_l_1().border_dashed().border_color(t.line)
                .bg(column_fill(&t, date, today));
            if !synced {
                column = column.child(mono("Not synced this far", t.ink2).absolute().top_0().p(px(8.)).debug_selector(move || format!("unsynced-day-{day}")));
            }
            for hour in 0..24 {
                column = column.child(div().absolute().top(px(hour as f32 * HOUR)).w_full().h(px(1.)).bg(t.line));
            }
            // Events sit in normal flow, not absolutely positioned: GPUI shapes and truncates the text of an
            // absolute child before it has a width, which cut titles to a few characters or to "…".
            // Each overlap group is a row of lanes; within a lane a top margin places each event.
            let blocks = layout(entries, date, &chrono::Local);
            let mut events = div().w_full().flex().flex_col();
            let (mut y, mut i) = (0., 0);
            while i < blocks.len() {
                let (top, lanes) = (blocks[i].start, blocks[i].lanes);
                let (mut j, mut bottom) = (i, top);
                while j < blocks.len() && (j == i || blocks[j].start < bottom) { bottom = f32::max(bottom, blocks[j].end); j += 1; }
                let mut row = div().w_full().flex().flex_shrink_0().mt(px((top - y) * HOUR / 60.)).h(px((bottom - top) * HOUR / 60.));
                for lane in 0..lanes {
                    let mut col = div().flex_1().min_w(px(0.)).flex().flex_col();
                    let mut at = top;
                    for b in blocks[i..j].iter().filter(|b| b.lane == lane) {
                        col = col.child(self.event_block(&entries[b.event], event_title(&entries[b.event].event), day, b.event, cx)
                            .flex_shrink_0().mt(px((b.start - at) * HOUR / 60.)).h(px((b.end - b.start) * HOUR / 60.)));
                        at = b.end;
                    }
                    row = row.child(col);
                }
                events = events.child(row);
                (y, i) = (bottom, j);
            }
            column = column.child(events);
            if let Some((selected, from, to)) = self.week.drag {
                if selected == day {
                    let (start, end) = new_event::drag_interval(from, to);
                    column = column.child(div().absolute().top(px(start as f32 * HOUR / 60.)).h(px((end - start) as f32 * HOUR / 60.))
                        .w_full().bg(Rgba { a: 0.2, ..t.accent }).border_1().border_color(t.accent));
                }
            }
            grid = grid.child(column);
        }
        let today_index = (today - start).num_days();
        if (0..count as i64).contains(&today_index) && self.week.view != View::Month {
            let gold = Theme::dark(crate::theme::Accent::Gold).accent;
            grid = grid.child(div().absolute().top(px((now.hour() as f32 + now.minute() as f32 / 60.) * HOUR)).left_0().right_0()
                .debug_selector(|| "current-time".into()).h(px(1.)).flex().items_center()
                .child(mono(now.format("%H:%M").to_string(), Theme::light(DEFAULT_ACCENT).ink).w(px(GUTTER)).flex_shrink_0()
                    .text_center().rounded(px(radius::CONTROL)).bg(gold))
                .child(div().relative().flex_1().h(px(1.)).bg(gold)
                    .child(div().absolute().left(relative(today_index as f32 / count as f32)).top(px(-3.)).size(px(7.)).rounded_full().bg(gold))));
        }
        if self.week.view == View::Month {
            calendar = calendar.child(self.calendar_month(cx));
        } else {
            calendar = calendar.child(headers).child(all_day)
                .when((0..count).any(|i| !synced_day(start + chrono::Duration::days(i as i64), &self.week.windows)), |d|
                    d.child(mono("Not synced this far", t.ink2).debug_selector(|| "week-unsynced".into())))
                .when(self.week.days.iter().any(|d| !d.is_empty()) || (0..count).any(|i| synced_day(start + chrono::Duration::days(i as i64), &self.week.windows)), |d|
                    d.child(div().id("week-grid").debug_selector(|| "week-grid".into()).relative().flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&self.week.scroll).child(grid)
                        .when(self.week.scroll_to_now, |d| {
                            let entity = cx.entity().downgrade();
                            d.child(gpui::canvas(|bounds, _, _| bounds.size.height, move |_, height, _, cx| {
                                let entity = entity.clone();
                                cx.defer(move |cx| {
                                    entity.update(cx, |this, cx| {
                                        if this.week.scroll_to_now && height > px(0.) {
                                            this.week.scroll_to_now = false;
                                            this.week.scroll.set_offset(gpui::point(px(0.), px(-now_scroll((now.hour() * 60 + now.minute()) as f32, f32::from(height)))));
                                            cx.notify();
                                        }
                                    }).ok();
                                });
                            }).absolute().inset_0())
                        })));
        }
        body = body.child(calendar);
        if show_panel { body = body.child(self.calendar_panel(window, cx)); }
        page = page.child(body).child(mono("← → navigate · T today · N new event", t.ink2));
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
        page = page.child(self.new_event_sheet(cx));
        page
    }

    fn grid_minute(&self, y: Pixels) -> u32 {
        new_event::snap_minute(f32::from(y - self.week.scroll.bounds().top() - self.week.scroll.offset().y) * 60. / HOUR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn entry(day: NaiveDate, start: u32, end: u32, all_day: bool) -> AgendaEvent {
        let midnight = day.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        AgendaEvent { account: "fixture".into(), calendar: "Fixture calendar".into(), calendar_id: "primary".into(), event: CalendarEvent {
            remote_id: format!("{start}-{end}"), title: "Fixture event".into(), start: day.to_string(), end: day.succ_opt().unwrap().to_string(),
            start_utc: midnight + start as i64 * 60, end_utc: midnight + end as i64 * 60,
            all_day, location: Some("Fixture room".into()), join_url: Some("https://meet.example.test/fixture".into()),
            html_link: Some("https://www.google.com/calendar/event?eid=fixture".into()),
        } }
    }

    struct EventPreview { view: Entity<MainWindow>, event: AgendaEvent, width: f32, text: gpui::TextLayout }

    impl Render for EventPreview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let title = event_title(&self.event.event);
            self.text = title.layout().clone();
            div().relative().w(px(self.width)).h(px(45.)).child(self.view.update(cx, |view, cx| view.event_block(&self.event, title, 0, 0, cx).absolute().w(relative(1.)).h_full()))
        }
    }

    #[gpui::test]
    fn review_week_title_uses_full_block_width(cx: &mut TestAppContext) {
        let (view, _) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
        let mut event = entry(NaiveDate::from_ymd_opt(2026, 11, 9).unwrap(), 540, 585, false);
        event.event.title = "Plan the next sprint".into();
        assert_eq!(event.event.title.chars().count(), 20);
        let (preview, cx) = cx.add_window_view(|_, _| EventPreview { view, event, width: 130., text: Default::default() });
        cx.run_until_parked();
        let rendered = preview.read_with(cx, |p, _| p.text.text());
        // The headless font is monospace: 20 glyphs at 11 px need 132 px before padding.
        assert_eq!(rendered, "Plan the next sp…");
        let block = cx.debug_bounds("week-event-0-0").unwrap();
        let title = cx.debug_bounds("week-title").unwrap();
        let time = cx.debug_bounds("week-time").unwrap();
        assert_eq!(block.size.width, px(130.));
        assert_eq!(title.size.width, px(118.), "title must use the block minus padding and border");
        assert!(time.top() >= title.bottom(), "time must be on the second line");
        assert!(time.bottom() <= block.bottom(), "both lines must fit a 45-minute event");
        preview.update(cx, |p, cx| { p.width = 160.; cx.notify(); });
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |p, _| p.text.text()), "Plan the next sprint");
    }

    #[gpui::test]
    fn review_week_short_events_use_one_line_below_thirty_minutes(cx: &mut TestAppContext) {
        for minutes in [15, 29, 30, 45] {
            let (view, _) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
            let mut event = entry(NaiveDate::from_ymd_opt(2026, 11, 9).unwrap(), 540, 540 + minutes, false);
            event.event.title = "Planning".into();
            let expected = if minutes < 30 { format!("Planning · {}", short_time(&event.event)) } else { "Planning".into() };
            let (preview, cx) = cx.add_window_view(|_, _| EventPreview { view, event, width: 240., text: Default::default() });
            cx.run_until_parked();
            assert_eq!(preview.read_with(cx, |p, _| p.text.text()), expected);
            assert_eq!(cx.debug_bounds("week-time").is_some(), minutes >= 30);
        }
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
            assert_eq!(v.week.days.iter().map(Vec::len).sum::<usize>(), 4);
            v.move_week(1, cx);
            assert!(v.week.days.iter().all(Vec::is_empty));
            v.move_week(-1, cx);
            assert_eq!(v.week.days[day], v.today.events);
            assert_eq!(v.week.days.iter().map(Vec::len).sum::<usize>(), 4);
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

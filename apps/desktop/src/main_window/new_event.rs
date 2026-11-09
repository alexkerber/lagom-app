use super::*;
use chrono::{NaiveDate, NaiveTime, TimeZone, Timelike};
use lagom_core::{calendar::{AgendaEvent, NewEvent}, oauth};

pub(super) struct Editor {
    fields: [Entity<TextInput>; 7],
    calendars: Vec<(String, String, String)>,
    calendar: usize,
    all_day: bool,
    meet: bool,
    error: Option<String>,
    pending: Option<u64>,
}

const TITLE: usize = 0;
const DATE: usize = 1;
const START: usize = 2;
const END_DATE: usize = 3;
const END: usize = 4;
const GUESTS: usize = 5;
const DESCRIPTION: usize = 6;

pub(super) fn snap_minute(minute: f32) -> u32 { ((minute.clamp(0., 1440.) / 15.).floor() as u32) * 15 }
pub(super) fn drag_interval(from: u32, to: u32) -> (u32, u32) {
    let start = from.min(to).min(1425);
    (start, from.max(to).max(start + 15).min(1440))
}

fn interval<T: TimeZone>(date: &str, end_date: &str, start: &str, end: &str, all_day: bool, zone: &T) -> Result<(String, String), String> {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "Enter dates as YYYY-MM-DD")?;
    let end_date = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").map_err(|_| "Enter dates as YYYY-MM-DD")?;
    if all_day {
        if end_date < date { return Err("Last day must be on or after the first day".into()); }
        return Ok((date.to_string(), end_date.succ_opt().ok_or("End date is out of range")?.to_string()));
    }
    let time = |value| {
        NaiveTime::parse_from_str(value, "%H:%M").ok().filter(|t| t.minute() % 15 == 0)
            .ok_or("Use 15-minute steps, for example 09:00 or 09:15")
    };
    let local = |date: NaiveDate, value| zone.from_local_datetime(&date.and_time(time(value)?)).single()
        .ok_or("This local time is skipped or repeated by daylight saving; choose an unambiguous time");
    let (start, end) = (local(date, start)?, local(end_date, end)?);
    if end <= start { return Err("End must be later than start".into()); }
    Ok((start.to_rfc3339(), end.to_rfc3339()))
}

impl MainWindow {
    pub(super) fn new_event_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = chrono::Local::now();
        let today = now.date_naive();
        let day = match self.week.view {
            week::View::Week if (0..7).contains(&(today - self.week.start).num_days()) => today,
            week::View::Week => self.week.start,
            _ => self.week.anchor,
        };
        let minute = if day == today { ((now.hour() * 60 + now.minute() + 14) / 15 * 15).min(1425) } else { 9 * 60 };
        self.open_new_event(day, minute, minute + 60, window, cx);
    }

    pub(super) fn open_new_event(&mut self, day: NaiveDate, start: u32, end: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.week.editor.is_some() || self.palette.is_some() || !matches!(self.page, Page::Calendar) { return; }
        let calendars = (|| -> lagom_core::Result<Vec<_>> {
            if self.demo { return Ok(vec![("demo".into(), "demo".into(), "Demo calendar".into())]); }
            let Some(live) = &self.live else { return Ok(vec![]) };
            let mut calendars = Vec::new();
            for account in &live.accounts {
                if account.auth != lagom_core::account::Auth::OAuth(oauth::Provider::Google)
                    || !oauth::calendar_writable(&live.store.account_scopes(&account.id)?) { continue; }
                for calendar in live.store.calendars(&account.id)?.into_iter().filter(|c| c.writable) {
                    calendars.push((account.id.clone(), calendar.remote_id, format!("{} · {}", calendar.name, account.email)));
                }
            }
            Ok(calendars)
        })();
        let calendars = match calendars {
            Ok(calendars) if !calendars.is_empty() => calendars,
            Ok(_) => { self.notify("Allow calendar access and sync a writable Google calendar first", cx); return; }
            Err(error) => { self.notify(format!("Could not read calendars: {error}"), cx); return; }
        };
        let at = |minutes: u32| day.and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::minutes(minutes as i64);
        let values = [String::new(), day.to_string(), at(start).format("%H:%M").to_string(), at(end).date().to_string(),
            at(end).format("%H:%M").to_string(), self.calendar_guests(cx).join(", "), String::new()];
        let placeholders = ["Event title", "YYYY-MM-DD", "09:00", "YYYY-MM-DD", "10:00", "guest@example.com, guest2@example.com", "Description (optional)"];
        let fields = std::array::from_fn(|i| cx.new(|cx| TextInput::new(window, cx).bordered().placeholder(placeholders[i]).with_text(&values[i])));
        let current = self.live.as_ref().and_then(|l| l.account());
        let calendar = current.and_then(|account| calendars.iter().position(|c| c.0 == account.id)).unwrap_or(0);
        self.week.detail = None;
        self.week.editor = Some(Editor { fields, calendars, calendar, all_day: false, meet: false, error: None, pending: None });
        window.focus(&self.week.editor.as_ref().unwrap().fields[TITLE].focus_handle(cx));
        cx.notify();
    }

    pub(super) fn cancel_new_event(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.week.editor.as_ref().is_some_and(|e| e.pending.is_some()) { return; }
        self.week.editor = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn event_draft(&self, cx: &App) -> Result<NewEvent, String> {
        let editor = self.week.editor.as_ref().ok_or("No event is open")?;
        let text = |i: usize| editor.fields[i].read(cx).text();
        let (start, end) = interval(text(DATE), text(END_DATE), text(START), text(END), editor.all_day, &chrono::Local)?;
        let time_zone = if editor.all_day { String::new() } else {
            iana_time_zone::get_timezone().map_err(|_| "Could not read the local time zone")?
        };
        let draft = NewEvent {
            title: text(TITLE).trim().into(), start, end, time_zone, all_day: editor.all_day,
            description: (!text(DESCRIPTION).trim().is_empty()).then(|| text(DESCRIPTION).to_owned()),
            attendees: lagom_core::contacts::split_recipients(text(GUESTS)).iter()
                .map(|guest| lagom_core::contacts::recipient_address(guest).ok_or("Enter valid guest email addresses".to_owned()))
                .collect::<Result<Vec<_>, _>>()?, meet: editor.meet,
        };
        draft.event("validation".into()).map_err(|e| e.to_string())?;
        Ok(draft)
    }

    fn save_new_event(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.week.editor else { return };
        if editor.pending.is_some() { return; }
        let draft = match self.event_draft(cx) {
            Ok(draft) => draft,
            Err(error) => { self.week.editor.as_mut().unwrap().error = Some(error); cx.notify(); return; }
        };
        let (account, calendar, name) = editor.calendars[editor.calendar].clone();
        if self.demo {
            let event = draft.event(format!("demo-created-{}", self.week.created.len())).unwrap();
            self.week.created.push(AgendaEvent { account, calendar: name, calendar_id: calendar, event });
            self.week.editor = None;
            self.reload_agenda(cx);
            self.reload_week(cx);
            window.focus(&self.focus);
        } else if let Some(live) = &self.live {
            let op = live.event_op();
            if live.bridge.send(Cmd::CreateEvent { op, account, calendar, event: draft }) {
                let editor = self.week.editor.as_mut().unwrap();
                editor.pending = Some(op);
                editor.error = None;
                window.focus(&self.focus);
            } else { self.worker_gone(cx); }
        }
        cx.notify();
    }

    pub(super) fn event_created(&mut self, op: u64, error: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.week.editor.as_mut().filter(|e| e.pending == Some(op)) {
            editor.pending = None;
            if let Some(error) = error {
                editor.error = Some(error);
                if matches!(self.page, Page::Calendar) { window.focus(&editor.fields[TITLE].focus_handle(cx)); }
            }
            else {
                self.week.editor = None;
                window.focus(&self.focus);
                self.notify("Event created", cx);
            }
        } else if let Some(error) = error { self.notify(format!("Could not create event: {error}"), cx); }
        cx.notify();
    }

    pub(super) fn event_worker_gone(&mut self) {
        if let Some(editor) = &mut self.week.editor {
            if editor.pending.take().is_some() {
                editor.error = Some("Event creation unconfirmed. Check your calendar before retrying.".into());
            }
        }
    }

    pub(super) fn demo_created_on(&self, day: NaiveDate) -> Vec<AgendaEvent> {
        let (start, end) = (agenda::day_start(day, &chrono::Local), agenda::day_start(day.succ_opt().unwrap(), &chrono::Local));
        let date = day.to_string();
        self.week.created.iter().filter(|e| if e.event.all_day { e.event.start <= date && e.event.end > date }
            else { e.event.start_utc < end && e.event.end_utc > start }).cloned().collect()
    }

    fn nudge_event_time(&mut self, field: usize, step: i64, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.week.editor else { return };
        if editor.pending.is_some() { return; }
        let input = editor.fields[field].clone();
        let Ok(time) = NaiveTime::parse_from_str(input.read(cx).text(), "%H:%M") else { return };
        let minute = (time.hour() as i64 * 60 + time.minute() as i64).div_euclid(15) * 15 + step * 15;
        let date_field = if field == START { DATE } else { END_DATE };
        if !(0..1440).contains(&minute) {
            let date = editor.fields[date_field].clone();
            let Ok(day) = NaiveDate::parse_from_str(date.read(cx).text(), "%Y-%m-%d") else { return };
            let Some(day) = day.checked_add_signed(chrono::Duration::days(minute.div_euclid(1440))) else { return };
            date.update(cx, |input, cx| input.set_text(&day.to_string(), cx));
        }
        let minute = minute.rem_euclid(1440);
        input.update(cx, |input, cx| input.set_text(&format!("{:02}:{:02}", minute / 60, minute % 60), cx));
        cx.notify();
    }

    pub(super) fn new_event_sheet(&self, cx: &mut Context<Self>) -> Div {
        let Some(editor) = &self.week.editor else { return div() };
        let t = self.theme;
        let pending = editor.pending.is_some();
        let field = |index, label: &'static str, cx: &mut Context<Self>| div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.))
            .child(mono(label, t.ink2))
            .child(div().debug_selector(move || format!("event-field-{index}"))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if !matches!(index, START | END) || event.keystroke.modifiers != gpui::Modifiers::none() { return; }
                    let step = match event.keystroke.key.as_str() { "up" => 1, "down" => -1, _ => return };
                    this.nudge_event_time(index, step, cx);
                    cx.stop_propagation();
                }))
                .child(editor.fields[index].clone()));
        let mut sheet = div().id("new-event-sheet").debug_selector(|| "new-event-sheet".into())
            .key_context("NewEvent").relative().occlude().w(px(400.)).max_h(relative(0.95)).overflow_y_scroll().p(px(20.))
            .rounded(px(radius::PANEL)).border_1().border_color(t.line).bg(t.raised).shadow_lg()
            .flex().flex_col().gap(px(12.))
            .on_action(|_: &crate::text_input::FocusNext, window, _| window.focus_next())
            .on_action(|_: &crate::text_input::FocusPrev, window, _| window.focus_prev())
            .capture_action(cx.listener(|this, _: &crate::text_input::Enter, window, cx| { this.save_new_event(window, cx); cx.stop_propagation(); }))
            .capture_action(cx.listener(|this, _: &crate::text_input::Escape, window, cx| { this.cancel_new_event(window, cx); cx.stop_propagation(); }))
            .child(div().text_size(px(size::TITLE)).font_weight(FontWeight::SEMIBOLD).child("New event"))
            .child(field(TITLE, "Title", cx))
            .child(div().flex().gap(px(8.)).child(field(DATE, "Date", cx)).child(field(END_DATE, if editor.all_day { "Last day" } else { "End date" }, cx)))
            .when(!editor.all_day, |d| d.child(div().flex().gap(px(8.)).child(field(START, "Start · ↑ ↓ 15 min", cx)).child(field(END, "End · ↑ ↓ 15 min", cx))))
            .child(crate::settings::toggle_row(&t, "All day", editor.all_day).child(icon("icons/sun.svg", t.ink2, 16.)).focusable().tab_stop(true)
                .focus(|s| s.border_1().border_color(t.accent))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "space" {
                        if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.all_day = !editor.all_day; cx.notify(); }
                        cx.stop_propagation();
                    }
                })).on_click(cx.listener(|this, _, _, cx| {
                if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.all_day = !editor.all_day; cx.notify(); }
            })));
        if editor.calendars.len() > 1 {
            sheet = sheet.child(div().id("event-calendars").max_h(px(96.)).overflow_y_scroll().flex().flex_col().gap(px(4.))
                .children(editor.calendars.iter().enumerate().map(|(index, (_, _, label))| {
                    pressable(div().id(("event-calendar", index)), &t, index == editor.calendar).px(px(8.)).py(px(6.)).rounded(px(radius::CONTROL))
                        .debug_selector(move || format!("event-calendar-{index}"))
                        .focusable().tab_stop(true).focus(|s| s.border_1().border_color(t.accent))
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if event.keystroke.key == "space" {
                                if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.calendar = index; cx.notify(); }
                                cx.stop_propagation();
                            }
                        }))
                        .when(index == editor.calendar, |d| d.bg(t.sel)).child(label.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.calendar = index; cx.notify(); }
                        }))
                })));
        } else { sheet = sheet.child(mono(editor.calendars[editor.calendar].2.clone(), t.ink2)); }
        sheet = sheet
            .child(crate::settings::toggle_row(&t, "Add Google Meet", editor.meet).child(icon("icons/camera.svg", t.ink2, 16.)).focusable().tab_stop(true)
                .focus(|s| s.border_1().border_color(t.accent))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "space" {
                        if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.meet = !editor.meet; cx.notify(); }
                        cx.stop_propagation();
                    }
                })).on_click(cx.listener(|this, _, _, cx| {
                if let Some(editor) = this.week.editor.as_mut().filter(|e| e.pending.is_none()) { editor.meet = !editor.meet; cx.notify(); }
            })))
            // shortcut: Compose's recipient widget owns draft state; share it when extracting a reusable recipient editor.
            .child(field(GUESTS, "Guests · email addresses (optional)", cx))
            .child(field(DESCRIPTION, "Description", cx))
            .when(self.demo && editor.meet, |d| d.child(mono("Demo: no meeting link or invitations will be created", t.ink2)))
            .when_some(editor.error.clone(), |d, error| d.child(div().text_color(t.danger).child(error)))
            .child(div().flex().justify_end().gap(px(8.))
                .child(button(&t, "Cancel", None).focusable().tab_stop(true).focus(|s| s.border_1().border_color(t.accent))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "space" { this.cancel_new_event(window, cx); cx.stop_propagation(); }
                    })).on_click(cx.listener(|this, _, window, cx| this.cancel_new_event(window, cx))))
                .child(button(&t, if pending { "Saving…" } else { "Save event" }, Some(t.accent)).focusable().tab_stop(true).focus(|s| s.border_1().border_color(t.ink))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "space" { this.save_new_event(window, cx); cx.stop_propagation(); }
                    })).on_click(cx.listener(|this, _, window, cx| this.save_new_event(window, cx)))));
        if pending { sheet = sheet.child(div().absolute().inset_0().occlude()); }
        div().absolute().inset_0().flex().items_center().justify_center().occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(sheet.with_animation("new-event-arrive", Animation::new(Duration::from_millis(200)), |d, v|
                d.mt(px(12. * (1. - spring(v)))).opacity(ease_out_quint()(v))))
    }
}

#[cfg(test)]
mod tests;

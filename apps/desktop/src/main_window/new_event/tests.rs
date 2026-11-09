use super::*;
use chrono::Datelike;
use gpui::TestAppContext;

fn keys(cx: &mut TestAppContext) {
    cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
}

#[test]
fn snapping_reverse_drag_and_day_boundaries() {
    for (minute, expected) in [(-20., 0), (8., 0), (14.9, 0), (15., 15), (587., 585), (1445., 1440)] {
        assert_eq!(snap_minute(minute), expected);
    }
    for (from, to, expected) in [(585, 630, (585, 630)), (630, 585, (585, 630)),
        (585, 585, (585, 600)), (1425, 1440, (1425, 1440)), (1440, 1440, (1425, 1440))] {
        assert_eq!(drag_interval(from, to), expected);
    }
}

// The fixture uses the real 2026 spring transition without changing process-global TZ.
#[derive(Clone)]
struct Spring;
impl TimeZone for Spring {
    type Offset = chrono::FixedOffset;
    fn from_offset(_: &Self::Offset) -> Self { Self }
    fn offset_from_local_date(&self, date: &NaiveDate) -> chrono::LocalResult<Self::Offset> {
        self.offset_from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
    }
    fn offset_from_local_datetime(&self, time: &chrono::NaiveDateTime) -> chrono::LocalResult<Self::Offset> {
        let day = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
        let hour = if *time < day.and_hms_opt(2, 0, 0).unwrap() { 1 }
            else if *time < day.and_hms_opt(3, 0, 0).unwrap() { return chrono::LocalResult::None }
            else { 2 };
        chrono::LocalResult::Single(chrono::FixedOffset::east_opt(hour * 3600).unwrap())
    }
    fn offset_from_utc_date(&self, date: &NaiveDate) -> Self::Offset { self.offset_from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap()) }
    fn offset_from_utc_datetime(&self, time: &chrono::NaiveDateTime) -> Self::Offset {
        let transition = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(1, 0, 0).unwrap();
        chrono::FixedOffset::east_opt(if *time < transition { 3600 } else { 7200 }).unwrap()
    }
}

#[test]
fn local_interval_resolves_each_dst_offset_and_rejects_missing_times() {
    let (start, end) = interval("2026-03-29", "2026-03-29", "01:30", "03:30", false, &Spring).unwrap();
    assert_eq!(start, "2026-03-29T01:30:00+01:00");
    assert_eq!(end, "2026-03-29T03:30:00+02:00");
    assert_eq!(chrono::DateTime::parse_from_rfc3339(&end).unwrap().timestamp() - chrono::DateTime::parse_from_rfc3339(&start).unwrap().timestamp(), 3600);
    assert!(interval("2026-03-29", "2026-03-29", "02:15", "03:30", false, &Spring).is_err());
    assert!(interval("2026-03-29", "2026-03-29", "01:31", "03:30", false, &Spring).is_err());
    assert!(interval("2026-03-29", "2026-03-29", "03:30", "01:30", false, &Spring).is_err());
    assert_eq!(interval("2026-03-29", "2026-03-29", "", "", true, &Spring).unwrap(), ("2026-03-29".into(), "2026-03-30".into()));
    assert_eq!(interval("2026-12-31", "2027-01-02", "", "", true, &Spring).unwrap(), ("2026-12-31".into(), "2027-01-03".into()));
    assert!(interval("2026-12-31", "2026-12-30", "", "", true, &Spring).is_err());
}

#[gpui::test]
fn n_opens_sheet_escape_discards_and_demo_return_updates_week_and_home(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    cx.simulate_keystrokes("n");
    cx.run_until_parked();
    assert!(cx.debug_bounds("new-event-sheet").is_some());
    cx.simulate_input("Discard this");
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none() && v.week.created.is_empty() && v.live.is_none()));
    cx.simulate_keystrokes("shift-n");
    cx.simulate_input("Fixture meeting");
    view.update(cx, |v, cx| {
        let editor = v.week.editor.as_mut().unwrap();
        let today = chrono::Local::now().date_naive().to_string();
        for (field, text) in [(DATE, today.as_str()), (END_DATE, today.as_str()), (START, "09:00"), (END, "09:45")] {
            editor.fields[field].update(cx, |input, cx| input.set_text(text, cx));
        }
        editor.meet = true;
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.live.is_none());
        assert!(v.week.editor.is_none());
        assert_eq!(v.week.created.len(), 1);
        assert_eq!(v.week.created[0].event.title, "Fixture meeting");
        assert_eq!(v.week.created[0].event.join_url, None, "demo must not invent a Meet link");
        assert!(v.today.events.iter().any(|e| e.event.title == "Fixture meeting"));
    });
    let day = chrono::Local::now().weekday().num_days_from_monday();
    assert!(cx.debug_bounds(format!("week-event-{day}-3").leak()).is_some());
    cx.simulate_keystrokes("right left");
    assert_eq!(view.read_with(cx, |v, _| v.week.created.len()), 1);
    assert!(cx.debug_bounds(format!("week-event-{day}-3").leak()).is_some());
}

#[gpui::test]
fn empty_grid_drag_opens_snapped_sheet_and_event_click_only_opens_detail(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    cx.run_until_parked();
    let day = chrono::Local::now().weekday().num_days_from_monday() as usize;
    let column = cx.debug_bounds(format!("week-day-{day}").leak()).unwrap();
    let at = |minute: f32| gpui::point(column.center().x, column.top() + px(minute));
    cx.simulate_mouse_down(at(607.), MouseButton::Left, Default::default());
    cx.simulate_mouse_move(at(652.), MouseButton::Left, Default::default());
    cx.simulate_mouse_up(at(652.), MouseButton::Left, Default::default());
    view.read_with(cx, |v, cx| {
        let editor = v.week.editor.as_ref().expect("drag opens sheet");
        assert_eq!(editor.fields[START].read(cx).text(), "10:00");
        assert_eq!(editor.fields[END].read(cx).text(), "10:45");
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let event = cx.debug_bounds(format!("week-event-{day}-0").leak()).unwrap();
    cx.simulate_click(event.center(), Default::default());
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none() && v.week.detail.is_some()));
}

#[gpui::test]
fn demo_screen_nudges_times_across_midnight_and_validates_before_saving(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "new-event", window, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("new-event-sheet").is_some());
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |v, _| v.week.editor.as_ref().unwrap().error.is_some()));
    view.update_in(cx, |v, window, cx| {
        let editor = v.week.editor.as_mut().unwrap();
        editor.fields[END_DATE].update(cx, |input, cx| input.set_text("2026-12-31", cx));
        editor.fields[END].update(cx, |input, cx| input.set_text("23:45", cx));
        window.focus(&editor.fields[END].focus_handle(cx));
    });
    cx.simulate_keystrokes("up");
    view.read_with(cx, |v, cx| {
        let editor = v.week.editor.as_ref().unwrap();
        assert_eq!(editor.fields[END].read(cx).text(), "00:00");
        assert_eq!(editor.fields[END_DATE].read(cx).text(), "2027-01-01");
    });
    cx.simulate_keystrokes("down");
    view.read_with(cx, |v, cx| {
        let editor = v.week.editor.as_ref().unwrap();
        assert_eq!(editor.fields[END].read(cx).text(), "23:45");
        assert_eq!(editor.fields[END_DATE].read(cx).text(), "2026-12-31");
    });
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none()));
}

#[gpui::test]
fn n_opens_demo_sheet_outside_cached_weeks(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    cx.simulate_keystrokes("right right right n");
    cx.run_until_parked();
    assert!(cx.debug_bounds("new-event-sheet").is_some());
    view.read_with(cx, |v, cx| {
        assert_eq!(v.week.editor.as_ref().unwrap().fields[DATE].read(cx).text(), v.week.start.to_string());
    });
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none() && v.week.created.is_empty()));
}

#[gpui::test]
fn sheet_switches_and_optional_fields_work_from_keyboard(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "new-event", window, cx));
    cx.simulate_input("Keyboard fixture");
    cx.simulate_keystrokes("tab tab tab tab tab space tab space tab");
    cx.simulate_input("guest@example.test");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("Fixture notes");
    view.read_with(cx, |v, cx| {
        let draft = v.event_draft(cx).unwrap();
        assert!(draft.all_day && draft.meet);
        assert_eq!(draft.attendees, ["guest@example.test"]);
        assert_eq!(draft.description.as_deref(), Some("Fixture notes"));
        assert_eq!(draft.start.len(), 10);
        assert_eq!(draft.end.len(), 10);
    });
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none()));
}

#[gpui::test]
fn worker_loss_releases_pending_sheet_and_keeps_the_draft(cx: &mut TestAppContext) {
    keys(cx);
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "new-event", window, cx));
    cx.simulate_input("Keep fixture");
    view.update(cx, |v, cx| {
        v.week.editor.as_mut().unwrap().pending = Some(7);
        v.event_worker_gone();
        let editor = v.week.editor.as_ref().unwrap();
        assert_eq!(editor.pending, None);
        assert!(editor.error.as_ref().unwrap().contains("unconfirmed"));
        assert_eq!(editor.fields[TITLE].read(cx).text(), "Keep fixture");
    });
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_none()));
}

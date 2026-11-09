use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let at = cx.debug_bounds(selector.to_owned().leak()).unwrap_or_else(|| panic!("missing {selector}")).center();
    cx.simulate_click(at, Default::default());
    cx.run_until_parked();
}

#[test]
fn calendar_ranges_cover_six_weeks_and_navigation_crosses_years_and_leap_days() {
    for (date, start) in [("2026-11-09", "2026-10-26"), ("2027-01-03", "2026-12-28"), ("2028-02-29", "2028-01-31")] {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        assert_eq!(View::Month.start(date).to_string(), start);
        assert_eq!(View::Month.days(), 42);
        assert_eq!(View::Day.start(date), date);
        assert_eq!(View::Day.days(), 1);
        let last = View::Month.start(date) + chrono::Duration::days(41);
        assert!(last >= date.with_day(1).unwrap().checked_add_months(chrono::Months::new(1)).unwrap().pred_opt().unwrap());
    }
    let day = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
    assert_eq!(shift_month(day, 1).to_string(), "2027-01-01");
    assert_eq!(shift_month(day, -1).to_string(), "2026-11-01");
}

#[test]
fn now_scroll_positions_one_third_down_and_clamps_at_day_edges() {
    assert_eq!(now_scroll(942., 600.), 742.);
    assert_eq!(now_scroll(30., 600.), 0.);
    assert_eq!(now_scroll(1439., 600.), 840.);
    assert_eq!(now_scroll(942., 1600.), 0.);
}

#[gpui::test]
fn day_week_month_controls_load_their_ranges_and_month_overflow_opens_day(cx: &mut TestAppContext) {
    cx.update(|cx| super::super::bind_keys(cx));
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar-month", window, cx));
    cx.simulate_resize(gpui::size(px(1440.), px(1000.)));
    cx.run_until_parked();
    view.read_with(cx, |v, _| { assert_eq!(v.week.view, View::Month); assert_eq!(v.week.days.len(), 42); });
    for index in 0..42 { assert!(cx.debug_bounds(format!("month-cell-{index}").leak()).is_some()); }
    assert_eq!(cx.debug_bounds("calendar-title-month").unwrap().size.height, cx.debug_bounds("calendar-title-year").unwrap().size.height);
    let index = view.read_with(cx, |v, _| (chrono::Local::now().date_naive() - v.week.start).num_days());
    click(cx, &format!("month-more-{index}"));
    view.read_with(cx, |v, _| { assert_eq!(v.week.view, View::Day); assert_eq!(v.week.days.len(), 1); assert_eq!(v.week.start, chrono::Local::now().date_naive()); });
    click(cx, "Week");
    view.read_with(cx, |v, _| { assert_eq!(v.week.view, View::Week); assert_eq!(v.week.days.len(), 7); });
    click(cx, "Day");
    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |v, _| v.week.start), chrono::Local::now().date_naive().succ_opt().unwrap());
    cx.simulate_keystrokes("t");
    click(cx, "Month");
    let next = view.read_with(cx, |v, _| shift_month(v.week.anchor, 1));
    cx.simulate_keystrokes("right");
    assert_eq!(view.read_with(cx, |v, _| v.week.anchor), next);
}

#[gpui::test]
fn week_now_line_spans_grid_and_hour_minutes_are_smaller(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    cx.run_until_parked();
    let line = cx.debug_bounds("current-time").unwrap();
    let grid = cx.debug_bounds("week-grid").unwrap();
    assert_eq!(line.size.width, grid.size.width);
    assert_eq!(line.size.height, px(1.));
    let today = chrono::Local::now().weekday().num_days_from_monday();
    let number = cx.debug_bounds(format!("day-number-{today}").leak()).unwrap();
    let weekday = cx.debug_bounds(format!("weekday-{today}").leak()).unwrap();
    assert_eq!(number.size, gpui::size(px(30.), px(30.)));
    assert!(number.size.height > weekday.size.height);
    let all_day = cx.debug_bounds("week-all-day").unwrap();
    let event = cx.debug_bounds(format!("week-event-{today}-3").leak()).unwrap();
    assert!(event.top() >= all_day.top() && event.bottom() <= all_day.bottom());
    assert!(event.size.height <= px(26.), "all-day titles use one compact line");
    let hour = cx.debug_bounds("hour-13").unwrap();
    let minutes = cx.debug_bounds("hour-minutes-13").unwrap();
    assert!(minutes.size.height < hour.size.height);
    assert_eq!(minutes.top(), hour.top());
    assert_eq!(hour.right(), minutes.left());
}

#[gpui::test]
fn panel_default_is_width_based_and_toggle_survives_navigation_and_resize(cx: &mut TestAppContext) {
    cx.update(|cx| super::super::bind_keys(cx));
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    cx.simulate_keystrokes("secondary-3");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.week.panel), Some(true));
    assert!(cx.debug_bounds("calendar-panel").is_some());
    click(cx, "calendar-panel-toggle");
    cx.simulate_keystrokes("secondary-2 secondary-3");
    cx.simulate_resize(gpui::size(px(1000.), px(900.)));
    assert_eq!(view.read_with(cx, |v, _| v.week.panel), Some(false));
    click(cx, "calendar-panel-toggle");
    assert_eq!(view.read_with(cx, |v, _| v.week.panel), Some(true));
    view.update(cx, |v, cx| { v.week.panel = None; cx.notify(); });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.week.panel), Some(true), "new window state reuses the session preference");
    view.update(cx, |v, cx| { v.week.panel = None; cx.remove_global::<PanelPreference>(); cx.notify(); });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.week.panel), Some(false));
}

#[gpui::test]
fn mini_month_navigation_does_not_change_week_until_a_date_is_clicked(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    view.update(cx, |v, cx| { v.week.panel = Some(true); cx.notify(); });
    cx.run_until_parked();
    let start = view.read_with(cx, |v, _| v.week.start);
    click(cx, "mini-next");
    let next = view.read_with(cx, |v, _| { assert_eq!(v.week.start, start); v.week.mini_month });
    click(cx, &format!("mini-date-{next}"));
    view.read_with(cx, |v, _| { assert_eq!(v.week.start, monday(next)); assert_eq!(v.week.anchor, next); });
    click(cx, "mini-previous");
    assert_eq!(view.read_with(cx, |v, _| v.week.mini_month), shift_month(next, -1));
}

#[gpui::test]
fn meet_with_uses_contact_suggestions_validates_and_keeps_uncommitted_guests(cx: &mut TestAppContext) {
    cx.update(|cx| { super::super::bind_keys(cx); crate::text_input::bind_keys(cx); });
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "calendar", window, cx));
    view.update(cx, |v, cx| { v.week.panel = Some(true); cx.notify(); });
    click(cx, "meet-with");
    cx.simulate_input("plan");
    cx.run_until_parked();
    assert!(cx.debug_bounds("meet-suggestion-0").is_some());
    cx.simulate_keystrokes("down enter");
    assert_eq!(view.read_with(cx, |v, _| v.week.guests.clone()), ["Planning team <planning@example.com>"]);
    cx.simulate_input("invalid");
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |v, _| v.week.guest_error.is_some()));
    view.update(cx, |v, cx| v.week.guest_input.clone().unwrap().update(cx, |i, cx| i.set_text("other@example.test", cx)));
    assert_eq!(view.read_with(cx, |v, cx| v.calendar_guests(cx)), ["Planning team <planning@example.com>", "other@example.test"]);
    click(cx, "calendar-panel-toggle");
    cx.simulate_keystrokes("n");
    assert!(view.read_with(cx, |v, _| v.week.editor.is_some()), "hiding the guest input must return keyboard focus");
}

#[test]
fn today_and_weekend_fills_follow_each_appearance() {
    let monday = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
    let sunday = monday + chrono::Duration::days(6);
    for t in [Theme::light(DEFAULT_ACCENT), Theme::dark(DEFAULT_ACCENT)] {
        let today = column_fill(&t, monday, monday);
        assert_eq!(today.a, if t.dark { 0.10 } else { 0.06 });
        assert_eq!((today.r, today.g, today.b), (t.accent.r, t.accent.g, t.accent.b));
        assert_ne!(column_fill(&t, sunday, monday), t.bg);
        assert_eq!(column_fill(&t, sunday, sunday), today, "today takes precedence at weekends");
        assert_eq!(column_fill(&t, monday.succ_opt().unwrap(), monday), t.bg);
    }
}

#[gpui::test]
fn opening_calendar_scrolls_to_now_once_and_preserves_manual_scroll(cx: &mut TestAppContext) {
    cx.update(|cx| super::super::bind_keys(cx));
    let (view, cx) = cx.add_window_view(|window, cx| MainWindow::with_screen(true, "inbox", window, cx));
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    cx.simulate_keystrokes("secondary-3");
    cx.run_until_parked();
    view.update(cx, |v, cx| {
        let now = chrono::Local::now();
        let height = f32::from(v.week.scroll.bounds().size.height);
        let expected = now_scroll((now.hour() * 60 + now.minute()) as f32, height);
        assert!((f32::from(v.week.scroll.offset().y) + expected).abs() <= 1., "offset {:?}, expected {expected}, height {height}, pending {}", v.week.scroll.offset().y, v.week.scroll_to_now);
        v.week.scroll.set_offset(gpui::point(px(0.), px(-300.)));
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.week.scroll.offset().y), px(-300.));
}

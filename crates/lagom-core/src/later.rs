//! Shared local-calendar Later choices.

use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone};

/// When a message on Later comes back to the inbox. Times are local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaterWhen {
    /// 18:00 today, or tomorrow once 18:00 has passed.
    Evening,
    /// 08:00 tomorrow. L and a left swipe use this.
    Tomorrow,
    /// 08:00 on the next Monday.
    NextWeek,
}

impl LaterWhen {
    pub const fn label(self) -> &'static str {
        match self {
            LaterWhen::Evening => "Later: this evening 18:00",
            LaterWhen::Tomorrow => "Later: tomorrow 08:00",
            LaterWhen::NextWeek => "Later: next week, Monday 08:00",
        }
    }

    pub fn at<T: TimeZone>(self, now: DateTime<T>) -> DateTime<T> {
        let zone = now.timezone();
        let today = now.date_naive();
        let (day, hour) = match self {
            LaterWhen::Evening if now.time() < NaiveTime::from_hms_opt(18, 0, 0).unwrap() => (today, 18),
            LaterWhen::Evening => (today + Duration::days(1), 18),
            LaterWhen::Tomorrow => (today + Duration::days(1), 8),
            LaterWhen::NextWeek => {
                let ahead = 7 - now.weekday().num_days_from_monday() as i64;
                (today + Duration::days(ahead), 8)
            }
        };
        let local = day.and_hms_opt(hour, 0, 0).unwrap();
        // A daylight-saving gap skips the hour: take the first valid time after it.
        zone.from_local_datetime(&local).earliest().unwrap_or_else(|| zone.from_utc_datetime(&local) + Duration::hours(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).earliest().unwrap()
    }
    #[test]
    fn later_times_are_local() {
        // Friday 6 November 2026, 10:12.
        let now = at(2026, 11, 6, 10, 12);
        assert_eq!(LaterWhen::Evening.at(now), at(2026, 11, 6, 18, 0));
        assert_eq!(LaterWhen::Tomorrow.at(now), at(2026, 11, 7, 8, 0));
        assert_eq!(LaterWhen::NextWeek.at(now), at(2026, 11, 9, 8, 0));
        // Past 18:00, "this evening" is tomorrow evening; on a Monday, next week is 7 days on.
        assert_eq!(LaterWhen::Evening.at(at(2026, 11, 6, 19, 0)), at(2026, 11, 7, 18, 0));
        assert_eq!(LaterWhen::NextWeek.at(at(2026, 11, 9, 7, 0)), at(2026, 11, 16, 8, 0));
        // Sunday night to Monday morning, and over a month end.
        assert_eq!(LaterWhen::NextWeek.at(at(2026, 11, 8, 23, 0)), at(2026, 11, 9, 8, 0));
        assert_eq!(LaterWhen::Tomorrow.at(at(2026, 11, 30, 23, 59)), at(2026, 12, 1, 8, 0));
        for w in [LaterWhen::Evening, LaterWhen::Tomorrow, LaterWhen::NextWeek] {
            assert!(w.at(now) > now);
        }
    }

    // Real European 2026 transitions, without mutating the process-wide timezone.
    #[derive(Clone, Debug)]
    struct Europe;
    impl TimeZone for Europe {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &Self::Offset) -> Self { Self }
        fn offset_from_local_date(&self, date: &chrono::NaiveDate) -> chrono::LocalResult<Self::Offset> {
            self.offset_from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, time: &chrono::NaiveDateTime) -> chrono::LocalResult<Self::Offset> {
            let spring = chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(2, 0, 0).unwrap();
            let fall = chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap().and_hms_opt(2, 0, 0).unwrap();
            let winter = chrono::FixedOffset::east_opt(3600).unwrap();
            let summer = chrono::FixedOffset::east_opt(7200).unwrap();
            if *time >= spring && *time < spring + Duration::hours(1) { chrono::LocalResult::None }
            else if *time >= fall && *time < fall + Duration::hours(1) { chrono::LocalResult::Ambiguous(summer, winter) }
            else { chrono::LocalResult::Single(if *time >= spring && *time < fall { summer } else { winter }) }
        }
        fn offset_from_utc_date(&self, date: &chrono::NaiveDate) -> Self::Offset {
            self.offset_from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, time: &chrono::NaiveDateTime) -> Self::Offset {
            let spring = chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(1, 0, 0).unwrap();
            let fall = chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap().and_hms_opt(1, 0, 0).unwrap();
            chrono::FixedOffset::east_opt(if *time >= spring && *time < fall { 7200 } else { 3600 }).unwrap()
        }
    }

    #[test]
    fn later_keeps_wall_clock_across_short_and_long_days() {
        for (month, day, hours) in [(3, 28, 23), (10, 24, 25)] {
            let now = Europe.with_ymd_and_hms(2026, month, day, 8, 0, 0).unwrap();
            let tomorrow = LaterWhen::Tomorrow.at(now.clone());
            assert_eq!(tomorrow, Europe.with_ymd_and_hms(2026, month, day + 1, 8, 0, 0).unwrap());
            assert_eq!((tomorrow - now.clone()).num_hours(), hours);
            assert_eq!(LaterWhen::NextWeek.at(now), Europe.with_ymd_and_hms(2026, month, day + 2, 8, 0, 0).unwrap());
        }
        let now = Europe.with_ymd_and_hms(2026, 3, 28, 18, 0, 0).unwrap();
        assert_eq!(LaterWhen::Evening.at(now), Europe.with_ymd_and_hms(2026, 3, 29, 18, 0, 0).unwrap());
    }
}

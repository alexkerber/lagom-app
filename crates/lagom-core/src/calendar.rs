use crate::{account::Account, store::Store, Error, Result};
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calendar {
    pub remote_id: String,
    pub name: String,
    pub color: Option<String>,
    pub shown: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub remote_id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub start_utc: i64,
    pub end_utc: i64,
    pub all_day: bool,
    pub location: Option<String>,
    pub join_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaEvent {
    pub account: String,
    pub calendar: String,
    pub event: Event,
}

impl Store {
    pub fn account_scopes(&self, account: &str) -> Result<String> {
        Ok(self.db.query_row("SELECT scopes FROM accounts WHERE id=?1", [account], |r| r.get(0))?)
    }

    pub fn set_account_scopes(&self, account: &str, scopes: &str) -> Result<()> {
        if self.db.execute("UPDATE accounts SET scopes=?2 WHERE id=?1", params![account, scopes])? == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    pub fn save_authorized_account(&self, account: &Account, scopes: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        self.save_account(account)?;
        self.set_account_scopes(&account.id, scopes)?;
        tx.commit()?;
        Ok(())
    }

    /// Publish a complete window only after every page and calendar has arrived.
    pub fn replace_calendar_window(&self, account: &str, start: i64, end: i64, calendars: &[(Calendar, Vec<Event>)]) -> Result<()> {
        if start >= end { return Err(Error::Invalid("invalid calendar window")); }
        let tx = self.db.unchecked_transaction()?;
        self.db.execute("UPDATE calendars SET shown=0 WHERE account=?1", [account])?;
        for (calendar, events) in calendars {
            if calendar.remote_id.is_empty() { return Err(Error::Invalid("calendar ID missing")); }
            self.db.execute("INSERT INTO calendars(account,remote_id,name,color,shown,writable) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(account,remote_id) DO UPDATE SET name=excluded.name,color=excluded.color,shown=excluded.shown,writable=excluded.writable",
                params![account, calendar.remote_id, calendar.name, calendar.color, calendar.shown, calendar.writable])?;
            let id: i64 = self.db.query_row("SELECT id FROM calendars WHERE account=?1 AND remote_id=?2",
                params![account, calendar.remote_id], |r| r.get(0))?;
            self.db.execute("DELETE FROM events WHERE calendar=?1 AND start_utc<?3 AND end_utc>?2", params![id, start, end])?;
            for event in events {
                if event.remote_id.is_empty() || event.end_utc <= event.start_utc {
                    return Err(Error::Invalid("invalid calendar event"));
                }
                self.db.execute("INSERT INTO events(calendar,remote_id,title,start,end,start_utc,end_utc,all_day,location,join_url) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                    ON CONFLICT(calendar,remote_id) DO UPDATE SET title=excluded.title,start=excluded.start,end=excluded.end,
                    start_utc=excluded.start_utc,end_utc=excluded.end_utc,all_day=excluded.all_day,location=excluded.location,join_url=excluded.join_url",
                    params![id, event.remote_id, event.title, event.start, event.end, event.start_utc, event.end_utc, event.all_day, event.location, event.join_url])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Timed events overlap the local day's UTC bounds; all-day dates stay date-only.
    pub fn today_events(&self, day: &str, start: i64, end: i64) -> Result<Vec<AgendaEvent>> {
        let mut query = self.db.prepare("SELECT c.account,c.name,e.remote_id,e.title,e.start,e.end,e.start_utc,e.end_utc,e.all_day,e.location,e.join_url
            FROM events e JOIN calendars c ON c.id=e.calendar WHERE c.shown=1 AND
            ((e.all_day=1 AND e.start<=?1 AND e.end>?1) OR (e.all_day=0 AND e.start_utc<?3 AND e.end_utc>?2))
            ORDER BY e.all_day DESC,e.start_utc,c.account,c.name,e.remote_id")?;
        let rows = query.query_map(params![day, start, end], |r| Ok(AgendaEvent {
            account: r.get(0)?, calendar: r.get(1)?, event: Event {
                remote_id: r.get(2)?, title: r.get(3)?, start: r.get(4)?, end: r.get(5)?,
                start_utc: r.get(6)?, end_utc: r.get(7)?, all_day: r.get(8)?, location: r.get(9)?, join_url: r.get(10)?,
            },
        }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;

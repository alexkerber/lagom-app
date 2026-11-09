mod create;
pub use create::{create_event, create_with};

use crate::{account::Account, calendar::{Calendar, Event}, oauth, Error, Result};
use serde::Deserialize;
use std::{collections::HashSet, future::Future, time::Duration};
use url::Url;
use zeroize::Zeroizing;

pub async fn fetch_calendar(account: &Account, start: i64, end: i64) -> Result<Vec<(Calendar, Vec<Event>)>> {
    // shortcut: refetch window, move to syncToken/delta when request counts matter.
    tokio::time::timeout(Duration::from_secs(90), fetch_with(
        "https://www.googleapis.com/calendar/v3/", start, end,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await },
    )).await.map_err(|_| Error::Network("calendar sync timed out"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "T: Deserialize<'de>"))]
struct Page<T> {
    #[serde(default)]
    items: Vec<T>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteCalendar {
    id: String,
    #[serde(default)]
    summary: String,
    background_color: Option<String>,
    access_role: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteEvent {
    id: String,
    #[serde(default)]
    status: String,
    summary: Option<String>,
    start: Option<EventTime>,
    end: Option<EventTime>,
    location: Option<String>,
    hangout_link: Option<String>,
    html_link: Option<String>,
    conference_data: Option<Conference>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventTime { date: Option<String>, date_time: Option<String> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Conference { #[serde(default)] entry_points: Vec<EntryPoint> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryPoint { entry_point_type: String, uri: String }

pub async fn fetch_with<F, Fut>(base: &str, start: i64, end: i64, mut token: F) -> Result<Vec<(Calendar, Vec<Event>)>>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    if start >= end { return Err(Error::Invalid("invalid calendar window")); }
    let client = oauth::http(Duration::from_secs(30))?;
    let base = Url::parse(base).map_err(|_| Error::Invalid("invalid calendar endpoint"))?;
    let mut list = base.join("users/me/calendarList").map_err(|_| Error::Invalid("invalid calendar endpoint"))?;
    list.query_pairs_mut().extend_pairs([
        ("minAccessRole", "reader"), ("maxResults", "100"),
        ("fields", "nextPageToken,items(id,summary,backgroundColor,accessRole,hidden,deleted)"),
    ]);
    let calendars: Vec<RemoteCalendar> = pages(&client, list, &mut token).await?;
    let mut result = Vec::new();
    for remote in calendars.into_iter().filter(|c| !c.hidden && !c.deleted && c.access_role != "freeBusyReader") {
        if remote.id.is_empty() { return Err(Error::Network("calendar ID missing")); }
        let mut url = base.clone();
        url.path_segments_mut().map_err(|_| Error::Invalid("invalid calendar endpoint"))?
            .pop_if_empty().extend(["calendars", remote.id.as_str(), "events"]);
        // shortcut: no RRULE, add when CalDAV lands.
        url.query_pairs_mut().extend_pairs([
            ("singleEvents", "true"), ("showDeleted", "false"), ("maxResults", "50"), ("timeZone", "UTC"),
            ("fields", "nextPageToken,items(id,status,summary,start,end,location,hangoutLink,htmlLink,conferenceData/entryPoints(entryPointType,uri))"),
            ("timeMin", &mail_parser::DateTime::from_timestamp(start).to_rfc3339()),
            ("timeMax", &mail_parser::DateTime::from_timestamp(end).to_rfc3339()),
        ]);
        let events: Vec<RemoteEvent> = pages(&client, url, &mut token).await?;
        let events = events.into_iter().filter(|e| e.status != "cancelled").map(event).collect::<Result<_>>()?;
        result.push((Calendar {
            remote_id: remote.id, name: remote.summary, color: remote.background_color, shown: true,
            writable: matches!(remote.access_role.as_str(), "owner" | "writer" | "writerWithoutPrivateAccess"),
        }, events));
    }
    Ok(result)
}

async fn pages<T, F, Fut>(client: &reqwest::Client, url: Url, token: &mut F) -> Result<Vec<T>>
where T: serde::de::DeserializeOwned, F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut next: Option<String> = None;
    loop {
        let mut page_url = url.clone();
        if let Some(page) = &next { page_url.query_pairs_mut().append_pair("pageToken", page); }
        let mut access = token(None).await?;
        let mut response = client.get(page_url.clone()).bearer_auth(access.as_str()).send().await
            .map_err(|_| Error::Network("calendar unavailable"))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            access = token(Some(access.to_string())).await?;
            response = client.get(page_url).bearer_auth(access.as_str()).send().await
                .map_err(|_| Error::Network("calendar unavailable"))?;
        }
        let status = response.status();
        let bytes = oauth::body(response).await?;
        if status == reqwest::StatusCode::UNAUTHORIZED { return Err(Error::ReauthRequired); }
        if status == reqwest::StatusCode::FORBIDDEN {
            let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let scope_error = error["error"]["errors"].as_array().is_some_and(|errors|
                errors.iter().any(|e| e["reason"] == "insufficientPermissions"))
                || error["error"]["details"].as_array().is_some_and(|details|
                    details.iter().any(|e| e["reason"] == "ACCESS_TOKEN_SCOPE_INSUFFICIENT"));
            return Err(if scope_error { Error::CalendarConsent } else { Error::Network("calendar access refused") });
        }
        if !status.is_success() { return Err(Error::Network("calendar request failed")); }
        let page: Page<T> = serde_json::from_slice(&bytes).map_err(|_| Error::Network("invalid calendar response"))?;
        result.extend(page.items);
        next = page.next_page_token.filter(|s| !s.is_empty());
        let Some(page) = &next else { return Ok(result) };
        if !seen.insert(page.clone()) { return Err(Error::Network("repeated calendar page")); }
    }
}

pub fn safe_join_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some()
        && url.username().is_empty() && url.password().is_none())
}

pub fn safe_calendar_url(value: &str) -> bool {
    safe_join_url(value) && Url::parse(value).is_ok_and(|url|
        matches!(url.host_str(), Some("calendar.google.com" | "www.google.com")) && url.path().starts_with("/calendar/"))
}

fn event(remote: RemoteEvent) -> Result<Event> {
    let (start, start_utc, all_day) = event_time(remote.start.ok_or(Error::Network("event start missing"))?)?;
    let (end, end_utc, end_all_day) = event_time(remote.end.ok_or(Error::Network("event end missing"))?)?;
    if remote.id.is_empty() || all_day != end_all_day || end_utc <= start_utc {
        return Err(Error::Network("invalid event interval"));
    }
    let join_url = remote.hangout_link.filter(|s| safe_join_url(s)).or_else(|| remote.conference_data.and_then(|c|
        c.entry_points.into_iter().find(|e| e.entry_point_type == "video" && safe_join_url(&e.uri)).map(|e| e.uri)));
    Ok(Event { remote_id: remote.id, title: remote.summary.filter(|s| !s.is_empty()).unwrap_or_else(|| "Untitled event".into()),
        start, end, start_utc, end_utc, all_day, location: remote.location, join_url, html_link: remote.html_link.filter(|s| safe_calendar_url(s)) })
}

fn event_time(time: EventTime) -> Result<(String, i64, bool)> {
    let (value, all_day) = match (time.date, time.date_time) {
        (Some(date), None) => (date, true),
        (None, Some(time)) => (time, false),
        _ => return Err(Error::Network("invalid event time")),
    };
    let timestamp = if all_day { format!("{value}T00:00:00Z") } else { value.clone() };
    let bytes = timestamp.as_bytes();
    let shape = bytes.len() >= 20 && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b'T'
        && bytes[13] == b':' && bytes[16] == b':'
        && bytes[..19].iter().enumerate().all(|(i, b)| matches!(i, 4 | 7 | 10 | 13 | 16) || b.is_ascii_digit());
    let zone = timestamp.strip_suffix('Z').or_else(|| {
        let at = timestamp.len().checked_sub(6)?;
        let tail = bytes.get(at..)?;
        (at >= 19 && matches!(tail[0], b'+' | b'-') && tail[3] == b':'
            && [1, 2, 4, 5].iter().all(|i| tail[*i].is_ascii_digit())).then(|| &timestamp[..at])
    });
    let fraction = zone.is_some_and(|s| s.len() == 19 || (s.as_bytes().get(19) == Some(&b'.')
        && s.len() > 20 && s.as_bytes()[20..].iter().all(u8::is_ascii_digit)));
    let date = mail_parser::DateTime::parse_rfc3339(&timestamp).filter(|d| d.is_valid());
    if !shape || !fraction || (all_day && value.len() != 10) { return Err(Error::Network("invalid event time")); }
    let date = date.ok_or(Error::Network("invalid event time"))?;
    let midnight = mail_parser::DateTime { hour: 0, minute: 0, second: 0, tz_hour: 0, tz_minute: 0, ..date };
    if mail_parser::DateTime::from_timestamp(midnight.to_timestamp()).to_rfc3339()[..10] != timestamp[..10] {
        return Err(Error::Network("invalid event date"));
    }
    Ok((value, date.to_timestamp(), all_day))
}

#[cfg(test)]
mod tests;

use super::*;
use crate::calendar::NewEvent;
use reqwest::{RequestBuilder, Response, StatusCode};

const BASE: &str = "https://www.googleapis.com/calendar/v3/";

pub(super) fn event_url(base: &str, calendar: &str, id: &str) -> Result<Url> {
    if [calendar, id].iter().any(|s| matches!(*s, "" | "." | "..")) {
        return Err(Error::Invalid("missing calendar or event ID"));
    }
    let mut url = Url::parse(base).map_err(|_| Error::Invalid("invalid calendar endpoint"))?;
    url.path_segments_mut().map_err(|_| Error::Invalid("invalid calendar endpoint"))?
        .pop_if_empty().extend(["calendars", calendar, "events", id]);
    Ok(url)
}

// If-Match must name exactly one revision, never a wildcard or a list.
fn valid_etag(tag: &str) -> bool {
    tag.strip_prefix('"').and_then(|s| s.strip_suffix('"')).is_some_and(|s|
        s.bytes().all(|b| b.is_ascii_graphic() && b != b'"'))
}

#[derive(Debug)]
pub struct EventSnapshot {
    pub event: Event,
    pub description: Option<String>,
    pub attendees: Vec<String>,
    pub etag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Details {
    #[serde(flatten)]
    remote: RemoteEvent,
    description: Option<String>,
    etag: Option<String>,
    #[serde(default)]
    attendees: Vec<Attendee>,
    #[serde(default)]
    attendees_omitted: bool,
}

#[derive(Deserialize)]
struct Attendee { email: String }

pub async fn fetch_event(account: &Account, calendar: &str, id: &str) -> Result<EventSnapshot> {
    fetch_event_with(BASE, calendar, id,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub(super) async fn fetch_event_with<F, Fut>(base: &str, calendar: &str, id: &str, mut token: F) -> Result<EventSnapshot>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let url = event_url(base, calendar, id)?;
    let client = oauth::http(Duration::from_secs(30))?;
    let details: Details = get(&client, url, &mut token).await?;
    if details.remote.status == "cancelled" { return Err(Error::NotFound); }
    if details.remote.id != id || details.attendees_omitted { return Err(Error::Network("invalid calendar response")); }
    let etag = details.etag.filter(|s| valid_etag(s)).ok_or(Error::Network("invalid calendar response"))?;
    Ok(EventSnapshot { event: event(details.remote)?, etag, description: details.description,
        attendees: details.attendees.into_iter().map(|a| a.email).collect() })
}

pub(super) async fn write_event<F, Fut>(mut token: F, request: impl Fn(&str) -> RequestBuilder) -> Result<Response>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let failed = |err: reqwest::Error| if err.is_connect() || err.is_builder() {
        Error::Network("calendar unavailable")
    } else { Error::Uncertain };
    let access = token(None).await?;
    let mut response = request(&access).send().await.map_err(failed)?;
    if response.status() == StatusCode::UNAUTHORIZED {
        let fresh = token(Some(access.to_string())).await?;
        response = request(&fresh).send().await.map_err(failed)?;
    }
    match response.status() {
        StatusCode::UNAUTHORIZED => Err(Error::ReauthRequired),
        StatusCode::FORBIDDEN => Err(calendar_forbidden(&oauth::body(response).await?)),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
        StatusCode::PRECONDITION_FAILED => Err(Error::Conflict),
        status if status.is_success() => Ok(response),
        _ => Err(Error::Network("calendar request failed")),
    }
}

/// Empty attendees, absent description and meet=false leave those fields alone.
pub async fn update_event(store: &Store, account: &Account, calendar: &str, id: &str, etag: &str, draft: &NewEvent) -> Result<()> {
    change_with(store, &account.id, BASE, calendar, id, etag, Some(draft),
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub async fn delete_event(store: &Store, account: &Account, calendar: &str, id: &str, etag: &str) -> Result<()> {
    change_with(store, &account.id, BASE, calendar, id, etag, None,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub(super) async fn change_with<F, Fut>(store: &Store, account: &str, base: &str, calendar: &str, id: &str,
    etag: &str, draft: Option<&NewEvent>, token: F) -> Result<()>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    if !valid_etag(etag) { return Err(Error::Invalid("read the event again before changing it")); }
    if !store.calendars(account)?.iter().any(|c| c.remote_id == calendar) {
        return Err(Error::NotFound);
    }
    let mut url = event_url(base, calendar, id)?;
    url.query_pairs_mut().append_pair("sendUpdates", "all");
    let client = oauth::http(Duration::from_secs(30))?;
    let Some(draft) = draft else {
        write_event(token, |access| client.delete(url.clone())
            .bearer_auth(access).header("If-Match", etag)).await?;
        return store.delete_calendar_event(account, calendar, id);
    };
    url.query_pairs_mut().append_pair("conferenceDataVersion", "1");
    let body = draft.body()?.to_string();
    let response = write_event(token, |access| client.patch(url.clone())
        .bearer_auth(access).header("If-Match", etag).header("Content-Type", "application/json")
        .body(body.clone())).await?;
    let bytes = oauth::body(response).await.map_err(|_| Error::Uncertain)?;
    let remote: RemoteEvent = serde_json::from_slice(&bytes).map_err(|_| Error::Uncertain)?;
    if remote.id != id { return Err(Error::Uncertain); }
    store.insert_calendar_event(account, calendar, &event(remote).map_err(|_| Error::Uncertain)?)
}

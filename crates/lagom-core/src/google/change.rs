use super::*;
use crate::calendar::NewEvent;
use reqwest::{RequestBuilder, Response, StatusCode};

const BASE: &str = "https://www.googleapis.com/calendar/v3/";

fn check_id(id: &str) -> Result<()> {
    if matches!(id, "" | "." | "..") || id.chars().any(char::is_control) {
        return Err(Error::Invalid("invalid calendar or event ID"));
    }
    Ok(())
}

pub(super) fn events_url(base: &str, calendar: &str) -> Result<Url> {
    check_id(calendar)?;
    let mut url = Url::parse(base).map_err(|_| Error::Invalid("invalid calendar endpoint"))?;
    url.path_segments_mut().map_err(|_| Error::Invalid("invalid calendar endpoint"))?
        .pop_if_empty().extend(["calendars", calendar, "events"]);
    Ok(url)
}

pub(super) fn event_url(base: &str, calendar: &str, id: &str) -> Result<Url> {
    check_id(id)?;
    let mut url = events_url(base, calendar)?;
    url.path_segments_mut().map_err(|_| Error::Invalid("invalid calendar endpoint"))?.push(id);
    let mut segments = url.path_segments().unwrap().rev();
    if !segments.next().is_some_and(|s| percent_encoding::percent_decode_str(s).eq(id.bytes()))
        || segments.next() != Some("events") {
        return Err(Error::Invalid("invalid calendar or event ID"));
    }
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
    #[serde(flatten)]
    revision: Revision,
    description: Option<String>,
    #[serde(default)]
    attendees: Vec<Attendee>,
    #[serde(default)]
    attendees_omitted: bool,
}

#[derive(Deserialize)]
struct WriteTarget {
    id: String,
    #[serde(default)]
    status: String,
    #[serde(flatten)]
    revision: Revision,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Revision {
    recurrence: Option<Vec<String>>,
    recurring_event_id: Option<String>,
    etag: Option<String>,
}

impl Revision {
    fn checked_etag(self, expected: &str, id: &str, status: &str) -> Result<String> {
        if id != expected { return Err(Error::Network("invalid calendar response")); }
        if status == "cancelled" { return Err(Error::NotFound); }
        if self.recurrence.is_some_and(|rules| !rules.is_empty()) || self.recurring_event_id.as_deref() == Some(id) {
            return Err(Error::RecurringSeries);
        }
        self.etag.filter(|s| valid_etag(s)).ok_or(Error::Network("invalid calendar response"))
    }
}

#[derive(Deserialize)]
struct Attendee { email: String }

pub async fn fetch_event(account: &Account, calendar: &str, id: &str) -> Result<EventSnapshot> {
    fetch_event_with(BASE, calendar, id,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub async fn fetch_event_with<F, Fut>(base: &str, calendar: &str, id: &str, mut token: F) -> Result<EventSnapshot>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let url = event_url(base, calendar, id)?;
    let client = oauth::http(Duration::from_secs(30))?;
    let details: Details = get(&client, url, &mut token).await?;
    let etag = details.revision.checked_etag(id, &details.remote.id, &details.remote.status)?;
    if details.attendees_omitted { return Err(Error::Network("invalid calendar response")); }
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
        status if status.is_server_error() => Err(Error::Uncertain),
        status if status.is_success() => Ok(response),
        _ => Err(Error::Network("calendar request failed")),
    }
}

/// Empty attendees, absent description and meet=false leave those fields alone.
pub async fn update_event(store: &Store, account: &Account, calendar: &str, id: &str, etag: &str, draft: &NewEvent) -> Result<()> {
    change_event_checked(store, account, calendar, id, etag, Some(draft), || Ok(())).await
}

pub async fn delete_event(store: &Store, account: &Account, calendar: &str, id: &str, etag: &str) -> Result<()> {
    change_event_checked(store, account, calendar, id, etag, None, || Ok(())).await
}

pub async fn change_event_checked(store: &Store, account: &Account, calendar: &str, id: &str, etag: &str,
    draft: Option<&NewEvent>, check: impl Fn() -> Result<()>) -> Result<()> {
    let check = &check;
    change_with(store, &account.id, BASE, calendar, id, etag, draft, |rejected| async move {
        let access = oauth::access_token(account, rejected.as_deref()).await?;
        check()?;
        Ok(access)
    }).await
}

pub async fn change_with<F, Fut>(store: &Store, account: &str, base: &str, calendar: &str, id: &str,
    etag: &str, draft: Option<&NewEvent>, mut token: F) -> Result<()>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    if !valid_etag(etag) { return Err(Error::Invalid("read the event again before changing it")); }
    if !store.calendars(account)?.iter().any(|c| c.remote_id == calendar) {
        return Err(Error::NotFound);
    }
    let mut url = event_url(base, calendar, id)?;
    let client = oauth::http(Duration::from_secs(30))?;
    let body = draft.map(NewEvent::patch_body).transpose()?;
    let remove_cached = || store.delete_calendar_event(account, calendar, id);
    let change = async {
        let mut check = url.clone();
        check.query_pairs_mut().append_pair("fields", "id,status,recurrence,recurringEventId,etag");
        let target: WriteTarget = get(&client, check, &mut token).await?;
        if target.revision.checked_etag(id, &target.id, &target.status)? != etag { return Err(Error::Conflict); }
        url.query_pairs_mut().append_pair("sendUpdates", "all");
        if body.is_some() { url.query_pairs_mut().append_pair("conferenceDataVersion", "1"); }
        write_event(&mut token, |access| {
            let request = match &body {
                Some(body) => client.patch(url.clone()).header("Content-Type", "application/json").body(body.clone()),
                None => client.delete(url.clone()),
            };
            request.bearer_auth(access).header("If-Match", etag)
        }).await
    }.await;
    let response = match change {
        Err(Error::NotFound) => {
            log_cache_error(remove_cached());
            return Err(Error::NotFound);
        }
        other => other?,
    };
    let cached = if draft.is_some() {
        let mut updated = oauth::body(response).await.ok()
            .and_then(|bytes| serde_json::from_slice::<RemoteEvent>(&bytes).ok())
            .filter(|remote| remote.id == id && remote.status != "cancelled")
            .and_then(|remote| event(remote).ok());
        if updated.is_none() {
            updated = fetch_created_with(base, calendar, id, token).await.ok().map(|created| created.event);
        }
        match updated {
            Some(event) => store.insert_calendar_event(account, calendar, &event),
            // The write succeeded. Drop the stale row until the next calendar sync.
            None => remove_cached(),
        }
    } else { remove_cached() };
    log_cache_error(cached);
    Ok(())
}

fn log_cache_error(result: Result<()>) {
    if let Err(error) = result { eprintln!("lagom: calendar event cache update failed: {error:?}"); }
}

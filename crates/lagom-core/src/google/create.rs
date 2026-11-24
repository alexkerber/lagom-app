use super::*;
use crate::calendar::NewEvent;
use serde_json::{json, Value};

impl NewEvent {
    pub fn event(&self, id: String) -> Result<Event> {
        if self.title.trim().is_empty() { return Err(Error::Invalid("enter an event title")); }
        if !self.all_day && (self.time_zone.is_empty() || self.time_zone.chars().any(|c| c.is_whitespace() || c.is_control())) {
            return Err(Error::Invalid("local time zone unavailable"));
        }
        for address in &self.attendees {
            if address.chars().any(char::is_control) || address.parse::<lettre::Address>().is_err() {
                return Err(Error::Invalid("enter valid guest email addresses"));
            }
        }
        let time = |value: &String| EventTime {
            date: self.all_day.then(|| value.clone()), date_time: (!self.all_day).then(|| value.clone()),
        };
        event(RemoteEvent {
            id, status: String::new(), summary: Some(self.title.trim().into()), start: Some(time(&self.start)), end: Some(time(&self.end)),
            location: None, hangout_link: None, html_link: None, conference_data: None,
        }).map_err(|_| Error::Invalid("enter a valid start and later end"))
    }

    pub(super) fn patch_body(&self) -> Result<String> {
        let mut body = self.body()?;
        for key in ["start", "end"] {
            if self.all_day {
                body[key]["dateTime"] = Value::Null;
                body[key]["timeZone"] = Value::Null;
            } else {
                body[key]["date"] = Value::Null;
            }
        }
        Ok(body.to_string())
    }

    pub(super) fn body(&self) -> Result<Value> {
        self.event("validation".into())?;
        let time = |value: &str| if self.all_day { json!({ "date": value }) }
            else { json!({ "dateTime": value, "timeZone": self.time_zone }) };
        let mut body = json!({ "summary": self.title.trim(), "start": time(&self.start), "end": time(&self.end) });
        if let Some(description) = &self.description { body["description"] = json!(description); }
        if !self.attendees.is_empty() {
            body["attendees"] = self.attendees.iter().map(|email| json!({ "email": email })).collect();
        }
        if self.meet {
            body["conferenceData"] = json!({ "createRequest": {
                "requestId": uuid::Uuid::new_v4().to_string(), "conferenceSolutionKey": { "type": "hangoutsMeet" }
            } });
        }
        Ok(body)
    }
}

pub async fn create_event(account: &Account, calendar: &str, draft: &NewEvent) -> Result<CreatedEvent> {
    create_with("https://www.googleapis.com/calendar/v3/", calendar, draft,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub async fn create_with<F, Fut>(base: &str, calendar: &str, draft: &NewEvent, token: F) -> Result<CreatedEvent>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let mut url = change::events_url(base, calendar)?;
    let body = draft.body()?.to_string();
    let client = oauth::http(Duration::from_secs(30))?;
    url.query_pairs_mut().append_pair("conferenceDataVersion", "1");
    if !draft.attendees.is_empty() { url.query_pairs_mut().append_pair("sendUpdates", "all"); }
    let response = write_event(token, |access| client.post(url.clone()).bearer_auth(access)
        .header("Content-Type", "application/json").body(body.clone())).await?;
    let bytes = oauth::body(response).await.map_err(|_| Error::Uncertain)?;
    created(serde_json::from_slice(&bytes).map_err(|_| Error::Uncertain)?).map_err(|_| Error::Uncertain)
}

#[derive(Clone, Debug)]
pub struct CreatedEvent {
    pub event: Event,
    pub conference_pending: bool,
}

fn created(remote: RemoteEvent) -> Result<CreatedEvent> {
    let conference_pending = remote.conference_data.as_ref().and_then(|c| c.create_request.as_ref())
        .and_then(|r| r.status.as_ref()).is_some_and(|s| s.status_code == "pending");
    Ok(CreatedEvent { event: event(remote)?, conference_pending })
}

pub async fn fetch_created(account: &Account, calendar: &str, id: &str) -> Result<CreatedEvent> {
    fetch_created_with("https://www.googleapis.com/calendar/v3/", calendar, id,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub async fn fetch_created_with<F, Fut>(base: &str, calendar: &str, id: &str, mut token: F) -> Result<CreatedEvent>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let url = event_url(base, calendar, id)?;
    let client = oauth::http(Duration::from_secs(30))?;
    let remote: RemoteEvent = get(&client, url, &mut token).await?;
    if remote.status == "cancelled" { return Err(Error::NotFound); }
    if remote.id != id { return Err(Error::Network("calendar event ID changed")); }
    created(remote)
}

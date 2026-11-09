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

    fn body(&self) -> Result<Value> {
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

pub async fn create_event(account: &Account, calendar: &str, draft: &NewEvent) -> Result<Event> {
    create_with("https://www.googleapis.com/calendar/v3/", calendar, draft,
        |rejected| async move { oauth::access_token(account, rejected.as_deref()).await }).await
}

pub async fn create_with<F, Fut>(base: &str, calendar: &str, draft: &NewEvent, mut token: F) -> Result<Event>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    if calendar.is_empty() { return Err(Error::Invalid("choose a calendar")); }
    let body = draft.body()?.to_string();
    let client = oauth::http(Duration::from_secs(30))?;
    let mut url = Url::parse(base).map_err(|_| Error::Invalid("invalid calendar endpoint"))?;
    url.path_segments_mut().map_err(|_| Error::Invalid("invalid calendar endpoint"))?
        .pop_if_empty().extend(["calendars", calendar, "events"]);
    url.query_pairs_mut().append_pair("conferenceDataVersion", "1");
    if !draft.attendees.is_empty() { url.query_pairs_mut().append_pair("sendUpdates", "all"); }
    let send = |access: Zeroizing<String>| client.post(url.clone()).bearer_auth(access.as_str())
        .header("Content-Type", "application/json").body(body.clone()).send();
    let mut access = token(None).await?;
    // shortcut: ambiguous transport failures are not retried; add durable idempotent creation with offline editing.
    let mut response = send(access.clone()).await.map_err(|_| Error::Network("event creation unconfirmed; check your calendar before retrying"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        access = token(Some(access.to_string())).await?;
        response = send(access).await.map_err(|_| Error::Network("event creation unconfirmed; check your calendar before retrying"))?;
    }
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED { return Err(Error::ReauthRequired); }
    if status == reqwest::StatusCode::FORBIDDEN { return Err(Error::CalendarConsent); }
    if !status.is_success() { return Err(Error::Network("could not create event")); }
    let bytes = oauth::body(response).await.map_err(|_| Error::Network("event creation unconfirmed; check your calendar before retrying"))?;
    event(serde_json::from_slice(&bytes).map_err(|_| Error::Network("event creation unconfirmed; check your calendar before retrying"))?)
}

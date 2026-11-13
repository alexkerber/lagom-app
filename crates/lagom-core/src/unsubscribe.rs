//! Unsubscribe from a list: List-Unsubscribe (RFC 2369) and List-Unsubscribe-Post (RFC 8058).
//! Sync stores the choice from headers it already has. Nothing here runs until the person confirms.

use crate::{smtp::Outgoing, store::Store, Error, Result};
use rusqlite::OptionalExtension;
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsubscribe {
    /// RFC 8058: lagom POSTs once, HTTPS only.
    OneClick(Url),
    /// A `mailto:` sent through the account's outbox.
    Mail(Url),
    /// An HTTPS page the browser opens after the person confirms. lagom never fetches it.
    Link(Url),
}

const ONE_CLICK: &str = "List-Unsubscribe=One-Click";
// Fixed and generic: no version, account or device data.
const USER_AGENT: &str = "lagom";
const TIMEOUT: Duration = Duration::from_secs(15);

impl Unsubscribe {
    /// What confirming does, in the confirm step of every platform.
    pub fn explain(&self) -> &'static str {
        match self {
            Self::OneClick(_) => "lagom asks the list to remove you.",
            Self::Mail(_) => "lagom sends an unsubscribe email from your account.",
            Self::Link(_) => "Opens the list's unsubscribe page in your browser.",
        }
    }

    fn stored(&self) -> String {
        match self {
            Self::OneClick(url) => format!("one-click {url}"),
            Self::Mail(url) | Self::Link(url) => url.to_string(),
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        if let Some(url) = value.strip_prefix("one-click ") {
            return Url::parse(url).ok().filter(|u| u.scheme() == "https").map(Self::OneClick);
        }
        let url = Url::parse(value).ok()?;
        match url.scheme() {
            "mailto" => outgoing(&url).map(|_| Self::Mail(url)),
            "https" => Some(Self::Link(url)),
            _ => None,
        }
    }
}

/// From a message's raw header block.
pub fn from_headers(raw: &[u8]) -> Option<Unsubscribe> {
    let parsed = mail_parser::MessageParser::default().parse_headers(raw)?;
    let headers = parsed.headers();
    let value = |name| headers.get(name).map(|h| String::from_utf8_lossy(h.raw_value()).into_owned());
    parse(&value("List-Unsubscribe")?, value("List-Unsubscribe-Post").as_deref())
}

/// One-click when the list offers it over HTTPS, else a mailto, else an HTTPS page.
pub fn parse(list_unsubscribe: &str, post: Option<&str>) -> Option<Unsubscribe> {
    let uris = uris(list_unsubscribe);
    let https = uris.iter().find(|u| u.scheme() == "https");
    // RFC 8058 §3.1: one-click needs the exact key=value and an HTTPS URI. http:// never qualifies.
    if post.is_some_and(|p| p.trim() == ONE_CLICK) {
        if let Some(url) = https { return Some(Unsubscribe::OneClick(url.clone())); }
    }
    uris.iter().find(|u| u.scheme() == "mailto" && outgoing(u).is_some()).cloned().map(Unsubscribe::Mail)
        .or_else(|| https.cloned().map(Unsubscribe::Link))
}

/// RFC 2369: `<uri>` items split by commas, with comments and folding white space around them.
/// White space inside the brackets is dropped, since long URIs may be folded.
fn uris(value: &str) -> Vec<Url> {
    let (mut out, mut comment, mut uri) = (Vec::new(), 0usize, None::<String>);
    for c in value.chars() {
        match (c, uri.as_mut()) {
            ('>', Some(_)) => out.extend(uri.take().and_then(|u| Url::parse(&u).ok())),
            (c, Some(u)) => if !c.is_whitespace() { u.push(c) },
            ('(', None) => comment += 1,
            (')', None) => comment = comment.saturating_sub(1),
            ('<', None) if comment == 0 => uri = Some(String::new()),
            _ => {}
        }
    }
    out
}

/// The unsubscribe mail a `mailto:` URI describes (RFC 6068): one address, its subject and body.
pub fn outgoing(url: &Url) -> Option<Outgoing> {
    let to = decode(url.path())?;
    // shortcut: one address and no `to=`/`cc=` fields; widen when real lists need more.
    if to.is_empty() || to.contains([',', '<', '>']) || to.chars().any(char::is_control) { return None; }
    let (mut subject, mut text) = (String::new(), String::new());
    for pair in url.query().unwrap_or_default().split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case("subject") { subject = decode(value)?; }
        if key.eq_ignore_ascii_case("body") { text = decode(value)?; }
    }
    // A folded or injected header line is refused, not repaired.
    if subject.chars().any(char::is_control) { return None; }
    if subject.trim().is_empty() { subject = "Unsubscribe".into(); }
    if text.trim().is_empty() { text = "Unsubscribe".into(); }
    Some(Outgoing { to: vec![to], subject, text, ..Default::default() })
}

/// Percent-decoding only: RFC 6068 keeps `+` literal.
fn decode(value: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut bytes = value.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// Redirects stay on HTTPS, a few hops at most.
fn follow(url: &Url, hops: usize) -> bool { url.scheme() == "https" && hops < 5 }

fn client() -> Result<reqwest::Client> {
    // No cookie store: this reqwest build has no cookie feature, and none is set here.
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect_policy(reqwest::redirect::Policy::custom(|a| {
            if follow(a.url(), a.previous().len()) { a.follow() } else { a.stop() }
        }))
        .build()
        .map_err(|_| Error::Network("HTTP client failed"))
}

/// RFC 8058 §3.2: a form POST with only the one-click key, no credentials.
pub fn one_click_request(client: &reqwest::Client, url: &Url) -> Result<reqwest::Request> {
    if url.scheme() != "https" { return Err(Error::Invalid("one-click unsubscribe needs an HTTPS link")); }
    client.post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(ONE_CLICK)
        .build()
        .map_err(|_| Error::Invalid("invalid unsubscribe link"))
}

pub async fn one_click(url: &Url) -> Result<()> {
    let client = client()?;
    let response = client.execute(one_click_request(&client, url)?).await
        .map_err(|_| Error::Network("the list could not be reached"))?;
    if response.status().is_success() { Ok(()) } else { Err(Error::Network("the list did not confirm the unsubscribe")) }
}

impl Store {
    pub fn set_unsubscribe(&self, message: i64, choice: Option<&Unsubscribe>) -> Result<()> {
        self.db.execute("UPDATE messages SET unsubscribe=?2 WHERE id=?1",
            rusqlite::params![message, choice.map(Unsubscribe::stored).unwrap_or_default()])?;
        Ok(())
    }

    /// `None` when the message names no usable way out. Local only.
    pub fn unsubscribe(&self, message: i64) -> Result<Option<Unsubscribe>> {
        let value: String = self.db.query_row("SELECT unsubscribe FROM messages WHERE id=?1", [message], |r| r.get(0))
            .optional()?.ok_or(Error::NotFound)?;
        Ok(Unsubscribe::from_stored(&value))
    }

    /// Queues the unsubscribe mail of a `mailto:` choice; the caller sends it like any outbox row.
    pub fn enqueue_unsubscribe(&self, message: i64) -> Result<String> {
        let Some(Unsubscribe::Mail(url)) = self.unsubscribe(message)? else {
            return Err(Error::Invalid("this message has no unsubscribe address"));
        };
        let owner = self.message_summary(message)?.account;
        let account = self.accounts()?.into_iter().find(|a| a.id == owner).ok_or(Error::NotFound)?;
        self.enqueue(&account, &outgoing(&url).ok_or(Error::Invalid("invalid unsubscribe address"))?, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url { Url::parse(value).unwrap() }

    #[test]
    fn several_uris_brackets_whitespace_and_comments() {
        let header = " (Use either) <mailto:leave@list.example?subject=unsubscribe>,\r\n\t<https://list.example/u/\r\n abc> (web)";
        assert_eq!(uris(header), vec![url("mailto:leave@list.example?subject=unsubscribe"), url("https://list.example/u/abc")]);
        // A bracket inside a comment is not a URI.
        assert_eq!(uris("(old <https://old.example>) <https://new.example/u>"), vec![url("https://new.example/u")]);
        // Without One-Click, mailto wins over a page.
        assert_eq!(parse(header, None), Some(Unsubscribe::Mail(url("mailto:leave@list.example?subject=unsubscribe"))));
        assert_eq!(parse("<https://list.example/u>", None), Some(Unsubscribe::Link(url("https://list.example/u"))));
        assert_eq!(parse("leave@list.example", None), None);
    }

    #[test]
    fn one_click_needs_https() {
        let header = "<http://list.example/u>, <https://list.example/one>";
        assert_eq!(parse(header, Some(" List-Unsubscribe=One-Click\r\n")), Some(Unsubscribe::OneClick(url("https://list.example/one"))));
        // http:// is refused for one-click and never offered as a page either.
        assert_eq!(parse("<http://list.example/u>", Some(ONE_CLICK)), None);
        assert_eq!(parse("<http://list.example/u>, <mailto:leave@list.example>", Some(ONE_CLICK)), Some(Unsubscribe::Mail(url("mailto:leave@list.example"))));
        let client = reqwest::Client::new();
        assert!(matches!(one_click_request(&client, &url("http://list.example/u")), Err(Error::Invalid(_))));
        assert_eq!(Unsubscribe::from_stored("one-click http://list.example/u"), None);
    }

    #[test]
    fn one_click_request_shape() {
        let client = client().unwrap();
        let request = one_click_request(&client, &url("https://list.example/u?id=7")).unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), "https://list.example/u?id=7");
        assert_eq!(request.headers()[reqwest::header::CONTENT_TYPE], "application/x-www-form-urlencoded");
        assert!(request.headers().get(reqwest::header::COOKIE).is_none());
        assert!(request.headers().get(reqwest::header::AUTHORIZATION).is_none());
        assert_eq!(request.body().and_then(|b| b.as_bytes()), Some(&b"List-Unsubscribe=One-Click"[..]));
        assert!(follow(&url("https://list.example/done"), 1));
        assert!(!follow(&url("http://list.example/done"), 1));
        assert!(!follow(&url("https://list.example/done"), 5));
    }

    #[test]
    fn mailto_with_subject_and_body() {
        let mail = outgoing(&url("mailto:leave+news@list.example?Subject=Stop%20please&body=remove+me")).unwrap();
        assert_eq!(mail.to, ["leave+news@list.example"]);
        assert_eq!((mail.subject.as_str(), mail.text.as_str()), ("Stop please", "remove+me"));
        let plain = outgoing(&url("mailto:leave@list.example")).unwrap();
        assert_eq!((plain.to[0].as_str(), plain.subject.as_str()), ("leave@list.example", "Unsubscribe"));
        assert!(outgoing(&url("mailto:a@x.example,b@x.example")).is_none());
        assert!(outgoing(&url("mailto:a@x.example?subject=%0D%0ABcc:%20x")).is_none());
    }

    #[test]
    fn stored_choice_and_unsubscribe_mail_through_the_outbox() {
        use crate::{account::Account, store::{FolderRole, Message, MessageSummary}};
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("unsubscribe.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("self@gmail.com").unwrap();
        let account = Account::new("self@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store.save_folder(&account.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
        let id = store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder: inbox, uid: 1, ..Default::default() }, ..Default::default() }).unwrap();
        assert_eq!(store.unsubscribe(id).unwrap(), None);
        assert!(store.enqueue_unsubscribe(id).is_err());
        let choice = Unsubscribe::Mail(url("mailto:leave@list.example?subject=stop"));
        store.set_unsubscribe(id, Some(&choice)).unwrap();
        assert_eq!(store.unsubscribe(id).unwrap(), Some(choice));
        let queued = store.pending(&store.enqueue_unsubscribe(id).unwrap()).unwrap();
        assert_eq!((queued.mail.to, queued.mail.subject.as_str()), (vec!["leave@list.example".to_owned()], "stop"));
        assert!(matches!(store.unsubscribe(-1), Err(Error::NotFound)));
    }

    #[test]
    fn headers_round_trip_through_the_store_format() {
        let raw = b"From: News <news@list.example>\r\nList-Unsubscribe: <mailto:leave@list.example>,\r\n <https://list.example/one>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\n";
        let choice = from_headers(raw).unwrap();
        assert_eq!(choice, Unsubscribe::OneClick(url("https://list.example/one")));
        assert_eq!(Unsubscribe::from_stored(&choice.stored()), Some(choice));
        let mail = Unsubscribe::Mail(url("mailto:leave@list.example?subject=x"));
        assert_eq!(Unsubscribe::from_stored(&mail.stored()), Some(mail));
        assert_eq!(from_headers(b"From: a@b.example\r\n\r\n"), None);
    }
}

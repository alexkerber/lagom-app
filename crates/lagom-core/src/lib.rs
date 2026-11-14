//! Local-first mail services. Blocking network and keychain calls belong on a worker.
pub mod account;
pub mod contacts;
pub mod compose;
pub mod calendar;
pub mod google;
pub mod attachment;
pub mod discovery;
pub use discovery::{discover, Discovery, Preset};
pub mod imap_sync;
pub mod new_mail;
pub mod mime;
pub mod html;
pub mod inline;
pub mod presets;
pub mod oauth;
pub mod smtp;
pub mod outbox;
pub mod later;
mod drafts;
pub mod store;
pub mod threading;
pub mod tasks;
pub mod unsubscribe;

/// `Display` is the text the person reads; `Debug` keeps the technical detail for logs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}", plain(.0))]
    Invalid(&'static str),
    #[error("lagom could not read or save its data on this device")]
    Store(#[from] rusqlite::Error),
    #[error("lagom could not read or save a file on this device")]
    Io(#[from] std::io::Error),
    #[error("lagom could not open the password saved on this device")]
    Secret,
    #[error("{}", plain(.0))]
    Network(&'static str),
    #[error("message sent, but the Sent copy could not be saved; do not resend")]
    SentCopy,
    #[error("your mail provider refused to send the message")]
    Rejected,
    #[error("sign in again")]
    ReauthRequired,
    #[error("Allow calendar access")]
    CalendarConsent,
    #[error("Allow Google Tasks access")]
    TasksConsent,
    #[error("sign-in cancelled")]
    Cancelled,
    #[error("your mail provider did not accept your sign-in")]
    Authentication,
    #[error("it is no longer here")]
    NotFound,
    /// The connection broke after the message may have been handed over.
    #[error("may have been sent; check Sent before sending again")]
    MaybeSent,
    /// Sync of this folder failed.
    #[error("{folder}: {source}")]
    Folder { folder: String, source: Box<Error> },
}

impl Error {
    /// Errors that fail every folder alike, so a sync stops at the first one.
    pub fn is_account_wide(&self) -> bool {
        matches!(self, Error::ReauthRequired | Error::Authentication | Error::Secret | Error::Cancelled)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Plain words for a technical detail; other details are already written for the person.
fn plain(detail: &'static str) -> &'static str {
    match detail {
        "IMAP operation timed out" | "IMAP login timed out" | "IMAP folder listing timed out" | "draft fetch timed out"
        | "calendar sync timed out" | "Google Tasks sync timed out" => "your mail provider did not answer in time",
        "IMAP connection lost" | "IMAP operation failed" | "IMAP greeting missing" | "IMAP IDLE failed" | "IMAP IDLE connection closed"
        | "IMAP folder listing interrupted" | "IMAP search interrupted" | "draft fetch interrupted" | "IMAP TLS handshake failed"
        | "SMTP TLS handshake failed" | "TLS configuration failed" | "HTTP client failed" | "HTTP response failed"
        | "IMAP authentication failed" | "token endpoint unavailable" | "invalid calendar endpoint" | "invalid Google Tasks endpoint" => "could not reach your mail provider",
        "IMAP folder listing failed" | "IMAP search failed" | "IMAP expunge failed" | "draft fetch failed" | "calendar request failed"
        | "Google Tasks request failed" => "your mail provider refused the request; try again later",
        "IMAP UIDVALIDITY missing" | "IMAP BODYSTRUCTURE missing" | "IMAP message header missing" | "HTTP response too large"
        | "invalid token response" | "invalid access token" | "invalid Google Tasks page" | "invalid Google Tasks response"
        | "invalid Google task" | "invalid Google task list" | "invalid Google default task list" | "Google task ID missing"
        | "Google task list ID missing" | "repeated Google Tasks page" | "repeated calendar page" | "invalid calendar response"
        | "calendar event ID changed" | "event start missing" | "event end missing" | "invalid event date" | "invalid event interval"
        | "invalid event time" => "your mail provider sent something lagom could not read",
        "server omitted a message; sync checkpoint retained" => "some mail did not download; lagom tries again at the next sync",
        "MIME text part missing" | "invalid MIME message" | "invalid mail header" => "lagom could not read this message",
        "could not construct message" | "could not encode local mail" => "lagom could not prepare this message",
        "MIME input exceeds the size limit" => "this message is too large",
        "attachment part missing" | "invalid attachment MIME type" | "invalid attachment encoding" | "unsupported attachment encoding"
        | "invalid attachment section" => "lagom could not read this attachment",
        "invalid draft MIME" | "invalid draft Message-ID" | "invalid draft envelope" | "invalid draft upload token" => "lagom could not read this draft",
        "draft mailbox changed during upload; retry pending" | "uploaded draft not found; retry pending" => "lagom tries again later",
        "attachment exceeds 64 MiB" => "attachments can be at most 64 MB",
        "inline parts exceed 10 MiB" => "the pictures in this message are too large to show",
        "mailbox changed; resync" | "mailbox changed; resync before download" | "mailbox changed; resync before loading inline images"
        | "mailbox changed; resync before operation" => "this folder changed; sync and try again",
        "authorization code missing" | "authorization failed" | "consent failed" | "email missing from ID token" | "ID token missing"
        | "invalid ID token" | "invalid ID token claims" | "invalid ID token email" | "invalid ID token name" | "invalid sign-in callback"
        | "invalid sign-in redirect" | "sign-in state mismatch" | "token request rejected" | "invalid authorization endpoint"
        | "sign-in task failed" | "no free sign-in port" => "sign-in was interrupted; try again",
        "sign-in provider is not configured" | "invalid Google client ID" => "sign-in with this provider is not available in this version",
        "invalid TLS server name" | "invalid SMTP TLS host" => "the server address is not valid",
        "invalid server configuration" => "check the server address, port and username",
        "unencrypted connections require localhost or a loopback IP" | "unencrypted peer must be loopback" => "the server needs encryption",
        "unknown account authentication" | "unknown transport security" | "invalid account" => "lagom could not read this account's settings",
        "Mail access is required; saved credentials were not replaced" => "lagom needs access to your mail; your saved sign-in did not change",
        "empty Trash requires UIDPLUS" => "your mail provider does not let lagom empty Trash",
        "server does not support IDLE" => "your mail provider does not report new mail right away",
        "not an OAuth account" => "this account signs in with a password",
        "one-click unsubscribe needs an HTTPS link" => "this unsubscribe link is not secure",
        "outbox account mismatch" => "message belongs to another account",
        "cross-account moves unsupported" => "mail can only move within one account",
        "invalid IMAP mailbox encoding" => "lagom could not read this folder name",
        "missing calendar or event ID" => "lagom could not find this event",
        "calendar ID missing" => "lagom could not find this calendar",
        "platform data directory unavailable" => "lagom could not find a place to save mail on this device",
        "store was created by a newer lagom version" => "your saved mail is from a newer lagom; update lagom",
        "calendar sync task failed" | "Google Tasks sync task failed" => "something went wrong; try again",
        detail => detail,
    }
}

#[cfg(test)]
mod tests {
    /// Every error text a person can read stays free of protocol and programmer words.
    #[test]
    fn error_text_is_plain() {
        const JARGON: &[&str] = &["IMAP", "SMTP", "TLS", "STARTTLS", "OAuth", "XOAUTH2", "MIME", "UID", "UIDVALIDITY", "UIDPLUS",
            "BODYSTRUCTURE", "IDLE", "HTTP", "HTTPS", "ID", "MiB", "token", "endpoint", "resync", "loopback", "localhost", "IP", "peer",
            "envelope", "callback", "redirect", "expunge", "operation", "authentication", "authorization", "socket", "certificate",
            "handshake", "checkpoint", "configuration", "encoding", "interval", "consent", "platform", "store", "transport", "claims",
            "mismatch", "construct", "encode", "omitted", "credentials", "fetch", "pending", "unsupported"];
        let mut details = vec!["IMAP operation timed out", "IMAP connection lost", "IMAP operation failed"];
        let mut sources = Vec::new();
        for dir in ["src", "../lagom-ffi/src", "../../apps/desktop/src"] {
            let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)];
            while let Some(path) = stack.pop() {
                if path.is_dir() { stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path())); }
                else if path.extension().is_some_and(|e| e == "rs") { sources.push(std::fs::read_to_string(&path).unwrap()); }
            }
        }
        for source in &sources {
            for start in ["Error::Network(", "Error::Invalid(", "net_as("] {
                for (at, _) in source.match_indices(start) {
                    let rest = source[at + start.len()..].trim_start();
                    if let Some(literal) = rest.strip_prefix('"').and_then(|r| r.split_once('"')).map(|(s, _)| s) {
                        details.push(Box::leak(literal.to_string().into_boxed_str()));
                    }
                }
            }
        }
        assert!(details.len() > 150, "the scan found {} details", details.len());
        let jargon: Vec<_> = details.into_iter().map(|d| super::Error::Network(d).to_string())
            .filter(|shown| shown.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| JARGON.contains(&w))).collect();
        assert!(jargon.is_empty(), "map these in plain(): {jargon:#?}");
    }
}

//! Local-first mail services. Blocking network and keychain calls belong on a worker.
pub mod account;
pub mod discovery;
pub use discovery::{discover, Discovery, Preset};
pub mod imap_sync;
pub mod mime;
pub mod presets;
pub mod oauth;
pub mod smtp;
pub mod store;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("local store: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("OS credential store operation failed")]
    Secret,
    #[error("{0}")]
    Network(&'static str),
    #[error("message sent, but the Sent copy could not be saved; do not resend")]
    SentCopy,
    #[error("sign in again")]
    ReauthRequired,
    #[error("sign-in cancelled")]
    Cancelled,
    #[error("authentication rejected")]
    Authentication,
    #[error("not found")]
    NotFound,
    /// The connection broke after the message may have been handed over.
    #[error("may have been sent; check Sent before sending again")]
    MaybeSent,
}
pub type Result<T> = std::result::Result<T, Error>;

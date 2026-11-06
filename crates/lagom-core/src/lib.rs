//! Local-first mail services. Blocking network and keychain calls belong on a worker.
pub mod account;
pub mod presets;
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
    #[error("not found")]
    NotFound,
}
pub type Result<T> = std::result::Result<T, Error>;

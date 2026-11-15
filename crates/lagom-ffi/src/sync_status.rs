#[derive(Clone, Debug, uniffi::Enum)]
pub enum SyncState {
    Syncing,
    Synced { at: i64 },
    Failed,
    ReauthRequired,
    SigningIn,
    NotSynced,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct SyncStatus {
    pub text: String,
    pub icon: String,
}

#[uniffi::export]
pub fn sync_status(state: SyncState, remaining: Option<u64>) -> SyncStatus {
    use lagom_core::sync_status::SyncState as State;
    let status = lagom_core::sync_status::sync_status(match state {
        SyncState::Syncing => State::Syncing,
        SyncState::Synced { at } => State::synced_at(at),
        SyncState::Failed => State::Failed,
        SyncState::ReauthRequired => State::ReauthRequired,
        SyncState::SigningIn => State::SigningIn,
        SyncState::NotSynced => State::NotSynced,
    }, remaining);
    SyncStatus { text: status.text, icon: status.icon.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_every_state_and_local_time() {
        let at = 1_700_000_000;
        let time = chrono::DateTime::from_timestamp(at, 0).unwrap().with_timezone(&chrono::Local).format("%H:%M").to_string();
        for (state, text, icon) in [
            (SyncState::Syncing, "syncing…".into(), "cloud-syncing"),
            (SyncState::Synced { at }, format!("synced {time}"), "cloud-synced"),
            (SyncState::Failed, "not synced".into(), "cloud-failed"),
            (SyncState::ReauthRequired, "Sign in again".into(), "cloud-failed"),
            (SyncState::SigningIn, "Signing in…".into(), "cloud-syncing"),
            (SyncState::NotSynced, "not synced".into(), "cloud-failed"),
        ] {
            assert_eq!(sync_status(state.clone(), None).text, text);
            let status = sync_status(state, Some(7));
            assert_eq!(status.text, format!("7 left · {text}"));
            assert_eq!(status.icon, icon);
        }
    }
}

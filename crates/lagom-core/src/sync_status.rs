//! Shared sync copy; callers supply the local wall-clock time.
use chrono::NaiveTime;

#[derive(Clone, Copy, Debug)]
pub enum SyncState {
    Syncing,
    Synced(Option<NaiveTime>),
    Failed,
    ReauthRequired,
    SigningIn,
    NotSynced,
}

impl SyncState {
    /// Convert at the boundary so formatting itself needs no clock or timezone.
    pub fn synced_at(unix: i64) -> Self {
        Self::Synced(chrono::DateTime::from_timestamp(unix, 0).map(|at| at.with_timezone(&chrono::Local).time()))
    }
}

#[derive(Debug, PartialEq)]
pub struct SyncStatus {
    pub text: String,
    pub icon: &'static str,
}

pub fn sync_status(state: SyncState, remaining: Option<u64>) -> SyncStatus {
    let (text, icon) = match state {
        SyncState::Syncing => ("syncing…".into(), "cloud-syncing"),
        SyncState::Synced(at) => (format!("synced {}", at.map(|at| at.format("%H:%M").to_string()).unwrap_or_default()), "cloud-synced"),
        SyncState::Failed | SyncState::NotSynced => ("not synced".into(), "cloud-failed"),
        SyncState::ReauthRequired => ("Sign in again".into(), "cloud-failed"),
        SyncState::SigningIn => ("Signing in…".into(), "cloud-syncing"),
    };
    SyncStatus { text: match remaining { Some(left) => format!("{left} left · {text}"), None => text }, icon }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_with_and_without_remaining() {
        for (state, text, icon) in [
            (SyncState::Syncing, "syncing…", "cloud-syncing"),
            (SyncState::Synced(NaiveTime::from_hms_opt(9, 5, 59)), "synced 09:05", "cloud-synced"),
            (SyncState::Synced(NaiveTime::from_hms_opt(23, 59, 0)), "synced 23:59", "cloud-synced"),
            (SyncState::synced_at(i64::MAX), "synced ", "cloud-synced"),
            (SyncState::Failed, "not synced", "cloud-failed"),
            (SyncState::ReauthRequired, "Sign in again", "cloud-failed"),
            (SyncState::SigningIn, "Signing in…", "cloud-syncing"),
            (SyncState::NotSynced, "not synced", "cloud-failed"),
        ] {
            assert_eq!(sync_status(state, None), SyncStatus { text: text.into(), icon });
            for left in [0, 1, 42, u64::MAX] {
                assert_eq!(sync_status(state, Some(left)), SyncStatus { text: format!("{left} left · {text}"), icon });
            }
        }
    }
}

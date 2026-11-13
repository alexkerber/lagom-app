//! Select new Inbox mail from the snapshots surrounding a sync.
use crate::store::{Folder, FolderRole, MessageSummary};

pub fn new_mail(before: &Folder, after: &Folder, messages: Vec<MessageSummary>) -> Vec<MessageSummary> {
    if before.role != FolderRole::Inbox || after.role != FolderRole::Inbox
        || before.id != after.id || before.account != after.account
        || before.uidvalidity.is_none() || before.uidvalidity != after.uidvalidity
        || (before.last_uid == 0 && before.uidnext.is_none())
    {
        return Vec::new();
    }
    messages.into_iter().filter(|m| {
        m.account == after.account && m.folder == after.id && m.uid > before.last_uid
            && m.uid <= after.last_uid && !m.seen && m.snoozed_until.is_none()
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> Folder {
        Folder { id: 1, account: "fixture".into(), name: "INBOX".into(), delimiter: None,
            role: FolderRole::Inbox, uidvalidity: Some(4), uidnext: Some(11), last_uid: 10,
            highest_modseq: None, unread: 0 }
    }
    fn message(uid: u32) -> MessageSummary {
        MessageSummary { id: uid as i64, account: "fixture".into(), folder: 1, uid,
            from_address: "sender@example.test".into(), subject: "Subject".into(), ..Default::default() }
    }
    #[test]
    fn initial_sync_partial_initial_retry_and_uidvalidity_reset_are_silent() {
        let after = folder();
        let mut before = after.clone();
        before.uidvalidity = None;
        before.uidnext = None;
        before.last_uid = 0;
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
        before.uidvalidity = after.uidvalidity;
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
        before.uidnext = Some(1);
        before.uidvalidity = Some(3);
        assert!(new_mail(&before, &after, vec![message(10)]).is_empty());
    }
    #[test]
    fn empty_synced_inbox_and_cached_restart_accept_new_uids() {
        let after = folder();
        let mut before = after.clone();
        before.last_uid = 0;
        before.uidnext = Some(1);
        assert_eq!(new_mail(&before, &after, vec![message(10)]), vec![message(10)]);
        before.last_uid = 9;
        assert_eq!(new_mail(&before, &after, vec![message(10)]), vec![message(10)]);
        assert!(new_mail(&after, &after, vec![message(10)]).is_empty());
    }
    #[test]
    fn only_new_unread_unsnoozed_inbox_mail_counts() {
        let before = folder();
        let mut after = before.clone();
        after.last_uid = 20;
        let mut messages: Vec<_> = (10..=17).map(message).collect();
        messages[2].seen = true;
        messages[3].snoozed_until = Some(i64::MAX);
        messages[4].snoozed_until = Some(1);
        messages[5].folder = 2;
        messages[6].account = "other".into();
        messages.push(message(21));
        assert_eq!(new_mail(&before, &after, messages), vec![message(11), message(17)]);
        after.role = FolderRole::Archive;
        assert!(new_mail(&before, &after, vec![message(11)]).is_empty());
        let mut before = before;
        before.role = FolderRole::Archive;
        assert!(new_mail(&before, &after, vec![message(11)]).is_empty());
    }
}

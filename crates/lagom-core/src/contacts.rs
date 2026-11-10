//! Account-local address lookup. Build off the UI thread; prefix lookups never scan messages.
use std::collections::{HashMap, HashSet};
use lettre::message::Mailbox;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use crate::{store::Store, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    pub name: String,
    pub address: String,
    pub score: f64,
    /// The user sent mail to this address (Sent or Drafts). Others rank lower and are never auto-picked.
    pub written: bool,
}

impl Contact {
    pub fn recipient(&self) -> String {
        format_recipient(&self.name, &self.address)
    }
}

pub fn format_recipient(name: &str, address: &str) -> String {
    if name.is_empty() { return address.to_owned(); }
    let name = name.replace('\\', "\\\\").replace('"', "\\\"");
    if name.contains([',', ';', '"', '<', '>', '(', ')', ':', '\\', '[', ']', '@', '.']) {
        format!("\"{name}\" <{address}>")
    } else {
        format!("{name} <{address}>")
    }
}

pub fn valid_recipient(value: &str) -> bool {
    !value.chars().any(char::is_control) && value.parse::<Mailbox>().is_ok()
}

pub fn recipient_address(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) { return None; }
    value.parse::<Mailbox>().ok().map(|mailbox| mailbox.email.to_string())
}

/// Keep invalid pieces too: silently dropping a pasted recipient could send incomplete mail.
pub fn split_recipients(value: &str) -> Vec<String> {
    let (mut quoted, mut escaped, mut angle, mut comments, mut start) = (false, false, false, 0usize, 0);
    let mut parts = Vec::new();
    for (at, c) in value.char_indices() {
        if escaped { escaped = false; continue; }
        match c {
            '\\' if quoted || comments > 0 => escaped = true,
            '"' if comments == 0 => quoted = !quoted,
            '(' if !quoted => comments += 1,
            ')' if !quoted => comments = comments.saturating_sub(1),
            '<' if !quoted && comments == 0 => angle = true,
            '>' if !quoted && comments == 0 => angle = false,
            ',' | ';' | '\n' | '\r' if !quoted && !angle && comments == 0 => {
                let part = value[start..at].trim();
                if !part.is_empty() { parts.push(part.to_owned()); }
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    let part = value[start..].trim();
    if !part.is_empty() { parts.push(part.to_owned()); }
    parts
}

fn folded(value: &str) -> String {
    value.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

#[derive(Default)]
pub struct Contacts {
    entries: Vec<Contact>,
    prefixes: HashMap<String, Vec<usize>>,
}

impl Contacts {
    pub fn new(mut entries: Vec<Contact>) -> Self {
        entries.sort_by(|a, b| b.written.cmp(&a.written).then(b.score.total_cmp(&a.score)).then(a.address.cmp(&b.address)));
        let mut prefixes: HashMap<String, Vec<usize>> = HashMap::new();
        for (id, contact) in entries.iter().enumerate() {
            for value in [&contact.name, &contact.address] {
                // shortcut: index only the first 256 folded characters per field to bound untrusted headers; use a compact index if longer matches are needed.
                let text: String = folded(value).chars().take(256).collect();
                // Index word starts, retaining punctuation in the suffix so full addresses match too.
                for (at, _) in text.char_indices().filter(|(at, c)| c.is_alphanumeric() && (*at == 0 || !text[..*at].chars().next_back().unwrap().is_alphanumeric())) {
                    for (end, c) in text[at..].char_indices() {
                        if c.is_whitespace() { break; }
                        let ids = prefixes.entry(text[at..at + end + c.len_utf8()].to_owned()).or_default();
                        if ids.len() < 6 && ids.last() != Some(&id) { ids.push(id); }
                    }
                }
            }
        }
        Self { entries, prefixes }
    }

    pub fn suggest(&self, query: &str) -> Vec<Contact> {
        self.prefixes.get(&folded(query.trim())).into_iter().flatten().map(|i| self.entries[*i].clone()).collect()
    }
}

impl Store {
    /// Snapshot of known correspondents, excluding the configured account address.
    // shortcut: aliases are not inferred from mail; include them when explicit send-as identities are supported.
    // shortcut: old cached To/Cc have no names; learn them from From or subsequent sync.
    // shortcut: refreshed when compose opens; refresh an open sheet on sync if live suggestions are needed.
    pub fn contacts(&self, account: &str, now: i64) -> Result<Contacts> {
        let tx = self.db.unchecked_transaction()?;
        let mut own = HashSet::new();
        let mut stmt = self.db.prepare("SELECT email FROM accounts WHERE id=?1")?;
        for row in stmt.query_map([account], |r| r.get::<_, String>(0))? {
            own.insert(row?.to_lowercase());
        }
        // (contact, name source as (from sent mail, date), score from received mail)
        let mut entries: HashMap<String, (Contact, (bool, i64), f64)> = HashMap::new();
        let mut messages = HashSet::new();
        // Hostile mail lands in Junk and Trash, and a From header proves nothing: only Sent and Drafts hold the user's mail.
        let mut stmt = self.db.prepare("SELECT m.from_name,m.from_address,m.to_addresses,m.cc,m.date,m.message_id,f.role IN ('sent','drafts') FROM messages m JOIN folders f ON f.id=coalesce(m.move_target,m.folder) WHERE m.account=?1 AND f.role NOT IN ('junk','trash') ORDER BY 7 DESC")?;
        let mut rows = stmt.query([account])?;
        while let Some(row) = rows.next()? {
            let message_id: String = row.get(5)?;
            // Sent copies come first, so an Archive copy of the same Message-ID cannot take the sent score.
            // shortcut: copies without Message-ID count separately; deduplicate when a reliable provider identity is stored.
            let first_message = message_id.is_empty() || messages.insert(message_id);
            let from: String = row.get(1)?;
            let sent = own.contains(&from.to_lowercase()) && row.get::<_, bool>(6)?;
            let date: i64 = row.get(4)?;
            let age = now.saturating_sub(date).max(0) as f64 / 86400.;
            let score = if sent { 4. } else { 1. } / (1. + age / 30.);
            let mut addresses = split_recipients(&row.get::<_, String>(2)?);
            addresses.extend(split_recipients(&row.get::<_, String>(3)?));
            addresses.push(format_recipient(&row.get::<_, String>(0)?, &from));
            let mut seen = HashSet::new();
            for value in addresses {
                if value.chars().any(char::is_control) { continue; }
                let Ok(mailbox) = value.parse::<Mailbox>() else { continue; };
                let address = mailbox.email.to_string().to_lowercase();
                let local = address.split('@').next().unwrap_or_default();
                if own.contains(&address) || ["noreply", "no-reply", "donotreply", "mailer-daemon"].iter().any(|s| local.contains(s)) { continue; }
                let name = mailbox.name.unwrap_or_default();
                let named = !name.trim().is_empty() && !name.eq_ignore_ascii_case(&address);
                let first = seen.insert(address.clone());
                let (contact, named_at, received) = entries.entry(address.clone()).or_insert_with(|| (Contact { name: String::new(), address, score: 0., written: false }, (false, i64::MIN), 0.));
                if first && first_message { *if sent { &mut contact.score } else { received } += score; }
                contact.written |= sent;
                // A name the user wrote beats any name an incoming header claims for the address.
                if named && (sent, date) >= *named_at {
                    contact.name = name;
                    *named_at = (sent, date);
                }
            }
        }
        tx.commit()?;
        // Incoming mail must not rank or name an address the user wrote to: only sent mail does.
        Ok(Contacts::new(entries.into_values().map(|(mut c, (named_sent, _), received)| {
            if !c.written { c.score = received; } else if !named_sent { c.name.clear(); }
            c
        }).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{account::Account, store::{FolderRole, Message, MessageSummary}};

    fn fixture() -> (tempfile::TempDir, Store, Account, i64, i64) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("contacts.db")).unwrap();
        let (imap, smtp) = crate::presets::servers("self@gmail.com").unwrap();
        let account = Account::new("self@gmail.com".into(), imap, smtp).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store.save_folder(&account.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
        let sent = store.save_folder(&account.id, "Sent", Some("/"), FolderRole::Sent).unwrap();
        (temp, store, account, inbox, sent)
    }

    #[test]
    fn long_headers_have_bounded_prefix_storage() {
        let (_temp, store, account, inbox, _) = fixture();
        for name in ["x".repeat(1024), "x".repeat(200_000), "Ö-".repeat(100_000)] {
            store.save_message(&Message { summary: MessageSummary {
                account: account.id.clone(), folder: inbox, uid: 1,
                from_name: name.clone(), from_address: "long@example.com".into(),
                ..Default::default()
            }, ..Default::default() }).unwrap();
            let index = store.contacts(&account.id, 0).unwrap();
            assert!(index.prefixes.keys().all(|key| key.chars().count() <= 256));
            assert!(index.prefixes.keys().map(String::len).sum::<usize>() < 4 * 1024 * 1024);
            assert_eq!(index.suggest("long@example.com")[0].name, name);
            assert_eq!(index.suggest(&name.chars().next().unwrap().to_string()).len(), 1);
        }
    }

    #[test]
    fn folder_copies_do_not_change_rankings() {
        let (_temp, store, account, inbox, sent) = fixture();
        let mut received = Message { summary: MessageSummary {
            account: account.id.clone(), folder: inbox, uid: 1, date: 10,
            message_id: "received@example.com".into(), from_address: "received@example.com".into(),
            to: account.email.clone(), ..Default::default()
        }, ..Default::default() };
        store.save_message(&received).unwrap();
        store.save_message(&Message { summary: MessageSummary {
            account: account.id.clone(), folder: sent, uid: 1, date: 10,
            message_id: "sent@example.com".into(), from_address: account.email.clone(),
            to: "sent@example.com".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
        let before = store.contacts(&account.id, 10).unwrap().suggest("example");
        for n in 1..=4 {
            received.summary.folder = store.save_folder(&account.id, &format!("Copy {n}"), None, FolderRole::Other).unwrap();
            store.save_message(&received).unwrap();
        }
        assert_eq!(store.contacts(&account.id, 10).unwrap().suggest("example"), before);
        received.summary.uid = 2;
        received.summary.message_id = "another@example.com".into();
        store.save_message(&received).unwrap();
        assert_eq!(store.contacts(&account.id, 10).unwrap().suggest("received")[0].score, 2.);
    }

    #[test]
    fn folder_copies_contribute_names_without_duplicate_scores() {
        let (_temp, store, account, inbox, sent) = fixture();
        let mut message = Message { summary: MessageSummary {
            account: account.id.clone(), folder: inbox, uid: 1, date: 10,
            message_id: "copy@example.com".into(), from_address: "sender@example.com".into(),
            to: "west@example.com".into(), cc: "east@example.com".into(), ..Default::default()
        }, ..Default::default() };
        store.save_message(&message).unwrap();
        let before = store.contacts(&account.id, 10).unwrap().suggest("example");
        message.summary.folder = sent;
        message.summary.from_name = "Team Sender".into();
        message.summary.to = "Team West <west@example.com>".into();
        message.summary.cc = "Team East <east@example.com>".into();
        store.save_message(&message).unwrap();
        let contacts = store.contacts(&account.id, 10).unwrap();
        let named = contacts.suggest("Team");
        assert_eq!(named.len(), 3);
        for contact in named {
            assert_eq!(contact.score, before.iter().find(|old| old.address == contact.address).unwrap().score);
            assert!(contact.name.starts_with("Team "));
        }
    }

    #[test]
    fn sent_placement_does_not_establish_address_ownership() {
        let (_temp, store, account, inbox, sent) = fixture();
        let mut received = Message { summary: MessageSummary {
            account: account.id.clone(), folder: inbox, uid: 1, date: 10,
            from_address: "received@example.com".into(), to: account.email.clone(),
            cc: "team@example.com".into(), message_id: "received@example.com".into(), ..Default::default()
        }, ..Default::default() };
        let id = store.save_message(&received).unwrap();
        let before = store.contacts(&account.id, 10).unwrap().suggest("example");
        received.summary.folder = sent;
        store.save_message(&received).unwrap();
        assert_eq!(store.contacts(&account.id, 10).unwrap().suggest("example"), before);
        store.delete_message(id).unwrap();
        assert_eq!(store.contacts(&account.id, 10).unwrap().suggest("example"), before);
        assert_eq!(before.len(), 2);
        assert!(before.iter().all(|contact| contact.score == 1.));
    }

    #[test]
    fn ranking_names_exclusions_and_account_isolation() {
        let (_temp, store, account, inbox, sent) = fixture();
        let now = 1_800_000_000;
        for (uid, folder, from, to, name, date) in [
            (1, inbox, "received@example.com", "self@gmail.com", "Équipe Ångström Öst", now),
            (2, sent, "SELF@gmail.com", "Sent Team <sent@example.com>", "", now),
            (3, inbox, "old@example.com", "self@gmail.com", "", now - 86400 * 365),
            (4, inbox, "no-reply@example.com", "noreply@example.com, donotreply@example.com, mailer-daemon@example.com", "", now),
            (5, inbox, "received@example.com", "self@gmail.com", "", now + 1),
        ] {
            store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid, from_address: from.into(), to: to.into(), from_name: name.into(), date, ..Default::default() }, body_text: String::new(), body_html: None }).unwrap();
        }
        let index = store.contacts(&account.id, now).unwrap();
        assert_eq!(index.suggest("example").iter().map(|c| c.address.as_str()).collect::<Vec<_>>(), ["sent@example.com", "received@example.com", "old@example.com"]);
        for query in ["eq", "ANG", "oSt", "a\u{30a}ng"] {
            assert_eq!(index.suggest(query)[0].name, "Équipe Ångström Öst");
        }
        assert!(index.suggest("ngstrom").is_empty());
        assert!(store.contacts("other", now).unwrap().suggest("example").is_empty());
        assert!(index.suggest("self").is_empty());
        assert!(index.suggest("noreply").is_empty());
    }

    #[test]
    fn sent_frequency_and_recency_are_independent_ranking_signals() {
        let (_temp, store, account, inbox, sent) = fixture();
        let now = 1_800_000_000;
        for (uid, folder, from, to, date) in [
            (1, sent, "self@gmail.com", "rank-sent@example.com", now),
            (2, inbox, "rank-recent@example.com", "self@gmail.com", now),
            (3, inbox, "rank-old@example.com", "self@gmail.com", now - 86400 * 365),
            (4, inbox, "rank-frequent@example.com", "self@gmail.com", now),
            (5, inbox, "rank-frequent@example.com", "self@gmail.com", now),
        ] {
            store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid, from_address: from.into(), to: to.into(), date, ..Default::default() }, ..Default::default() }).unwrap();
        }
        let mut other = account.clone();
        other.id = "other".into();
        other.email = "other@gmail.com".into();
        store.save_account(&other).unwrap();
        let folder = store.save_folder(&other.id, "INBOX", None, FolderRole::Inbox).unwrap();
        store.save_message(&Message { summary: MessageSummary { account: other.id.clone(), folder, uid: 1, from_address: "rank-other@example.com".into(), date: now, ..Default::default() }, ..Default::default() }).unwrap();
        let results = store.contacts(&account.id, now).unwrap().suggest("rank");
        assert_eq!(results.iter().map(|c| c.address.as_str()).collect::<Vec<_>>(), ["rank-sent@example.com", "rank-frequent@example.com", "rank-recent@example.com", "rank-old@example.com"]);
        assert_eq!(results[0].score, 4.);
        assert_eq!(results[1].score, 2.);
        assert_eq!(store.contacts(&other.id, now).unwrap().suggest("rank")[0].address, "rank-other@example.com");
    }

    #[test]
    fn junk_trash_and_spoofed_from_cannot_outrank_a_written_address() {
        let (_temp, store, account, inbox, sent) = fixture();
        let junk = store.save_folder(&account.id, "Junk", None, FolderRole::Junk).unwrap();
        let trash = store.save_folder(&account.id, "Trash", None, FolderRole::Trash).unwrap();
        let save = |uid, folder, from: &str, to: &str| store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid, from_address: from.into(), to: to.into(), date: 10, ..Default::default() }, ..Default::default() }).unwrap();
        save(1, sent, "self@gmail.com", "Anna Berg <anna.berg@corp.example>");
        for uid in 2..=6 {
            save(uid, junk, "self@gmail.com", "Anna Berg <anna.berg@junk.example>");
            save(uid, trash, "self@gmail.com", "Anna Berg <anna.berg@trash.example>");
            // The user's own From, but outside Sent and Drafts: received weight, not written.
            save(uid, inbox, "self@gmail.com", "Anna Berg <anna.berg@spoof.example>");
            save(uid + 10, inbox, "anna.berg@lookalike.example", "self@gmail.com");
        }
        let results = store.contacts(&account.id, 10).unwrap().suggest("anna");
        assert_eq!(results.iter().map(|c| (c.address.as_str(), c.written)).collect::<Vec<_>>(),
            [("anna.berg@corp.example", true), ("anna.berg@lookalike.example", false), ("anna.berg@spoof.example", false)]);
        assert_eq!((results[0].score, results[1].score, results[2].score), (4., 5., 5.));
    }

    #[test]
    fn incoming_mail_neither_names_nor_ranks_a_written_address() {
        let (_temp, store, account, inbox, sent) = fixture();
        let save = |uid, folder, name: &str, from: &str, to: &str, id: &str| store.save_message(&Message { summary: MessageSummary { account: account.id.clone(), folder, uid, from_name: name.into(), from_address: from.into(), to: to.into(), message_id: id.into(), date: 10, ..Default::default() }, ..Default::default() }).unwrap();
        // The Inbox copy of a sent mail is read first; the Sent copy must still score.
        save(1, inbox, "", "self@gmail.com", "vendor@example.com", "<m1@self>");
        save(1, sent, "", "self@gmail.com", "vendor@example.com", "<m1@self>");
        for uid in 2..=9 { save(uid, inbox, "Finance Director", "vendor@example.com", "self@gmail.com", ""); }
        let contacts = store.contacts(&account.id, 10).unwrap();
        assert!(contacts.suggest("finance").is_empty(), "an incoming header must not name a written address");
        let vendor = &contacts.suggest("vendor")[0];
        assert_eq!((vendor.name.as_str(), vendor.written, vendor.score), ("", true, 4.));
    }

    #[test]
    fn snapshots_follow_updates_deletion_and_the_best_nonempty_name() {
        let (_temp, store, account, inbox, _) = fixture();
        let mut mail = Message { summary: MessageSummary { account: account.id.clone(), folder: inbox, uid: 1, from_name: "Team West".into(), from_address: "west@example.com".into(), to: "west@example.com".into(), date: 10, ..Default::default() }, ..Default::default() };
        let id = store.save_message(&mail).unwrap();
        let result = store.contacts(&account.id, 10).unwrap().suggest("west");
        assert_eq!(result[0].name, "Team West");
        assert_eq!(result[0].score, 1.);
        mail.summary.uid = 2;
        mail.summary.from_name = "west@example.com".into();
        mail.summary.date = 11;
        let second = store.save_message(&mail).unwrap();
        assert_eq!(store.contacts(&account.id, 11).unwrap().suggest("west")[0].name, "Team West");
        store.delete_message(second).unwrap();
        mail.summary.uid = 1;
        mail.summary.from_address = "east@example.com".into();
        mail.summary.from_name = "Team East".into();
        mail.summary.to.clear();
        store.save_message(&mail).unwrap();
        assert!(store.contacts(&account.id, 11).unwrap().suggest("west").is_empty());
        assert_eq!(store.contacts(&account.id, 11).unwrap().suggest("east")[0].name, "Team East");
        store.delete_message(id).unwrap();
        assert!(store.contacts(&account.id, 11).unwrap().suggest("east").is_empty());
    }

    #[test]
    fn split_and_validate_without_losing_invalid_recipients() {
        let parts = split_recipients("\"Team, West\" <west@example.com>; east@example.com\r\nbad@@example.com,missing, ");
        assert_eq!(parts, ["\"Team, West\" <west@example.com>", "east@example.com", "bad@@example.com", "missing"]);
        assert_eq!(parts.iter().map(|s| valid_recipient(s)).collect::<Vec<_>>(), [true, true, false, false]);
        assert!(!valid_recipient("x@example.com\r\nBcc: y@example.com"));
        let quoted = format_recipient("Team, \"West\"", "west@example.com");
        assert!(valid_recipient(&quoted));
        assert_eq!(split_recipients(&quoted), [quoted]);
        for name in ["Team. East", "Team [East]", "team@example.com"] {
            assert!(valid_recipient(&format_recipient(name, "east@example.com")));
        }
        assert_eq!(split_recipients("Team (West, Support) <west@example.com>, east@example.com"), ["Team (West, Support) <west@example.com>", "east@example.com"]);
    }

    #[test]
    fn lookup_50k_messages_under_5ms() {
        let (_temp, store, account, inbox, _) = fixture();
        let tx = store.db.unchecked_transaction().unwrap();
        {
            let mut insert = store.db.prepare("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text) VALUES(?1,?2,?3,'','','Équipe Öst',?4,'self@gmail.com','','',?3,'',0,0,0,0,'')").unwrap();
            for uid in 1..=50_000 {
                insert.execute(rusqlite::params![account.id, inbox, uid, format!("contact{uid}@example.com")]).unwrap();
            }
        }
        tx.commit().unwrap();
        let start = std::time::Instant::now();
        let index = store.contacts(&account.id, 50_000).unwrap();
        println!("50k background index build: {:?}", start.elapsed());
        for query in ["c", "contact50000", "o", "EQ", "example", "missing"] {
            let start = std::time::Instant::now();
            let result = index.suggest(query);
            let elapsed = start.elapsed();
            println!("50k lookup {query}: {elapsed:?}");
            assert_eq!(result.len(), if query == "missing" { 0 } else if query == "contact50000" { 1 } else { 6 });
            assert!(elapsed < std::time::Duration::from_millis(5), "{query}: {elapsed:?}");
        }
    }
}

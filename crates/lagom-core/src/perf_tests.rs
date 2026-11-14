//! Headless benchmarks for the budgets in `docs/performance-budgets.toml`. Ignored by default;
//! `python3 scripts/perf.py` runs them in release and compares each `perf <key> <value>` line
//! with its budget. Every measurement is the best of five runs.
use crate::account::Account;
use crate::store::{FolderRole, MessageSummary, Store};
use rusqlite::params;
use std::time::{Duration, Instant};

fn best(key: &str, mut run: impl FnMut()) {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        run();
        best = best.min(start.elapsed());
    }
    println!("perf {key} {:.3}", best.as_secs_f64() * 1000.);
}

fn store(dir: &tempfile::TempDir) -> (Store, Account, i64) {
    let store = Store::open(dir.path().join("budget.db")).unwrap();
    let (i, s) = crate::presets::servers("budget@gmail.com").unwrap();
    let a = Account::new("budget@gmail.com".into(), i, s).unwrap();
    store.save_account(&a).unwrap();
    let f = store.save_folder(&a.id, "INBOX", Some("/"), FolderRole::Inbox).unwrap();
    (store, a, f)
}

/// A typical newsletter: a short text part and an HTML part of about 25 KB.
fn typical_mime(uid: u32) -> String {
    let html = include_str!("../tests/fixtures/newsletter.html").repeat(30);
    let text = "Hello and welcome to this week's news.\r\n".repeat(60);
    format!("From: News <news@example.com>\r\nTo: budget@gmail.com\r\nSubject: Weekly news {uid}\r\nMessage-ID: <{uid}@example.com>\r\nDate: Fri, 13 Nov 2026 09:00:00 +0000\r\nList-Unsubscribe: <https://example.com/u/{uid}>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--b--\r\n")
}

#[test]
#[ignore = "budget benchmark; run with scripts/perf.py"]
fn search_and_open_100k() {
    let temp = tempfile::tempdir().unwrap();
    let (store, a, f) = store(&temp);
    let mut db = rusqlite::Connection::open(store.path()).unwrap();
    let tx = db.transaction().unwrap();
    {
        let mut insert = tx.prepare("INSERT INTO messages(account,folder,uid,message_id,thread_key,from_name,from_address,to_addresses,cc,subject,date,snippet,seen,flagged,has_attachments,size,body_text) VALUES(?1,?2,?3,'','','Alice','alice@example.com','budget@gmail.com','','Status update',?3,'',0,0,0,512,?4)").unwrap();
        for uid in 1..=100_000 {
            let body = format!("sqlite common body unique{uid} {}", "message text ".repeat(30));
            insert.execute(params![a.id, f, uid, body]).unwrap();
        }
    }
    tx.commit().unwrap();
    for (key, query) in [("search.unique", "unique50000"), ("search.common", "sqlite"), ("search.from", "Alice"), ("search.subject", "subject:Status")] {
        assert!(!store.search(&a.id, query, i64::MAX, 50, 0).unwrap().is_empty(), "{query}");
        best(key, || { store.search(&a.id, query, i64::MAX, 50, 0).unwrap(); });
    }
    // What the reading pane reads when a message opens (main_window/live.rs).
    let id: i64 = db.query_row("SELECT id FROM messages WHERE uid=50000", [], |r| r.get(0)).unwrap();
    best("open.message", || {
        store.message(id).unwrap();
        store.attachments(id).unwrap();
        store.inline_parts(id).unwrap();
    });
}

#[test]
#[ignore = "budget benchmark; run with scripts/perf.py"]
fn mime_and_html() {
    let typical = typical_mime(1);
    assert!(!crate::mime::parse(typical.as_bytes()).unwrap().body_text.is_empty());
    best("mime.typical", || { crate::mime::parse(typical.as_bytes()).unwrap(); });
    // Just under the 4 MiB sync limit: an HTML body and a base64 PDF.
    let pdf = "JVBERi0xLjQKJcfsj6IK".repeat(3_000_000 / 20).as_bytes().chunks(76).map(|l| std::str::from_utf8(l).unwrap()).collect::<Vec<_>>().join("\r\n");
    let large = format!("From: a@example.com\r\nSubject: Large\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\n{}\r\n--m\r\nContent-Type: application/pdf; name=report.pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\n{pdf}\r\n--m--\r\n",
        typical_mime(2).split_once("MIME-Version: 1.0\r\n").unwrap().1);
    assert!(large.len() < crate::mime::MAX_MIME_BYTES);
    assert!(!crate::mime::parse(large.as_bytes()).unwrap().body_text.is_empty());
    best("mime.large", || { crate::mime::parse(large.as_bytes()).unwrap(); });
    let html = include_str!("../tests/fixtures/newsletter.html").repeat(120);
    assert!(html.len() > 100_000 && crate::mime::html_to_text(&html).contains("Hello & welcome"));
    best("html.to_text_100kb", || { crate::mime::html_to_text(&html); });
}

/// Parse and save as sync does for each fetched message (imap_sync.rs `save`), one transaction each.
#[test]
#[ignore = "budget benchmark; run with scripts/perf.py"]
fn sync_save_1000() {
    let raws: Vec<_> = (1..=1000).map(typical_mime).collect();
    best("sync.parse_save_1000", || {
        let temp = tempfile::tempdir().unwrap();
        let (store, a, f) = store(&temp);
        for (uid, raw) in (1..).zip(&raws) {
            let mut m = crate::mime::parse(raw.as_bytes()).unwrap();
            m.summary = MessageSummary { account: a.id.clone(), folder: f, uid, ..m.summary };
            let id = store.save_synced_message(&m, &[]).unwrap();
            store.set_unsubscribe(id, None).unwrap();
        }
    });
}

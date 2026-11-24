use super::{
    audit::*,
    id::*,
    policy::{Level, Resource::*},
    protocol::{Code, Response},
    token::TokenScope,
};
use crate::{
    account::Account,
    store::{
        mail_tests::{mail, setup},
        FolderRole,
    },
};
use rusqlite::params;
use sha2::{Digest, Sha256};

#[test]
fn effective_intersects_live_matrices_and_token_scope() {
    let (_dir, s, a, _) = setup();
    let b = Account::new("b@example.org".into(), a.imap.clone(), a.smtp.clone()).unwrap();
    s.save_account(&b).unwrap();
    let agent = s.create_agent("test", "remote", "host", true).unwrap();
    let token = s
        .issue_token(
            &agent,
            TokenScope::Remote {
                accounts: std::slice::from_ref(&a.id),
                level: Level::Triage,
                ranges: &[],
                expires: None,
            },
        )
        .unwrap();
    assert!(matches!(
        s.authenticate(Some(&token), None).map_err(Code::from),
        Err(Code::Unauthorized)
    ));
    let level = |kind, id: &str| s.effective(Some(&token), Some("::1"), kind, id).unwrap();
    let set = |who: &str, kind: &str, id: &str, n: Level| {
        s.db.execute(
            "INSERT OR REPLACE INTO agent_access VALUES(?,?,?,?)",
            params![who, kind, id, n as i64],
        )
        .unwrap();
    };
    s.db.execute(
        "UPDATE agent_settings SET value='1' WHERE key='enabled'",
        [],
    )
    .unwrap();
    assert_eq!(level(Mail, &a.id), Level::None);
    set("", "mail", "", Level::Triage);
    assert_eq!(level(Mail, &a.id), Level::Read);
    set(&agent, "mail", "", Level::Send);
    set("", "mail", &a.id, Level::Read);
    assert_eq!(level(Mail, &a.id), Level::Read);
    set("", "mail", &a.id, Level::Send);
    assert_eq!(level(Mail, &a.id), Level::Triage);
    assert_eq!(level(Mail, &b.id), Level::None);
    set(&agent, "mail", &a.id, Level::None);
    assert_eq!(level(Mail, &a.id), Level::None);
    assert_eq!(level(Calendar, &a.id), Level::None);
    set("", "calendar", "", Level::Send);
    assert_eq!(level(Calendar, &a.id), Level::Read);
    set(&agent, "calendar", "", Level::Send);
    assert_eq!(level(Calendar, &a.id), Level::Read);
    s.db.execute(
        "UPDATE agent_settings SET value='0' WHERE key='enabled'",
        [],
    )
    .unwrap();
    assert_eq!(level(Calendar, &a.id), Level::None);
    s.delete_account(&a.id).unwrap();
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM agent_access WHERE account=?",
            [&a.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn authenticate_checks_digest_expiry_revocation_and_source() {
    let (_dir, s, a, _) = setup();
    let remote = s.create_agent("test", "remote", "host", true).unwrap();
    let ranges = [
        "203.0.113.0/24".parse().unwrap(),
        "2001:db8::/48".parse().unwrap(),
    ];
    let token = s
        .issue_token(
            &remote,
            TokenScope::Remote {
                accounts: &[a.id],
                level: Level::Read,
                ranges: &ranges,
                expires: None,
            },
        )
        .unwrap();
    let auth = |raw: Option<&str>, ip: Option<&str>| {
        s.authenticate(raw, ip).map(|_| ()).map_err(Code::from)
    };
    let hash: Vec<u8> =
        s.db.query_row("SELECT hash FROM agent_tokens", [], |r| r.get(0))
            .unwrap();
    assert_eq!(hash, Sha256::digest(token.as_bytes()).as_slice());
    for ip in ["203.0.113.255", "2001:db8:0:ffff::1"] {
        assert_eq!(auth(Some(&token), Some(ip)), Ok(()));
    }
    for ip in [
        Some("203.0.112.255"),
        Some("2001:db8:1::"),
        Some("bad"),
        None,
    ] {
        assert_eq!(auth(Some(&token), ip), Err(Code::Unauthorized));
    }
    let ip = Some("203.0.113.1");
    let wrong = format!("{}_{}", &token[..12], "A".repeat(43));
    let other = if &token[4..12] == "00000000" {
        "ffffffff"
    } else {
        "00000000"
    };
    let unknown = format!("lgm_{other}_{}", &token[13..]);
    for raw in [None, Some("lgm_bad"), Some(&wrong), Some(&unknown)] {
        assert_eq!(auth(raw, ip), Err(Code::Unauthorized));
    }
    s.db.execute("UPDATE agent_tokens SET expires=unixepoch()", [])
        .unwrap();
    assert_eq!(auth(Some(&token), ip), Err(Code::Unauthorized));
    s.db.execute("UPDATE agent_tokens SET expires=created+86400", [])
        .unwrap();
    assert_eq!(auth(Some(&token), ip), Ok(()));
    s.revoke_agent_tokens(&remote).unwrap();
    assert_eq!(auth(Some(&token), ip), Err(Code::Unauthorized));
    let local = s.create_agent("test", "local", "host", false).unwrap();
    let old = s.issue_token(&local, TokenScope::Local).unwrap();
    let new = s.issue_token(&local, TokenScope::Local).unwrap();
    assert_eq!(auth(Some(&old), None), Err(Code::Unauthorized));
    assert_eq!(auth(Some(&new), None), Ok(()));
}

#[test]
fn resolution_checks_account_before_lookup_and_all_copies_before_write() {
    let (_dir, s, a, inbox) = setup();
    let agent = s.create_agent("test", "remote", "host", true).unwrap();
    let token = s
        .issue_token(
            &agent,
            TokenScope::Remote {
                accounts: std::slice::from_ref(&a.id),
                level: Level::Send,
                ranges: &[],
                expires: None,
            },
        )
        .unwrap();
    s.db.execute_batch("UPDATE agent_settings SET value='1' WHERE key='enabled';
        INSERT INTO agent_access VALUES('','mail','',3); UPDATE agent_access SET level=3 WHERE kind='mail';").unwrap();
    let lookup = |key: &MessageId, folder: Option<&str>, required| {
        s.resolve_agent_message(Some(&token), Some("::1"), &key.encode(), folder, required)
            .map_err(Code::from)
    };
    let key = MessageId::Header(a.id.clone(), "m@example.org".into());
    let mut m = mail(&a, inbox, 1);
    m.summary.message_id = "m@example.org".into();
    let first = s.save_message(&m).unwrap();
    m.summary.folder = s
        .save_folder(&a.id, "Archive", None, FolderRole::Archive)
        .unwrap();
    let second = s.save_message(&m).unwrap();
    assert_eq!(lookup(&key, None, Level::Triage).unwrap().row, first);
    assert_eq!(
        lookup(&key, Some("Archive"), Level::Read).unwrap().row,
        second
    );
    s.db.execute("UPDATE messages SET size=size+1 WHERE id=?", [second])
        .unwrap();
    assert!(lookup(&key, Some("INBOX"), Level::Read).unwrap().ambiguous);
    assert_eq!(
        lookup(&key, Some("INBOX"), Level::Triage),
        Err(Code::Conflict)
    );
    m.summary.folder = inbox;
    m.summary.uid = 2;
    s.save_message(&m).unwrap();
    s.db.execute("UPDATE messages SET size=0", []).unwrap();
    assert_eq!(lookup(&key, None, Level::Triage), Err(Code::Conflict));
    let uid = MessageId::Uid(a.id.clone(), "INBOX".into(), 7, 3);
    s.db.execute("UPDATE folders SET uidvalidity=7 WHERE id=?", [inbox])
        .unwrap();
    let row = s.save_message(&mail(&a, inbox, 3)).unwrap();
    assert_eq!(lookup(&uid, None, Level::Read).unwrap().row, row);
    for id in [
        ObjectId::Message(key),
        ObjectId::Message(uid.clone()),
        ObjectId::Attachment(uid, "2.1".into()),
        ObjectId::Draft(a.id.clone(), "draft".into()),
        ObjectId::Event(a.id.clone(), "cal".into(), "event".into()),
        ObjectId::Thread(a.id.clone(), "tid".into()),
    ] {
        assert_eq!(ObjectId::decode(&id.encode()), Ok(id));
    }
    let absent = lookup(
        &MessageId::Header(a.id.clone(), "missing".into()),
        None,
        Level::Read,
    )
    .unwrap_err();
    // Any message lookup now fails to prepare.
    s.db.execute_batch("ALTER TABLE messages RENAME TO inaccessible")
        .unwrap();
    let foreign = lookup(
        &MessageId::Header("other".into(), "forged".into()),
        None,
        Level::Read,
    )
    .unwrap_err();
    assert_eq!(foreign, Code::NotFound);
    assert_eq!(
        serde_json::to_vec(&Response::Failure(foreign)).unwrap(),
        serde_json::to_vec(&Response::Failure(absent)).unwrap()
    );
}

#[test]
fn audit_limits_anonymous_refusals_and_prunes_by_age_and_count() {
    let (_dir, s, _, _) = setup();
    let now = chrono::Utc::now().timestamp();
    for _ in 0..63 {
        s.record_agent_audit_at(
            AuditRow {
                identity: None,
                account: None,
                command: "subject:private",
                target: None,
                outcome: Err(Failure::Unauthorized(TokenFailure::Malformed)),
                source_ip: None,
                recipients: &["private@example.org".parse().unwrap()],
            },
            now,
        )
        .unwrap();
    }
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM agent_audit WHERE command='unknown' AND detail IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        60
    );
    assert_eq!(
        s.db.query_row(
            "SELECT result FROM agent_audit WHERE command='refusals'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "unauthorized:3"
    );
    let now = chrono::Utc::now().timestamp() + 1;
    s.db.execute(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50001)
        INSERT INTO agent_audit(at,command,result) SELECT ?,'emails','ok' FROM n",
        [now],
    )
    .unwrap();
    s.db.execute(
        "INSERT INTO agent_audit(at,command,result) VALUES(0,'old','ok')",
        [],
    )
    .unwrap();
    s.prune_agent_audit(now).unwrap();
    assert_eq!(
        s.db.query_row("SELECT count(*),min(at) FROM agent_audit", [], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?
        )))
        .unwrap(),
        (50000, now)
    );
}

#[test]
fn replies_accept_only_one_payload_and_fixed_error_text() {
    for reply in [
        Response::Success(serde_json::Value::Null),
        Response::Failure(Code::Unauthorized),
    ] {
        let bytes = serde_json::to_vec(&reply).unwrap();
        assert_eq!(serde_json::from_slice::<Response>(&bytes).unwrap(), reply);
    }
    for line in [
        r#"{"v":1,"ok":true,"error":{"code":"internal","message":"Internal error."}}"#,
        r#"{"v":1,"ok":false,"error":{"code":"internal","message":"private data"}}"#,
        r#"{"v":2,"ok":true,"result":null}"#,
    ] {
        assert!(serde_json::from_str::<Response>(line).is_err());
    }
}

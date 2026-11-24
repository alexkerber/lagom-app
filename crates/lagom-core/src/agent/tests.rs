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
    for ip in [
        "203.0.113.255",
        "::ffff:203.0.113.255",
        "2001:db8:0:ffff::1",
    ] {
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
    s.db.execute("UPDATE agent_tokens SET cidr='[\"::/0\"]'", [])
        .unwrap();
    for ip in ["203.0.113.1", "::ffff:203.0.113.1"] {
        assert_eq!(auth(Some(&token), Some(ip)), Err(Code::Unauthorized));
    }
    assert_eq!(auth(Some(&token), Some("::1")), Ok(()));
    s.db.execute("UPDATE agent_tokens SET cidr='[]'", [])
        .unwrap();
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
    let check_refusal = |source, reason: &str| {
        let failure = s.authenticate(Some(&token), source).err().unwrap();
        assert_eq!(Code::from(failure.clone()), Code::Unauthorized);
        s.record_agent_audit(AuditRow {
            identity: None,
            account: None,
            command: "emails",
            target: None,
            outcome: Err(failure),
            source_ip: None,
            recipients: &[],
        })
        .unwrap();
        let actual =
            s.db.query_row(
                "SELECT agent,token,result FROM agent_audit ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            actual,
            (
                remote.clone(),
                token[4..12].into(),
                format!("unauthorized:{reason}")
            )
        );
    };
    check_refusal(None, "source");
    s.db.execute("UPDATE agent_tokens SET expires=unixepoch()", [])
        .unwrap();
    assert_eq!(auth(Some(&token), ip), Err(Code::Unauthorized));
    check_refusal(ip, "expired");
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
    assert_eq!(auth(Some(&new), ip), Err(Code::Unauthorized));
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
    let identity = s.authenticate(Some(&token), Some("::1")).unwrap();
    let noncanonical = format!("m1.{}", {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string_pretty(&(&a.id, "m@example.org")).unwrap())
    });
    let found = s
        .resolve_agent_message(Some(&token), Some("::1"), &noncanonical, None, Level::Read)
        .unwrap();
    for (outcome, target, expected) in [
        (Ok(()), Some(&found), Some(key.encode())),
        (Err(Failure::OutsideScope), Some(&found), None),
        (Err(Code::NotFound.into()), None, None),
    ] {
        s.record_agent_audit(AuditRow {
            identity: Some(&identity),
            account: None,
            command: "thread",
            target,
            outcome,
            source_ip: None,
            recipients: &[],
        })
        .unwrap();
        let stored: Option<String> =
            s.db.query_row(
                "SELECT target FROM agent_audit ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, expected);
    }
    let long = MessageId::Header(a.id.clone(), "x".repeat(2048)).encode();
    assert_eq!(MessageId::decode(&long), Err(Code::NotFound));
    assert_eq!(
        ObjectId::decode(&ObjectId::Draft(a.id.clone(), "x".repeat(2048)).encode()),
        Err(Code::NotFound)
    );
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
fn audit_limits_anonymous_refusals() {
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
}

#[test]
fn audit_caps_reads_per_agent_at_append_and_expires_writes_by_age() {
    let (_dir, s, _, _) = setup();
    let now = chrono::Utc::now().timestamp();
    s.db.execute("INSERT INTO agent_audit(at,agent,command,result) VALUES
        (?1,'quiet','emails','ok'),(?1,'quiet','draft.send','ok'),(?1,NULL,'unknown','unauthorized:missing'),
        (?1,'busy','draft.send','ok'),(?1,'busy','draft.edit','ok'),
        (?1,'busy','action.move','ok'),(?1,'busy','event.update','ok'),(0,'busy','draft.send','ok')",[now]).unwrap();
    s.db.execute(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<60000)
        INSERT INTO agent_audit(at,agent,command,result) SELECT ?,'busy','emails','ok' FROM n",
        [now],
    )
    .unwrap();
    let count = |condition: &str| {
        s.db.query_row(
            &format!("SELECT count(*) FROM agent_audit WHERE {condition}"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(count("agent='busy' AND command='emails'"), 50_000);
    assert_eq!(count("agent='busy' AND command<>'emails'"), 5);
    assert_eq!(count("agent='quiet'"), 2);
    assert_eq!(count("agent IS NULL"), 1);
    s.prune_agent_audit(now).unwrap();
    assert_eq!(count("agent='busy' AND command<>'emails'"), 4);
}

#[test]
fn replies_accept_only_one_payload_and_fixed_error_text() {
    for (code, name, message) in [
        (Code::Unauthorized, "unauthorized", "Token missing, invalid, expired or revoked."),
        (Code::Forbidden, "forbidden", "This agent's access level does not allow this command."),
        (Code::NotFound, "not_found", "Not found."),
        (Code::InvalidRequest, "invalid_request", "Invalid request."),
        (Code::Denied, "denied", "Not approved."),
        (Code::Conflict, "conflict", "Changed or ambiguous. Read it again."),
        (Code::Uncertain, "uncertain", "The outcome is unknown. Check before you retry."),
        (Code::Busy, "busy", "Too many requests. Try again."),
        (Code::Unavailable, "unavailable", "Temporarily unavailable. Try again."),
        (Code::Unsupported, "unsupported", "Not supported."),
        (Code::TooLarge, "too_large", "Too large."),
        (Code::Internal, "internal", "Internal error."),
    ] {
        assert_eq!(serde_json::to_value(Response::Failure(code)).unwrap(),
            serde_json::json!({"v":1,"ok":false,"error":{"code":name,"message":message}}));
    }
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

#[test]
fn settings_apis_store_switches_matrices_receipts_and_revoke_all() {
    use super::policy::{Access, Setting};
    let (_dir, s, a, _) = setup();
    assert!(!s.agent_setting(Setting::Enabled).unwrap());
    assert!(s.agent_setting(Setting::Confirm).unwrap());
    assert!(!s.agent_setting(Setting::Sidebar).unwrap());
    s.set_agent_setting(Setting::Enabled, true).unwrap();
    assert!(s.agent_setting(Setting::Enabled).unwrap());
    let agent = s.create_agent("Codex", "Codex", "host", false).unwrap();
    let other = s.create_agent("Cursor", "Cursor", "host", false).unwrap();
    s.issue_token(&agent, TokenScope::Local).unwrap();
    s.issue_token(&other, TokenScope::Local).unwrap();
    assert!(s.agents().unwrap().iter().all(|r| r.active && !r.remote));
    let send = Access {
        resource: Calendar,
        account: a.id.clone(),
        level: Level::Send,
    };
    s.set_agent_access("", &send).unwrap();
    assert!(s.agent_access("").unwrap().contains(&send));
    let triage = Access {
        level: Level::Triage,
        ..send.clone()
    };
    assert!(matches!(
        s.set_agent_access("", &triage),
        Err(Code::InvalidRequest)
    ));
    // A new agent starts at Read for both resources.
    assert_eq!(
        s.agent_access(&agent)
            .unwrap()
            .iter()
            .map(|r| r.level)
            .collect::<Vec<_>>(),
        [Level::Read; 2]
    );
    s.set_agent_connection(&agent, Some("{}")).unwrap();
    assert_eq!(s.agent_connection(&agent).unwrap().as_deref(), Some("{}"));
    assert_eq!(s.agent_connection(&other).unwrap(), None);
    s.db.execute("UPDATE agent_tokens SET expires=1 WHERE agent=?", [&other]).unwrap();
    assert_eq!(s.agents().unwrap().iter().filter(|r| r.active).count(), 1, "an expired token is not active");
    s.revoke_all_agents().unwrap();
    assert!(s.agents().unwrap().iter().all(|r| !r.active));
    assert!(!s.agent_setting(Setting::Enabled).unwrap());
    s.set_agent_connection(&agent, None).unwrap();
    assert_eq!(s.agent_connection(&agent).unwrap(), None);
    for (at, command) in [(1, "emails"), (2, "search")] {
        s.db.execute(
            "INSERT INTO agent_audit(at,agent,command,result) VALUES(?,?,?,'ok')",
            params![at, agent, command],
        )
        .unwrap();
    }
    let log = s.agent_audit(10).unwrap();
    assert_eq!(
        log.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(),
        ["search", "emails"]
    );
}

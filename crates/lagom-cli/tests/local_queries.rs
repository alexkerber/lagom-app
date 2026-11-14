use lagom_core::{
    account::Account,
    presets,
    store::{FolderRole, Message, MessageSummary, Store},
};
fn run(db: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lagom-cli"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn cli_reads_saved_mail_without_network_or_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("cli.db");
    let store = Store::open(&db).unwrap();
    let (i, s) = presets::servers("cli@gmail.com").unwrap();
    let a = Account::new("cli@gmail.com".into(), i, s).unwrap();
    store.save_account(&a).unwrap();
    let f = store
        .save_folder(&a.id, "INBOX", Some("/"), FolderRole::Inbox)
        .unwrap();
    let id = store
        .save_message(&Message {
            summary: MessageSummary {
                account: a.id,
                folder: f,
                uid: 1,
                subject: "CLI fixture".into(),
                from_address: "alice@example.com".into(),
                ..Default::default()
            },
            body_text: "Searchable mail body".into(),
            body_html: None,
        })
        .unwrap();
    drop(store);
    assert!(run(&db, &["accounts"]).contains("cli@gmail.com"));
    assert!(run(&db, &["folders"]).contains("INBOX"));
    assert!(run(&db, &["list", "--limit", "1"]).contains("CLI fixture"));
    assert!(run(&db, &["show", &id.to_string()]).contains("Searchable mail body"));
    assert!(run(&db, &["search", "Searchable"]).contains("CLI fixture"));
    assert!(run(&db, &["--help"]).contains("add-account"));
    assert!(run(&db, &["sync", "--help"]).contains("ACCOUNT"));
}

#[test]
fn discovery_and_unconfigured_sign_in() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("discovery.db");
    assert_eq!(
        run(&db, &["discover", "alice@gmail.com"]),
        "OAuth: google\n"
    );
    assert_eq!(
        run(&db, &["discover", "alice@hotmail.co.uk"]),
        "OAuth: microsoft\n"
    );
    assert!(run(&db, &["discover", "alice@icloud.com"])
        .contains("https://account.apple.com/account/manage"));
    assert_eq!(
        run(&db, &["discover", "invalid"]),
        "Manual configuration required\n"
    );
    for provider in ["google", "microsoft"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lagom-cli"))
            .args(["--database", db.to_str().unwrap(), "sign-in", provider])
            .env("LAGOM_GOOGLE_CLIENT_ID", "")
            .env("LAGOM_MICROSOFT_CLIENT_ID", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not available in this version")
        );
    }
    assert!(Store::open(&db).unwrap().accounts().unwrap().is_empty());
}

#[test]
fn body_file_is_read_as_utf8_before_account_access() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("empty.db");
    let path = temp.path().join("mail.md");
    for (bytes, error) in [(b"# H\xc3\xa4lsningar".as_slice(), "No matching account"), (b"\xff".as_slice(), "as UTF-8")] {
        std::fs::write(&path, bytes).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lagom-cli"))
            .arg("--database").arg(&db)
            .args(["send", "--to", "a@example.com", "--subject", "Hi", "--body-file"])
            .arg(&path).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
    }
}

#[test]
fn tasks_work_without_accounts_and_include_done_on_request() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("tasks.db");
    assert_eq!(run(&db, &["tasks", "lists"]), "1\tWork\n2\tPrivate\n");
    let added = run(&db, &["tasks", "add", "wOrK", "  CLI task  "]);
    assert_eq!(added, "1\t \tCLI task\n");
    assert_eq!(run(&db, &["tasks", "list", "1"]), added);
    assert_eq!(run(&db, &["tasks", "done", "1"]), "Updated 1\n");
    assert!(run(&db, &["tasks", "list", "Work"]).is_empty());
    assert_eq!(run(&db, &["tasks", "list", "WORK", "--done"]), "1\tx\tCLI task\n");
    assert!(run(&db, &["tasks", "list", "Private", "--done"]).is_empty());
    assert!(Store::open(&db).unwrap().accounts().unwrap().is_empty());
}

#[test]
fn tasks_from_mail_use_default_or_selected_list_without_network_or_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("mail-tasks.db");
    let store = Store::open(&db).unwrap();
    let (i, o) = presets::servers("cli@gmail.com").unwrap();
    let account = Account::new("cli@gmail.com".into(), i, o).unwrap();
    store.save_account(&account).unwrap();
    let folder = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
    let id = store.save_message(&Message {
        summary: MessageSummary { account: account.id.clone(), folder, uid: 1, message_id: "<cli@test>".into(), subject: "Reply tomorrow".into(), ..Default::default() },
        ..Default::default()
    }).unwrap().to_string();
    assert_eq!(run(&db, &["tasks", "from-mail", &id]), "1\t \tReply tomorrow\n");
    assert_eq!(run(&db, &["tasks", "from-mail", &id, "--list", "pRiVaTe"]), "2\t \tReply tomorrow\n");
    assert_eq!(run(&db, &["tasks", "from-mail", &id, "--list", "2"]), "3\t \tReply tomorrow\n");
    store.set_default_task_list(2).unwrap();
    assert_eq!(run(&db, &["tasks", "from-mail", &id]), "4\t \tReply tomorrow\n");
    let tasks = store.tasks_for_mail(&account.id, "<cli@test>").unwrap();
    assert_eq!(tasks.len(), 4);
    assert_eq!(tasks.iter().map(|t| t.list).collect::<Vec<_>>(), [1, 2, 2, 2]);
    assert!(tasks.iter().all(|t| t.mail_subject.as_deref() == Some("Reply tomorrow")));
}

#[test]
fn tasks_reject_missing_ambiguous_and_invalid_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("errors.db");
    let store = Store::open(&db).unwrap();
    store.add_task_list("WORK").unwrap();
    for (args, error) in [
        (vec!["tasks", "list", "work"], "ambiguous"),
        (vec!["tasks", "list", "missing"], "missing"),
        (vec!["tasks", "add", "1", "  "], "title cannot be empty"),
        (vec!["tasks", "done", "999"], "no longer here"),
        (vec!["tasks", "from-mail", "999"], "no longer here"),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lagom-cli"))
            .arg("--database").arg(&db).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(error), "{}", String::from_utf8_lossy(&output.stderr));
    }
    // Numeric ids remain usable when names are ambiguous or look like another id.
    store.add_task_list("1").unwrap();
    assert!(run(&db, &["tasks", "add", "1", "By id"]).contains("By id"));
    assert_eq!(store.tasks(1, false).unwrap().len(), 1);
}

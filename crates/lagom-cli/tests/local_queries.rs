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
            String::from_utf8_lossy(&output.stderr).contains("sign-in provider is not configured")
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

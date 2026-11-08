use lagom_core::{
    account::{Account, Security, ServerConfig},
    imap_sync::ImapSync,
    store::{FolderRole, Message, MessageSummary, Store},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Copy)]
enum Case { Full, Empty, NoUidplus, ChangedValidity, SearchFailed, StoreFailed, ExpungeFailed, WrongFolder, WrongAccount }

async fn empty_trash(case: Case) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "fixture".into() };
    let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        socket.get_mut().write_all(b"* OK fixture\r\n").await.unwrap();
        let capabilities = if matches!(case, Case::NoUidplus) { "IMAP4rev1 SPECIAL-USE" } else { "IMAP4rev1 SPECIAL-USE UIDPLUS" };
        let mut script = vec![
            ("LOGIN \"fixture\" \"fixture\"".to_string(), String::new(), true),
            ("CAPABILITY".into(), format!("* CAPABILITY {capabilities}\r\n"), true),
            ("LIST \"\" \"*\" RETURN (SPECIAL-USE)".into(), "* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Trash) \"/\" \"[Provider]/Bin\"\r\n* LIST (\\Sent) \"/\" \"Outgoing\"\r\n* LIST (\\Drafts) \"/\" \"Unsent\"\r\n".into(), true),
        ];
        if !matches!(case, Case::NoUidplus | Case::WrongFolder | Case::WrongAccount) {
            let validity = if matches!(case, Case::ChangedValidity) { 8 } else { 7 };
            let selected = format!("* 201 EXISTS\r\n* OK [UIDVALIDITY {validity}] valid\r\n* OK [UIDNEXT 202] next\r\n");
            script.push(("SELECT \"[Provider]/Bin\"".into(), selected, true));
            if !matches!(case, Case::ChangedValidity) {
                let uids: Vec<_> = match case { Case::Empty => Vec::new(), Case::Full => (1..=201).collect(), _ => vec![1] };
                let search = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
                script.push(("UID SEARCH ALL".into(), format!("* SEARCH {search}\r\n"), !matches!(case, Case::SearchFailed)));
                for chunk in uids.chunks(100) {
                    if matches!(case, Case::SearchFailed) { break; }
                    let set = chunk.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                    script.push((format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"), String::new(), !matches!(case, Case::StoreFailed)));
                    if matches!(case, Case::StoreFailed) { break; }
                    script.push((format!("UID EXPUNGE {set}"), String::new(), !matches!(case, Case::ExpungeFailed)));
                    if matches!(case, Case::ExpungeFailed) { break; }
                }
                if matches!(case, Case::Full | Case::Empty) {
                    script.push(("SELECT \"[Provider]/Bin\"".into(), "* 0 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 202] next\r\n".into(), true));
                    for _ in 0..2 { script.push(("UID SEARCH ALL".into(), "* SEARCH\r\n".into(), true)); }
                }
            }
        }
        for (command, data, ok) in script {
            let mut line = String::new();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
            let (tag, actual) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(actual, command);
            socket.get_mut().write_all(format!("{data}{tag} {} fixture\r\n", if ok { "OK" } else { "NO" }).as_bytes()).await.unwrap();
        }
        let mut line = String::new();
        assert_eq!(socket.read_line(&mut line).await.unwrap(), 0, "unexpected command: {line}");
    });
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path().join("mail.db")).unwrap();
    store.save_account(&account).unwrap();
    let mut imap = ImapSync::login(&account, "fixture").await.unwrap();
    let folders = imap.list_folders(&store).await.unwrap();
    let trash = folders.iter().find(|f| f.role == FolderRole::Trash).unwrap().id;
    let inbox = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap().id;
    assert_eq!(store.folder(trash).unwrap().name, "[Provider]/Bin");
    assert!(folders.iter().any(|f| f.role == FolderRole::Sent && f.name == "Outgoing"));
    assert!(folders.iter().any(|f| f.role == FolderRole::Drafts && f.name == "Unsent"));
    for folder in [trash, inbox] {
        store.reset_uidvalidity(folder, 7).unwrap();
        store.save_message(&Message { summary: MessageSummary {
            account: account.id.clone(), folder, uid: 1, subject: "Fixture".into(), ..Default::default()
        }, ..Default::default() }).unwrap();
    }
    let target = match case {
        Case::WrongFolder => inbox,
        Case::WrongAccount => {
            let mut other = account.clone();
            other.id = "other".into();
            other.email = "other@example.com".into();
            store.save_account(&other).unwrap();
            store.save_folder(&other.id, "Bin", None, FolderRole::Trash).unwrap()
        }
        _ => trash,
    };
    let result = imap.empty_trash(&mut store, target).await;
    assert_eq!(result.is_ok(), matches!(case, Case::Full | Case::Empty), "{result:?}");
    assert_eq!(store.known_uids(inbox).unwrap(), [1]);
    if matches!(case, Case::Full | Case::Empty | Case::ChangedValidity) {
        assert!(store.known_uids(trash).unwrap().is_empty());
    } else {
        assert_eq!(store.known_uids(trash).unwrap(), [1], "failure must not claim local deletion");
    }
    drop(imap);
    server.await.unwrap();
}

#[tokio::test]
async fn localized_trash_is_emptied_in_uid_batches_including_uncached_mail() { empty_trash(Case::Full).await; }
#[tokio::test]
async fn empty_trash_reconciles_stale_cache_without_store_or_expunge() { empty_trash(Case::Empty).await; }
#[tokio::test]
async fn empty_trash_requires_uidplus() { empty_trash(Case::NoUidplus).await; }
#[tokio::test]
async fn empty_trash_guards_uidvalidity() { empty_trash(Case::ChangedValidity).await; }
#[tokio::test]
async fn empty_trash_stops_after_store_failure() { empty_trash(Case::StoreFailed).await; }
#[tokio::test]
async fn empty_trash_stops_after_expunge_failure() { empty_trash(Case::ExpungeFailed).await; }
#[tokio::test]
async fn empty_trash_rejects_other_folders_and_accounts() {
    empty_trash(Case::WrongFolder).await;
    empty_trash(Case::WrongAccount).await;
}

#[tokio::test]
async fn empty_trash_stops_after_search_failure() { empty_trash(Case::SearchFailed).await; }

use lagom_core::{account::{Account, Security, ServerConfig}, imap_sync::ImapSync, outbox::Draft, smtp::Outgoing, store::Store};
use std::{collections::BTreeMap, sync::{Arc, Mutex}};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Default)]
struct Mailbox {
    messages: BTreeMap<u32, (Vec<u8>, bool)>,
    flags: BTreeMap<u32, String>,
    next: u32,
    validity: u32,
    appends: usize,
    deletions: Vec<u32>,
    refuse: bool,
    drop_append: bool,
    no_uidplus: bool,
    add_header: bool,
    reset_on_append: bool,
    pause_append: Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>,
}
impl Mailbox {
    fn insert(&mut self, raw: Vec<u8>) -> u32 {
        self.next += 1;
        self.messages.insert(self.next, (raw, false));
        self.next
    }
    fn visible(&self) -> Vec<&Vec<u8>> {
        self.messages.values().filter(|(_, deleted)| !deleted).map(|(raw, _)| raw).collect()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    account: Account,
    mailbox: Arc<Mutex<Mailbox>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture { fn drop(&mut self) { self.server.abort(); } }
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mailbox = Arc::new(Mutex::new(Mailbox { validity: 7, ..Default::default() }));
        let state = mailbox.clone();
        let server = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (read, mut write) = socket.into_split();
                let mut read = BufReader::new(read);
                write.write_all(b"* OK fixture\r\n").await.unwrap();
                loop {
                    let mut line = String::new();
                    if read.read_line(&mut line).await.unwrap() == 0 { break; }
                    let (tag, command) = line.trim_end().split_once(' ').unwrap();
                    if command.starts_with("APPEND ") {
                        assert!(command.starts_with("APPEND \"[Gmail]/Drafts\" (\\Draft) {"), "{command}");
                        if state.lock().unwrap().refuse {
                            write.write_all(format!("{tag} NO offline fixture\r\n").as_bytes()).await.unwrap();
                            continue;
                        }
                        let len: usize = command.rsplit_once('{').unwrap().1.trim_end_matches('}').parse().unwrap();
                        write.write_all(b"+ continue\r\n").await.unwrap();
                        let mut raw = vec![0; len];
                        read.read_exact(&mut raw).await.unwrap();
                        let mut crlf = [0; 2];
                        read.read_exact(&mut crlf).await.unwrap();
                        assert_eq!(&crlf, b"\r\n");
                        let (uid, validity, drop_reply, pause) = {
                            let mut state = state.lock().unwrap();
                            state.appends += 1;
                            if std::mem::take(&mut state.reset_on_append) { state.validity += 1; }
                            if state.add_header { raw.splice(..0, b"Received: from fixture\r\n".iter().copied()); }
                            let uid = state.insert(raw);
                            (uid, state.validity, std::mem::take(&mut state.drop_append), state.pause_append.take())
                        };
                        if let Some((ready, resume)) = pause { ready.send(()).unwrap(); resume.await.unwrap(); }
                        if drop_reply { break; }
                        write.write_all(format!("{tag} OK [APPENDUID {validity} {uid}] appended\r\n").as_bytes()).await.unwrap();
                        continue;
                    }
                    let response = {
                        let mut state = state.lock().unwrap();
                        if command.starts_with("LOGIN ") { String::new() }
                        else if command == "CAPABILITY" { format!("* CAPABILITY IMAP4rev1 SPECIAL-USE{}\r\n", if state.no_uidplus { "" } else { " UIDPLUS" }) }
                        else if command.starts_with("LIST ") { "* LIST (\\Drafts) \"/\" \"[Gmail]/Drafts\"\r\n".into() }
                        else if command == "SELECT \"[Gmail]/Drafts\"" {
                            format!("* {} EXISTS\r\n* OK [UIDVALIDITY {}] valid\r\n* OK [UIDNEXT {}] next\r\n", state.messages.len(), state.validity, state.next + 1)
                        } else if command.starts_with("UID SEARCH ") {
                            let token = command.split_once("HEADER Message-ID \"").map(|(_, tail)| tail.trim_end_matches('"'));
                            let ids = state.messages.iter().filter(|(_, (raw, deleted))| !deleted && token.is_none_or(|token| String::from_utf8_lossy(raw).contains(token)))
                                .map(|(uid, _)| uid.to_string()).collect::<Vec<_>>().join(" ");
                            format!("* SEARCH{}\r\n", if ids.is_empty() { String::new() } else { format!(" {ids}") })
                        } else if command.starts_with("UID FETCH ") {
                            assert!(command.ends_with("(UID FLAGS BODY.PEEK[])"));
                            let uid = command.split_whitespace().nth(2).unwrap().parse::<u32>().unwrap();
                            state.messages.get(&uid).map(|(raw, deleted)| format!("* 1 FETCH (UID {uid} FLAGS ({}{}) BODY[] {{{}}}\r\n{})\r\n", state.flags.get(&uid).map_or("\\Draft", String::as_str), if *deleted { " \\Deleted" } else { "" }, raw.len(), String::from_utf8_lossy(raw))).unwrap_or_default()
                        } else if command.starts_with("UID STORE ") {
                            assert!(command.ends_with("+FLAGS.SILENT (\\Deleted)"));
                            let uid = command.split_whitespace().nth(2).unwrap().parse::<u32>().unwrap();
                            state.deletions.push(uid);
                            if let Some((_, deleted)) = state.messages.get_mut(&uid) { *deleted = true; }
                            String::new()
                        } else if command.starts_with("UID EXPUNGE ") {
                            let uid = command.split_whitespace().nth(2).unwrap().parse::<u32>().unwrap();
                            if state.messages.get(&uid).is_some_and(|(_, deleted)| *deleted) { state.messages.remove(&uid); }
                            String::new()
                        } else if command == "LOGOUT" { "* BYE bye\r\n".into() }
                        else { panic!("unexpected fixture command: {command}") }
                    };
                    write.write_all(format!("{response}{tag} OK done\r\n").as_bytes()).await.unwrap();
                    if command == "LOGOUT" { break; }
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("mail.db")).unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("sender@example.com".into(), config.clone(), config).unwrap();
        store.save_account(&account).unwrap();
        Self { _dir: dir, store, account, mailbox, server }
    }
    async fn connect(&self) -> ImapSync { ImapSync::login(&self.account, "fixture").await.unwrap() }
    fn draft(&self) -> Draft {
        Draft { id: "<draft@example.com>".into(), account: self.account.id.clone(), to: "recipient@example.com".into(),
            subject: "Saved subject".into(), text: "åäö **keep this text**".into(), ..Default::default() }
    }
    async fn read(&mut self, imap: &mut ImapSync) {
        let folder = imap.list_folders(&self.store).await.unwrap().remove(0);
        imap.sync_folder(&mut self.store, folder.id, 500).await.unwrap();
    }
}

#[tokio::test]
async fn save_update_throttle_close_send_undo_and_discard() {
    let mut f = Fixture::new().await;
    let mut imap = f.connect().await;
    let mut draft = f.draft();
    draft.to = "unfinished address".into();
    f.store.save_draft(&draft).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().appends, 1);
    draft.text = "latest text".into();
    f.store.save_draft(&draft).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 9999).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().appends, 1);
    imap.sync_draft(&mut f.store, &draft.id, 10000).await.unwrap();
    {
        let state = f.mailbox.lock().unwrap();
        assert_eq!(state.appends, 2);
        assert_eq!(state.deletions, [1]);
        assert_eq!(state.visible().len(), 1);
        assert!(String::from_utf8_lossy(state.visible()[0]).contains("latest text"));
    }
    draft.text = "closed edits".into();
    f.store.save_draft(&draft).unwrap();
    f.store.flush_draft(&draft.id).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 10001).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().appends, 3);
    let mail = Outgoing { to: vec!["recipient@example.com".into()], text: draft.text.clone(), message_id: Some(draft.id.clone()), ..Default::default() };
    f.store.enqueue(&f.account, &mail, 0).unwrap();
    f.store.flush_draft(&draft.id).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 10002).await.unwrap();
    assert!(f.mailbox.lock().unwrap().visible().is_empty());
    assert!(f.store.undo_send(&draft.id).unwrap());
    imap.sync_draft(&mut f.store, &draft.id, 10003).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().visible().len(), 1);
    f.store.delete_draft(&draft.id).unwrap();
    f.store.flush_draft(&draft.id).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 10004).await.unwrap();
    assert!(f.mailbox.lock().unwrap().visible().is_empty());
    assert!(f.store.due_drafts(&f.account.id, i64::MAX).unwrap().is_empty());
    imap.sync_draft(&mut f.store, &draft.id, 10005).await.unwrap();
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn foreign_draft_is_read_only_until_taken_over_and_keeps_attachments() {
    let mut f = Fixture::new().await;
    let raw = b"To: Recipient <recipient@example.com>\r\nCc: copy@example.com\r\nBcc: private@example.com\r\nSubject: Foreign\r\nIn-Reply-To: <parent@example.com>\r\nReferences: <root@example.com>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nForeign text\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=data.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--x--\r\n";
    f.mailbox.lock().unwrap().insert(raw.to_vec());
    let mut imap = f.connect().await;
    f.read(&mut imap).await;
    let draft = f.store.drafts().unwrap().remove(0);
    assert_eq!(draft.subject, "Foreign");
    assert_eq!(draft.bcc, "private@example.com");
    assert!(draft.text.contains("Foreign text"));
    assert_eq!(draft.attachments[0].data, [0, 1, 2]);
    assert_eq!(draft.attachments[0].filename, "data.bin");
    let summaries = f.store.draft_summaries().unwrap();
    assert_eq!(summaries[0].id, draft.id);
    assert_eq!(summaries[0].subject, "Foreign");
    assert!(summaries[0].snippet.contains("Foreign text"));
    assert!(f.store.owned_drafts().unwrap().is_empty());
    assert!(f.store.due_drafts(&f.account.id, 0).unwrap().is_empty());
    f.read(&mut imap).await;
    assert_eq!(f.store.drafts().unwrap().len(), 1);
    let mut draft = f.store.take_draft(&draft.id).unwrap();
    draft.text = "taken over".into();
    f.store.save_draft(&draft).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().deletions, [1]);
    assert_eq!(f.mailbox.lock().unwrap().visible().len(), 1);
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn offline_retry_survives_restart_without_losing_newer_text() {
    let mut f = Fixture::new().await;
    let mut draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    f.mailbox.lock().unwrap().refuse = true;
    let mut imap = f.connect().await;
    let error = imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap_err();
    f.store.draft_sync_failed(&draft.id, &error, 0).unwrap();
    drop(imap);
    draft.text = "edited while offline".into();
    f.store.save_draft(&draft).unwrap();
    f.store.flush_draft(&draft.id).unwrap();
    assert!(f.store.due_drafts(&f.account.id, 59999).unwrap().is_empty());
    f.store = Store::open(f.store.path()).unwrap();
    assert_eq!(f.store.draft(&draft.id).unwrap(), draft);
    f.mailbox.lock().unwrap().refuse = false;
    let mut imap = f.connect().await;
    imap.sync_draft(&mut f.store, &draft.id, 60000).await.unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 70000).await.unwrap();
    assert_eq!(f.store.draft(&draft.id).unwrap().text, "edited while offline");
    let state = f.mailbox.lock().unwrap();
    assert_eq!(state.visible().len(), 1);
    assert!(String::from_utf8_lossy(state.visible()[0]).contains("edited while offline"));
    drop(state);
    assert!(f.store.draft_sync_error(&draft.id).unwrap().is_none());
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn lost_append_reply_is_reconciled_without_duplicate_after_restart() {
    let mut f = Fixture::new().await;
    let draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    f.mailbox.lock().unwrap().drop_append = true;
    f.mailbox.lock().unwrap().add_header = true;
    let mut imap = f.connect().await;
    assert!(imap.sync_draft(&mut f.store, &draft.id, 0).await.is_err());
    drop(imap);
    f.store = Store::open(f.store.path()).unwrap();
    let mut imap = f.connect().await;
    f.read(&mut imap).await;
    assert_eq!(f.store.drafts().unwrap().len(), 1);
    imap.sync_draft(&mut f.store, &draft.id, 60000).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().appends, 1);
    assert_eq!(f.mailbox.lock().unwrap().visible().len(), 1);
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn lost_append_reply_keeps_another_clients_changed_reply_address() {
    let mut f = Fixture::new().await;
    let draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    f.mailbox.lock().unwrap().drop_append = true;
    let mut imap = f.connect().await;
    assert!(imap.sync_draft(&mut f.store, &draft.id, 0).await.is_err());
    drop(imap);
    f.mailbox.lock().unwrap().messages.get_mut(&1).unwrap().0.splice(..0, b"Reply-To: changed@example.com\r\n".iter().copied());
    let mut imap = f.connect().await;
    f.read(&mut imap).await;
    assert_eq!(f.store.drafts().unwrap().len(), 2);
    imap.sync_draft(&mut f.store, &draft.id, 60000).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().visible().len(), 2);
    assert!(f.mailbox.lock().unwrap().deletions.is_empty());
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn conflict_and_uidvalidity_reset_keep_both_copies() {
    for reset in [false, true] {
        let mut f = Fixture::new().await;
        let mut imap = f.connect().await;
        let mut draft = f.draft();
        f.store.save_draft(&draft).unwrap();
        imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
        {
            let mut state = f.mailbox.lock().unwrap();
            if reset { state.validity = 8; }
            state.messages.get_mut(&1).unwrap().0 = b"Subject: Other client\r\n\r\nOther client's edits".to_vec();
        }
        draft.text = "local edits".into();
        f.store.save_draft(&draft).unwrap();
        imap.sync_draft(&mut f.store, &draft.id, 10000).await.unwrap();
        assert!(f.mailbox.lock().unwrap().deletions.is_empty());
        assert_eq!(f.mailbox.lock().unwrap().visible().len(), 2);
        f.read(&mut imap).await;
        assert_eq!(f.store.drafts().unwrap().len(), 2);
        assert!(f.store.drafts().unwrap().iter().any(|d| d.text == "Other client's edits"));
        assert_eq!(f.store.draft(&draft.id).unwrap().text, "local edits");
        imap.logout().await.unwrap();
    }
}

#[tokio::test]
async fn discard_after_uncertain_append_removes_only_that_copy_without_uidplus() {
    let mut f = Fixture::new().await;
    {
        let mut state = f.mailbox.lock().unwrap();
        state.no_uidplus = true;
        state.drop_append = true;
        let uid = state.insert(b"Subject: Unrelated\r\n\r\nkeep".to_vec());
        state.messages.get_mut(&uid).unwrap().1 = true;
    }
    let draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    let mut imap = f.connect().await;
    assert!(imap.sync_draft(&mut f.store, &draft.id, 0).await.is_err());
    drop(imap);
    f.store.delete_draft(&draft.id).unwrap();
    let mut imap = f.connect().await;
    imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
    assert!(f.mailbox.lock().unwrap().visible().is_empty());
    assert!(f.mailbox.lock().unwrap().messages.contains_key(&1));
    assert_eq!(f.mailbox.lock().unwrap().deletions, [2]);
    f.read(&mut imap).await;
    assert!(f.store.drafts().unwrap().is_empty());
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn untouched_foreign_deletion_does_not_resurrect_it() {
    let mut f = Fixture::new().await;
    f.mailbox.lock().unwrap().insert(b"Subject: Foreign\r\n\r\nkeep".to_vec());
    f.mailbox.lock().unwrap().insert(b"Subject: Another\r\n\r\nkeep too".to_vec());
    let mut imap = f.connect().await;
    let folder = imap.list_folders(&f.store).await.unwrap().remove(0);
    imap.sync_folder(&mut f.store, folder.id, 1).await.unwrap();
    assert_eq!(f.store.drafts().unwrap().len(), 1);
    imap.sync_folder(&mut f.store, folder.id, 1).await.unwrap();
    assert_eq!(f.store.drafts().unwrap().len(), 2);
    f.mailbox.lock().unwrap().messages.clear();
    f.read(&mut imap).await;
    assert!(f.store.drafts().unwrap().is_empty());
    assert!(f.store.due_drafts(&f.account.id, i64::MAX).unwrap().is_empty());
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn retry_backoff_caps_and_account_removal_cascades() {
    let f = Fixture::new().await;
    let draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    let mut now = 0;
    for delay in [60000, 120000, 240000, 480000, 960000, 1920000, 3600000, 3600000] {
        f.store.draft_sync_failed(&draft.id, &lagom_core::Error::Network("offline"), now).unwrap();
        f.store.flush_draft(&draft.id).unwrap();
        assert!(f.store.due_drafts(&f.account.id, now + delay - 1).unwrap().is_empty());
        now += delay;
        assert_eq!(f.store.due_drafts(&f.account.id, now).unwrap(), [draft.id.clone()]);
    }
    f.store.delete_account(&f.account.id).unwrap();
    assert!(f.store.drafts().unwrap().is_empty());
    assert!(f.store.due_drafts(&f.account.id, i64::MAX).unwrap().is_empty());
}

#[tokio::test]
async fn validity_change_during_append_retries_without_duplicate() {
    let mut f = Fixture::new().await;
    let draft = f.draft();
    f.store.save_draft(&draft).unwrap();
    f.mailbox.lock().unwrap().reset_on_append = true;
    let mut imap = f.connect().await;
    assert!(imap.sync_draft(&mut f.store, &draft.id, 0).await.is_err());
    drop(imap);
    let mut imap = f.connect().await;
    imap.sync_draft(&mut f.store, &draft.id, 60000).await.unwrap();
    assert_eq!(f.mailbox.lock().unwrap().appends, 1);
    assert_eq!(f.store.draft(&draft.id).unwrap().text, draft.text);
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn edits_send_and_discard_during_append_cannot_be_overwritten_or_resurrected() {
    for action in ["edit", "send", "discard"] {
        let mut f = Fixture::new().await;
        let mut draft = f.draft();
        f.store.save_draft(&draft).unwrap();
        let writer = Store::open(f.store.path()).unwrap();
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let (resume, paused) = tokio::sync::oneshot::channel();
        f.mailbox.lock().unwrap().pause_append = Some((ready, paused));
        let mut imap = f.connect().await;
        let id = draft.id.clone();
        let (result, ()) = tokio::join!(imap.sync_draft(&mut f.store, &id, 0), async {
            waiting.await.unwrap();
            draft.text = "newer local text".into();
            writer.save_draft(&draft).unwrap();
            match action {
                "send" => { writer.enqueue(&f.account, &Outgoing {
                    to: vec![draft.to.clone()], text: draft.text.clone(), message_id: Some(id.clone()), ..Default::default()
                }, 0).unwrap(); },
                "discard" => writer.delete_draft(&id).unwrap(),
                _ => {},
            }
            resume.send(()).unwrap();
        });
        result.unwrap();
        if action == "edit" {
            assert_eq!(f.store.draft(&id).unwrap().text, "newer local text");
            imap.sync_draft(&mut f.store, &id, 10000).await.unwrap();
            let state = f.mailbox.lock().unwrap();
            assert_eq!(state.visible().len(), 1);
            assert!(String::from_utf8_lossy(state.visible()[0]).contains("newer local text"));
        } else {
            assert!(f.store.drafts().unwrap().is_empty());
            assert!(f.mailbox.lock().unwrap().visible().is_empty());
            if action == "send" { assert_eq!(f.store.pending(&id).unwrap().mail.text, "newer local text"); }
        }
        imap.logout().await.unwrap();
    }
}

#[tokio::test]
async fn imported_inline_media_survives_edit_and_replacement() {
    let mut f = Fixture::new().await;
    let raw = b"Subject: Inline photo\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nPhoto below\r\n--x\r\nContent-Type: image/png; name=photo.png\r\nContent-Disposition: inline; filename=photo.png\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC/w==\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=data.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAwQF\r\n--x--\r\n";
    f.mailbox.lock().unwrap().insert(raw.to_vec());
    let mut imap = f.connect().await;
    f.read(&mut imap).await;
    let mut draft = f.store.drafts().unwrap().remove(0);
    assert_eq!(draft.attachments.len(), 2);
    let photo = draft.attachments.iter().find(|a| a.filename == "photo.png").unwrap();
    assert_eq!(photo.mime_type, "image/png");
    assert_eq!(photo.data, [0, 1, 2, 255]);
    draft.text = "Edited text".into();
    f.store.save_draft(&draft).unwrap();
    imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
    {
        let state = f.mailbox.lock().unwrap();
        assert_eq!(state.deletions, [1]);
        assert_eq!(state.visible().len(), 1);
        let parsed = mail_parser::MessageParser::default().parse(state.visible()[0]).unwrap();
        let attachments: Vec<_> = parsed.attachments().collect();
        assert_eq!(attachments.len(), 2);
        let photo = attachments.iter().find(|a| a.attachment_name() == Some("photo.png")).unwrap();
        assert_eq!(photo.decoded().as_ref(), &[0, 1, 2, 255]);
        assert_eq!(attachments.iter().find(|a| a.attachment_name() == Some("data.bin")).unwrap().decoded().as_ref(), &[3, 4, 5]);
    }
    imap.logout().await.unwrap();
}

#[tokio::test]
async fn foreign_drafts_without_draft_flag_are_replaced_and_discarded() {
    for update in [false, true] {
        let mut f = Fixture::new().await;
        {
            let mut state = f.mailbox.lock().unwrap();
            let uid = state.insert(b"Subject: Foreign\r\n\r\nOriginal text".to_vec());
            state.flags.insert(uid, "\\Seen".into());
        }
        let mut imap = f.connect().await;
        f.read(&mut imap).await;
        let mut draft = f.store.drafts().unwrap().remove(0);
        if update {
            draft.text = "Edited text".into();
            f.store.save_draft(&draft).unwrap();
        } else {
            f.store.delete_draft(&draft.id).unwrap();
        }
        imap.sync_draft(&mut f.store, &draft.id, 0).await.unwrap();
        assert_eq!(f.mailbox.lock().unwrap().deletions, [1]);
        assert_eq!(f.mailbox.lock().unwrap().visible().len(), usize::from(update));
        f.read(&mut imap).await;
        assert_eq!(f.store.drafts().unwrap().len(), usize::from(update));
        if update { assert_eq!(f.store.draft(&draft.id).unwrap().text, "Edited text"); }
        imap.logout().await.unwrap();
    }
}

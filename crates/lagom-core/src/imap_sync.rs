//! async-imap provides CONDSTORE and cancellable IDLE without a dedicated polling
//! thread; Tokio/rustls also shares the desktop's existing runtime dependencies.
use crate::{
    account::{Account, Security, ServerConfig},
    mime, presets,
    store::{Folder, FolderRole, Store},
    Error, Result,
};
use async_imap::{
    imap_proto::{BodyStructure, MessageSection, SectionPath},
    types::{Capabilities, Fetch, Flag, NameAttribute},
    Client, Session,
};
use futures_util::TryStreamExt;
use std::{collections::HashSet, fmt::Debug, future::Future, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{self, pki_types::ServerName},
    TlsConnector,
};

trait Io: AsyncRead + AsyncWrite + Unpin + Send + Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Debug> Io for T {}
type Wire = Box<dyn Io>;
pub const DEFAULT_LIMIT: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(60);
async fn net<T, E>(f: impl Future<Output = std::result::Result<T, E>>) -> Result<T> {
    net_as("IMAP operation failed", f).await
}
/// Like `net`, with a message that tells the user which step failed: host, TLS or login.
async fn net_as<T, E>(
    what: &'static str,
    f: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .map_err(|_| Error::Network("IMAP operation timed out"))?
        .map_err(|_| Error::Network(what))
}
async fn tls(stream: TcpStream, host: &str) -> Result<Wire> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Error::Network("TLS configuration failed"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| Error::Invalid("invalid TLS server name"))?;
    Ok(Box::new(
        net_as(
            "IMAP TLS handshake failed",
            TlsConnector::from(Arc::new(config)).connect(name, stream),
        )
        .await?,
    ))
}
async fn connect(config: &ServerConfig, password: &str) -> Result<Session<Wire>> {
    config.validate()?;
    let socket = net_as(
        "could not reach the IMAP server",
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await?;
    // Validate the resolved peer too: a poisoned localhost mapping must not expose LOGIN.
    if config.security == Security::Plain && !socket.peer_addr()?.ip().is_loopback() {
        return Err(Error::Invalid("unencrypted peer must be loopback"));
    }
    let client: Client<Wire> = match config.security {
        Security::Tls => {
            let mut client = Client::new(tls(socket, &config.host).await?);
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            client
        }
        Security::StartTls => {
            let mut client = Client::new(socket);
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            net(client.run_command_and_check_ok("STARTTLS", None)).await?;
            Client::new(tls(client.into_inner(), &config.host).await?)
        }
        Security::Plain => {
            let mut client = Client::new(Box::new(socket) as Wire);
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            client
        }
    };
    net_as(
        "IMAP login rejected: check the address and app password",
        client.login(&config.username, password),
    )
    .await
}
pub struct ImapSync {
    account: Account,
    session: Session<Wire>,
    capabilities: Capabilities,
}
impl ImapSync {
    pub async fn login(account: &Account, password: &str) -> Result<Self> {
        account.validate()?;
        let mut session = connect(&account.imap, password).await?;
        let capabilities = net(session.capabilities()).await?;
        Ok(Self {
            account: account.clone(),
            session,
            capabilities,
        })
    }
    pub async fn logout(mut self) -> Result<()> {
        net(self.session.logout()).await
    }
    pub async fn list_folders(&mut self, store: &Store) -> Result<Vec<Folder>> {
        let mut rows = Vec::new();
        if self.capabilities.has_str("SPECIAL-USE") {
            let tag = net(self
                .session
                .run_command("LIST \"\" \"*\" RETURN (SPECIAL-USE)"))
            .await?;
            loop {
                let response = net(self.session.read_response())
                    .await?
                    .ok_or(Error::Network("IMAP folder listing interrupted"))?;
                use async_imap::imap_proto::{MailboxDatum, Response, Status};
                match response.parsed() {
                    Response::MailboxData(MailboxDatum::List(data)) => {
                        if !data.name_attributes.contains(&NameAttribute::NoSelect) {
                            rows.push((
                                data.name.to_string(),
                                data.delimiter.as_deref().map(str::to_owned),
                                role(&data.name, &data.name_attributes),
                            ));
                        }
                    }
                    Response::Done {
                        tag: done, status, ..
                    } if *done == tag => {
                        if *status != Status::Ok {
                            return Err(Error::Network("IMAP folder listing failed"));
                        }
                        break;
                    }
                    _ => {}
                }
            }
        } else {
            let names = net(async {
                self.session
                    .list(Some(""), Some("*"))
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
            for name in &names {
                if name.attributes().contains(&NameAttribute::NoSelect) {
                    continue;
                }
                rows.push((
                    name.name().to_owned(),
                    name.delimiter().map(str::to_owned),
                    role(name.name(), name.attributes()),
                ));
            }
        }
        let mut ids = Vec::new();
        for (name, delimiter, role) in rows {
            ids.push(store.save_folder(&self.account.id, &name, delimiter.as_deref(), role)?);
        }
        store.retain_folders(&self.account.id, &ids)?;
        ids.into_iter().map(|id| store.folder(id)).collect()
    }
    pub async fn sync(&mut self, store: &mut Store, limit: usize) -> Result<()> {
        if limit == 0 {
            return Err(Error::Invalid("sync limit must be positive"));
        }
        for folder in self.list_folders(store).await? {
            self.sync_folder(store, folder.id, limit).await?;
        }
        Ok(())
    }
    async fn select(
        &mut self,
        store: &mut Store,
        id: i64,
    ) -> Result<(Folder, async_imap::types::Mailbox)> {
        let folder = store.folder(id)?;
        if folder.account != self.account.id {
            return Err(Error::Invalid("folder belongs to another account"));
        }
        let mailbox = if self.capabilities.has_str("CONDSTORE") {
            net(self.session.select_condstore(&folder.name)).await?
        } else {
            net(self.session.select(&folder.name)).await?
        };
        let validity = mailbox
            .uid_validity
            .filter(|v| *v > 0)
            .ok_or(Error::Network("IMAP UIDVALIDITY missing"))?;
        store.reset_uidvalidity(id, validity)?;
        Ok((store.folder(id)?, mailbox))
    }
    pub async fn sync_folder(&mut self, store: &mut Store, id: i64, limit: usize) -> Result<()> {
        if limit == 0 {
            return Err(Error::Invalid("sync limit must be positive"));
        }
        let (folder, mailbox) = self.select(store, id).await?;
        let present = net(self.session.uid_search("ALL")).await?;
        let mut sorted: Vec<_> = present.iter().copied().filter(|u| *u > 0).collect();
        sorted.sort_unstable();
        let known: HashSet<_> = store.known_uids(id)?.into_iter().collect();
        let last = folder.last_uid;
        let initial = last == 0;
        let candidates: Vec<_> = if initial {
            sorted.iter().rev().take(limit).copied().collect()
        } else {
            sorted.iter().filter(|u| **u > last).copied().collect()
        };
        for &uid in &candidates {
            if !known.contains(&uid) {
                self.fetch_message(store, &folder, uid).await?;
            }
        }
        let query =
            if let (Some(previous), Some(_)) = (folder.highest_modseq, mailbox.highest_modseq) {
                format!("(UID FLAGS MODSEQ) (CHANGEDSINCE {previous})")
            } else {
                "(UID FLAGS)".into()
            };
        let sets = if mailbox.highest_modseq.is_some() && folder.highest_modseq.is_some() {
            vec!["1:*".to_owned()]
        } else {
            sorted
                .iter()
                .rev()
                .take(limit)
                .copied()
                .collect::<Vec<_>>()
                .chunks(100)
                .map(uid_set)
                .collect()
        };
        for set in sets {
            let updates = net(async {
                self.session
                    .uid_fetch(set, &query)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
            for fetch in updates {
                if let Some(uid) = fetch.uid {
                    let (seen, flagged) = flags(&fetch);
                    store.set_flags(id, uid, seen, flagged)?;
                }
            }
        }
        // SEARCH again catches expunges that occurred while bodies were fetched.
        let present = net(self.session.uid_search("ALL")).await?;
        let saved: HashSet<_> = store.known_uids(id)?.into_iter().collect();
        if candidates
            .iter()
            .any(|uid| present.contains(uid) && !saved.contains(uid))
        {
            return Err(Error::Network(
                "server omitted a message; sync checkpoint retained",
            ));
        }
        store.finish_sync(
            id,
            &present,
            sorted.last().copied().unwrap_or(last),
            mailbox.uid_next,
            mailbox.highest_modseq,
        )?;
        Ok(())
    }
    async fn fetch_message(&mut self, store: &Store, folder: &Folder, uid: u32) -> Result<()> {
        let metadata=net(async {self.session.uid_fetch(uid.to_string(),"(UID ENVELOPE FLAGS BODYSTRUCTURE RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER]<0.262144>)").await?.try_collect::<Vec<_>>().await}).await?;
        let Some(fetch) = metadata.iter().find(|f| f.uid == Some(uid)) else {
            return Ok(());
        };
        let header = fetch
            .header()
            .ok_or(Error::Network("IMAP message header missing"))?;
        if header.len() >= 262144 {
            return Err(Error::Invalid("message header exceeds 256 KiB"));
        }
        let mut message;
        let body = fetch
            .bodystructure()
            .ok_or(Error::Network("IMAP BODYSTRUCTURE missing"))?;
        let mut parts = Vec::new();
        let mut attachments = false;
        text_parts(body, &[], 0, &mut parts, &mut attachments)?;
        // shortcut: first preferred inline body only, combine multiple inline parts when needed.
        parts.sort_by_key(|p| p.1);
        if let Some((path, _, size)) = parts.first() {
            if *size as usize > mime::MAX_MIME_BYTES {
                return Err(Error::Invalid("MIME text part exceeds size limit"));
            }
            let section = path
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".");
            // GreenMail emits malformed partial text responses; check octets before full part fetch.
            let query = if path.is_empty() {
                "(UID BODY.PEEK[TEXT])".to_owned()
            } else {
                format!("(UID BODY.PEEK[{section}.MIME] BODY.PEEK[{section}])")
            };
            let bodies = net(async {
                self.session
                    .uid_fetch(uid.to_string(), query)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
            if let Some(body) = bodies.iter().find(|f| f.uid == Some(uid)) {
                let headers = if path.is_empty() {
                    header
                } else {
                    body.section(&SectionPath::Part(path.clone(), Some(MessageSection::Mime)))
                        .ok_or(Error::Network("MIME part headers missing"))?
                };
                let text = if path.is_empty() {
                    body.text()
                } else {
                    body.section(&SectionPath::Part(path.clone(), None))
                }
                .ok_or(Error::Network("MIME text part missing"))?;
                if headers.len() >= 65536 || headers.len() + text.len() > mime::MAX_MIME_BYTES {
                    return Err(Error::Invalid("MIME text part exceeds size limit"));
                }
                let mut raw = headers.to_vec();
                if !raw.ends_with(b"\r\n") {
                    raw.extend_from_slice(b"\r\n");
                }
                if !raw.ends_with(b"\r\n\r\n") {
                    raw.extend_from_slice(b"\r\n");
                }
                raw.extend_from_slice(text);
                let parsed = mime::parse(&raw)?;
                if path.is_empty() {
                    message = parsed;
                } else {
                    message = mime::parse(header)?;
                    message.body_text = parsed.body_text;
                    message.body_html = parsed.body_html;
                    message.summary.snippet = parsed.summary.snippet;
                }
            } else {
                return Ok(());
            }
        } else {
            message = mime::parse(header)?;
        }
        let (seen, flagged) = flags(fetch);
        let m = &mut message.summary;
        m.account = self.account.id.clone();
        m.folder = folder.id;
        m.uid = uid;
        if m.thread_key.is_empty() {
            m.thread_key = format!(
                "{}:{}:{uid}",
                folder.id,
                folder.uidvalidity.unwrap_or_default()
            );
        }
        m.seen = seen;
        m.flagged = flagged;
        m.has_attachments = attachments;
        m.size = fetch.size.unwrap_or_default();
        if m.date == 0 {
            m.date = fetch
                .internal_date()
                .map(|d| d.timestamp())
                .unwrap_or_default();
        }
        store.save_message(&message)?;
        Ok(())
    }
    pub async fn mark_seen(&mut self, store: &mut Store, id: i64, seen: bool) -> Result<()> {
        let message = store.message(id)?.summary;
        let before = store.folder(message.folder)?.uidvalidity;
        let (folder, _) = self.select(store, message.folder).await?;
        if folder.uidvalidity != before {
            return Err(Error::Invalid("mailbox changed; resync before operation"));
        }
        let command = if seen {
            "+FLAGS.SILENT (\\Seen)"
        } else {
            "-FLAGS.SILENT (\\Seen)"
        };
        net(async {
            self.session
                .uid_store(message.uid.to_string(), command)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?;
        store.set_flags(message.folder, message.uid, seen, message.flagged)
    }
    pub async fn create_folder(&mut self, store: &Store, name: &str) -> Result<i64> {
        validate_folder(name)?;
        net(self.session.create(name)).await?;
        self.list_folders(store).await?;
        store
            .folders(&self.account.id, 0)?
            .into_iter()
            .find(|f| f.name == name)
            .map(|f| f.id)
            .ok_or(Error::NotFound)
    }
    pub async fn move_message(&mut self, store: &mut Store, id: i64, target: i64) -> Result<()> {
        let message = store.message(id)?.summary;
        let destination = store.folder(target)?;
        if message.account != self.account.id || destination.account != self.account.id {
            return Err(Error::Invalid("cross-account moves unsupported"));
        }
        if message.folder == target {
            return Ok(());
        }
        let before = store.folder(message.folder)?.uidvalidity;
        let (source, _) = self.select(store, message.folder).await?;
        if before != source.uidvalidity {
            return Err(Error::Invalid("mailbox changed; resync before operation"));
        }
        if self.capabilities.has_str("MOVE") {
            net(self
                .session
                .uid_mv(message.uid.to_string(), &destination.name))
            .await?;
        } else {
            if !self.capabilities.has_str("UIDPLUS") {
                return Err(Error::Network("safe move requires MOVE or UIDPLUS"));
            }
            net(self
                .session
                .uid_copy(message.uid.to_string(), &destination.name))
            .await?;
            net(async {
                self.session
                    .uid_store(message.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
            net(async {
                self.session
                    .uid_expunge(message.uid.to_string())
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
        }
        // Destination UIDs are assigned by the server. Resync before removing the old local row.
        self.sync_folder(store, target, DEFAULT_LIMIT).await?;
        store.delete_message(id)?;
        self.sync_folder(store, message.folder, DEFAULT_LIMIT).await
    }
    pub(crate) async fn ensure_role(
        &mut self,
        store: &Store,
        wanted: FolderRole,
        fallback: &str,
    ) -> Result<i64> {
        let folders = self.list_folders(store).await?;
        if let Some(f) = folders.iter().find(|f| f.role == wanted) {
            return Ok(f.id);
        }
        self.create_folder(store, fallback).await
    }
    pub async fn archive(&mut self, store: &mut Store, id: i64) -> Result<()> {
        if store.message(id)?.summary.account != self.account.id {
            return Err(Error::Invalid("message belongs to another account"));
        }
        let fallback = if presets::is_gmail(&self.account) {
            "[Gmail]/All Mail"
        } else {
            "Archive"
        };
        let target = self
            .ensure_role(store, FolderRole::Archive, fallback)
            .await?;
        self.move_message(store, id, target).await
    }
    pub async fn trash(&mut self, store: &mut Store, id: i64) -> Result<()> {
        if store.message(id)?.summary.account != self.account.id {
            return Err(Error::Invalid("message belongs to another account"));
        }
        let target = self.ensure_role(store, FolderRole::Trash, "Trash").await?;
        self.move_message(store, id, target).await
    }
    pub async fn append_sent(&mut self, store: &mut Store, raw: &[u8]) -> Result<()> {
        let target = self.ensure_role(store, FolderRole::Sent, "Sent").await?;
        let folder = store.folder(target)?;
        net(self
            .session
            .append(&folder.name, Some("(\\Seen)"), None, raw))
        .await?;
        self.sync_folder(store, target, DEFAULT_LIMIT).await
    }
    /// Wait until INBOX changes. Dropping this future closes its connection; no UI thread is created.
    pub async fn idle_inbox(mut self) -> Result<bool> {
        if !self.capabilities.has_str("IDLE") {
            return Err(Error::Network("server does not support IDLE"));
        }
        net(self.session.select("INBOX")).await?;
        loop {
            let mut idle = self.session.idle();
            net(idle.init()).await?;
            let response = {
                let (wait, _cancel) = idle.wait();
                wait.await.map_err(|_| Error::Network("IMAP IDLE failed"))?
            };
            self.session = net(idle.done()).await?;
            match response {
                async_imap::extensions::idle::IdleResponse::NewData(_) => return Ok(true),
                async_imap::extensions::idle::IdleResponse::Timeout => {}
                _ => return Err(Error::Network("IMAP IDLE connection closed")),
            }
        }
    }
}
fn flags(fetch: &Fetch) -> (bool, bool) {
    (
        fetch.flags().any(|f| f == Flag::Seen),
        fetch.flags().any(|f| f == Flag::Flagged),
    )
}
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
fn validate_folder(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
        Err(Error::Invalid("invalid folder name"))
    } else {
        Ok(())
    }
}
fn role(name: &str, attrs: &[NameAttribute<'_>]) -> FolderRole {
    for attr in attrs {
        match attr {
            NameAttribute::All | NameAttribute::Archive => return FolderRole::Archive,
            NameAttribute::Sent => return FolderRole::Sent,
            NameAttribute::Drafts => return FolderRole::Drafts,
            NameAttribute::Trash => return FolderRole::Trash,
            NameAttribute::Junk => return FolderRole::Junk,
            _ => {}
        }
    }
    FolderRole::from_name(name)
}
fn text_parts(
    body: &BodyStructure<'_>,
    path: &[u32],
    depth: usize,
    parts: &mut Vec<(Vec<u32>, bool, u32)>,
    attachments: &mut bool,
) -> Result<()> {
    if depth > 32 {
        return Err(Error::Invalid("MIME nesting exceeds 32 levels"));
    }
    let common = match body {
        BodyStructure::Basic { common, .. }
        | BodyStructure::Text { common, .. }
        | BodyStructure::Message { common, .. }
        | BodyStructure::Multipart { common, .. } => common,
    };
    if common
        .disposition
        .as_ref()
        .is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"))
    {
        *attachments = true;
        return Ok(());
    }
    match body {
        BodyStructure::Multipart { bodies, .. } => {
            for (index, child) in bodies.iter().enumerate() {
                let mut next = path.to_vec();
                next.push(index as u32 + 1);
                text_parts(child, &next, depth + 1, parts, attachments)?;
            }
        }
        BodyStructure::Text { other, .. }
            if common.ty.subtype.eq_ignore_ascii_case("plain")
                || common.ty.subtype.eq_ignore_ascii_case("html") =>
        {
            parts.push((
                path.to_vec(),
                !common.ty.subtype.eq_ignore_ascii_case("plain"),
                other.octets,
            ));
        }
        _ => {
            *attachments = true;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_and_input() {
        assert_eq!(role("Localized", &[NameAttribute::Sent]), FolderRole::Sent);
        assert_eq!(role("Archive", &[]), FolderRole::Archive);
        assert!(validate_folder("bad\r\nDELETE").is_err());
        assert_eq!(uid_set(&[1, 5, 9]), "1,5,9");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    #[tokio::test]
    async fn condstore_special_use_and_uidvalidity_guard() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (r, mut w) = socket.into_split();
            let mut read = BufReader::new(r);
            w.write_all(b"* OK Test server\r\n").await.unwrap();
            let mut selections = 0;
            loop {
                let mut line = String::new();
                if read.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let response = if command.starts_with("LOGIN ") {
                    format!("{tag} OK Logged in\r\n")
                } else if command == "CAPABILITY" {
                    format!(
                        "* CAPABILITY IMAP4rev1 CONDSTORE SPECIAL-USE\r\n{tag} OK capabilities\r\n"
                    )
                } else if command == "LIST \"\" \"*\" RETURN (SPECIAL-USE)" {
                    format!("* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Archive) \"/\" \"Localized\"\r\n{tag} OK folders\r\n")
                } else if command == "SELECT \"INBOX\" (CONDSTORE)" {
                    selections += 1;
                    let validity = if selections == 1 { 7 } else { 8 };
                    format!("* 1 EXISTS\r\n* OK [UIDVALIDITY {validity}] valid\r\n* OK [UIDNEXT 2] next\r\n* OK [HIGHESTMODSEQ 11] highest\r\n{tag} OK selected\r\n")
                } else if command == "UID SEARCH ALL" {
                    format!("* SEARCH 1\r\n{tag} OK search\r\n")
                } else if command == "UID FETCH 1:* (UID FLAGS MODSEQ) (CHANGEDSINCE 10)" {
                    format!("* 1 FETCH (UID 1 FLAGS (\\Seen \\Flagged) MODSEQ (11))\r\n{tag} OK fetch\r\n")
                } else if command == "LOGOUT" {
                    w.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes())
                        .await
                        .unwrap();
                    break;
                } else {
                    panic!("unexpected command (credentials omitted)");
                };
                w.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let config = ServerConfig {
            host: "127.0.0.1".into(),
            port,
            security: Security::Plain,
            username: "unit".into(),
        };
        let account = Account::new("unit@test.local".into(), config.clone(), config).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("protocol.db")).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store
            .save_folder(&account.id, "INBOX", Some("/"), FolderRole::Inbox)
            .unwrap();
        store.reset_uidvalidity(inbox, 7).unwrap();
        let id = store
            .save_message(&crate::store::Message {
                summary: crate::store::MessageSummary {
                    account: account.id.clone(),
                    folder: inbox,
                    uid: 1,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .finish_sync(inbox, &[1].into_iter().collect(), 1, Some(2), Some(10))
            .unwrap();
        let mut imap = ImapSync::login(&account, "test").await.unwrap();
        let folders = imap.list_folders(&store).await.unwrap();
        assert!(folders
            .iter()
            .any(|f| f.name == "Localized" && f.role == FolderRole::Archive));
        imap.sync_folder(&mut store, inbox, DEFAULT_LIMIT)
            .await
            .unwrap();
        let message = store.message(id).unwrap();
        assert!(message.summary.seen && message.summary.flagged);
        assert_eq!(store.folder(inbox).unwrap().highest_modseq, Some(11));
        assert!(matches!(
            imap.mark_seen(&mut store, id, false).await,
            Err(Error::Invalid(_))
        ));
        assert!(store.message(id).is_err());
        imap.logout().await.unwrap();
        server.await.unwrap();
        assert!(log::STATIC_MAX_LEVEL < log::LevelFilter::Trace);
    }
}

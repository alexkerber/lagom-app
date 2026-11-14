//! async-imap provides CONDSTORE and cancellable IDLE without a dedicated polling
//! thread; Tokio/rustls also shares the desktop's existing runtime dependencies.
mod guard;
mod drafts;
mod inline;
mod mailbox;
pub(crate) use mailbox::decode as decode_mailbox;
use mailbox::encode as encode_mailbox;
#[cfg(test)]
mod folder_tests;
use guard::Guarded;
use crate::{
    account::{Account, Auth, Security, ServerConfig},
    attachment::{self, Attachment},
    mime, presets,
    store::{Folder, FolderRole, Message, Store},
    Error, Result,
};
use async_imap::{
    imap_proto::{BodyStructure, ContentEncoding, SectionPath},
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

mod bounded;
use bounded::{Bounded, MAX_LITERAL};

trait Io: AsyncRead + AsyncWrite + Unpin + Send + Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Debug> Io for T {}
type Wire = Box<dyn Io>;
pub const DEFAULT_LIMIT: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(60);
const TIMED_OUT: &str = "IMAP operation timed out";
const DROPPED: &str = "IMAP connection lost";
const FAILED: &str = "IMAP operation failed";
/// Whether an error is the transport's fault. Only Lagom's own limits carry `Rejected`.
trait Transient {
    fn transient(&self) -> bool { false }
}
impl Transient for std::io::Error {
    fn transient(&self) -> bool { !self.get_ref().is_some_and(|e| e.is::<guard::Rejected>()) }
}
impl<T: Transient, U> Transient for (T, U) {
    fn transient(&self) -> bool { self.0.transient() }
}
impl Transient for async_imap::error::Error {
    fn transient(&self) -> bool {
        match self { Self::Io(e) => e.transient(), Self::ConnectionLost => true, _ => false }
    }
}
async fn net<T, E: Transient>(f: impl Future<Output = std::result::Result<T, E>>) -> Result<T> {
    net_as(FAILED, f).await
}
/// Like `net`, with a message that tells the user which step failed: host, TLS or login.
async fn net_as<T, E: Transient>(
    what: &'static str,
    f: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .map_err(|_| Error::Network(TIMED_OUT))?
        .map_err(|e| Error::Network(if e.transient() { DROPPED } else { what }))
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
    Ok(Box::new(Guarded::new(
        net_as(
            "IMAP TLS handshake failed",
            TlsConnector::from(Arc::new(config)).connect(name, stream),
        )
        .await?,
    )))
}
async fn connect(config: &ServerConfig, password: &str, oauth: bool) -> Result<Session<Bounded<Wire>>> {
    config.validate()?;
    let socket = net_as(
        "could not reach your mail provider",
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await?;
    // Validate the resolved peer too: a poisoned localhost mapping must not expose LOGIN.
    if config.security == Security::Plain && !socket.peer_addr()?.ip().is_loopback() {
        return Err(Error::Invalid("unencrypted peer must be loopback"));
    }
    let client: Client<Bounded<Wire>> = match config.security {
        Security::Tls => {
            let mut client = Client::new(Bounded::new(tls(socket, &config.host).await?));
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            client
        }
        Security::StartTls => {
            let mut client = Client::new(Bounded::new(Guarded::new(socket)));
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            net(client.run_command_and_check_ok("STARTTLS", None)).await?;
            Client::new(Bounded::new(tls(client.into_inner().inner.inner, &config.host).await?))
        }
        Security::Plain => {
            let mut client = Client::new(Bounded::new(Box::new(Guarded::new(socket)) as Wire));
            if net(client.read_response()).await?.is_none() {
                return Err(Error::Network("IMAP greeting missing"));
            }
            client
        }
    };
    if oauth {
        tokio::time::timeout(TIMEOUT, client.authenticate("XOAUTH2", Xoauth2(Some(xoauth2(&config.username, password)))))
            .await.map_err(|_| Error::Network("IMAP login timed out"))?
            .map_err(|(error, _)| match error {
                async_imap::error::Error::No(_) => Error::Authentication,
                _ => Error::Network("IMAP authentication failed"),
            })
    } else {
        net_as("The email address or password was not accepted", client.login(&config.username, password)).await
    }
}
fn xoauth2(email: &str, token: &str) -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(format!("user={email}\x01auth=Bearer {token}\x01\x01"))
}
struct Xoauth2(Option<zeroize::Zeroizing<String>>);
impl async_imap::Authenticator for Xoauth2 {
    type Response = zeroize::Zeroizing<String>;
    fn process(&mut self, _: &[u8]) -> Self::Response {
        // A second challenge is an OAuth error: acknowledge it without resending the token.
        self.0.take().unwrap_or_default()
    }
}

pub struct ImapSync {
    account: Account,
    session: Session<Bounded<Wire>>,
    capabilities: Capabilities,
    refresh_error: Option<Error>,
    /// Kept to reconnect after a message whose response left the session unusable.
    password: zeroize::Zeroizing<String>,
}
impl ImapSync {
    pub async fn login(account: &Account, password: &str) -> Result<Self> {
        account.validate()?;
        let mut session = match account.auth {
            Auth::Password => connect(&account.imap, password, false).await?,
            Auth::OAuth(_) => {
                let token = crate::oauth::access_token(account, None).await?;
                match connect(&account.imap, &token, true).await {
                    Err(Error::Authentication) => {
                        let fresh = crate::oauth::access_token(account, Some(&token)).await?;
                        connect(&account.imap, &fresh, true).await?
                    }
                    result => result?,
                }
            }
        };
        let capabilities = net(session.capabilities()).await?;
        Ok(Self {
            account: account.clone(),
            session,
            capabilities,
            refresh_error: None,
            // OAuth reconnects with its own token; do not hold the refresh token for the session's life.
            password: zeroize::Zeroizing::new(if matches!(account.auth, Auth::Password) { password.to_owned() } else { String::new() }),
        })
    }
    /// A fresh session with `folder` selected again, as it was before the failed command.
    async fn reconnect(&mut self, store: &mut Store, folder: &Folder) -> Result<()> {
        // Drop the old socket first: some servers cap connections per account. The stand-in reads end-of-file.
        self.session.get_mut().inner = Box::new(tokio::io::duplex(1).0);
        let fresh = Self::login(&self.account, &self.password).await?;
        (self.session, self.capabilities) = (fresh.session, fresh.capabilities);
        let (selected, _) = self.select(store, folder.id).await?;
        if selected.uidvalidity != folder.uidvalidity {
            return Err(Error::Invalid("mailbox changed; resync"));
        }
        Ok(())
    }
    /// False when the message failed on a fresh session too; the session is usable either way.
    async fn try_fetch(&mut self, store: &mut Store, folder: &Folder, uid: u32) -> Result<bool> {
        for retry in [false, true] {
            match self.fetch_message(store, folder, uid, mime::MAX_MIME_BYTES).await {
                Ok(()) => return Ok(true),
                Err(e) if e.is_account_wide() || matches!(e, Error::Store(_) | Error::Io(_)) => return Err(e),
                // The network's fault, not the message's: keep the checkpoint and try again next sync.
                Err(e) if matches!(e, Error::Network(TIMED_OUT)) || (retry && matches!(e, Error::Network(DROPPED))) => return Err(e),
                // A rejected response may be half-read; the first retry also covers a dropped connection.
                Err(e) => {
                    // A clean end-of-file after a row reads as a short response, not an error: ask the session.
                    // After FAILED the stream is dead by Lagom's own rejection. async-imap's noop() ignores end-of-file.
                    if retry && !matches!(e, Error::Network(FAILED)) && matches!(net(self.session.run_command_and_check_ok("NOOP")).await, Err(Error::Network(DROPPED))) {
                        return Err(Error::Network(DROPPED));
                    }
                    self.reconnect(store, folder).await?
                }
            }
        }
        Ok(false)
    }
    /// A confirmed move can succeed while its subsequent refresh fails.
    pub fn take_refresh_error(&mut self) -> Option<Error> {
        self.refresh_error.take()
    }
    pub async fn logout(mut self) -> Result<()> {
        net(self.session.logout()).await
    }
    pub async fn list_folders(&mut self, store: &Store) -> Result<Vec<Folder>> {
        // async-imap's LIST stream can end without checking the tagged status.
        let command = if self.capabilities.has_str("SPECIAL-USE") {
            "LIST \"\" \"*\" RETURN (SPECIAL-USE)"
        } else { "LIST \"\" \"*\"" };
        let rows = tokio::time::timeout(TIMEOUT, async {
            let mut rows = Vec::new();
            let tag = net(self.session.run_command(command)).await?;
            loop {
                let response = net(self.session.read_response()).await?
                    .ok_or(Error::Network("IMAP folder listing interrupted"))?;
                use async_imap::imap_proto::{MailboxDatum, Response, Status};
                match response.parsed() {
                    Response::MailboxData(MailboxDatum::List(data)) if listed(&data.name, &data.name_attributes) => {
                        let name = decode_mailbox(&data.name)?;
                        let role = role(&name, &data.name_attributes);
                        rows.push((name, data.delimiter.as_deref().map(str::to_owned), role));
                    }
                    Response::Done { tag: done, status, .. } if *done == tag => {
                        return if *status == Status::Ok { Ok(rows) } else { Err(Error::Network("IMAP folder listing failed")) };
                    }
                    _ => {}
                }
            }
        }).await.map_err(|_| Error::Network("IMAP folder listing timed out"))??;
        store.replace_folders(&self.account.id, &rows)
    }
    pub async fn sync(&mut self, store: &mut Store, limit: usize) -> Result<()> {
        if limit == 0 {
            return Err(Error::Invalid("sync limit must be positive"));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        for id in store.due_drafts(&self.account.id, now)? {
            if let Err(error) = self.sync_draft(store, &id, now).await {
                store.draft_sync_failed(&id, &error, now)?;
                return Err(error);
            }
        }
        for folder in self.list_folders(store).await? {
            // Stop here: after an error this session may hold a half-read response.
            self.sync_folder(store, folder.id, limit).await.map_err(|e| match e {
                e if e.is_account_wide() => e,
                e => Error::Folder { folder: folder.name, source: Box::new(e) },
            })?;
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
            net(self.session.select_condstore(&encode_mailbox(&folder.name))).await?
        } else {
            net(self.session.select(&encode_mailbox(&folder.name))).await?
        };
        let validity = mailbox
            .uid_validity
            .filter(|v| *v > 0)
            .ok_or(Error::Network("IMAP UIDVALIDITY missing"))?;
        store.reset_uidvalidity(id, validity)?;
        Ok((store.folder(id)?, mailbox))
    }
    async fn all_uids(&mut self) -> Result<HashSet<u32>> {
        self.search_uids("ALL").await
    }
    async fn search_uids(&mut self, query: &str) -> Result<HashSet<u32>> {
        use async_imap::imap_proto::{MailboxDatum, Response, Status};
        // async-imap's search parser discards the tagged status, even after partial results.
        tokio::time::timeout(TIMEOUT, async {
            let tag = self.session.run_command(format!("UID SEARCH {query}")).await
                .map_err(|_| Error::Network("IMAP operation failed"))?;
            let mut uids = HashSet::new();
            loop {
                let response = self.session.read_response().await
                    .map_err(|_| Error::Network("IMAP operation failed"))?
                    .ok_or(Error::Network("IMAP search interrupted"))?;
                match response.parsed() {
                    Response::MailboxData(MailboxDatum::Search(ids)) => uids.extend(ids.iter().copied().filter(|u| *u > 0)),
                    Response::Done { tag: done, status, .. } if *done == tag => {
                        return if *status == Status::Ok { Ok(uids) } else { Err(Error::Network("IMAP search failed")) };
                    }
                    _ => {}
                }
            }
        }).await.map_err(|_| Error::Network("IMAP operation timed out"))?
    }
    pub async fn sync_folder(&mut self, store: &mut Store, id: i64, limit: usize) -> Result<()> {
        if limit == 0 {
            return Err(Error::Invalid("sync limit must be positive"));
        }
        let (folder, mailbox) = self.select(store, id).await?;
        if folder.role == FolderRole::Drafts {
            return self.read_drafts(store, &folder, limit).await;
        }
        let present = self.all_uids().await?;
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
            if !known.contains(&uid) && !self.try_fetch(store, &folder, uid).await? {
                // Otherwise one unreadable message holds the checkpoint below it on every sync.
                let mut message = mime::placeholder();
                message.summary.subject = "(message could not be read)".into();
                message.body_text = "lagom could not read this message. Open it in another mail app.".into();
                self.save(store, &folder, uid, None, message, &[])?;
            }
        }
        for uid in store.missing_metadata(id, limit)? {
            if present.contains(&uid) {
                // On failure the stored row stays as it is.
                self.try_fetch(store, &folder, uid).await?;
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
        let present = self.all_uids().await?;
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
    /// Stores one message with its first text and HTML parts, if together they fit in `limit` bytes.
    async fn fetch_message(&mut self, store: &Store, folder: &Folder, uid: u32, limit: usize) -> Result<()> {
        // shortcut: async-imap exposes no X-GM-THRID accessor; add it to this FETCH when the dependency supports it.
        let metadata=net(async {self.session.uid_fetch(uid.to_string(),"(UID ENVELOPE FLAGS BODYSTRUCTURE RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER]<0.262144>)").await?.try_collect::<Vec<_>>().await}).await?;
        let Some(fetch) = metadata.iter().find(|f| f.uid == Some(uid)) else {
            return Ok(());
        };
        let header = fetch
            .header()
            .ok_or(Error::Network("IMAP message header missing"))?;
        let body = fetch.bodystructure().ok_or(Error::Network("IMAP BODYSTRUCTURE missing"))?;
        let mut parts = Vec::new();
        let mut attachments = Vec::new();
        text_parts(body, &[], 0, false, &mut parts, &mut attachments);
        // Only the first 256 KiB were fetched: keep what parses and go on, so one message
        // never stops the folder's later UIDs.
        if header.len() >= 262144 {
            let message = mime::metadata_only(header).unwrap_or_else(|_| mime::placeholder());
            return self.save(store, folder, uid, Some(fetch), message, &attachments).map(|_| ());
        }
        let mut message;
        // The first text part (search, and the reading pane where HTML does not render) and the
        // first HTML part (the rendered view), in one fetch.
        parts.sort_by_key(|p| p.1);
        let picks: Vec<_> = [parts.iter().find(|p| !p.1), parts.iter().find(|p| p.1)].into_iter().flatten().collect();
        if picks.is_empty() {
            message = mime::parse(header)?;
        } else {
            if picks.iter().map(|p| p.2 as usize).sum::<usize>() > limit {
                return self.save(store, folder, uid, Some(fetch), mime::metadata_only(header)?, &attachments).map(|_| ());
            }
            let section = |path: &[u32]| path.iter().map(u32::to_string).collect::<Vec<_>>().join(".");
            // GreenMail emits malformed partial text responses; check octets before full part fetch.
            // An empty path is a single-part message: one pick.
            let query = if picks[0].0.is_empty() {
                "(UID BODY.PEEK[TEXT])".to_owned()
            } else {
                let items: Vec<String> = picks.iter().map(|p| format!("BODY.PEEK[{}]", section(&p.0))).collect();
                format!("(UID {})", items.join(" "))
            };
            // BODYSTRUCTURE sizes come from the server: also bound what this FETCH may read.
            self.session.get_mut().remaining = Some(limit + 65536);
            let bodies = net(async {
                self.session
                    .uid_fetch(uid.to_string(), query)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await;
            self.session.get_mut().remaining = None;
            let bodies = bodies?;
            let Some(body) = bodies.iter().find(|f| f.uid == Some(uid)) else {
                return Ok(());
            };
            message = mime::parse(header)?;
            let mut total = 0;
            for (path, html, _, part_headers) in picks.iter().copied() {
                let headers = if path.is_empty() { header } else { part_headers.as_bytes() };
                let text = if path.is_empty() {
                    body.text()
                } else {
                    body.section(&SectionPath::Part(path.clone(), None))
                }
                .ok_or(Error::Network("MIME text part missing"))?;
                total += headers.len() + text.len();
                if headers.len() >= 65536 || total > limit {
                    return self.save(store, folder, uid, Some(fetch), mime::metadata_only(header)?, &attachments).map(|_| ());
                }
                let mut raw = headers.to_vec();
                if !raw.ends_with(b"\r\n") {
                    raw.extend_from_slice(b"\r\n");
                }
                if !raw.ends_with(b"\r\n\r\n") {
                    raw.extend_from_slice(b"\r\n");
                }
                raw.extend_from_slice(text);
                // +4: the line breaks added above.
                let parsed = mime::parse_within(&raw, limit + 4)?;
                if path.is_empty() {
                    message = parsed;
                } else if *html {
                    // HTML only: use the same visible text as the MIME parser.
                    if picks.len() == 1 {
                        message.body_text = parsed.body_text;
                    }
                    message.body_html = parsed.body_html;
                } else {
                    message.body_text = parsed.body_text;
                }
            }
            message.summary.snippet = mime::snippet(&message.body_text, message.body_html.as_deref());
        }
        let inline = self.inline_parts(uid, body, message.body_html.as_deref().unwrap_or_default()).await?;
        let id = self.save(store, folder, uid, Some(fetch), message, &attachments)?;
        store.save_inline_parts(id, &inline)
    }
    /// Fills the IMAP-side fields of a parsed message and stores it.
    fn save(&self, store: &Store, folder: &Folder, uid: u32, fetch: Option<&Fetch>, mut message: Message, attachments: &[Attachment]) -> Result<i64> {
        let (seen, flagged) = fetch.map(flags).unwrap_or_default();
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
        m.has_attachments = !attachments.is_empty();
        m.size = fetch.and_then(|f| f.size).unwrap_or_default();
        if m.date == 0 {
            m.date = fetch
                .and_then(|f| f.internal_date())
                .map(|d| d.timestamp())
                .unwrap_or_default();
        }
        let id = store.save_synced_message(&message, attachments)?;
        // From the headers already fetched: opening the message never goes to the network for it.
        store.set_unsubscribe(id, fetch.and_then(Fetch::header).and_then(crate::unsubscribe::from_headers).as_ref())?;
        Ok(id)
    }
    pub async fn fetch_attachment(&mut self, store: &mut Store, id: i64, section: &str) -> Result<Vec<u8>> {
        let message = store.message(id)?.summary;
        let part = store.attachments(id)?.into_iter().find(|p| p.section == section).ok_or(Error::NotFound)?;
        let path = section.split('.').map(|s| s.parse::<u32>().ok().filter(|n| *n > 0))
            .collect::<Option<Vec<_>>>().ok_or(Error::Invalid("invalid attachment section"))?;
        // shortcut: buffer encoded parts up to 64 MiB; stream to disk when larger attachments are needed.
        if part.size as usize > MAX_LITERAL {
            return Err(Error::Invalid("attachment exceeds 64 MiB"));
        }
        // Include SELECT/read-ahead in the budget; allow 64 KiB for protocol framing.
        self.session.get_mut().remaining = Some(part.size as usize + 65536);
        let before = store.folder(message.folder)?.uidvalidity;
        let (folder, _) = self.select(store, message.folder).await?;
        if folder.uidvalidity != before {
            return Err(Error::Invalid("mailbox changed; resync before download"));
        }
        let path = SectionPath::Part(path, None);
        let query = format!("(UID BODY.PEEK[{section}])");
        let fetched = net(async {
            let mut stream = self.session.uid_fetch(message.uid.to_string(), query).await?;
            let mut found = None;
            while let Some(fetch) = stream.try_next().await? {
                if found.is_none() && fetch.uid == Some(message.uid) && fetch.section(&path).is_some() {
                    found = Some(fetch);
                }
            }
            Ok::<_, async_imap::error::Error>(found)
        }).await?;
        self.session.get_mut().remaining = None;
        let bytes = fetched.as_ref().and_then(|f| f.section(&path))
            .ok_or(Error::Network("attachment part missing"))?;
        if bytes.len() > MAX_LITERAL {
            return Err(Error::Invalid("attachment exceeds 64 MiB"));
        }
        if bytes.len() != part.size as usize {
            return Err(Error::Network("attachment size changed or download incomplete"));
        }
        let data = attachment::decode(bytes, &part.encoding)?;
        store.cache_attachment(id, section, &data)?;
        Ok(data)
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
        // async-imap's STORE stream discards tagged NO/BAD replies.
        net(self.session.run_command_and_check_ok(format!("UID STORE {} {command}", message.uid))).await?;
        store.set_flags(message.folder, message.uid, seen, message.flagged)
    }
    pub async fn create_folder(&mut self, store: &Store, name: &str) -> Result<i64> {
        validate_folder(name)?;
        net(self.session.create(&encode_mailbox(name))).await?;
        self.list_folders(store).await?;
        store
            .folders(&self.account.id, 0)?
            .into_iter()
            .find(|f| f.name == name)
            .map(|f| f.id)
            .ok_or(Error::NotFound)
    }
    pub async fn move_message(&mut self, store: &mut Store, id: i64, target: i64) -> Result<()> {
        let result = self.move_on_server(store, id, target).await;
        if result.is_err() { store.cancel_move(id)?; }
        result
    }
    async fn move_on_server(&mut self, store: &mut Store, id: i64, target: i64) -> Result<()> {
        let message = store.message(id)?.summary;
        let destination = store.folder(target)?;
        if message.account != self.account.id || destination.account != self.account.id {
            return Err(Error::Invalid("cross-account moves unsupported"));
        }
        if message.folder == target {
            return store.cancel_move(id);
        }
        store.begin_move(id, target)?;
        let before = store.folder(message.folder)?.uidvalidity;
        let (source, _) = self.select(store, message.folder).await?;
        if before != source.uidvalidity {
            return Err(Error::Invalid("mailbox changed; resync before operation"));
        }
        if source.role == FolderRole::Trash {
            // An interrupted empty may have marked the message; MOVE and COPY preserve flags.
            net(self.session.run_command_and_check_ok(format!("UID STORE {} -FLAGS.SILENT (\\Deleted)", message.uid))).await?;
        }
        if self.capabilities.has_str("MOVE") {
            net(self
                .session
                .uid_mv(message.uid.to_string(), &encode_mailbox(&destination.name)))
            .await?;
        } else {
            // shortcut: COPY can leave a destination copy if deletion fails; reconcile duplicates when resumable moves are added.
            net(self
                .session
                .uid_copy(message.uid.to_string(), &encode_mailbox(&destination.name)))
            .await?;
            self.delete_uids(&message.uid.to_string(), true).await?;
        }
        // A confirmed move must not be rolled back because a subsequent refresh failed.
        if let Err(error) = store.delete_message(id) {
            self.refresh_error = Some(error);
            return Ok(());
        }
        if let Err(error) = self.sync_folder(store, target, DEFAULT_LIMIT).await {
            self.refresh_error = Some(error);
            return Ok(());
        }
        if let Err(error) = self.sync_folder(store, message.folder, DEFAULT_LIMIT).await {
            self.refresh_error = Some(error);
        }
        Ok(())
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
    async fn delete_uids(&mut self, set: &str, restore_on_refusal: bool) -> Result<()> {
        // async-imap's STORE/EXPUNGE streams discard tagged NO/BAD replies.
        net(self.session.run_command_and_check_ok(format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"))).await?;
        // shortcut: without UIDPLUS, EXPUNGE also removes other deleted mail; use UID EXPUNGE whenever advertised.
        let command = if self.capabilities.has_str("UIDPLUS") { format!("UID EXPUNGE {set}") } else { "EXPUNGE".into() };
        let result = tokio::time::timeout(TIMEOUT, self.session.run_command_and_check_ok(command)).await
            .map_err(|_| Error::Network("IMAP operation timed out"))?;
        if restore_on_refusal && matches!(&result, Err(async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_))) {
            // A refused move must not leave the original waiting for another client's EXPUNGE.
            net(self.session.run_command_and_check_ok(format!("UID STORE {set} -FLAGS.SILENT (\\Deleted)"))).await?;
        }
        result.map_err(|_| Error::Network("IMAP expunge failed"))
    }
    pub async fn empty_trash(&mut self, store: &mut Store, id: i64) -> Result<()> {
        let folder = store.folder(id)?;
        if folder.account != self.account.id || folder.role != FolderRole::Trash {
            return Err(Error::Invalid("not this account's Trash folder"));
        }
        if !self.capabilities.has_str("UIDPLUS") {
            return Err(Error::Network("empty Trash requires UIDPLUS"));
        }
        let (selected, _) = self.select(store, id).await?;
        if folder.uidvalidity.is_some() && folder.uidvalidity != selected.uidvalidity {
            return Err(Error::Invalid("mailbox changed; resync before operation"));
        }
        let mut uids: Vec<_> = self.all_uids().await?.into_iter().collect();
        uids.sort_unstable();
        // Exact UIDs keep mail arriving during the operation out of the expunge.
        for chunk in uids.chunks(100) {
            self.delete_uids(&uid_set(chunk), false).await?;
        }
        self.sync_folder(store, id, DEFAULT_LIMIT).await
    }
    pub async fn append_sent(&mut self, store: &mut Store, raw: &[u8]) -> Result<i64> {
        let target = self.ensure_role(store, FolderRole::Sent, "Sent").await?;
        let folder = store.folder(target)?;
        net(self
            .session
            .append(&encode_mailbox(&folder.name), Some("(\\Seen)"), None, raw))
        .await?;
        Ok(target)
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
fn encoding_name(encoding: &ContentEncoding<'_>) -> String {
    match encoding {
        ContentEncoding::SevenBit => "7bit".into(),
        ContentEncoding::EightBit => "8bit".into(),
        ContentEncoding::Binary => "binary".into(),
        ContentEncoding::Base64 => "base64".into(),
        ContentEncoding::QuotedPrintable => "quoted-printable".into(),
        ContentEncoding::Other(value) => value.to_string(),
    }
}
fn attachment_filename(common: &async_imap::imap_proto::BodyContentCommon<'_>) -> Option<String> {
    // Let the MIME parser decode RFC 2231 continuations and encoded words too.
    let params = |values: &async_imap::imap_proto::BodyParams<'_>| values.iter().flatten().map(|(k, v)| {
        let key = k.replace(['\r', '\n', '"', '\\'], "");
        let value = v.replace(['\r', '\n'], "").replace('\\', "\\\\").replace('"', "\\\"");
        format!("; {key}=\"{value}\"")
    }).collect::<String>();
    let disposition = common.disposition.as_ref().map(|d| params(&d.params)).unwrap_or_default();
    let header = format!("Content-Type: application/octet-stream{}\r\nContent-Disposition: attachment{disposition}\r\n\r\nx", params(&common.ty.params));
    let message = mail_parser::MessageParser::default().parse(header.as_bytes())?;
    let name = message.attachments().next()?.attachment_name().map(attachment::strip_bidi).filter(|s| !s.is_empty());
    name
}
type TextPart = (Vec<u32>, bool, u32, String);

fn text_parts(
    body: &BodyStructure<'_>,
    path: &[u32],
    depth: usize,
    attached_parent: bool,
    parts: &mut Vec<TextPart>,
    attachments: &mut Vec<Attachment>,
) {
    let common = match body {
        BodyStructure::Basic { common, .. }
        | BodyStructure::Text { common, .. }
        | BodyStructure::Message { common, .. }
        | BodyStructure::Multipart { common, .. } => common,
    };
    let filename = attachment_filename(common);
    // The parser caps recursion at 40; deeper text branches remain downloadable attachments.
    let attached = attached_parent || depth > 32 || filename.is_some() || common.disposition.as_ref().is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"));
    if let BodyStructure::Basic { other, .. } | BodyStructure::Text { other, .. } | BodyStructure::Message { other, .. } = body {
        let readable = matches!(body, BodyStructure::Text { .. }) &&
            (common.ty.subtype.eq_ignore_ascii_case("plain") || common.ty.subtype.eq_ignore_ascii_case("html"));
        if attached || !readable {
            let section = if path.is_empty() { "1".into() } else { path.iter().map(u32::to_string).collect::<Vec<_>>().join(".") };
            attachments.push(Attachment {
                filename: filename.unwrap_or_else(|| if matches!(body, BodyStructure::Message { .. }) { format!("message-{section}.eml") } else { format!("attachment-{section}") }),
                section,
                mime_type: format!("{}/{}", common.ty.ty, common.ty.subtype).to_ascii_lowercase(),
                size: other.octets,
                encoding: encoding_name(&other.transfer_encoding),
            });
            return;
        }
    }
    match body {
        BodyStructure::Multipart { bodies, .. } => {
            // shortcut: attached multipart containers expose leaf files; add container downloads when needed.
            for (index, child) in bodies.iter().enumerate() {
                let mut next = path.to_vec();
                next.push(index as u32 + 1);
                text_parts(child, &next, depth + 1, attached, parts, attachments);
            }
        }
        BodyStructure::Text { other, .. }
            if common.ty.subtype.eq_ignore_ascii_case("plain")
                || common.ty.subtype.eq_ignore_ascii_case("html") =>
        {
            // The part's headers from BODYSTRUCTURE: some servers answer `n.m.MIME` with the
            // parent's headers. Server strings lose CR and LF so they stay one header each.
            let clean = |v: &str| v.replace(['\r', '\n', '"', '\\'], "");
            let params: String = common.ty.params.iter().flatten().map(|(k, v)| format!("; {}=\"{}\"", clean(k), clean(v))).collect();
            let encoding = clean(&encoding_name(&other.transfer_encoding));
            parts.push((
                path.to_vec(),
                !common.ty.subtype.eq_ignore_ascii_case("plain"),
                other.octets,
                format!("Content-Type: text/{}{params}\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n", clean(&common.ty.subtype)),
            ));
        }
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parsed_parts(structure: &str) -> (Vec<TextPart>, Vec<Attachment>) {
        use async_imap::imap_proto::{AttributeValue, Response};
        let wire = format!("* 1 FETCH (BODYSTRUCTURE {structure})\r\n");
        let (_, Response::Fetch(_, attrs)) = Response::parse(wire.as_bytes()).unwrap() else { panic!("fetch") };
        let body = attrs.iter().find_map(|a| match a { AttributeValue::BodyStructure(b) => Some(b), _ => None }).unwrap();
        let (mut text, mut attachments) = (Vec::new(), Vec::new());
        text_parts(body, &[], 0, false, &mut text, &mut attachments);
        (text, attachments)
    }

    #[test]
    fn attachment_sections_names_and_encodings() {
        let (text, parts) = parsed_parts(r#"(("TEXT" "PLAIN" NIL NIL NIL "7BIT" 5 1)(("TEXT" "HTML" NIL NIL NIL "8BIT" 8 1)("TEXT" "PLAIN" NIL NIL NIL "QUOTED-PRINTABLE" 12 1 NIL ("ATTACHMENT" ("FILENAME*0*" "utf-8''r%C3%A4" "FILENAME*1*" "ksm%C3%B6rg%C3%A5s.txt")))("IMAGE" "PNG" ("NAME" "picture.png") NIL NIL "BASE64" 16 NIL ("INLINE" NIL)) "MIXED")("APPLICATION" "PDF" NIL NIL NIL "BINARY" 30) "MIXED")"#);
        assert_eq!(text.iter().map(|p| (&p.0, p.1)).collect::<Vec<_>>(), [(&vec![1], false), (&vec![2, 1], true)]);
        assert_eq!(parts.iter().map(|p| (p.section.as_str(), p.filename.as_str(), p.mime_type.as_str(), p.size, p.encoding.as_str())).collect::<Vec<_>>(), [
            ("2.2", "räksmörgås.txt", "text/plain", 12, "quoted-printable"),
            ("2.3", "picture.png", "image/png", 16, "base64"),
            ("3", "attachment-3", "application/pdf", 30, "binary"),
        ]);
        let (text, parts) = parsed_parts(r#"("APPLICATION" "PDF" NIL NIL NIL "BASE64" 8 NIL ("ATTACHMENT" ("FILENAME" "=?UTF-8?Q?r=C3=A4ksm=C3=B6rg=C3=A5s.pdf?=")))"#);
        assert!(text.is_empty());
        assert_eq!((&*parts[0].section, &*parts[0].filename), ("1", "räksmörgås.pdf"));
        let (_, parts) = parsed_parts("(\"APPLICATION\" \"OCTET-STREAM\" (\"NAME\" \"invoice\u{202E}fdp.exe\") NIL NIL \"BASE64\" 8)");
        assert_eq!(parts[0].filename, "invoicefdp.exe");
        let (text, parts) = parsed_parts(r#"(("TEXT" "PLAIN" NIL NIL NIL "7BIT" 5 1) "MIXED" NIL ("ATTACHMENT" ("FILENAME" "bundle")))"#);
        assert!(text.is_empty());
        assert_eq!(parts[0].section, "1");
        let (text, parts) = parsed_parts(r#"("MESSAGE" "RFC822" NIL NIL NIL "7BIT" 100 (NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL) ("TEXT" "PLAIN" NIL NIL NIL "7BIT" 5 1) 3)"#);
        assert!(text.is_empty());
        assert_eq!((&*parts[0].section, &*parts[0].filename, &*parts[0].mime_type), ("1", "message-1.eml", "message/rfc822"));
    }

    #[test]
    fn text_parts_skip_excessive_nesting_and_keep_siblings() {
        use async_imap::imap_proto::{AttributeValue, Response};
        let text = r#"("TEXT" "PLAIN" NIL NIL NIL "7BIT" 4 1)"#;
        for depth in [32, 33, 39] {
            let nested = format!("{}{text}{}", "(".repeat(depth - 1), " \"MIXED\")".repeat(depth - 1));
            let raw = format!("* 1 FETCH (BODYSTRUCTURE ({nested}{text} \"MIXED\"))\r\n");
            let (_, Response::Fetch(_, attrs)) = Response::parse(raw.as_bytes()).unwrap() else {
                panic!("expected FETCH");
            };
            let AttributeValue::BodyStructure(body) = &attrs[0] else {
                panic!("expected BODYSTRUCTURE");
            };
            let mut parts = Vec::new();
            let mut attachments = Vec::new();
            text_parts(body, &[], 0, false, &mut parts, &mut attachments);
            assert_eq!(parts.len(), if depth == 32 { 2 } else { 1 });
            assert_eq!(!attachments.is_empty(), depth > 32);
            if depth > 32 {
                assert_eq!(attachments[0].section, vec!["1"; depth].join("."));
                assert_eq!(attachments[0].size, 4);
            }
            assert_eq!(parts.last().unwrap().0, vec![2]);
            if depth == 32 {
                assert_eq!(parts[0].0, vec![1; depth]);
            }
        }
    }

    #[tokio::test]
    async fn xoauth2_wire_and_error_acknowledgement() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        assert_eq!(&*xoauth2("a@example.com", "token"), "user=a@example.com\x01auth=Bearer token\x01\x01");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Plain, username: "a@example.com".into() };
        let server = tokio::spawn(async move {
            for reject in [true, false] {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                socket.get_mut().write_all(b"* OK fake\r\n").await.unwrap();
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();
                let tag = line.split_whitespace().next().unwrap().to_owned();
                assert!(line.contains(" AUTHENTICATE XOAUTH2\r\n"));
                socket.get_mut().write_all(b"+ \r\n").await.unwrap();
                line.clear(); socket.read_line(&mut line).await.unwrap();
                assert_eq!(STANDARD.decode(line.trim()).unwrap(), xoauth2("a@example.com", "token").as_bytes());
                if reject {
                    socket.get_mut().write_all(format!("+ {}\r\n", STANDARD.encode(r#"{"status":"401"}"#)).as_bytes()).await.unwrap();
                    line.clear(); socket.read_line(&mut line).await.unwrap();
                    assert_eq!(line, "\r\n");
                }
                socket.get_mut().write_all(format!("{tag} {}\r\n", if reject { "NO rejected" } else { "OK authenticated" }).as_bytes()).await.unwrap();
            }
        });
        assert!(matches!(connect(&config, "token", true).await, Err(Error::Authentication)));
        assert!(connect(&config, "token", true).await.is_ok());
        server.await.unwrap();
    }

    #[test]
    fn roles_and_input() {
        assert_eq!(role("Localized", &[NameAttribute::Sent]), FolderRole::Sent);
        assert_eq!(role("Localized", &[NameAttribute::Drafts]), FolderRole::Drafts);
        for name in ["Drafts", "Draft", "[Gmail]/Drafts"] { assert_eq!(role(name, &[]), FolderRole::Drafts); }
        assert_eq!(role("Archive", &[]), FolderRole::Archive);
        assert!(validate_folder("bad\r\nDELETE").is_err());
        assert_eq!(uid_set(&[1, 5, 9]), "1,5,9");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test(start_paused = true)]
    async fn search_deadline_includes_all_untagged_responses() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            for response in ["", "* CAPABILITY IMAP4rev1\r\n"] {
                let mut line = String::new();
                server.read_line(&mut line).await.unwrap();
                let tag = line.split_whitespace().next().unwrap();
                server.get_mut().write_all(format!("{response}{tag} OK done\r\n").as_bytes()).await.unwrap();
            }
            let mut line = String::new();
            server.read_line(&mut line).await.unwrap();
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(command, "UID SEARCH ALL");
            for _ in 0..3 {
                tokio::time::sleep(TIMEOUT / 2).await;
                if server.get_mut().write_all(b"* SEARCH 1\r\n").await.is_err() { return; }
            }
            let _ = server.get_mut().write_all(format!("{tag} OK done\r\n").as_bytes()).await;
        });
        let config = ServerConfig { host: "localhost".into(), port: 3143, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        let mut session = Client::new(Bounded::new(Box::new(client) as Wire)).login("fixture", "fixture").await.unwrap();
        let capabilities = session.capabilities().await.unwrap();
        let mut imap = ImapSync { account, session, capabilities, refresh_error: None, password: Default::default() };
        let start = tokio::time::Instant::now();
        let result = imap.all_uids().await;
        assert!(matches!(result, Err(Error::Network("IMAP operation timed out"))), "{result:?}");
        assert_eq!(start.elapsed(), TIMEOUT);
        drop(imap);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn understated_attachment_is_rejected_before_reading_the_literal() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            for response in ["", "* CAPABILITY IMAP4rev1\r\n", "* 1 EXISTS\r\n* OK [UIDVALIDITY 7] valid\r\n"] {
                let mut line = String::new();
                server.read_line(&mut line).await.unwrap();
                let tag = line.split_whitespace().next().unwrap();
                server.get_mut().write_all(format!("{response}{tag} OK done\r\n").as_bytes()).await.unwrap();
            }
            let mut line = String::new();
            server.read_line(&mut line).await.unwrap();
            assert!(line.contains("UID FETCH 1 (UID BODY.PEEK[1])"));
            server.get_mut().write_all(b"* 1 FETCH (UID 1 BODY[1] {268435456}\r\n").await.unwrap();
            // Withhold the body: rejection must not wait for, or allocate, the literal.
            line.clear();
            server.read_line(&mut line).await.unwrap();
        });
        let config = ServerConfig { host: "localhost".into(), port: 3143, security: Security::Plain, username: "fixture".into() };
        let account = Account::new("fixture@example.com".into(), config.clone(), config).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("mail.db")).unwrap();
        store.save_account(&account).unwrap();
        let inbox = store.save_folder(&account.id, "INBOX", None, FolderRole::Inbox).unwrap();
        store.reset_uidvalidity(inbox, 7).unwrap();
        let message = Message { summary: crate::store::MessageSummary {
            account: account.id.clone(), folder: inbox, uid: 1, ..Default::default()
        }, ..Default::default() };
        store.save_synced_message(&message, &[Attachment {
            section: "1".into(), size: 1024, encoding: "binary".into(), ..Default::default()
        }]).unwrap();
        let id = store.list_messages(inbox, 0, 1, 0).unwrap()[0].id;
        let mut session = Client::new(Bounded::new(Box::new(client) as Wire)).login("fixture", "fixture").await.unwrap();
        let capabilities = session.capabilities().await.unwrap();
        let mut imap = ImapSync { account, session, capabilities, refresh_error: None, password: Default::default() };
        let result = tokio::time::timeout(Duration::from_secs(2), imap.fetch_attachment(&mut store, id, "1")).await;
        drop(imap);
        server.await.unwrap();
        assert!(result.expect("must reject the literal header without waiting for its body").is_err());
    }

    #[tokio::test]
    async fn only_lagoms_own_limits_are_not_transport_errors() {
        use std::io::{Error, ErrorKind};
        // tokio-rustls reports a bad TLS record as InvalidData too: it must keep the checkpoint.
        for transport in [Error::new(ErrorKind::InvalidData, "corrupt TLS record"), Error::other("tls alert"), ErrorKind::ConnectionReset.into(), ErrorKind::UnexpectedEof.into()] {
            assert!(transport.transient(), "{transport}");
            assert!(matches!(net(async { Err::<(), _>(async_imap::error::Error::Io(transport)) }).await, Err(crate::Error::Network(DROPPED))));
        }
        assert!(async_imap::error::Error::ConnectionLost.transient());
        let limit = Error::new(ErrorKind::InvalidData, guard::Rejected("limit"));
        assert!(!limit.transient());
        assert!(matches!(net(async { Err::<(), _>(async_imap::error::Error::Io(limit)) }).await, Err(crate::Error::Network("IMAP operation failed"))));
    }
    /// A fake server with three mails. UID 2 has 39 nested multiparts: the wire guard rejects its
    /// FETCH and the session. With `cut`, the server instead sends a bare row for UID 2 and closes.
    async fn fake(cut: bool) -> (u16, tokio::task::JoinHandle<()>) {
        let text = r#"("TEXT" "PLAIN" NIL NIL NIL "7BIT" 4 1)"#;
        let deep = format!("{}{text}{}", "(".repeat(39), " \"MIXED\")".repeat(39));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                let deep = deep.clone();
                tokio::spawn(async move {
                    let (r, mut w) = socket.into_split();
                    let mut r = BufReader::new(r);
                    w.write_all(b"* OK fake\r\n").await.unwrap();
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
                        let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                        let mut out = String::new();
                        if cmd.starts_with("LIST") { out += "* LIST () \"/\" \"INBOX\"\r\n"; }
                        else if cmd == "CAPABILITY" { out += "* CAPABILITY IMAP4rev1\r\n"; }
                        else if cmd.starts_with("SELECT") { out += "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] v\r\n* OK [UIDNEXT 4] n\r\n"; }
                        else if cmd == "UID SEARCH ALL" { out += "* SEARCH 1 2 3\r\n"; }
                        else if let Some(rest) = cmd.strip_prefix("UID FETCH ") {
                            let (set, items) = rest.split_once(' ').unwrap();
                            for uid in set.split(',').filter_map(|u| u.parse::<u32>().ok()) {
                                if cut && uid == 2 {
                                    out += "* 2 FETCH (UID 2 FLAGS ())\r\n";
                                    let _ = w.write_all(out.as_bytes()).await;
                                    return;
                                }
                                if items.contains("BODYSTRUCTURE") {
                                    let header = format!("From: a@example.com\r\nSubject: mail {uid}\r\nMessage-ID: <{uid}@x>\r\n\r\n");
                                    let body = if uid == 2 { deep.as_str() } else { text };
                                    out += &format!("* {uid} FETCH (UID {uid} FLAGS () RFC822.SIZE 10 BODYSTRUCTURE {body} BODY[HEADER]<0> {{{}}}\r\n{header})\r\n", header.len());
                                } else if items.contains("BODY.PEEK[TEXT]") {
                                    out += &format!("* {uid} FETCH (UID {uid} BODY[TEXT] {{4}}\r\nbody)\r\n");
                                } else {
                                    out += &format!("* {uid} FETCH (UID {uid} FLAGS ())\r\n");
                                }
                            }
                        } else if cmd == "LOGOUT" { out += "* BYE bye\r\n"; }
                        out += &format!("{tag} OK done\r\n");
                        if w.write_all(out.as_bytes()).await.is_err() { return; }
                    }
                });
            }
        });
        (port, server)
    }
    fn fake_account(port: u16) -> Account {
        let config = ServerConfig { host: "127.0.0.1".into(), port, security: Security::Plain, username: "u".into() };
        Account::new("u@example.com".into(), config.clone(), config).unwrap()
    }
    #[tokio::test]
    async fn end_of_file_after_a_row_keeps_the_checkpoint() {
        let (port, server) = fake(true).await;
        let account = fake_account(port);
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("mail.db")).unwrap();
        store.save_account(&account).unwrap();
        let mut imap = ImapSync::login(&account, "pw").await.unwrap();
        let result = imap.sync(&mut store, DEFAULT_LIMIT).await;
        assert!(matches!(&result, Err(Error::Folder { source, .. }) if matches!(**source, Error::Network(DROPPED))), "{result:?}");
        let inbox = store.folders(&account.id, 0).unwrap().into_iter().find(|f| f.name == "INBOX").unwrap();
        assert_eq!(inbox.last_uid, 0);
        assert!(store.list_messages(inbox.id, i64::MAX, 100, 0).unwrap().iter().all(|m| m.uid != 2));
        server.abort();
    }
    #[tokio::test]
    async fn unreadable_message_is_skipped_and_the_checkpoint_moves() {
        let (port, server) = fake(false).await;
        let account = fake_account(port);
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().join("mail.db")).unwrap();
        store.save_account(&account).unwrap();
        for _ in 0..2 {
            let mut imap = ImapSync::login(&account, "pw").await.unwrap();
            imap.sync(&mut store, DEFAULT_LIMIT).await.unwrap();
            imap.logout().await.unwrap();
            let inbox = store.folders(&account.id, 0).unwrap().into_iter().find(|f| f.name == "INBOX").unwrap();
            assert_eq!(inbox.last_uid, 3);
            let mut stored: Vec<_> = store.list_messages(inbox.id, i64::MAX, 100, 0).unwrap().into_iter().map(|m| (m.uid, m.subject)).collect();
            stored.sort();
            assert_eq!(stored, [(1, "mail 1".into()), (2, "(message could not be read)".into()), (3, "mail 3".into())]);
        }
        server.abort();
    }

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
        store
            .save_synced_message(&crate::store::Message {
                summary: crate::store::MessageSummary {
                    account: account.id.clone(),
                    folder: inbox,
                    uid: 1,
                    ..Default::default()
                },
                ..Default::default()
            }, &[])
            .unwrap();
        let id = store.list_messages(inbox, 0, 1, 0).unwrap()[0].id;
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

fn listed(name: &str, attrs: &[NameAttribute]) -> bool {
    selectable(attrs) && !attrs.iter().any(|a| match a {
        NameAttribute::Flagged => true,
        NameAttribute::Extension(x) => x.eq_ignore_ascii_case("\\Important"),
        _ => false,
    }) && !matches!(name.to_ascii_lowercase().as_str(),
        "[gmail]/important" | "[gmail]/starred" | "[googlemail]/important" | "[googlemail]/starred")
}

/// RFC 5258: `\NonExistent` implies `\Noselect`; Gmail sends it for `[Gmail]`.
fn selectable(attrs: &[NameAttribute]) -> bool {
    !attrs.iter().any(|a| match a {
        NameAttribute::NoSelect => true,
        NameAttribute::Extension(x) => x.eq_ignore_ascii_case("\\NonExistent"),
        _ => false,
    })
}

#[cfg(test)]
mod selectable_tests {
    use super::*;
    #[test]
    fn nonexistent_and_noselect_are_skipped() {
        assert!(!selectable(&[NameAttribute::Extension("\\HasChildren".into()), NameAttribute::Extension("\\NonExistent".into())]));
        assert!(!selectable(&[NameAttribute::NoSelect]));
        assert!(selectable(&[NameAttribute::Extension("\\HasNoChildren".into()), NameAttribute::Sent]));
    }
}

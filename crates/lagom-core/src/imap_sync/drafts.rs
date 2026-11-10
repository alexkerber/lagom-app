use super::*;
use crate::drafts::{fingerprint, same_upload};
use async_imap::imap_proto::{AttributeValue, Response, Status};
use rusqlite::OptionalExtension;

impl ImapSync {
    async fn draft_raw(&mut self, uid: u32) -> Result<Option<(Vec<u8>, bool, bool)>> {
        // shortcut: draft MIME is capped at 64 MiB, stream larger drafts before raising this bound.
        // Bound the complete response, not just a server-supplied RFC822.SIZE.
        self.session.get_mut().remaining = Some(MAX_LITERAL + 65536);
        let result = tokio::time::timeout(TIMEOUT, async {
            let tag = net(self.session.run_command(format!("UID FETCH {uid} (UID FLAGS BODY.PEEK[])"))).await?;
            let mut raw = None;
            let mut deleted = false;
            let mut draft = false;
            loop {
                let response = net(self.session.read_response()).await?.ok_or(Error::Network("draft fetch interrupted"))?;
                match response.parsed() {
                    Response::Fetch(_, attrs) if attrs.iter().any(|a| matches!(a, AttributeValue::Uid(value) if *value==uid)) => {
                        for attr in attrs {
                            match attr {
                                AttributeValue::BodySection { section: None, index: None, data: Some(data) } => raw = Some(data.to_vec()),
                                AttributeValue::Flags(flags) => {
                                    deleted = flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"));
                                    draft = flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"));
                                }
                                _ => {}
                            }
                        }
                    }
                    Response::Done { tag: done, status, .. } if *done == tag => {
                        return if *status == Status::Ok { Ok(raw.map(|raw| (raw, deleted, draft))) }
                            else { Err(Error::Network("draft fetch failed")) };
                    }
                    _ => {}
                }
            }
        }).await.map_err(|_| Error::Network("draft fetch timed out"))?;
        self.session.get_mut().remaining = None;
        result
    }

    pub(super) async fn read_drafts(&mut self, store: &Store, folder: &Folder, limit: usize) -> Result<()> {
        let validity = folder.uidvalidity.ok_or(Error::Network("IMAP UIDVALIDITY missing"))?;
        let present = self.search_uids("UNDELETED").await?;
        let pending = store.pending_draft_uploads(&self.account.id)?;
        let mut sorted: Vec<_> = present.iter().copied().collect();
        sorted.sort_unstable();
        let mut fetched = 0;
        for uid in sorted {
            if store.known_draft_uid(&self.account.id, &folder.name, validity, uid)? { continue; }
            if fetched == limit { break; }
            fetched += 1;
            let Some((raw, false, _)) = self.draft_raw(uid).await? else { continue };
            // An APPEND whose reply was lost belongs to its durable pending upload.
            let mut uploading = false;
            for staged in &pending { uploading |= same_upload(&raw, staged)?; }
            if !uploading { store.import_draft(&self.account.id, &folder.name, validity, uid, &raw)?; }
        }
        store.reconcile_drafts(&self.account.id, &folder.name, validity, &present)
    }

    /// The caller persists backoff too when login fails, before a session is available.
    pub async fn sync_draft(&mut self, store: &mut Store, id: &str, now: i64) -> Result<()> {
        let account: Option<String> = store.db.query_row("SELECT account FROM draft_sync WHERE id=?1", [id], |r| r.get(0)).optional()?;
        let Some(account) = account else { return Ok(()) };
        if account != self.account.id { return Err(Error::Invalid("draft belongs to another account")); }
        if !store.due_drafts(&account, now)?.iter().any(|due| due == id) { return Ok(()); }
        store.stage_draft(&self.account, id)?;
        if let Some(upload) = store.draft_upload(id)? {
            let fallback = if presets::is_gmail(&self.account) { "[Gmail]/Drafts" } else { "Drafts" };
            let folder_id = self.ensure_role(store, FolderRole::Drafts, fallback).await?;
            let (folder, _) = self.select(store, folder_id).await?;
            let validity = folder.uidvalidity.ok_or(Error::Network("IMAP UIDVALIDITY missing"))?;
            // The token is generated locally and survives uncertain APPEND results and restarts.
            if upload.token.len() != 32 || !upload.token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::Invalid("invalid draft upload token"));
            }
            let query = format!("UNDELETED HEADER Message-ID \"<{}@draft.lagommail.app>\"", upload.token);
            let mut found = Vec::new();
            for uid in self.search_uids(&query).await? {
                if let Some((raw, false, true)) = self.draft_raw(uid).await? {
                    if same_upload(&raw, &upload.raw)? { found.push((uid, raw)); }
                }
            }
            if found.is_empty() && store.draft_exists(id)? {
                net(self.session.append(&encode_mailbox(&folder.name), Some("(\\Draft)"), None, &upload.raw)).await?;
                for uid in self.search_uids(&query).await? {
                    if let Some((raw, false, true)) = self.draft_raw(uid).await? {
                        if same_upload(&raw, &upload.raw)? { found.push((uid, raw)); }
                    }
                }
                if found.is_empty() { return Err(Error::Network("uploaded draft not found; retry pending")); }
            }
            if !found.is_empty() && self.select(store, folder_id).await?.0.uidvalidity != Some(validity) {
                return Err(Error::Network("draft mailbox changed during upload; retry pending"));
            }
            if !found.is_empty() { store.record_draft_upload(id, &upload, &folder.name, validity, &found, now)?; }
            if !store.draft_exists(id)? {
                store.db.execute("UPDATE draft_sync SET pending_raw=NULL,pending_revision=NULL,pending_token=NULL WHERE id=?1", [id])?;
            }
        }
        let revision = store.synced_draft_revision(id)?;
        let copies = store.draft_copies(id)?;
        if !copies.is_empty() { self.list_folders(store).await?; }
        let keep = copies.iter().filter(|copy| copy.revision == revision).max_by_key(|copy| copy.uid);
        for copy in &copies {
            if store.draft_exists(id)? && keep.is_some_and(|keep| std::ptr::eq(keep, copy)) { continue; }
            let folder = store.folders(&account, 0)?.into_iter().find(|f| f.name == copy.folder);
            if let Some(folder) = folder {
                let (folder, _) = self.select(store, folder.id).await?;
                if folder.uidvalidity == Some(copy.validity) {
                    if let Some((raw, _, _)) = self.draft_raw(copy.uid).await? {
                        // IMAP bodies are immutable per UID; a changed body or validity is another copy.
                        if fingerprint(&raw) == copy.hash {
                            net(self.session.run_command_and_check_ok(format!("UID STORE {} +FLAGS.SILENT (\\Deleted)", copy.uid))).await?;
                            if self.capabilities.has_str("UIDPLUS") {
                                net(self.session.run_command_and_check_ok(format!("UID EXPUNGE {}", copy.uid))).await?;
                            }
                            // shortcut: servers without UIDPLUS retain Deleted copies, purge them when UID-specific expunge is supported.
                        }
                    }
                }
            }
            store.forget_draft_copy(id, copy)?;
        }
        store.finish_draft_sync(id)
    }
}

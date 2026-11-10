use crate::{outbox::Draft, store::Store, Error, Result};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

pub(crate) fn fingerprint(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}

fn parse_draft(raw: &[u8]) -> Result<Draft> {
    let parsed = mail_parser::MessageParser::default().parse(raw).ok_or(Error::Invalid("invalid draft MIME"))?;
    let addresses = |list: Option<mail_parser::AddressList<'_>>| list.map(|list| list.mailboxes().filter_map(|m|
        m.address().map(|address| crate::contacts::format_recipient(m.name().unwrap_or_default(), address))).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let in_reply_to = parsed.in_reply_to().and_then(|ids| ids.first()).map(|id| format!("<{id}>"));
    let references: Vec<_> = parsed.references().into_iter().flat_map(|ids| ids.iter()).map(|id| format!("<{id}>")).collect();
    let reply = (in_reply_to.is_some() || !references.is_empty()).then_some((in_reply_to, references));
    Ok(Draft {
        to: addresses(parsed.to()), cc: addresses(parsed.cc()), bcc: addresses(parsed.bcc()), subject: parsed.subject().unwrap_or_default().into(),
        // shortcut: foreign HTML becomes text, retain HTML when rich-text editing exists.
        text: parsed.text_bodies().map(|text| text.into_owned()).collect::<Vec<_>>().join("\n"),
        reply,
        attachments: parsed.attachments().map(|part| crate::smtp::OutgoingAttachment {
            filename: part.attachment_name().unwrap_or("attachment").into(),
            mime_type: part.content_type().map(|ty| ty.subtype().map_or_else(|| ty.ctype().to_string(), |sub| format!("{}/{sub}", ty.ctype())))
                .unwrap_or_else(|| "application/octet-stream".into()), data: part.decoded().into_owned(),
        }).collect(),
        ..Default::default()
    })
}

pub(crate) fn same_upload(raw: &[u8], staged: &[u8]) -> Result<bool> {
    if raw == staged { return Ok(true); }
    let actual_message = mail_parser::MessageParser::default().parse(raw).ok_or(Error::Invalid("invalid draft MIME"))?;
    let expected_message = mail_parser::MessageParser::default().parse(staged).ok_or(Error::Invalid("invalid draft MIME"))?;
    if actual_message.from() != expected_message.from() || actual_message.sender() != expected_message.sender()
        || actual_message.reply_to() != expected_message.reply_to() { return Ok(false); }
    let message_id = |raw: &[u8]| mail_parser::MessageParser::default().parse(raw)
        .and_then(|m| m.message_id().map(str::to_owned));
    let html = |raw: &[u8]| mail_parser::MessageParser::default().parse(raw).map(|m|
        m.html_body().filter_map(|part| part.text().map(|text| text.replace("\r\n", "\n"))).collect::<Vec<_>>());
    let mut actual = parse_draft(raw)?;
    let mut expected = parse_draft(staged)?;
    actual.text = actual.text.replace("\r\n", "\n");
    expected.text = expected.text.replace("\r\n", "\n");
    Ok(message_id(raw) == message_id(staged) && html(raw) == html(staged) && actual == expected)
}

pub(crate) struct Upload {
    pub revision: String,
    pub token: String,
    pub raw: Vec<u8>,
}

pub(crate) struct Copy {
    pub folder: String,
    pub validity: u32,
    pub uid: u32,
    pub hash: String,
    pub revision: String,
}

impl Store {
    pub(crate) fn draft_exists(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM drafts WHERE id=?1)", [id], |r| r.get(0))?)
    }

    /// List refreshes must not decode every draft's attachment bytes.
    pub fn draft_summaries(&self) -> Result<Vec<crate::outbox::DraftSummary>> {
        let mut query = self.db.prepare("SELECT id,account,json_extract(content,'$.to'),json_extract(content,'$.subject'),
            substr(json_extract(content,'$.text'),1,800) FROM drafts ORDER BY rowid")?;
        let rows = query.query_map([], |r| Ok(crate::outbox::DraftSummary {
            id: r.get(0)?, account: r.get(1)?, to: r.get(2)?, subject: r.get(3)?, snippet: r.get(4)?,
        }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn draft(&self, id: &str) -> Result<Draft> {
        let content: String = self.db.query_row("SELECT content FROM drafts WHERE id=?1", [id], |r| r.get(0))
            .optional()?.ok_or(Error::NotFound)?;
        serde_json::from_str(&content).map_err(|_| Error::Invalid("could not read local draft"))
    }

    /// Opening a foreign draft pins its local text before another client can remove it.
    pub fn take_draft(&self, id: &str) -> Result<Draft> {
        let tx = self.db.unchecked_transaction()?;
        let draft = self.draft(id)?;
        self.db.execute("UPDATE draft_sync SET owned=1 WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(draft)
    }

    pub fn owned_drafts(&self) -> Result<Vec<Draft>> {
        let mut query = self.db.prepare("SELECT d.content FROM drafts d JOIN draft_sync s ON s.id=d.id WHERE s.owned=1 ORDER BY d.rowid")?;
        let rows = query.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| serde_json::from_str(&row?).map_err(|_| Error::Invalid("could not read local draft"))).collect()
    }

    pub fn due_drafts(&self, account: &str, now: i64) -> Result<Vec<String>> {
        self.db.execute("DELETE FROM draft_sync WHERE account=?1 AND pending_raw IS NULL
            AND NOT EXISTS(SELECT 1 FROM drafts d WHERE d.id=draft_sync.id)
            AND NOT EXISTS(SELECT 1 FROM draft_copies c WHERE c.id=draft_sync.id)", [account])?;
        let mut query = self.db.prepare("SELECT s.id FROM draft_sync s LEFT JOIN drafts d ON d.id=s.id
            WHERE s.account=?1 AND s.ready_at<=?2 AND (d.id IS NULL OR d.revision<>s.revision OR s.pending_raw IS NOT NULL
                OR EXISTS (SELECT 1 FROM draft_copies c WHERE c.id=s.id AND c.revision<>s.revision)
                OR (SELECT count(*) FROM draft_copies c WHERE c.id=s.id)>1) ORDER BY s.rowid")?;
        let rows = query.query_map(params![account, now], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Closing flushes the throttle, but never bypasses failure backoff.
    pub fn flush_draft(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE draft_sync SET ready_at=0 WHERE id=?1 AND attempts=0", [id])?;
        Ok(())
    }

    pub fn draft_sync_failed(&self, id: &str, error: &Error, now: i64) -> Result<()> {
        self.db.execute("UPDATE draft_sync SET error=?2,ready_at=?3+min(60000*(1 << min(attempts,6)),3600000),attempts=attempts+1 WHERE id=?1",
            params![id, error.to_string(), now])?;
        Ok(())
    }

    pub fn draft_sync_error(&self, id: &str) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT error FROM draft_sync WHERE id=?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    pub(crate) fn pending_draft_uploads(&self, account: &str) -> Result<Vec<Vec<u8>>> {
        let mut query = self.db.prepare("SELECT pending_raw FROM draft_sync WHERE account=?1 AND pending_raw IS NOT NULL")?;
        let rows = query.query_map([account], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub(crate) fn draft_upload(&self, id: &str) -> Result<Option<Upload>> {
        Ok(self.db.query_row("SELECT pending_revision,pending_token,pending_raw FROM draft_sync WHERE id=?1 AND pending_raw IS NOT NULL", [id], |r| {
            Ok(Upload { revision: r.get(0)?, token: r.get(1)?, raw: r.get(2)? })
        }).optional()?)
    }

    pub(crate) fn stage_draft(&self, account: &crate::account::Account, id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let dirty: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM drafts d JOIN draft_sync s ON d.id=s.id
            WHERE d.id=?1 AND d.account=?2 AND d.revision<>s.revision AND s.pending_raw IS NULL)", params![id, account.id], |r| r.get(0))?;
        if dirty {
            let draft = self.draft(id)?;
            if draft.forward_source.is_some() { return Err(Error::Invalid("draft attachments are still loading")); }
            let token = uuid::Uuid::new_v4().simple().to_string();
            let raw = crate::smtp::compose_draft(account, &draft, &token)?;
            self.db.execute("UPDATE draft_sync SET pending_revision=(SELECT revision FROM drafts WHERE id=?1),pending_token=?2,pending_raw=?3 WHERE id=?1",
                params![id, token, raw])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn draft_copies(&self, id: &str) -> Result<Vec<Copy>> {
        let mut query = self.db.prepare("SELECT folder,validity,uid,hash,revision FROM draft_copies WHERE id=?1")?;
        let rows = query.query_map([id], |r| Ok(Copy { folder: r.get(0)?, validity: r.get(1)?, uid: r.get(2)?, hash: r.get(3)?, revision: r.get(4)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub(crate) fn record_draft_upload(&self, id: &str, upload: &Upload, folder: &str, validity: u32, copies: &[(u32, Vec<u8>)], now: i64) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for (uid, raw) in copies {
            self.db.execute("INSERT OR REPLACE INTO draft_copies VALUES (?1,?2,?3,?4,?5,?6)",
                params![id, folder, validity, uid, fingerprint(raw), upload.revision])?;
        }
        self.db.execute("UPDATE draft_sync SET revision=?2,pending_revision=NULL,pending_token=NULL,pending_raw=NULL,
            ready_at=?3+10000,attempts=0,error=NULL WHERE id=?1", params![id, upload.revision, now])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn forget_draft_copy(&self, id: &str, copy: &Copy) -> Result<()> {
        self.db.execute("DELETE FROM draft_copies WHERE id=?1 AND folder=?2 AND validity=?3 AND uid=?4",
            params![id, copy.folder, copy.validity, copy.uid])?;
        Ok(())
    }

    pub(crate) fn finish_draft_sync(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE draft_sync SET attempts=0,error=NULL WHERE id=?1", [id])?;
        self.db.execute("DELETE FROM draft_sync WHERE id=?1 AND NOT EXISTS (SELECT 1 FROM drafts WHERE id=?1)
            AND pending_raw IS NULL AND NOT EXISTS (SELECT 1 FROM draft_copies WHERE id=?1)", [id])?;
        Ok(())
    }

    pub(crate) fn synced_draft_revision(&self, id: &str) -> Result<String> {
        Ok(self.db.query_row("SELECT revision FROM draft_sync WHERE id=?1", [id], |r| r.get(0))?)
    }

    pub(crate) fn import_draft(&self, account: &str, folder: &str, validity: u32, uid: u32, raw: &[u8]) -> Result<()> {
        let draft = Draft { id: crate::smtp::message_id("draft@lagommail.app"), account: account.into(), ..parse_draft(raw)? };
        let tx = self.db.unchecked_transaction()?;
        self.save_draft(&draft)?;
        let revision = self.draft_revision(&draft.id)?;
        self.db.execute("UPDATE draft_sync SET revision=?2,owned=0 WHERE id=?1", params![draft.id, revision])?;
        self.db.execute("INSERT INTO draft_copies VALUES (?1,?2,?3,?4,?5,?6)", params![draft.id, folder, validity, uid, fingerprint(raw), revision])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn known_draft_uid(&self, account: &str, folder: &str, validity: u32, uid: u32) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM draft_copies c JOIN draft_sync s ON c.id=s.id
            WHERE s.account=?1 AND c.folder=?2 AND c.validity=?3 AND c.uid=?4)", params![account, folder, validity, uid], |r| r.get(0))?)
    }

    pub(crate) fn reconcile_drafts(&self, account: &str, folder: &str, validity: u32, present: &std::collections::HashSet<u32>) -> Result<()> {
        let mut query = self.db.prepare("SELECT c.id,c.validity,c.uid FROM draft_copies c JOIN draft_sync s ON c.id=s.id WHERE s.account=?1 AND c.folder=?2")?;
        let rows = query.query_map(params![account, folder], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let tx = self.db.unchecked_transaction()?;
        for (id, old_validity, uid) in rows {
            if old_validity == validity && present.contains(&uid) { continue; }
            self.db.execute("DELETE FROM drafts WHERE id=?1 AND EXISTS(SELECT 1 FROM draft_sync s WHERE s.id=?1 AND s.owned=0 AND s.revision=drafts.revision)", [&id])?;
            self.db.execute("DELETE FROM draft_copies WHERE id=?1 AND folder=?2 AND validity=?3 AND uid=?4", params![id, folder, old_validity, uid])?;
            self.db.execute("UPDATE draft_sync SET revision='' WHERE id=?1", [&id])?;
        }
        tx.commit()?;
        Ok(())
    }
}

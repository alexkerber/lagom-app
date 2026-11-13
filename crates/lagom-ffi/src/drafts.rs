use super::*;
use lagom_core::outbox::Draft;
use rusqlite::OptionalExtension;

#[derive(Clone, Debug, uniffi::Record)]
pub struct LocalDraft {
    pub id: String, pub account: String, pub to: String, pub cc: String,
    pub subject: String, pub body: String, pub in_reply_to: Option<String>, pub references: Vec<String>,
    pub attachments: Vec<SendAttachment>,
    /// Unix seconds; zero means an older draft has no recorded edit time.
    pub updated_unix: i64,
}

fn metadata(store: &Store) -> lagom_core::Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open(store.path())?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(db)
}

pub(super) fn install(store: &Store) -> lagom_core::Result<()> {
    // Core owns draft content; triggers timestamp its writes in the same transaction, including Undo.
    metadata(store)?.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS mobile_draft_times (id TEXT PRIMARY KEY REFERENCES drafts(id) ON DELETE CASCADE, updated INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS mobile_draft_insert AFTER INSERT ON drafts BEGIN
            INSERT INTO mobile_draft_times VALUES (new.id,unixepoch()) ON CONFLICT(id) DO UPDATE SET updated=excluded.updated;
        END;
        CREATE TRIGGER IF NOT EXISTS mobile_draft_update AFTER UPDATE OF content ON drafts WHEN new.content<>old.content BEGIN
            INSERT INTO mobile_draft_times VALUES (new.id,unixepoch()) ON CONFLICT(id) DO UPDATE SET updated=excluded.updated;
        END;
        COMMIT;")?;
    Ok(())
}

fn record(store: &Store, draft: Draft) -> lagom_core::Result<LocalDraft> {
    let updated_unix = metadata(store)?.query_row("SELECT updated FROM mobile_draft_times WHERE id=?1", [&draft.id], |r| r.get(0)).optional()?.unwrap_or(0);
    let (in_reply_to, references) = draft.reply.unwrap_or_default();
    Ok(LocalDraft { id: draft.id, account: draft.account, to: draft.to, cc: draft.cc,
        subject: draft.subject, body: draft.text, in_reply_to, references,
        attachments: draft.attachments.into_iter().map(|a| SendAttachment { filename: a.filename, mime_type: a.mime_type, data: a.data }).collect(), updated_unix })
}

#[uniffi::export(async_runtime = "tokio")]
impl LagomCore {
    pub async fn local_drafts(&self) -> Result<Vec<LocalDraft>> {
        self.run(|s| {
            let mut drafts = s.owned_drafts()?.into_iter().map(|d| record(s, d)).collect::<lagom_core::Result<Vec<_>>>()?;
            drafts.sort_by_key(|d| std::cmp::Reverse(d.updated_unix));
            Ok(drafts)
        }).await
    }

    pub async fn saved_reply(&self, id: i64) -> Result<Option<LocalDraft>> {
        self.run(move |s| {
            let message = s.message_summary(id)?;
            if message.message_id.is_empty() { return Ok(None); }
            let parent = format!("<{}>", message.message_id.trim_matches(['<', '>']));
            s.owned_drafts()?.into_iter().rev().find(|d| d.account == message.account
                && d.reply.as_ref().and_then(|(id, _)| id.as_deref()) == Some(&parent))
                .map(|d| record(s, d)).transpose()
        }).await
    }

    /// Incomplete recipient text is valid in a draft. Validation belongs to enqueue.
    pub async fn save_draft(&self, id: String, account_id: String, to: String, cc: String, subject: String,
        body: String, reply_to: Option<i64>, attachments: Vec<SendAttachment>) -> Result<LocalDraft> {
        self.run(move |s| {
            account(s, &account_id)?;
            if id.is_empty() || id.contains(['\r', '\n']) { return Err(lagom_core::Error::Invalid("invalid draft Message-ID")); }
            // A queued Message-ID must never be resurrected by a delayed autosave.
            match s.pending(&id) {
                Ok(_) => return Err(lagom_core::Error::Invalid("draft is already queued")),
                Err(lagom_core::Error::NotFound) => {},
                Err(error) => return Err(error),
            }
            let mut draft = match s.draft(&id) {
                Ok(draft) => {
                    if draft.account != account_id { return Err(lagom_core::Error::Invalid("draft belongs to another account")); }
                    draft
                },
                Err(lagom_core::Error::NotFound) => {
                    let mut draft = Draft { id, account: account_id, ..Default::default() };
                    if let Some(id) = reply_to {
                        let m = s.message_summary(id)?;
                        if m.account != draft.account { return Err(lagom_core::Error::Invalid("reply belongs to another account")); }
                        if !m.message_id.is_empty() {
                            let parent = format!("<{}>", m.message_id.trim_matches(['<', '>']));
                            let mut refs: Vec<_> = m.thread_refs.split_whitespace().map(|r| format!("<{}>", r.trim_matches(['<', '>']))).collect();
                            refs.push(parent.clone());
                            draft.reply = Some((Some(parent), refs));
                        }
                    }
                    draft
                },
                Err(error) => return Err(error),
            };
            let [old_to, old_cc] = draft.recipient_tokens.take().unwrap_or_else(|| [split_recipients(draft.to.clone()), split_recipients(draft.cc.clone())]);
            draft.recipient_tokens = Some([
                if to == draft.to { old_to } else { split_recipients(to.clone()) },
                if cc == draft.cc { old_cc } else { split_recipients(cc.clone()) },
            ]);
            draft.to = to; draft.cc = cc; draft.subject = subject; draft.text = body;
            draft.attachments = attachments.into_iter().map(|a| smtp::OutgoingAttachment { filename: a.filename, mime_type: a.mime_type, data: a.data }).collect();
            s.save_draft(&draft)?;
            record(s, draft)
        }).await
    }

    pub async fn delete_draft(&self, id: String) -> Result<()> {
        self.run(move |s| s.delete_draft(&id)).await
    }

    /// Enqueue consumes the saved Message-ID and deletes its draft in one core transaction.
    pub async fn send_draft(&self, id: String) -> Result<String> {
        self.run(move |s| {
            let draft = s.draft(&id)?;
            if draft.forward_source.is_some() { return Err(lagom_core::Error::Invalid("draft attachments are still loading")); }
            let a = account(s, &draft.account)?;
            let [to, cc] = draft.recipient_tokens.unwrap_or_else(|| [split_recipients(draft.to), split_recipients(draft.cc)]);
            let (in_reply_to, references) = draft.reply.unwrap_or_default();
            s.enqueue(&a, &Outgoing { to, cc, bcc: split_recipients(draft.bcc), subject: draft.subject, text: draft.text,
                message_id: Some(id), in_reply_to, references, attachments: draft.attachments }, milliseconds())
        }).await
    }
}

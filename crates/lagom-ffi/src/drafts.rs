use super::*;
use lagom_core::outbox::Draft;

#[derive(Clone, Debug, uniffi::Record)]
pub struct LocalDraft {
    pub id: String, pub account: String, pub to: String, pub cc: String,
    pub subject: String, pub body: String, pub in_reply_to: Option<String>, pub references: Vec<String>,
    pub attachments: Vec<SendAttachment>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct DraftRow {
    pub id: String, pub account: String, pub to: String, pub subject: String,
    /// Unix seconds; zero means an older draft has no recorded edit time.
    pub updated_unix: i64,
}

impl From<lagom_core::outbox::OwnedDraft> for DraftRow {
    fn from(d: lagom_core::outbox::OwnedDraft) -> Self {
        Self { id: d.id, account: d.account, to: d.to, subject: d.subject, updated_unix: d.updated }
    }
}

fn record(draft: Draft) -> LocalDraft {
    let (in_reply_to, references) = draft.reply.unwrap_or_default();
    LocalDraft { id: draft.id, account: draft.account, to: draft.to, cc: draft.cc,
        subject: draft.subject, body: draft.text, in_reply_to, references,
        attachments: draft.attachments.into_iter().map(|a| SendAttachment { filename: a.filename, mime_type: a.mime_type, data: a.data }).collect() }
}

#[uniffi::export(async_runtime = "tokio")]
impl LagomCore {
    pub async fn local_drafts(&self) -> Result<Vec<DraftRow>> {
        self.run(|s| Ok(s.owned_draft_rows()?.into_iter().map(Into::into).collect())).await
    }

    pub async fn local_draft(&self, id: String) -> Result<LocalDraft> {
        self.run(move |s| Ok(record(s.draft(&id)?))).await
    }

    pub async fn saved_reply(&self, id: i64) -> Result<Option<LocalDraft>> {
        self.run(move |s| {
            let message = s.message_summary(id)?;
            if message.message_id.is_empty() { return Ok(None); }
            let parent = format!("<{}>", message.message_id.trim_matches(['<', '>']));
            s.owned_draft_rows()?.into_iter().find(|d| d.account == message.account
                && d.in_reply_to.as_deref() == Some(&parent))
                .map(|d| s.draft(&d.id).map(record)).transpose()
        }).await
    }

    /// Incomplete recipient text is valid in a draft. Validation belongs to enqueue.
    pub async fn save_draft(&self, id: String, account_id: String, to: String, cc: String, subject: String,
        body: String, reply_to: Option<i64>, attachments: Vec<SendAttachment>) -> Result<DraftRow> {
        self.run(move |s| {
            account(s, &account_id)?;
            if id.is_empty() || id.contains(['\r', '\n']) { return Err(lagom_core::Error::Invalid("invalid draft Message-ID")); }
            // shortcut: this guard lasts only while the outbox row exists; clients block late saves after SMTP success, add tombstones if other writers need protection.
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
                        let message = match s.message_summary(id) {
                            Ok(message) => Some(message),
                            Err(lagom_core::Error::NotFound) => None,
                            Err(error) => return Err(error),
                        };
                        if let Some(m) = message {
                            if m.account != draft.account { return Err(lagom_core::Error::Invalid("reply belongs to another account")); }
                            if !m.message_id.is_empty() {
                                let parent = format!("<{}>", m.message_id.trim_matches(['<', '>']));
                                let mut refs: Vec<_> = m.thread_refs.split_whitespace().map(|r| format!("<{}>", r.trim_matches(['<', '>']))).collect();
                                refs.push(parent.clone());
                                draft.reply = Some((Some(parent), refs));
                            }
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
            Ok(s.draft_row(&draft.id)?.into())
        }).await
    }

    pub async fn delete_draft(&self, id: String) -> Result<()> {
        self.run(move |s| s.delete_draft(&id)).await
    }

    pub async fn undo_send(&self, id: String) -> Result<bool> {
        self.run(move |s| s.undo_send(&id)).await
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
                message_id: Some(id), in_reply_to, references, attachments: draft.attachments }, milliseconds() + 5000)
        }).await
    }
}

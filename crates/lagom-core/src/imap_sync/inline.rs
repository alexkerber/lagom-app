use super::*;
use crate::inline::{self, InlinePart, MAX_INLINE_BYTES};

fn parts(body: &BodyStructure<'_>, path: &[u32], ids: &HashSet<String>, remaining: &mut usize, out: &mut Vec<(String, Attachment)>) {
    if path.len() > 32 { return; }
    match body {
        BodyStructure::Multipart { bodies, common, .. } => {
            if common.disposition.as_ref().is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment")) { return; }
            for (index, body) in bodies.iter().enumerate() {
                let mut path = path.to_vec();
                path.push(index as u32 + 1);
                parts(body, &path, ids, remaining, out);
            }
        }
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } => {
            let Some(cid) = other.id.as_deref().map(inline::content_id).filter(|cid| ids.contains(*cid)) else { return };
            // Bound wire bytes too, so base64 overhead never bypasses the download budget.
            if other.octets as usize > *remaining || out.iter().any(|(id, _)| id == cid) { return; }
            *remaining -= other.octets as usize;
            out.push((cid.into(), Attachment {
                section: if path.is_empty() { "1".into() } else { path.iter().map(u32::to_string).collect::<Vec<_>>().join(".") },
                mime_type: format!("{}/{}", common.ty.ty, common.ty.subtype).to_ascii_lowercase(),
                size: other.octets,
                encoding: encoding_name(&other.transfer_encoding),
                ..Default::default()
            }));
        }
        _ => {}
    }
}

impl ImapSync {
    pub async fn fetch_inline(&mut self, store: &mut Store, id: i64) -> Result<()> {
        if store.inline_loaded(id)? { return Ok(()); }
        // Pending moves change the visible folder before the server folder/UID.
        let stored = store.message(id)?;
        let message = stored.summary;
        if message.account != self.account.id { return Err(Error::NotFound); }
        let folder = self.select_unchanged(store, message.folder, "mailbox changed; resync before loading inline images").await?;
        if stored.body_html.is_none() {
            // Sync left this body out (over 4 MiB, or synced before HTML parts were kept). Fetching
            // the message again also loads its inline parts.
            // async-imap drops a tagged NO: no reply is an error, so a later open tries again.
            self.refetch = true;
            let fetched = self.fetch_message(store, &folder, message.uid, mime::MAX_OPEN_BYTES).await;
            self.refetch = false;
            if !fetched? {
                return Err(Error::Network("the server did not return this message"));
            }
            // One attempt: a message still too large is not fetched on every open.
            if !store.inline_loaded(id)? { store.save_inline_parts(id, &[])?; }
            return Ok(());
        }
        let metadata = net(async {
            self.session.uid_fetch(message.uid.to_string(), "(UID BODYSTRUCTURE)").await?.try_collect::<Vec<_>>().await
        }).await?;
        let body = metadata.iter().find(|f| f.uid == Some(message.uid) && f.bodystructure().is_some())
            .and_then(|f| f.bodystructure()).ok_or(Error::Network("IMAP BODYSTRUCTURE missing"))?;
        let html = stored.body_html.unwrap_or_default();
        let parts = self.inline_parts(message.uid, body, &html).await?;
        store.save_inline_parts(id, &parts)
    }

    pub(super) async fn inline_parts(&mut self, uid: u32, body: &BodyStructure<'_>, html: &str) -> Result<Vec<InlinePart>> {
        let mut picks = Vec::new();
        let mut budget = MAX_INLINE_BYTES;
        parts(body, &[], &inline::referenced_ids(html), &mut budget, &mut picks);
        let mut out = Vec::new();
        let mut remaining = MAX_INLINE_BYTES;
        for (cid, part) in picks {
            let path = SectionPath::Part(part.section.split('.').map(|n| n.parse().unwrap()).collect(), None);
            self.session.get_mut().remaining = Some(part.size as usize + 65536);
            let fetched = net(async {
                let mut stream = self.session.uid_fetch(uid.to_string(), format!("(UID BODY.PEEK[{}])", part.section)).await?;
                let mut found = None;
                while let Some(fetch) = stream.try_next().await? {
                    if found.is_none() && fetch.uid == Some(uid) && fetch.section(&path).is_some() { found = Some(fetch); }
                }
                Ok::<_, async_imap::error::Error>(found)
            }).await?;
            self.session.get_mut().remaining = None;
            // A missing or resized part loses one image, not the message.
            let Some(bytes) = fetched.as_ref().and_then(|f| f.section(&path)) else { continue };
            if bytes.len() != part.size as usize { continue; }
            let Ok(data) = attachment::decode(bytes, &part.encoding) else { continue };
            if data.len() > remaining { continue; }
            remaining -= data.len();
            out.push(InlinePart { cid, mime_type: part.mime_type, data });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_parts_use_content_ids_and_share_an_encoded_size_budget() {
        use async_imap::imap_proto::{AttributeValue, Response};
        let wire = format!(r#"* 1 FETCH (BODYSTRUCTURE (("TEXT" "HTML" NIL NIL NIL "7BIT" 50 1)("IMAGE" "PNG" NIL "<large>" NIL "BASE64" {})("IMAGE" "PNG" NIL "<exact>" NIL "BASE64" {})("IMAGE" "PNG" NIL "<extra>" NIL "BASE64" 1)("IMAGE" "PNG" NIL "<unused>" NIL "BASE64" 1) "RELATED"))
"#, MAX_INLINE_BYTES + 1, MAX_INLINE_BYTES).replace('\n', "\r\n");
        let (_, Response::Fetch(_, attrs)) = Response::parse(wire.as_bytes()).unwrap() else { panic!("fetch") };
        let AttributeValue::BodyStructure(body) = &attrs[0] else { panic!("bodystructure") };
        let mut out = Vec::new();
        let mut remaining = MAX_INLINE_BYTES;
        parts(body, &[], &inline::referenced_ids("<img src='cid:large'><img src='cid:exact'><img src='cid:extra'>"), &mut remaining, &mut out);
        assert_eq!(out.iter().map(|(cid, part)| (cid.as_str(), part.section.as_str(), part.mime_type.as_str(), part.encoding.as_str())).collect::<Vec<_>>(), [("exact", "3", "image/png", "base64")]);
        assert_eq!(remaining, 0);
        out.clear();
        remaining = MAX_INLINE_BYTES;
        parts(body, &[], &inline::referenced_ids("<img src='cid:extra'><img src='cid:EXACT'>"), &mut remaining, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "extra");
        assert_eq!(remaining, MAX_INLINE_BYTES - 1);
    }
}

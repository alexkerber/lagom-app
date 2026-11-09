use super::{Page, oauth};
use crate::{account::Account, store::Store, tasks::Task, Error, Result};
use reqwest::Method;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashSet, future::Future, time::Duration};
use url::Url;
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct RemoteList {
    id: String,
    title: String,
    etag: Option<String>,
    updated: Option<String>,
}

#[derive(Deserialize)]
struct RemoteTask {
    id: String,
    #[serde(default)] title: String,
    #[serde(default)] notes: String,
    due: Option<String>,
    completed: Option<String>,
    #[serde(default)] status: String,
    #[serde(default)] deleted: bool,
    etag: Option<String>,
    updated: Option<String>,
}

struct List {
    id: i64,
    remote: Option<String>,
    name: String,
    dirty: bool,
    deleted: bool,
    revision: i64,
    cursor: Option<String>,
}

impl Store {
    fn google_lists(&self, account: &str) -> Result<Vec<List>> {
        let mut stmt = self.db.prepare("SELECT id,remote_id,name,dirty,deleted,revision,pulled_through FROM task_lists WHERE account=?1 ORDER BY position,id")?;
        let rows = stmt.query_map([account], |r| Ok(List { id: r.get(0)?, remote: r.get(1)?, name: r.get(2)?,
            dirty: r.get(3)?, deleted: r.get(4)?, revision: r.get(5)?, cursor: r.get(6)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn link_google_lists(&self, account: &str, remote: &[RemoteList]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        // Claim each local list once, even when two account jobs discover lists concurrently.
        for list in remote {
            if list.id.is_empty() { return Err(Error::Network("Google task list ID missing")); }
            let existing: Option<i64> = tx.query_row("SELECT id FROM task_lists WHERE account=?1 AND remote_id=?2", params![account, list.id], |r| r.get(0)).optional()?;
            if existing.is_some() { continue; }
            let local: Option<i64> = tx.query_row("SELECT id FROM task_lists WHERE ((account IS NULL AND detached=0) OR account=?1) AND remote_id IS NULL AND deleted=0 AND name=?2 ORDER BY id LIMIT 1", params![account, list.title], |r| r.get(0)).optional()?;
            if let Some(id) = local {
                tx.execute("UPDATE task_lists SET account=?2,remote_id=?3,etag=?4,remote_updated=?5 WHERE id=?1", params![id, account, list.id, list.etag, list.updated])?;
            } else {
                tx.execute("INSERT INTO task_lists(account,remote_id,name,position,dirty,etag,remote_updated) SELECT ?1,?2,?3,COALESCE(MAX(position),-1)+1,0,?4,?5 FROM task_lists", params![account, list.id, list.title, list.etag, list.updated])?;
            }
        }
        tx.execute("UPDATE task_lists SET account=?1 WHERE account IS NULL AND detached=0 AND deleted=0", [account])?;
        tx.commit()?;
        Ok(())
    }

    fn pending_google_tasks(&self, list: i64) -> Result<Vec<(Task, i64, Option<String>)>> {
        let mut stmt = self.db.prepare(&format!("SELECT {},revision,remote_list_id FROM tasks WHERE list=?1 AND (dirty=1 OR (remote_id IS NULL AND deleted=0)) ORDER BY position,id", crate::tasks::TASK_COLUMNS))?;
        let rows = stmt.query_map([list], |r| Ok((crate::tasks::task_row(r)?, r.get(15)?, r.get(16)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

pub async fn sync_tasks(store: &mut Store, account: &Account) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(90), sync_tasks_with(store, &account.id,
        "https://tasks.googleapis.com/tasks/v1/", |rejected| async move {
            oauth::access_token(account, rejected.as_deref()).await
        })).await.map_err(|_| Error::Network("Google Tasks sync timed out"))?
}

struct Client<F> { http: reqwest::Client, base: Url, token: F }
impl<F, Fut> Client<F>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    fn url(&self, path: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut().map_err(|_| Error::Invalid("invalid Google Tasks endpoint"))?.pop_if_empty().extend(path);
        Ok(url)
    }

    async fn request(&mut self, method: Method, url: Url, body: Option<Value>) -> Result<Value> {
        let mut access = (self.token)(None).await?;
        for attempt in 0..2 {
            let mut request = self.http.request(method.clone(), url.clone()).bearer_auth(access.as_str());
            if let Some(body) = &body { request = request.header("Content-Type", "application/json").body(body.to_string()); }
            let response = request.send().await.map_err(|_| Error::Network("Google Tasks unavailable"))?;
            let status = response.status();
            // A page can contain 100 notes of 8192 characters, including JSON escapes.
            let bytes = oauth::body_limit(response, 16 * 1024 * 1024).await?;
            if status == reqwest::StatusCode::UNAUTHORIZED {
                if attempt == 1 { return Err(Error::ReauthRequired); }
                access = (self.token)(Some(access.to_string())).await?;
                continue;
            }
            let list_delete = method == Method::DELETE && url.path().contains("/users/@me/lists/");
            if list_delete && status == reqwest::StatusCode::BAD_REQUEST { return Err(Error::Invalid("Google task list cannot be deleted")); }
            if status == reqwest::StatusCode::FORBIDDEN {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or_default();
                let scope = error["error"]["errors"].as_array().is_some_and(|e| e.iter().any(|e| e["reason"] == "insufficientPermissions"))
                    || error["error"]["details"].as_array().is_some_and(|e| e.iter().any(|e| e["reason"] == "ACCESS_TOKEN_SCOPE_INSUFFICIENT"));
                if list_delete && !scope && error["error"]["errors"].as_array().is_some_and(|e| e.iter().any(|e| e["reason"] == "forbidden")) {
                    return Err(Error::Invalid("Google task list cannot be deleted"));
                }
                return Err(if scope { Error::TasksConsent } else { Error::Network("Google Tasks access refused") });
            }
            if method == Method::DELETE && (status.is_success() || status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE) { return Ok(Value::Null); }
            if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE { return Err(Error::NotFound); }
            if !status.is_success() { return Err(Error::Network("Google Tasks request failed")); }
            return serde_json::from_slice(&bytes).map_err(|_| Error::Network("invalid Google Tasks response"));
        }
        Err(Error::ReauthRequired)
    }

    async fn pages<T: serde::de::DeserializeOwned>(&mut self, url: Url) -> Result<Vec<T>> {
        let mut result = Vec::new();
        let mut next: Option<String> = None;
        let mut seen = HashSet::new();
        loop {
            let mut url = url.clone();
            if let Some(page) = &next { url.query_pairs_mut().append_pair("pageToken", page); }
            let page: Page<T> = serde_json::from_value(self.request(Method::GET, url, None).await?)
                .map_err(|_| Error::Network("invalid Google Tasks page"))?;
            result.extend(page.items);
            next = page.next_page_token.filter(|s| !s.is_empty());
            let Some(page) = &next else { return Ok(result) };
            if !seen.insert(page.clone()) { return Err(Error::Network("repeated Google Tasks page")); }
        }
    }
}

pub async fn sync_tasks_with<F, Fut>(store: &mut Store, account: &str, base: &str, token: F) -> Result<()>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    let mut client = Client { http: oauth::http(Duration::from_secs(30))?,
        base: Url::parse(base).map_err(|_| Error::Invalid("invalid Google Tasks endpoint"))?, token };
    let result = sync(store, account, &mut client).await;
    if matches!(result, Err(Error::TasksConsent)) {
        let scopes = store.account_scopes(account)?.split_whitespace().filter(|s| *s != oauth::GOOGLE_TASKS_SCOPE).collect::<Vec<_>>().join(" ");
        store.set_account_scopes(account, &scopes)?;
    }
    result
}

async fn sync<F, Fut>(store: &mut Store, account: &str, client: &mut Client<F>) -> Result<()>
where F: FnMut(Option<String>) -> Fut, Fut: Future<Output = Result<Zeroizing<String>>> {
    store.account_scopes(account)?;
    let mut url = client.url(&["users", "@me", "lists"])?;
    url.query_pairs_mut().append_pair("maxResults", "100");
    // List discovery is needed to map names before any task can be pushed.
    let remote: Vec<RemoteList> = client.pages(url).await?;
    store.link_google_lists(account, &remote)?;
    let default: RemoteList = serde_json::from_value(client.request(Method::GET, client.url(&["users", "@me", "lists", "@default"])?, None).await?)
        .map_err(|_| Error::Network("invalid Google default task list"))?;
    if default.id.is_empty() { return Err(Error::Network("Google task list ID missing")); }
    store.db.execute("UPDATE task_lists SET google_default=COALESCE(remote_id=?2,0) WHERE account=?1", params![account, default.id])?;
    // Recover tombstones made before the provider's default list was known.
    store.db.execute("UPDATE task_lists SET deleted=0,dirty=0 WHERE account=?1 AND google_default=1 AND deleted=1", [account])?;
    let mut pushed_lists = HashSet::new();
    let mut pushed_tasks = HashSet::new();
    for list in store.google_lists(account)? {
        if list.deleted {
            if let Some(id) = &list.remote {
                match client.request(Method::DELETE, client.url(&["users", "@me", "lists", id])?, None).await {
                    Err(Error::Invalid(_)) => {
                        store.db.execute("UPDATE task_lists SET deleted=0,dirty=0 WHERE id=?1 AND revision=?2", params![list.id, list.revision])?;
                        continue;
                    }
                    result => { result?; }
                }
            }
            store.db.execute("DELETE FROM task_lists WHERE id=?1 AND revision=?2 AND deleted=1", params![list.id, list.revision])?;
            continue;
        }
        if list.dirty || list.remote.is_none() || (!remote.iter().any(|r| Some(&r.id) == list.remote.as_ref()) && !store.pending_google_tasks(list.id)?.is_empty()) {
            let body = json!({"title": list.name});
            let response = if let Some(id) = &list.remote {
                client.request(Method::PATCH, client.url(&["users", "@me", "lists", id])?, Some(body.clone())).await
            } else { Err(Error::NotFound) };
            let value = match response {
                Err(Error::NotFound) => client.request(Method::POST, client.url(&["users", "@me", "lists"])?, Some(body)).await?,
                value => value?,
            };
            let saved: RemoteList = serde_json::from_value(value).map_err(|_| Error::Network("invalid Google task list"))?;
            if saved.id.is_empty() { return Err(Error::Network("Google task list ID missing")); }
            if list.remote.as_ref().is_some_and(|old| *old != saved.id) {
                store.db.execute("UPDATE tasks SET remote_id=NULL,remote_list_id=NULL,dirty=1 WHERE list=?1", [list.id])?;
            }
            store.db.execute("UPDATE task_lists SET remote_id=?2,etag=?3,remote_updated=?4,dirty=(revision<>?5) WHERE id=?1 AND account=?6",
                params![list.id, saved.id, saved.etag, saved.updated, list.revision, account])?;
            pushed_lists.insert(list.id);
        }
    }
    // shortcut: ordering and subtasks stay local, add tasks.move when syncing provider ordering.
    for list in store.google_lists(account)?.into_iter().filter(|l| !l.deleted) {
        let Some(remote_list) = &list.remote else { continue };
        for (task, revision, old_list) in store.pending_google_tasks(list.id)? {
            let moved = old_list.as_ref().is_some_and(|old| old != remote_list);
            if task.deleted || moved {
                if let Some(id) = &task.remote_id {
                    client.request(Method::DELETE, client.url(&["lists", old_list.as_deref().unwrap_or(remote_list), "tasks", id])?, None).await?;
                }
                if task.deleted {
                    store.db.execute("UPDATE tasks SET dirty=(revision<>?2) WHERE id=?1", params![task.id, revision])?;
                    pushed_tasks.insert(task.id);
                    continue;
                }
                store.db.execute("UPDATE tasks SET remote_id=NULL,remote_list_id=NULL,etag=NULL,remote_updated=NULL WHERE id=?1", [task.id])?;
            }
            let body = task_body(&task)?;
            let response = if let Some(id) = task.remote_id.as_ref().filter(|_| !moved) {
                client.request(Method::PATCH, client.url(&["lists", remote_list, "tasks", id])?, Some(body.clone())).await
            } else { Err(Error::NotFound) };
            // shortcut: retry after a lost POST response can duplicate a task, add reconciliation before promising exactly-once inserts.
            let value = match response {
                Err(Error::NotFound) => client.request(Method::POST, client.url(&["lists", remote_list, "tasks"])?, Some(body)).await?,
                value => value?,
            };
            let saved: RemoteTask = serde_json::from_value(value).map_err(|_| Error::Network("invalid Google task"))?;
            if saved.id.is_empty() { return Err(Error::Network("Google task ID missing")); }
            let tx = store.db.unchecked_transaction()?;
            let current_list: i64 = tx.query_row("SELECT list FROM tasks WHERE id=?1", [task.id], |r| r.get(0))?;
            let other: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE list=?1 AND remote_id=?2 AND id<>?3)", params![list.id, saved.id, task.id], |r| r.get(0))?;
            if current_list == list.id && !other {
                tx.execute("UPDATE tasks SET remote_id=?2,remote_list_id=?3,etag=?4,remote_updated=?5,dirty=(revision<>?6) WHERE id=?1",
                    params![task.id, saved.id, remote_list, saved.etag, saved.updated, revision])?;
            } else {
                // The user moved the task while this request was in flight; clean up its old copy.
                tx.execute("INSERT OR IGNORE INTO tasks(list,remote_id,remote_list_id,title,updated,created,dirty,deleted) VALUES(?1,?2,?3,?4,unixepoch(),unixepoch(),1,1)",
                    params![list.id, saved.id, remote_list, task.title])?;
            }
            tx.commit()?;
            pushed_tasks.insert(task.id);
        }
    }
    // A failed pull never rolls back acknowledged pushes or advances its list's cursor.
    for list in store.google_lists(account)?.into_iter().filter(|l| !l.deleted) {
        let Some(remote_id) = &list.remote else { continue };
        if !pushed_lists.contains(&list.id) {
            if let Some(remote) = remote.iter().find(|r| r.id == *remote_id) {
                store.db.execute("UPDATE task_lists SET name=?2,etag=?3,remote_updated=?4 WHERE id=?1 AND dirty=0 AND deleted=0", params![list.id, remote.title, remote.etag, remote.updated])?;
            } else if !list.dirty {
                if list.id == store.default_task_list()? {
                    store.db.execute("UPDATE task_lists SET remote_id=NULL,dirty=1,pulled_through=NULL WHERE id=?1", [list.id])?;
                    store.db.execute("UPDATE tasks SET remote_id=NULL,remote_list_id=NULL,dirty=1 WHERE list=?1", [list.id])?;
                } else {
                    store.delete_task_list(list.id)?;
                    store.db.execute("DELETE FROM task_lists WHERE id=?1", [list.id])?;
                }
                continue;
            }
        }
        let mut url = client.url(&["lists", remote_id, "tasks"])?;
        url.query_pairs_mut().extend_pairs([("showCompleted", "true"), ("showHidden", "true"), ("showDeleted", "true"), ("maxResults", "100")]);
        if let Some(cursor) = &list.cursor { url.query_pairs_mut().append_pair("updatedMin", cursor); }
        let tasks: Vec<RemoteTask> = client.pages(url).await?;
        let tx = store.db.unchecked_transaction()?;
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM task_lists WHERE id=?1 AND account=?2 AND deleted=0)", params![list.id, account], |r| r.get(0))?;
        if !active { continue; }
        let mut through = list.cursor.as_deref().and_then(timestamp);
        for task in tasks {
            if task.id.is_empty() { return Err(Error::Network("Google task ID missing")); }
            if let Some(updated) = task.updated.as_deref().and_then(timestamp) { through = Some(through.map_or(updated, |old| old.max(updated))); }
            let existing: Option<(i64, bool, bool)> = tx.query_row("SELECT id,dirty,deleted FROM tasks WHERE list=?1 AND remote_id=?2", params![list.id, task.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            if existing.is_some_and(|(id, dirty, deleted)| dirty || deleted || pushed_tasks.contains(&id)) { continue; }
            let (notes, mail) = split_notes(&task.notes);
            let done = if task.status == "completed" { Some(task.completed.as_deref().and_then(timestamp).unwrap_or(0)) } else { None };
            if let Some((id, _, _)) = existing {
                tx.execute("UPDATE tasks SET title=?2,notes=?3,due=?4,done_at=?5,deleted=?6,etag=?7,remote_updated=?8,mail_account=?9,mail_message_id=?10,mail_subject=?11 WHERE id=?1 AND dirty=0",
                    params![id, task.title, notes, task.due, done, task.deleted, task.etag, task.updated, mail.as_ref().map(|m| &m.0), mail.as_ref().map(|m| &m.1), mail.as_ref().and_then(|m| m.2.as_ref())])?;
            } else if !task.deleted {
                tx.execute("INSERT INTO tasks(list,remote_id,remote_list_id,title,notes,due,done_at,position,updated,created,etag,remote_updated,mail_account,mail_message_id,mail_subject)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,(SELECT COALESCE(MAX(position),-1)+1 FROM tasks WHERE list=?1),unixepoch(),unixepoch(),?8,?9,?10,?11,?12)",
                    params![list.id, task.id, remote_id, task.title, notes, task.due, done, task.etag, task.updated, mail.as_ref().map(|m| &m.0), mail.as_ref().map(|m| &m.1), mail.as_ref().and_then(|m| m.2.as_ref())])?;
            }
        }
        if let Some(through) = through {
            tx.execute("UPDATE task_lists SET pulled_through=?2 WHERE id=?1", params![list.id, mail_parser::DateTime::from_timestamp(through).to_rfc3339()])?;
        }
        tx.commit()?;
    }
    Ok(())
}

fn timestamp(value: &str) -> Option<i64> { mail_parser::DateTime::parse_rfc3339(value).filter(|d| d.is_valid()).map(|d| d.to_timestamp()) }

fn task_body(task: &Task) -> Result<Value> {
    let mut notes = task.notes.clone();
    if let (Some(account), Some(message)) = (&task.mail_account, &task.mail_message_id) {
        let mut link = Url::parse("lagom://mail").unwrap();
        link.query_pairs_mut().append_pair("account", account).append_pair("message-id", message);
        if let Some(subject) = &task.mail_subject { link.query_pairs_mut().append_pair("subject", subject); }
        if !notes.is_empty() { notes.push('\n'); }
        notes.push_str(link.as_str());
    }
    let due = task.due.as_ref().map(|due| if due.len() == 10 { format!("{due}T00:00:00Z") } else { due.clone() });
    if due.as_deref().is_some_and(|due| timestamp(due).is_none()) { return Err(Error::Invalid("invalid task due date")); }
    Ok(json!({"title": task.title, "notes": notes, "due": due,
        "status": if task.done_at.is_some() { "completed" } else { "needsAction" },
        "completed": task.done_at.map(|done| mail_parser::DateTime::from_timestamp(done).to_rfc3339())}))
}

type MailLink = (String, String, Option<String>);
fn split_notes(notes: &str) -> (String, Option<MailLink>) {
    let mut lines: Vec<_> = notes.split('\n').collect();
    for index in (0..lines.len()).rev() {
        let Some(link) = Url::parse(lines[index]).ok().filter(|u| u.scheme() == "lagom" && u.host_str() == Some("mail") && u.path().is_empty()) else { continue };
        let pairs: std::collections::HashMap<_, _> = link.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        if let (Some(account), Some(message)) = (pairs.get("account"), pairs.get("message-id")) {
            if !account.is_empty() && !message.is_empty() {
                lines.remove(index);
                return (lines.join("\n"), Some((account.clone(), message.clone(), pairs.get("subject").cloned())));
            }
        }
    }
    (notes.into(), None)
}

#[cfg(test)]
mod tests;

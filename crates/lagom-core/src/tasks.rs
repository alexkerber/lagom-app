//! Local tasks. Deleted tasks retain a dirty tombstone for future sync.
use crate::{
    store::{MessageSummary, Store},
    Error, Result,
};
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskList {
    pub id: i64,
    pub account: Option<String>,
    pub remote_id: Option<String>,
    pub name: String,
    pub position: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub list: i64,
    pub remote_id: Option<String>,
    pub title: String,
    pub notes: String,
    pub due: Option<String>,
    pub done_at: Option<i64>,
    pub position: Option<i64>,
    pub mail_account: Option<String>,
    pub mail_message_id: Option<String>,
    pub mail_subject: Option<String>,
    pub updated: i64,
    pub dirty: bool,
    pub deleted: bool,
}

const TASK_COLUMNS: &str = "id,list,remote_id,title,notes,due,done_at,position,mail_account,mail_message_id,mail_subject,updated,dirty,deleted";
fn task_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        list: r.get(1)?,
        remote_id: r.get(2)?,
        title: r.get(3)?,
        notes: r.get(4)?,
        due: r.get(5)?,
        done_at: r.get(6)?,
        position: r.get(7)?,
        mail_account: r.get(8)?,
        mail_message_id: r.get(9)?,
        mail_subject: r.get(10)?,
        updated: r.get(11)?,
        dirty: r.get(12)?,
        deleted: r.get(13)?,
    })
}
fn list_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskList> {
    Ok(TaskList {
        id: r.get(0)?,
        account: r.get(1)?,
        remote_id: r.get(2)?,
        name: r.get(3)?,
        position: r.get(4)?,
    })
}
fn title(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::Invalid("title cannot be empty"));
    }
    Ok(value)
}
fn changed(rows: usize) -> Result<()> {
    if rows == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

impl Store {
    pub fn task_lists(&self) -> Result<Vec<TaskList>> {
        let mut stmt = self.db.prepare(
            "SELECT id,account,remote_id,name,position FROM task_lists ORDER BY position,id",
        )?;
        let rows = stmt.query_map([], list_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn add_task_list(&self, name: &str) -> Result<TaskList> {
        Ok(self.db.query_row("INSERT INTO task_lists(name,position) SELECT ?1,COALESCE(MAX(position),-1)+1 FROM task_lists RETURNING id,account,remote_id,name,position", [title(name)?], list_row)?)
    }
    pub fn rename_task_list(&self, id: i64, name: &str) -> Result<()> {
        changed(self.db.execute(
            "UPDATE task_lists SET name=?2 WHERE id=?1",
            params![id, title(name)?],
        )?)
    }
    /// Deleting a list also deletes its tasks. At least one list must remain.
    pub fn delete_task_list(&self, id: i64) -> Result<()> {
        let rows = self.db.execute(
            "DELETE FROM task_lists WHERE id=?1 AND (SELECT COUNT(*) FROM task_lists)>1",
            [id],
        )?;
        if rows == 0
            && self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_lists WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Err(Error::Invalid("cannot delete the last task list"));
        }
        changed(rows)
    }
    /// Active tasks first, then completed tasks, each ordered by position and id.
    pub fn tasks(&self, list: i64, include_done: bool) -> Result<Vec<Task>> {
        let mut stmt = self.db.prepare(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE list=?1 AND deleted=0 AND (?2 OR done_at IS NULL) ORDER BY done_at IS NOT NULL,position,id"))?;
        let rows = stmt.query_map(params![list, include_done], task_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn add_task(
        &self,
        list: i64,
        title: &str,
        notes: &str,
        due: Option<String>,
    ) -> Result<Task> {
        self.insert_task(list, title, notes, due, None)
    }
    fn insert_task(
        &self,
        list: i64,
        name: &str,
        notes: &str,
        due: Option<String>,
        mail: Option<&MessageSummary>,
    ) -> Result<Task> {
        Ok(self.db.query_row(&format!("INSERT INTO tasks(list,title,notes,due,position,mail_account,mail_message_id,mail_subject,updated,dirty)
            VALUES(?1,?2,?3,?4,(SELECT COALESCE(MAX(position),-1)+1 FROM tasks WHERE list=?1 AND deleted=0),?5,?6,?7,unixepoch(),1) RETURNING {TASK_COLUMNS}"),
            params![list, title(name)?, notes, due, mail.map(|m| &m.account), mail.map(|m| &m.message_id), mail.map(|m| &m.subject)], task_row)?)
    }
    pub fn update_task(&self, id: i64, name: &str, notes: &str, due: Option<String>) -> Result<()> {
        changed(self.db.execute("UPDATE tasks SET title=?2,notes=?3,due=?4,updated=unixepoch(),dirty=1 WHERE id=?1 AND deleted=0", params![id, title(name)?, notes, due])?)
    }
    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        changed(self.db.execute("UPDATE tasks SET done_at=CASE WHEN ?2 THEN unixepoch() ELSE NULL END,updated=unixepoch(),dirty=1 WHERE id=?1 AND deleted=0", params![id, done])?)
    }
    pub fn move_task(&self, id: i64, list: i64, position: i64) -> Result<()> {
        changed(self.db.execute("UPDATE tasks SET list=?2,position=?3,updated=unixepoch(),dirty=1 WHERE id=?1 AND deleted=0", params![id, list, position])?)
    }
    pub fn delete_task(&self, id: i64) -> Result<()> {
        changed(self.db.execute(
            "UPDATE tasks SET deleted=1,updated=unixepoch(),dirty=1 WHERE id=?1 AND deleted=0",
            [id],
        )?)
    }
    pub fn task_from_mail(&self, list: i64, mail: &MessageSummary) -> Result<Task> {
        let name = if mail.subject.trim().is_empty() {
            "(no subject)"
        } else {
            &mail.subject
        };
        self.insert_task(
            list,
            name,
            "",
            None,
            if mail.message_id.is_empty() {
                None
            } else {
                Some(mail)
            },
        )
    }
    pub fn tasks_for_mail(&self, account: &str, message_id: &str) -> Result<Vec<Task>> {
        let mut stmt = self.db.prepare(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE mail_account=?1 AND mail_message_id=?2 AND deleted=0 ORDER BY done_at IS NOT NULL,position,id"))?;
        let rows = stmt.query_map(params![account, message_id], task_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account::Account,
        store::{FolderRole, Message},
    };

    fn setup() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("tasks.db")).unwrap();
        (temp, store)
    }
    fn reset_dirty(store: &Store, id: i64) {
        store
            .db
            .execute("UPDATE tasks SET updated=0,dirty=0 WHERE id=?1", [id])
            .unwrap();
    }
    fn assert_dirty(store: &Store, id: i64) {
        let (updated, dirty): (i64, bool) = store
            .db
            .query_row("SELECT updated,dirty FROM tasks WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        let now: i64 = store
            .db
            .query_row("SELECT unixepoch()", [], |r| r.get(0))
            .unwrap();
        assert!((now - 5..=now).contains(&updated));
        assert!(dirty);
    }
    #[test]
    fn lists_crud_order_and_last_list_guard() {
        let (_temp, s) = setup();
        let lists = s.task_lists().unwrap();
        assert_eq!(
            lists
                .iter()
                .map(|l| (l.name.as_str(), l.position))
                .collect::<Vec<_>>(),
            [("Work", 0), ("Private", 1)]
        );
        assert!(lists
            .iter()
            .all(|l| l.account.is_none() && l.remote_id.is_none()));
        let custom = s.add_task_list("  Shopping  ").unwrap();
        assert_eq!((custom.name.as_str(), custom.position), ("Shopping", 2));
        s.rename_task_list(custom.id, "  Groceries ").unwrap();
        assert_eq!(s.task_lists().unwrap()[2].name, "Groceries");
        let task = s.add_task(custom.id, "Milk", "", None).unwrap();
        s.delete_task_list(custom.id).unwrap();
        assert_eq!(
            s.db.query_row("SELECT COUNT(*) FROM tasks WHERE id=?1", [task.id], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        s.delete_task_list(lists[1].id).unwrap();
        let kept = s.add_task(lists[0].id, "Keep me", "", None).unwrap();
        assert!(matches!(
            s.delete_task_list(lists[0].id),
            Err(Error::Invalid(_))
        ));
        assert_eq!(s.task_lists().unwrap(), [lists[0].clone()]);
        assert_eq!(s.tasks(lists[0].id, true).unwrap(), [kept]);
        for name in ["", " \n\t"] {
            assert!(matches!(s.add_task_list(name), Err(Error::Invalid(_))));
            assert!(matches!(
                s.rename_task_list(lists[0].id, name),
                Err(Error::Invalid(_))
            ));
        }
        assert!(matches!(
            s.rename_task_list(999, "Missing"),
            Err(Error::NotFound)
        ));
        assert!(matches!(s.delete_task_list(999), Err(Error::NotFound)));
    }
    #[test]
    fn task_add_update_and_reopen() {
        let (temp, s) = setup();
        let list = s.task_lists().unwrap()[0].id;
        let task = s
            .add_task(list, "  Plan  ", "Keep notes\n", Some("2026-11-09".into()))
            .unwrap();
        assert_eq!(
            (
                task.title.as_str(),
                task.notes.as_str(),
                task.due.as_deref()
            ),
            ("Plan", "Keep notes\n", Some("2026-11-09"))
        );
        assert_eq!(task.position, Some(0));
        assert!(task.done_at.is_none() && task.remote_id.is_none() && !task.deleted);
        assert_dirty(&s, task.id);
        for name in ["", " \n"] {
            assert!(matches!(
                s.add_task(list, name, "", None),
                Err(Error::Invalid(_))
            ));
            assert!(matches!(
                s.update_task(task.id, name, "", None),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(s.tasks(list, true).unwrap(), [task.clone()]);
        reset_dirty(&s, task.id);
        s.update_task(task.id, "  Revised ", "new notes", None)
            .unwrap();
        assert_dirty(&s, task.id);
        drop(s);
        let s = Store::open(temp.path().join("tasks.db")).unwrap();
        let tasks = s.tasks(list, false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            (
                tasks[0].title.as_str(),
                tasks[0].notes.as_str(),
                tasks[0].due.as_deref()
            ),
            ("Revised", "new notes", None)
        );
        assert_eq!(s.task_lists().unwrap().len(), 2);
    }
    #[test]
    fn done_toggle_and_order_by_done_position_id() {
        let (_temp, s) = setup();
        let list = s.task_lists().unwrap()[0].id;
        let a = s.add_task(list, "A", "", None).unwrap();
        let b = s.add_task(list, "B", "", None).unwrap();
        let c = s.add_task(list, "C", "", None).unwrap();
        assert_eq!(b.position, Some(1));
        assert_eq!(c.position, Some(2));
        s.move_task(c.id, list, 0).unwrap();
        assert_eq!(
            s.tasks(list, true)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            [a.id, c.id, b.id]
        );
        reset_dirty(&s, a.id);
        s.set_done(a.id, true).unwrap();
        assert_dirty(&s, a.id);
        let all = s.tasks(list, true).unwrap();
        assert_eq!(
            all.iter().map(|t| t.id).collect::<Vec<_>>(),
            [c.id, b.id, a.id]
        );
        assert_eq!(all[2].done_at, Some(all[2].updated));
        assert_eq!(s.tasks(list, false).unwrap().len(), 2);
        reset_dirty(&s, a.id);
        s.set_done(a.id, false).unwrap();
        assert_dirty(&s, a.id);
        assert_eq!(s.tasks(list, false).unwrap().len(), 3);
        assert!(s
            .tasks(list, true)
            .unwrap()
            .iter()
            .all(|t| t.done_at.is_none()));
    }
    #[test]
    fn move_and_delete_preserve_dirty_tombstone() {
        let (_temp, s) = setup();
        let lists = s.task_lists().unwrap();
        let task = s.add_task(lists[0].id, "Move me", "", None).unwrap();
        reset_dirty(&s, task.id);
        s.move_task(task.id, lists[1].id, 7).unwrap();
        assert_dirty(&s, task.id);
        assert!(s.tasks(lists[0].id, true).unwrap().is_empty());
        assert_eq!(s.tasks(lists[1].id, true).unwrap()[0].position, Some(7));
        reset_dirty(&s, task.id);
        s.delete_task(task.id).unwrap();
        assert_dirty(&s, task.id);
        assert!(s.tasks(lists[1].id, true).unwrap().is_empty());
        assert!(s
            .db
            .query_row("SELECT deleted FROM tasks WHERE id=?1", [task.id], |r| r
                .get::<_, bool>(
                0
            ))
            .unwrap());
        for id in [task.id, 999] {
            assert!(matches!(
                s.update_task(id, "no", "", None),
                Err(Error::NotFound)
            ));
            assert!(matches!(s.set_done(id, true), Err(Error::NotFound)));
            assert!(matches!(
                s.move_task(id, lists[0].id, 0),
                Err(Error::NotFound)
            ));
            assert!(matches!(s.delete_task(id), Err(Error::NotFound)));
        }
    }
    #[test]
    fn invalid_list_does_not_create_or_move_tasks() {
        let (_temp, s) = setup();
        let list = s.task_lists().unwrap()[0].id;
        assert!(s.add_task(999, "Bad", "", None).is_err());
        let task = s.add_task(list, "Kept", "", None).unwrap();
        assert!(s.move_task(task.id, 999, 0).is_err());
        assert_eq!(s.tasks(list, true).unwrap(), [task]);
    }
    #[test]
    fn mail_links_are_account_scoped_and_survive_mail_moves_and_account_removal() {
        let (_temp, s) = setup();
        let (i, o) = crate::presets::servers("a@gmail.com").unwrap();
        let account = Account::new("a@gmail.com".into(), i, o).unwrap();
        s.save_account(&account).unwrap();
        let folder = s
            .save_folder(&account.id, "INBOX", None, FolderRole::Inbox)
            .unwrap();
        let mut mail = MessageSummary {
            account: account.id.clone(),
            folder,
            uid: 1,
            message_id: "<stable@test>".into(),
            subject: "  Follow up  ".into(),
            ..Default::default()
        };
        let id = s
            .save_message(&Message {
                summary: mail.clone(),
                ..Default::default()
            })
            .unwrap();
        let lists = s.task_lists().unwrap();
        let task = s.task_from_mail(lists[0].id, &mail).unwrap();
        assert_eq!(task.title, "Follow up");
        assert_eq!(task.mail_account.as_deref(), Some(account.id.as_str()));
        assert_eq!(task.mail_message_id.as_deref(), Some("<stable@test>"));
        assert_eq!(task.mail_subject.as_deref(), Some("  Follow up  "));
        assert_dirty(&s, task.id);
        s.delete_message(id).unwrap();
        mail.folder = s
            .save_folder(&account.id, "Done", None, FolderRole::Archive)
            .unwrap();
        let moved_id = s
            .save_message(&Message {
                summary: mail.clone(),
                ..Default::default()
            })
            .unwrap();
        assert_ne!(id, moved_id);
        assert_eq!(
            s.tasks_for_mail(&account.id, &mail.message_id).unwrap(),
            [task.clone()]
        );
        assert!(s
            .tasks_for_mail("other-account", &mail.message_id)
            .unwrap()
            .is_empty());
        assert!(s
            .tasks_for_mail(&account.id, "other-id")
            .unwrap()
            .is_empty());
        let second = s.task_from_mail(lists[1].id, &mail).unwrap();
        s.set_done(task.id, true).unwrap();
        assert_eq!(
            s.tasks_for_mail(&account.id, &mail.message_id)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            [second.id, task.id]
        );
        s.delete_task(second.id).unwrap();
        s.delete_account(&account.id).unwrap();
        assert!(s.message(moved_id).is_err());
        assert_eq!(s.task_lists().unwrap(), lists);
        assert_eq!(s.tasks(lists[0].id, true).unwrap().len(), 1);
        let linked = s.tasks_for_mail(&account.id, &mail.message_id).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].mail_subject, task.mail_subject);
    }
    #[test]
    fn missing_subject_and_message_id() {
        let (_temp, s) = setup();
        let list = s.task_lists().unwrap()[0].id;
        let mut mail = MessageSummary {
            account: "account".into(),
            subject: " \n ".into(),
            ..Default::default()
        };
        let task = s.task_from_mail(list, &mail).unwrap();
        assert_eq!(task.title, "(no subject)");
        assert!(
            task.mail_account.is_none()
                && task.mail_message_id.is_none()
                && task.mail_subject.is_none()
        );
        assert!(s.tasks_for_mail("account", "").unwrap().is_empty());
        mail.message_id = "<id@test>".into();
        assert_eq!(s.task_from_mail(list, &mail).unwrap().title, "(no subject)");
    }
}

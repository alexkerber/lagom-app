BEGIN IMMEDIATE;
ALTER TABLE task_lists ADD COLUMN detached INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_lists ADD COLUMN google_default INTEGER NOT NULL DEFAULT 0;
-- shortcut: v13 unowned lists stay local, revisit only with reliable ownership provenance.
UPDATE task_lists SET detached=1 WHERE account IS NULL;
DROP TRIGGER keep_tasks_before_account_removal;
CREATE TRIGGER keep_tasks_before_account_removal BEFORE DELETE ON accounts BEGIN
 UPDATE tasks SET remote_id=NULL,remote_list_id=NULL,etag=NULL,remote_updated=NULL,dirty=1
 WHERE list IN (SELECT id FROM task_lists WHERE account=OLD.id);
 UPDATE task_lists SET account=NULL,remote_id=NULL,etag=NULL,remote_updated=NULL,pulled_through=NULL,dirty=1,detached=1,google_default=0 WHERE account=OLD.id;
END;
PRAGMA user_version = 14;
COMMIT;

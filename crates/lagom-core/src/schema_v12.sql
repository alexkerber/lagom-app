BEGIN IMMEDIATE;
ALTER TABLE task_lists ADD COLUMN etag TEXT;
ALTER TABLE task_lists ADD COLUMN remote_updated TEXT;
ALTER TABLE task_lists ADD COLUMN dirty INTEGER NOT NULL DEFAULT 1;
ALTER TABLE task_lists ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_lists ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_lists ADD COLUMN pulled_through TEXT;
ALTER TABLE tasks ADD COLUMN etag TEXT;
ALTER TABLE tasks ADD COLUMN remote_updated TEXT;
ALTER TABLE tasks ADD COLUMN remote_list_id TEXT;
ALTER TABLE tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET remote_list_id=(SELECT remote_id FROM task_lists WHERE id=tasks.list) WHERE remote_id IS NOT NULL;
CREATE TRIGGER task_revision AFTER UPDATE OF title,notes,due,done_at,position,list,deleted ON tasks WHEN NEW.dirty=1 BEGIN
 UPDATE tasks SET revision=OLD.revision+1 WHERE id=NEW.id;
END;
CREATE TRIGGER task_list_revision AFTER UPDATE OF name,deleted ON task_lists WHEN NEW.dirty=1 BEGIN
 UPDATE task_lists SET revision=OLD.revision+1 WHERE id=NEW.id;
END;
-- Detach before the original account foreign key can cascade into local tasks.
CREATE TRIGGER keep_tasks_before_account_removal BEFORE DELETE ON accounts BEGIN
 UPDATE tasks SET remote_id=NULL,remote_list_id=NULL,etag=NULL,remote_updated=NULL,dirty=1
 WHERE list IN (SELECT id FROM task_lists WHERE account=OLD.id);
 UPDATE task_lists SET account=NULL,remote_id=NULL,etag=NULL,remote_updated=NULL,pulled_through=NULL,dirty=1 WHERE account=OLD.id;
END;
PRAGMA user_version = 12;
COMMIT;

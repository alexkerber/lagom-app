BEGIN IMMEDIATE;
ALTER TABLE tasks ADD COLUMN created INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET created=updated;
INSERT INTO task_lists(name,position) SELECT 'Work',0 WHERE NOT EXISTS(SELECT 1 FROM task_lists);
INSERT INTO task_lists(name,position) SELECT 'Private',1 WHERE changes()=1;
CREATE TABLE task_settings (
 id INTEGER PRIMARY KEY CHECK(id=1), list INTEGER NOT NULL REFERENCES task_lists(id) ON DELETE RESTRICT
);
INSERT INTO task_settings SELECT 1,id FROM task_lists ORDER BY position,id LIMIT 1;
CREATE INDEX tasks_today ON tasks(deleted,done_at,list,position);
PRAGMA user_version = 9;
COMMIT;

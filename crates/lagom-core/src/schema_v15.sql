BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN move_target INTEGER REFERENCES folders(id) ON DELETE SET NULL;
CREATE INDEX messages_visible_folder_date ON messages(coalesce(move_target,folder),date DESC,id DESC);
CREATE INDEX messages_visible_thread_folder ON messages(account,thread_id,coalesce(move_target,folder),date,id);
PRAGMA user_version = 15;
COMMIT;

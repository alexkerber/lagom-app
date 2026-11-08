BEGIN IMMEDIATE;
CREATE INDEX messages_thread_folder ON messages(account,thread_id,folder,date,id);
PRAGMA user_version = 10;
COMMIT;

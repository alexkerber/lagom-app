ALTER TABLE messages ADD COLUMN thread_id TEXT NOT NULL DEFAULT '';
ALTER TABLE messages ADD COLUMN thread_refs TEXT NOT NULL DEFAULT '';
ALTER TABLE messages ADD COLUMN threading_loaded INTEGER NOT NULL DEFAULT 0;
CREATE INDEX messages_thread ON messages(account,thread_id,date,id);
CREATE TABLE thread_links (
 account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 node TEXT NOT NULL, thread_id TEXT NOT NULL,
 PRIMARY KEY(account,node)
);
CREATE INDEX thread_links_thread ON thread_links(account,thread_id);
CREATE INDEX messages_later_date ON messages(account,date DESC,id DESC) WHERE snoozed_until IS NOT NULL;
CREATE INDEX messages_thread_folder ON messages(account,thread_id,folder,date,id);

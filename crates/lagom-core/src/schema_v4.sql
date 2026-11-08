BEGIN IMMEDIATE;
CREATE TABLE task_lists (
 id INTEGER PRIMARY KEY, account TEXT REFERENCES accounts(id) ON DELETE CASCADE,
 remote_id TEXT, name TEXT NOT NULL, position INTEGER NOT NULL, UNIQUE(account,remote_id)
);
INSERT INTO task_lists(name,position) VALUES('Work',0),('Private',1);
-- mail_account has no FK: local tasks survive removal of the linked account.
CREATE TABLE tasks (
 id INTEGER PRIMARY KEY, list INTEGER NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
 remote_id TEXT, title TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '', due TEXT, done_at INTEGER, position INTEGER,
 mail_account TEXT, mail_message_id TEXT, mail_subject TEXT, updated INTEGER NOT NULL,
 dirty INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, UNIQUE(list,remote_id)
);
PRAGMA user_version = 4;
COMMIT;

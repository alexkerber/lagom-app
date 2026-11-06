BEGIN IMMEDIATE;
CREATE TABLE folders (
 id INTEGER PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 name TEXT NOT NULL, delimiter TEXT, role TEXT NOT NULL DEFAULT 'other',
 uidvalidity INTEGER, uidnext INTEGER, last_uid INTEGER NOT NULL DEFAULT 0, highest_modseq INTEGER,
 UNIQUE(account,name)
);
CREATE TABLE messages (
 id INTEGER PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 folder INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE, uid INTEGER NOT NULL CHECK(uid>0),
 message_id TEXT NOT NULL, thread_key TEXT NOT NULL, from_name TEXT NOT NULL, from_address TEXT NOT NULL,
 to_addresses TEXT NOT NULL, cc TEXT NOT NULL, subject TEXT NOT NULL, date INTEGER NOT NULL,
 snippet TEXT NOT NULL, seen INTEGER NOT NULL, flagged INTEGER NOT NULL,
 has_attachments INTEGER NOT NULL, size INTEGER NOT NULL, body_text TEXT NOT NULL, body_html TEXT,
 snoozed_until INTEGER, UNIQUE(folder,uid)
);
CREATE INDEX messages_folder_date ON messages(folder,date DESC,id DESC);
CREATE INDEX messages_snooze ON messages(account,snoozed_until);
CREATE VIRTUAL TABLE messages_fts USING fts5(subject,from_name,from_address,body_text,content='messages',content_rowid='id');
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
 INSERT INTO messages_fts(rowid,subject,from_name,from_address,body_text) VALUES(new.id,new.subject,new.from_name,new.from_address,new.body_text);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
 INSERT INTO messages_fts(messages_fts,rowid,subject,from_name,from_address,body_text) VALUES('delete',old.id,old.subject,old.from_name,old.from_address,old.body_text);
END;
CREATE TRIGGER messages_au AFTER UPDATE OF subject,from_name,from_address,body_text ON messages BEGIN
 INSERT INTO messages_fts(messages_fts,rowid,subject,from_name,from_address,body_text) VALUES('delete',old.id,old.subject,old.from_name,old.from_address,old.body_text);
 INSERT INTO messages_fts(rowid,subject,from_name,from_address,body_text) VALUES(new.id,new.subject,new.from_name,new.from_address,new.body_text);
END;
PRAGMA user_version = 2;
COMMIT;

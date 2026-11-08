BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN attachments_loaded INTEGER NOT NULL DEFAULT 0;
CREATE TABLE attachments (
 message INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
 section TEXT NOT NULL, filename TEXT NOT NULL, mime_type TEXT NOT NULL,
 size INTEGER NOT NULL, encoding TEXT NOT NULL,
 PRIMARY KEY(message,section)
);
PRAGMA user_version = 5;
COMMIT;

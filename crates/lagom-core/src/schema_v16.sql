BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN inline_loaded INTEGER NOT NULL DEFAULT 0;
CREATE TABLE inline_parts (
    message INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    cid TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (message,cid)
);
PRAGMA user_version = 16;
COMMIT;

BEGIN IMMEDIATE;
CREATE TABLE draft_sync (
 id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 revision TEXT NOT NULL DEFAULT '', owned INTEGER NOT NULL DEFAULT 1,
 ready_at INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, error TEXT,
 pending_revision TEXT, pending_token TEXT, pending_raw BLOB
);
CREATE TABLE draft_copies (
 id TEXT NOT NULL REFERENCES draft_sync(id) ON DELETE CASCADE,
 folder TEXT NOT NULL, validity INTEGER NOT NULL, uid INTEGER NOT NULL,
 hash TEXT NOT NULL, revision TEXT NOT NULL,
 PRIMARY KEY(id,folder,validity,uid)
);
INSERT INTO draft_sync(id,account) SELECT id,account FROM drafts;
CREATE TRIGGER draft_insert AFTER INSERT ON drafts BEGIN
 INSERT INTO draft_sync(id,account) VALUES (new.id,new.account) ON CONFLICT(id) DO NOTHING;
END;
CREATE TRIGGER draft_edit AFTER UPDATE OF content ON drafts WHEN new.content<>old.content BEGIN
 UPDATE draft_sync SET owned=1 WHERE id=new.id;
END;
PRAGMA user_version = 17;
COMMIT;

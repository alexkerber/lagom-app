BEGIN IMMEDIATE;
CREATE TABLE drafts (
 id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 content TEXT NOT NULL
);
CREATE TABLE outbox (
 id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 content TEXT NOT NULL, raw BLOB NOT NULL, draft TEXT,
 ready_at INTEGER NOT NULL, started INTEGER NOT NULL DEFAULT 0,
 submitted INTEGER NOT NULL DEFAULT 0, error TEXT
);
CREATE INDEX outbox_due ON outbox(account,started,ready_at);
PRAGMA user_version = 5;
COMMIT;

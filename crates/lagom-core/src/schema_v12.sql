BEGIN IMMEDIATE;
ALTER TABLE events ADD COLUMN html_link TEXT;
CREATE INDEX messages_unread_date ON messages(date DESC,id DESC) WHERE seen=0;
CREATE TABLE calendar_windows (
    account TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    start_utc INTEGER NOT NULL,
    end_utc INTEGER NOT NULL CHECK (end_utc > start_utc)
);
PRAGMA user_version = 12;
COMMIT;

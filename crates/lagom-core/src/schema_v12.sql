BEGIN IMMEDIATE;
CREATE TABLE calendar_windows (
    account TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    start_utc INTEGER NOT NULL,
    end_utc INTEGER NOT NULL CHECK (end_utc > start_utc)
);
PRAGMA user_version = 12;
COMMIT;

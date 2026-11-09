BEGIN IMMEDIATE;
ALTER TABLE accounts ADD COLUMN scopes TEXT NOT NULL DEFAULT '';
CREATE TABLE calendars (
 id INTEGER PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 remote_id TEXT NOT NULL, name TEXT NOT NULL, color TEXT, shown INTEGER NOT NULL DEFAULT 1,
 writable INTEGER NOT NULL, UNIQUE(account,remote_id)
);
CREATE TABLE events (
 id INTEGER PRIMARY KEY, calendar INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
 remote_id TEXT NOT NULL, title TEXT NOT NULL, start TEXT NOT NULL, end TEXT NOT NULL,
 start_utc INTEGER NOT NULL, end_utc INTEGER NOT NULL, all_day INTEGER NOT NULL,
 location TEXT, join_url TEXT, UNIQUE(calendar,remote_id)
);
CREATE INDEX events_start ON events(start_utc);
PRAGMA user_version = 10;
COMMIT;

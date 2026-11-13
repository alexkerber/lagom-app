BEGIN IMMEDIATE;
-- The unsubscribe choice from List-Unsubscribe headers, as unsubscribe.rs stores it; empty for none.
ALTER TABLE messages ADD COLUMN unsubscribe TEXT NOT NULL DEFAULT '';
PRAGMA user_version = 19;
COMMIT;

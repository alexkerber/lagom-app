BEGIN IMMEDIATE;
ALTER TABLE accounts ADD COLUMN credential_key TEXT;
PRAGMA user_version = 20;
COMMIT;

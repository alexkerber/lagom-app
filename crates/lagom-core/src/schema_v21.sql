BEGIN IMMEDIATE;
ALTER TABLE accounts ADD COLUMN credential_key TEXT;
ALTER TABLE accounts ADD COLUMN provider_subject TEXT;
CREATE TABLE pending_credentials (
 account TEXT NOT NULL,
 credential_key TEXT NOT NULL,
 PRIMARY KEY(account, credential_key)
);
PRAGMA user_version = 21;
COMMIT;

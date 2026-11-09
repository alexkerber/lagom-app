BEGIN IMMEDIATE;
CREATE TABLE signatures (
 account TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
 text TEXT NOT NULL
);
CREATE TABLE attachment_cache (
 message INTEGER NOT NULL,
 section TEXT NOT NULL,
 data BLOB NOT NULL,
 PRIMARY KEY(message,section),
 FOREIGN KEY(message,section) REFERENCES attachments(message,section) ON DELETE CASCADE
);
PRAGMA user_version = 11;
COMMIT;

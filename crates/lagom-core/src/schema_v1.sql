BEGIN IMMEDIATE;
                CREATE TABLE accounts (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, email TEXT NOT NULL,
                imap_host TEXT NOT NULL, imap_port INTEGER NOT NULL, imap_security TEXT NOT NULL, imap_username TEXT NOT NULL,
                smtp_host TEXT NOT NULL, smtp_port INTEGER NOT NULL, smtp_security TEXT NOT NULL, smtp_username TEXT NOT NULL);
                PRAGMA user_version = 1; COMMIT;

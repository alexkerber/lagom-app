BEGIN IMMEDIATE;
ALTER TABLE accounts ADD COLUMN auth TEXT NOT NULL DEFAULT 'password' CHECK(auth IN ('password','google','microsoft'));
PRAGMA user_version = 3;
COMMIT;

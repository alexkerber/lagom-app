BEGIN IMMEDIATE;
CREATE TABLE omitted_fetches (
 folder INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
 uid INTEGER NOT NULL,
 PRIMARY KEY(folder,uid)
);
CREATE TRIGGER messages_clear_omitted_fetch AFTER INSERT ON messages BEGIN
 DELETE FROM omitted_fetches WHERE folder=NEW.folder AND uid=NEW.uid;
END;
CREATE TRIGGER folders_clear_omitted_fetches AFTER UPDATE OF uidvalidity ON folders
WHEN OLD.uidvalidity IS NOT NEW.uidvalidity BEGIN
 DELETE FROM omitted_fetches WHERE folder=NEW.id;
END;
PRAGMA user_version = 20;
COMMIT;

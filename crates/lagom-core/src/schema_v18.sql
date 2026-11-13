BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS mobile_draft_insert;
DROP TRIGGER IF EXISTS mobile_draft_update;
DROP TABLE IF EXISTS mobile_draft_times;
ALTER TABLE drafts ADD COLUMN updated INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER draft_updated_insert AFTER INSERT ON drafts BEGIN
 UPDATE drafts SET updated=unixepoch() WHERE id=new.id;
END;
CREATE TRIGGER draft_updated_edit AFTER UPDATE OF content ON drafts WHEN new.content<>old.content BEGIN
 UPDATE drafts SET updated=unixepoch() WHERE id=new.id;
END;
PRAGMA user_version = 18;
COMMIT;

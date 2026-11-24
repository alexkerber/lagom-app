BEGIN IMMEDIATE;
-- Pending and revoked write rows keep their history out of the read cap.
DROP TRIGGER agent_audit_cap;
CREATE TRIGGER agent_audit_cap AFTER INSERT ON agent_audit
 WHEN NEW.dispatched_write=0 OR NEW.result NOT IN ('ok','pending','revoked','superseded')
BEGIN
 UPDATE agent_audit SET read_seq=1+coalesce(
  (SELECT max(read_seq) FROM agent_audit WHERE agent IS NEW.agent AND read_seq IS NOT NULL),0)
 WHERE id=NEW.id;
 DELETE FROM agent_audit WHERE agent IS NEW.agent AND read_seq <=
  (SELECT read_seq-50000 FROM agent_audit WHERE id=NEW.id);
END;
CREATE TRIGGER agent_policy_issue AFTER INSERT ON agent_tokens BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
-- Existing drafts get 0, so the agent list order reveals no edit history.
ALTER TABLE drafts ADD COLUMN created INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER draft_created AFTER INSERT ON drafts BEGIN
 UPDATE drafts SET created=unixepoch() WHERE id=NEW.id;
END;
-- Agent sends queued before this version have no token and fail closed at the final check.
ALTER TABLE outbox ADD COLUMN agent_token TEXT REFERENCES agent_tokens(id) ON DELETE SET NULL;
ALTER TABLE outbox ADD COLUMN agent_policy_generation INTEGER;
PRAGMA user_version = 23;
COMMIT;

BEGIN IMMEDIATE;
DROP TRIGGER agent_audit_cap;
ALTER TABLE agent_audit ADD COLUMN dispatched_write INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_audit ADD COLUMN refusals INTEGER NOT NULL DEFAULT 0;
UPDATE agent_audit SET dispatched_write=1 WHERE read_seq IS NULL AND result='ok'
 AND command IN ('draft.create','draft.edit','draft.delete','draft.send','action.archive','action.move',
 'action.read','action.unread','action.star','action.unstar','action.snooze','action.unsnooze',
 'action.trash','event.create','event.update','event.delete');
WITH numbered AS MATERIALIZED (
 SELECT id,row_number() OVER (PARTITION BY agent ORDER BY id) AS seq FROM agent_audit WHERE dispatched_write=0
)
UPDATE agent_audit SET read_seq=(SELECT seq FROM numbered WHERE numbered.id=agent_audit.id) WHERE dispatched_write=0;
DELETE FROM agent_audit AS old WHERE old.read_seq <=
 (SELECT max(read_seq)-50000 FROM agent_audit WHERE agent IS old.agent AND read_seq IS NOT NULL);
CREATE UNIQUE INDEX anonymous_refusal_minute ON agent_audit(at) WHERE agent IS NULL AND command='refusals';
CREATE INDEX agent_refusal_bucket ON agent_audit(agent,at) WHERE refusals>0;
CREATE TRIGGER agent_audit_cap AFTER INSERT ON agent_audit
 WHEN NEW.dispatched_write=0 OR NEW.result<>'ok'
BEGIN
 UPDATE agent_audit SET read_seq=1+coalesce(
  (SELECT max(read_seq) FROM agent_audit WHERE agent IS NEW.agent AND read_seq IS NOT NULL),0)
 WHERE id=NEW.id;
 DELETE FROM agent_audit WHERE agent IS NEW.agent AND read_seq <=
  (SELECT read_seq-50000 FROM agent_audit WHERE id=NEW.id);
END;
INSERT INTO agent_settings VALUES('policy_generation','0');
CREATE TRIGGER agent_policy_revoke AFTER DELETE ON agent_tokens BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
CREATE TRIGGER agent_policy_scope AFTER UPDATE OF level,accounts,cidr,expires ON agent_tokens BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
CREATE TRIGGER agent_policy_access AFTER INSERT ON agent_access BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
CREATE TRIGGER agent_policy_level AFTER UPDATE ON agent_access BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
CREATE TRIGGER agent_policy_remove AFTER DELETE ON agent_access BEGIN
 UPDATE agent_settings SET value=CAST(value AS INTEGER)+1 WHERE key='policy_generation';
END;
PRAGMA user_version = 23;
COMMIT;

BEGIN IMMEDIATE;
ALTER TABLE outbox ADD COLUMN draft_agent TEXT;
-- Targets recorded before lookup verification have no reliable provenance.
UPDATE agent_audit SET target=NULL WHERE target IS NOT NULL;
ALTER TABLE agent_audit ADD COLUMN read_seq INTEGER;
CREATE INDEX agent_audit_reads ON agent_audit(agent,read_seq) WHERE read_seq IS NOT NULL;

-- Count in append order. Write rows have no sequence and expire only by age.
WITH numbered AS MATERIALIZED (
 SELECT id,row_number() OVER (PARTITION BY agent ORDER BY id) AS seq FROM agent_audit
 WHERE command NOT IN ('draft.create','draft.edit','draft.delete','draft.send')
 AND command NOT GLOB 'action.*' AND command NOT GLOB 'event.*'
)
UPDATE agent_audit SET read_seq=numbered.seq FROM numbered WHERE agent_audit.id=numbered.id;
DELETE FROM agent_audit WHERE read_seq <=
 (SELECT max(b.read_seq)-50000 FROM agent_audit b WHERE b.agent IS agent_audit.agent AND b.read_seq IS NOT NULL);

CREATE TRIGGER agent_audit_cap AFTER INSERT ON agent_audit
 WHEN NEW.command NOT IN ('draft.create','draft.edit','draft.delete','draft.send')
 AND NEW.command NOT GLOB 'action.*' AND NEW.command NOT GLOB 'event.*'
BEGIN
 UPDATE agent_audit SET read_seq=1+coalesce(
  (SELECT max(read_seq) FROM agent_audit WHERE agent IS NEW.agent AND read_seq IS NOT NULL),0)
 WHERE id=NEW.id;
 DELETE FROM agent_audit WHERE agent IS NEW.agent AND read_seq <=
  (SELECT read_seq-50000 FROM agent_audit WHERE id=NEW.id);
END;
PRAGMA user_version = 23;
COMMIT;

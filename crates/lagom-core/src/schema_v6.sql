BEGIN IMMEDIATE;
ALTER TABLE outbox ADD COLUMN state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE outbox ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE outbox ADD COLUMN copied INTEGER NOT NULL DEFAULT 0;
ALTER TABLE drafts ADD COLUMN revision TEXT NOT NULL DEFAULT '';
UPDATE drafts SET revision=lower(hex(randomblob(16)));
-- Older ambiguous failures must not become eligible during migration.
UPDATE outbox SET state='uncertain' WHERE submitted=0 AND (started=1 OR error IS NOT NULL);
PRAGMA user_version = 6;
COMMIT;

BEGIN IMMEDIATE;
CREATE TABLE agents (id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=12 AND id NOT GLOB '*[^0-9a-f]*'),
 kind TEXT NOT NULL, name TEXT NOT NULL, host TEXT NOT NULL, remote INTEGER NOT NULL DEFAULT 0 CHECK(remote IN (0,1)), created INTEGER NOT NULL);
CREATE TABLE agent_tokens (id TEXT PRIMARY KEY NOT NULL, agent TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
 hash BLOB NOT NULL CHECK(length(hash)=32), level INTEGER NOT NULL CHECK(level BETWEEN 0 AND 3),
 accounts TEXT, cidr TEXT, expires INTEGER, created INTEGER NOT NULL, last_used INTEGER);
CREATE INDEX agent_tokens_agent ON agent_tokens(agent);
CREATE TABLE agent_access (agent TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('mail','calendar')), account TEXT NOT NULL,
 level INTEGER NOT NULL CHECK(level BETWEEN 0 AND 3 AND (kind='mail' OR level<>2)), PRIMARY KEY(agent, kind, account));
-- Retired IDs can still occur in SSH forced commands.
CREATE TABLE agent_ids (id TEXT PRIMARY KEY NOT NULL);
CREATE TRIGGER agent_defaults AFTER INSERT ON agents BEGIN
 INSERT INTO agent_ids VALUES(NEW.id);
 INSERT INTO agent_access VALUES(NEW.id,'mail','',1),(NEW.id,'calendar','',1);
END;
CREATE TRIGGER agent_access_removed AFTER DELETE ON agents BEGIN DELETE FROM agent_access WHERE agent=OLD.id; END;
CREATE TRIGGER agent_account_removed AFTER DELETE ON accounts BEGIN DELETE FROM agent_access WHERE account=OLD.id; END;
CREATE TABLE agent_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO agent_settings VALUES('enabled','0'),('confirm','1');
CREATE TABLE agent_drafts (draft TEXT PRIMARY KEY REFERENCES drafts(id) ON DELETE CASCADE, agent TEXT NOT NULL);
CREATE TABLE agent_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, agent TEXT, token TEXT,
 account TEXT, command TEXT NOT NULL, target TEXT, result TEXT NOT NULL, source_ip TEXT, detail TEXT);
CREATE INDEX agent_audit_at ON agent_audit(at);
CREATE INDEX messages_account_message_id ON messages(account,message_id);
PRAGMA user_version = 22;
COMMIT;

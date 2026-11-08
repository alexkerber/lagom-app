BEGIN IMMEDIATE;
CREATE INDEX messages_later_date ON messages(account,date DESC,id DESC) WHERE snoozed_until IS NOT NULL;
PRAGMA user_version = 9;
COMMIT;

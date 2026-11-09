BEGIN IMMEDIATE;
ALTER TABLE events ADD COLUMN html_link TEXT;
CREATE INDEX messages_unread_date ON messages(date DESC,id DESC) WHERE seen=0;
PRAGMA user_version = 11;
COMMIT;

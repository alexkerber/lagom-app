BEGIN IMMEDIATE;
-- A previously uploaded mail link can predate the list's incremental sync cursor.
UPDATE task_lists SET pulled_through=NULL
WHERE EXISTS (SELECT 1 FROM tasks JOIN accounts ON accounts.id=tasks.mail_account
              WHERE tasks.list=task_lists.id AND accounts.id<>task_lists.account
                AND tasks.mail_message_id IS NOT NULL AND tasks.remote_id IS NOT NULL AND tasks.deleted=0);
PRAGMA user_version = 24;
COMMIT;

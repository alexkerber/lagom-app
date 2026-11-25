BEGIN IMMEDIATE;
-- An empty cursor requests one full pull with legacy link cleanup.
UPDATE task_lists SET pulled_through=''
WHERE EXISTS (SELECT 1 FROM tasks JOIN accounts ON accounts.id=tasks.mail_account
              WHERE tasks.list=task_lists.id AND accounts.id<>task_lists.account
                AND tasks.mail_message_id IS NOT NULL AND tasks.remote_id IS NOT NULL AND tasks.deleted=0);
PRAGMA user_version = 24;
COMMIT;

-- Add migration script here
-- SQLite does not support dropping constraints directly or altering unique constraints easily.
-- The standard procedure is to create a new table, copy data, drop old, rename new.
-- 1. Create the new messages table with the desired structure.
-- telegram_message_id is no longer globally unique.
-- Instead, the combination of (chat_id, telegram_message_id) should be unique.
CREATE TABLE
    messages_new (
        id INTEGER PRIMARY KEY AUTOINCREMENT, -- Local database ID
        telegram_message_id BIGINT NOT NULL, -- Telegram's unique message ID (within a chat)
        chat_id INTEGER NOT NULL, -- Foreign key to our chats table (local chats.id)
        sender_id INTEGER NOT NULL, -- Foreign key to our users table (local users.id)
        text TEXT, -- Message text content (can be null for non-text messages)
        sent_at TIMESTAMP NOT NULL, -- Timestamp when the message was sent (from Telegram)
        raw_message TEXT, -- Store the raw JSON of the message for future use/flexibility
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        FOREIGN KEY (chat_id) REFERENCES chats (id),
        FOREIGN KEY (sender_id) REFERENCES users (id),
        UNIQUE (chat_id, telegram_message_id) -- Ensures message_id is unique per chat
    );

-- 2. Copy data from the old messages table to the new one.
-- This assumes the old table exists and has compatible columns.
-- If there are existing violations of the new UNIQUE(chat_id, telegram_message_id) constraint 
-- in your current data, this INSERT might fail. You might need to clean data manually first
-- or use INSERT OR IGNORE / INSERT OR REPLACE if appropriate for your data integrity needs.
-- For now, we assume a direct copy is intended.
INSERT INTO
    messages_new (
        id,
        telegram_message_id,
        chat_id,
        sender_id,
        text,
        sent_at,
        raw_message,
        created_at
    )
SELECT
    id,
    telegram_message_id,
    chat_id,
    sender_id,
    text,
    sent_at,
    raw_message,
    created_at
FROM
    messages;

-- 3. Drop the old messages table.
DROP TABLE messages;

-- 4. Rename the new table to the original name.
ALTER TABLE messages_new
RENAME TO messages;

-- 5. Recreate indexes that were on the old messages table (if any beyond PK and the new UNIQUE constraint).
-- Our previous migration created these, so let's ensure they are on the new table.
CREATE INDEX IF NOT EXISTS idx_messages_chat_id ON messages (chat_id);

CREATE INDEX IF NOT EXISTS idx_messages_sender_id ON messages (sender_id);

CREATE INDEX IF NOT EXISTS idx_messages_sent_at ON messages (sent_at);
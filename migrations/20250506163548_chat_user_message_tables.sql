-- Add migration script here
-- Users table: Stores information about Telegram users encountered by the bot.
CREATE TABLE
    IF NOT EXISTS users (
        id INTEGER PRIMARY KEY AUTOINCREMENT, -- Local database ID
        telegram_id BIGINT UNIQUE NOT NULL, -- Telegram's unique user ID
        username TEXT, -- Telegram username (can be null)
        first_name TEXT NOT NULL,
        last_name TEXT, -- Telegram last name (can be null)
        is_bot BOOLEAN NOT NULL DEFAULT FALSE,
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

-- Index on telegram_id for faster lookups
CREATE INDEX IF NOT EXISTS idx_users_telegram_id ON users (telegram_id);

-- Chats table: Stores information about chats where the bot is active.
CREATE TABLE
    IF NOT EXISTS chats (
        id INTEGER PRIMARY KEY AUTOINCREMENT, -- Local database ID
        telegram_id BIGINT UNIQUE NOT NULL, -- Telegram's unique chat ID
        type TEXT NOT NULL, -- 'private', 'group', 'supergroup', 'channel'
        title TEXT, -- Chat title (for groups, supergroups, channels)
        username TEXT, -- Chat username (for channels and some supergroups, can be null)
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

-- Index on telegram_id for faster lookups
CREATE INDEX IF NOT EXISTS idx_chats_telegram_id ON chats (telegram_id);

-- Messages table: Stores messages received by the bot.
CREATE TABLE
    IF NOT EXISTS messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT, -- Local database ID
        telegram_message_id BIGINT UNIQUE NOT NULL, -- Telegram's unique message ID within a chat
        chat_id INTEGER NOT NULL, -- Foreign key to our chats table
        sender_id INTEGER NOT NULL, -- Foreign key to our users table (who sent the message)
        text TEXT, -- Message text content (can be null for non-text messages)
        sent_at TIMESTAMP NOT NULL, -- Timestamp when the message was sent (from Telegram)
        raw_message TEXT, -- Store the raw JSON of the message for future use/flexibility
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        FOREIGN KEY (chat_id) REFERENCES chats (id),
        FOREIGN KEY (sender_id) REFERENCES users (id)
    );

-- Index on chat_id and sender_id for common queries
CREATE INDEX IF NOT EXISTS idx_messages_chat_id ON messages (chat_id);

CREATE INDEX IF NOT EXISTS idx_messages_sender_id ON messages (sender_id);

-- Index on sent_at for time-based queries
CREATE INDEX IF NOT EXISTS idx_messages_sent_at ON messages (sent_at);

-- Optional: Triggers to update `updated_at` timestamps
CREATE TRIGGER IF NOT EXISTS users_updated_at AFTER
UPDATE ON users FOR EACH ROW BEGIN
UPDATE users
SET
    updated_at = CURRENT_TIMESTAMP
WHERE
    id = OLD.id;

END;

CREATE TRIGGER IF NOT EXISTS chats_updated_at AFTER
UPDATE ON chats FOR EACH ROW BEGIN
UPDATE chats
SET
    updated_at = CURRENT_TIMESTAMP
WHERE
    id = OLD.id;

END;
-- add migration script here
-- users table: stores information about telegram users encountered by the bot.
CREATE TABLE
    IF NOT EXISTS users (
        id SERIAL PRIMARY KEY, -- local database id
        telegram_id BIGINT UNIQUE NOT NULL, -- telegram's unique user id
        username TEXT, -- telegram username (can be null)
        first_name TEXT NOT NULL,
        last_name TEXT, -- telegram last name (can be null)
        is_bot BOOLEAN NOT NULL DEFAULT FALSE,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW (),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW () -- application will manage updates
    );

-- index on telegram_id for faster lookups
CREATE INDEX IF NOT EXISTS idx_users_telegram_id ON users (telegram_id);

-- chats table: stores information about chats where the bot is active.
CREATE TABLE
    IF NOT EXISTS chats (
        id SERIAL PRIMARY KEY, -- local database id
        telegram_id BIGINT UNIQUE NOT NULL, -- telegram's unique chat id
        type TEXT NOT NULL, -- 'private', 'group', 'supergroup', 'channel'
        title TEXT, -- chat title (for groups, supergroups, channels)
        username TEXT, -- chat username (for channels and some supergroups, can be null)
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW (),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW () -- application will manage updates
    );

-- index on telegram_id for faster lookups
CREATE INDEX IF NOT EXISTS idx_chats_telegram_id ON chats (telegram_id);

-- messages table: stores messages received by the bot.
CREATE TABLE
    IF NOT EXISTS messages (
        id SERIAL PRIMARY KEY, -- local database id
        telegram_message_id BIGINT NOT NULL, -- telegram's unique message id (scoped to chat_id)
        chat_id INTEGER NOT NULL, -- foreign key to our chats table (should match SERIAL type)
        sender_id INTEGER NOT NULL, -- foreign key to our users table (should match SERIAL type)
        text TEXT, -- message text content (can be null for non-text messages)
        sent_at TIMESTAMPTZ NOT NULL, -- timestamp when the message was sent (from telegram)
        raw_message TEXT, -- store the raw json of the message for future use/flexibility
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW (),
        FOREIGN KEY (chat_id) REFERENCES chats (id),
        FOREIGN KEY (sender_id) REFERENCES users (id),
        UNIQUE (chat_id, telegram_message_id) -- ensures message_id is unique per chat
    );

-- index on chat_id and sender_id for common queries
CREATE INDEX IF NOT EXISTS idx_messages_chat_id ON messages (chat_id);

CREATE INDEX IF NOT EXISTS idx_messages_sender_id ON messages (sender_id);

-- index on sent_at for time-based queries
CREATE INDEX IF NOT EXISTS idx_messages_sent_at ON messages (sent_at);
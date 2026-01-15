-- Add last_bot_message_at column to track when the bot last replied in a chat
-- This is used to fetch missed messages (messages since last bot response) for context
ALTER TABLE chats ADD COLUMN IF NOT EXISTS last_bot_message_at TIMESTAMPTZ NULL;

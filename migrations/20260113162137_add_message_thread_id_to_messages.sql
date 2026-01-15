-- Add message_thread_id column to track forum topic messages
ALTER TABLE messages
ADD COLUMN IF NOT EXISTS message_thread_id BIGINT NULL;

-- Index for efficient lookups by topic within a chat
CREATE INDEX IF NOT EXISTS idx_messages_chat_thread ON messages (chat_id, message_thread_id);

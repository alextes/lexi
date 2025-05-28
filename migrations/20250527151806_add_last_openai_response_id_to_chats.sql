-- Add migration script here
-- add a new column to the chats table to store the id of the last openai response
ALTER TABLE chats
ADD COLUMN last_openai_response_id TEXT NULL;
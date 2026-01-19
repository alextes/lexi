-- Scheduled jobs table for cron-based task execution
CREATE TABLE scheduled_jobs (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    cron_schedule TEXT NOT NULL,          -- e.g., "0 9 * * 1-5" (9am weekdays)
    prompt TEXT NOT NULL,                  -- The prompt to send to OpenAI
    telegram_chat_id BIGINT NOT NULL,      -- Target chat
    message_thread_id BIGINT,              -- Optional: target topic/thread
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for efficient enabled job lookup
CREATE INDEX idx_scheduled_jobs_enabled ON scheduled_jobs(enabled) WHERE enabled = true;

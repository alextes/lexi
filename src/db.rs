use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::{de::DeserializeOwned, Serialize};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr;

use crate::key_value_store::{KeyValueStore, PostgresKeyValueStore};
use crate::scheduler::types::ScheduledJob;
use crate::telegram::types::{
    Chat as TelegramChat, Message as TelegramMessage, User as TelegramUser,
};

#[allow(async_fn_in_trait)]
#[automock]
pub trait Db {
    async fn upsert_user(&self, user_data: &TelegramUser) -> Result<i32>;
    async fn upsert_chat(&self, chat_data: &TelegramChat) -> Result<i32>;
    async fn insert_message(
        &self,
        message_data: &TelegramMessage,
        local_chat_id: i32,
        local_user_id: i32,
        raw_message_json: &str,
    ) -> Result<i32>;
    async fn get_message_history(
        &self,
        local_chat_id: i32,
        limit: i64,
    ) -> Result<Vec<MessageFromHistory>>;
    async fn get_last_openai_response_id(&self, local_chat_id: i32) -> Result<Option<String>>;
    async fn update_last_openai_response_id(
        &self,
        local_chat_id: i32,
        response_id: &str,
    ) -> Result<()>;
    async fn clear_last_openai_response_id(&self, local_chat_id: i32) -> Result<()>;
    // NOTE: 'static keeps mockall expectations sound (mock storage is 'static).
    async fn set_kv<T: Serialize + 'static>(&self, key: &str, value: &T) -> Result<()>;
    // NOTE: 'static keeps mockall expectations sound (mock storage is 'static).
    async fn get_kv<T: DeserializeOwned + 'static>(
        &self,
        key: &str,
    ) -> Result<Option<(T, DateTime<Utc>)>>;
    async fn get_last_bot_message_at(&self, local_chat_id: i32) -> Result<Option<DateTime<Utc>>>;
    async fn update_last_bot_message_at(
        &self,
        local_chat_id: i32,
        timestamp: DateTime<Utc>,
    ) -> Result<()>;
    async fn get_messages_since(
        &self,
        local_chat_id: i32,
        message_thread_id: Option<i64>,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<MessageWithSender>>;
    async fn get_enabled_scheduled_jobs(&self) -> Result<Vec<ScheduledJob>>;
    async fn get_scheduled_jobs_for_chat(
        &self,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<Vec<ScheduledJob>>;
    async fn create_scheduled_job(
        &self,
        name: &str,
        cron_schedule: &str,
        prompt: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<i32>;
    async fn delete_scheduled_job_by_name(
        &self,
        name: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<bool>;
    async fn set_scheduled_job_enabled(
        &self,
        name: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
        enabled: bool,
    ) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct PostgresDb {
    pool: PgPool,
    key_value_store: PostgresKeyValueStore,
}

impl PostgresDb {
    pub async fn new(database_url: &str) -> Result<Self> {
        let options = PgConnectOptions::from_str(database_url)
            .with_context(|| format!("failed to parse database_url: '{database_url}'"))?;

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect_with(options)
            .await
            .with_context(|| {
                format!("failed to connect to postgresql database at {database_url}")
            })?;

        tracing::info!("running database migrations (postgresql)... ");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .with_context(|| "failed to run database migrations")?;
        tracing::info!("database migrations complete (postgresql).");

        let key_value_store = PostgresKeyValueStore::new(pool.clone());
        Ok(PostgresDb {
            pool,
            key_value_store,
        })
    }

    pub fn new_from_pool(pool: PgPool) -> Self {
        let key_value_store = PostgresKeyValueStore::new(pool.clone());
        Self {
            pool,
            key_value_store,
        }
    }
}

impl Db for PostgresDb {
    /// upsert a user: insert if not exists (based on telegram_id), or update if exists.
    /// returns the local database id of the user.
    async fn upsert_user(&self, user_data: &TelegramUser) -> Result<i32> {
        let query_result = sqlx::query!(
            r#"
            INSERT INTO users (telegram_id, username, first_name, last_name, is_bot, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, now(), now())
            ON CONFLICT (telegram_id) DO UPDATE SET
                username = EXCLUDED.username,
                first_name = EXCLUDED.first_name,
                last_name = EXCLUDED.last_name,
                is_bot = EXCLUDED.is_bot,
                updated_at = now()
            RETURNING id;
            "#,
            user_data.id,
            user_data.username,
            user_data.first_name,
            user_data.last_name,
            user_data.is_bot
        )
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("failed to upsert user with telegram_id {}", user_data.id))?;

        Ok(query_result.id)
    }

    /// upsert a chat: insert if not exists (based on telegram_id), or update if exists.
    /// returns the local database id of the chat.
    async fn upsert_chat(&self, chat_data: &TelegramChat) -> Result<i32> {
        let query_result = sqlx::query!(
            r#"
            INSERT INTO chats (telegram_id, type, title, username, created_at, updated_at)
            VALUES ($1, $2, $3, $4, now(), now())
            ON CONFLICT (telegram_id) DO UPDATE SET
                type = EXCLUDED.type,
                title = EXCLUDED.title,
                username = EXCLUDED.username,
                updated_at = now()
            RETURNING id;
            "#,
            chat_data.id,
            chat_data.chat_type,
            chat_data.title,
            chat_data.username
        )
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("failed to upsert chat with telegram_id {}", chat_data.id))?;

        Ok(query_result.id)
    }

    // insert a message. if it already exists (based on unique(chat_id, telegram_message_id)), ignore the insert.
    // returns the local database id of the message (either newly inserted or existing if ignored, though a return of 0 from last_insert_rowid() after ignore typically means no new row was inserted).
    // for simplicity, we will return the last_insert_rowid. if it's 0 after an ignore, the calling code should understand no new row was made.
    // a more robust way would be to select the id after an insert or ignore if a consistent id is always needed.
    async fn insert_message(
        &self,
        message_data: &TelegramMessage,
        local_chat_id: i32,
        local_user_id: i32,
        raw_message_json: &str,
    ) -> Result<i32> {
        let sent_at_datetime =
            DateTime::<Utc>::from_timestamp(message_data.date, 0).with_context(|| {
                format!(
                    "failed to convert telegram message date {} to naivedatetime",
                    message_data.date
                )
            })?;

        let insert_result = sqlx::query!(
            r#"
            INSERT INTO messages (telegram_message_id, chat_id, sender_id, text, sent_at, raw_message, message_thread_id, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, now())
            ON CONFLICT (chat_id, telegram_message_id) DO NOTHING
            RETURNING id;
            "#,
            message_data.message_id,
            local_chat_id,
            local_user_id,
            message_data.text,
            sent_at_datetime,
            raw_message_json,
            message_data.message_thread_id
        )
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("failed to execute insert_or_do_nothing for message with telegram_message_id {}", message_data.message_id))?;

        if let Some(row) = insert_result {
            tracing::debug!(
                telegram_message_id = message_data.message_id,
                chat_id = local_chat_id,
                "new message inserted, row_id: {}",
                row.id
            );
            Ok(row.id)
        } else {
            tracing::debug!(
                telegram_message_id = message_data.message_id,
                chat_id = local_chat_id,
                "message already existed, insert ignored. fetching existing id."
            );
            let existing_row = sqlx::query!(
                "SELECT id FROM messages WHERE telegram_message_id = $1 AND chat_id = $2",
                message_data.message_id,
                local_chat_id
            )
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("failed to fetch id for existing message (tg_id: {}, chat_id: {}) after insert_or_do_nothing", message_data.message_id, local_chat_id))?;
            Ok(existing_row.id)
        }
    }

    /// fetches the last n messages from a given chat for building conversation history.
    /// orders by sent_at ascending (oldest of the n to newest).
    async fn get_message_history(
        &self,
        local_chat_id: i32,
        limit: i64,
    ) -> Result<Vec<MessageFromHistory>> {
        let messages = sqlx::query_as!(
            MessageFromHistory,
            r#"
            SELECT sender_id, text --, sent_at, u.username -- include username if joining with users table
            FROM messages m
            -- optional: join users u on m.sender_id = u.id 
            WHERE m.chat_id = $1
            ORDER BY m.sent_at DESC -- get the latest n first
            LIMIT $2;
            "#,
            local_chat_id,
            limit
        )
        .fetch_all(&self.pool)
        .await
        .with_context(|| {
            format!(
                "failed to fetch message history for chat_id {local_chat_id} with limit {limit}"
            )
        })?;

        // the query gets latest n, but openai expects history oldest to newest.
        Ok(messages.into_iter().rev().collect())
    }

    /// retrieves the last_openai_response_id for a given chat.
    async fn get_last_openai_response_id(&self, local_chat_id: i32) -> Result<Option<String>> {
        let result = sqlx::query!(
            "SELECT last_openai_response_id FROM chats WHERE id = $1",
            local_chat_id
        )
        .fetch_one(&self.pool)
        .await
        .with_context(|| {
            format!("failed to fetch last_openai_response_id for chat_id {local_chat_id}")
        })?;
        Ok(result.last_openai_response_id)
    }

    /// updates the last_openai_response_id for a given chat.
    async fn update_last_openai_response_id(
        &self,
        local_chat_id: i32,
        response_id: &str,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE chats SET last_openai_response_id = $1, updated_at = now() WHERE id = $2",
            response_id,
            local_chat_id
        )
        .execute(&self.pool)
        .await
        .with_context(|| {
            format!(
                "failed to update last_openai_response_id for chat_id {local_chat_id} to {response_id}"
            )
        })?;
        Ok(())
    }

    /// clears the last_openai_response_id (sets to null) for a given chat.
    async fn clear_last_openai_response_id(&self, local_chat_id: i32) -> Result<()> {
        sqlx::query!(
            "UPDATE chats SET last_openai_response_id = null, updated_at = now() WHERE id = $1",
            local_chat_id
        )
        .execute(&self.pool)
        .await
        .with_context(|| {
            format!("failed to clear last_openai_response_id for chat_id {local_chat_id}")
        })?;
        Ok(())
    }

    async fn set_kv<T: Serialize + 'static>(&self, key: &str, value: &T) -> Result<()> {
        self.key_value_store.set(key, value).await
    }

    async fn get_kv<T: DeserializeOwned + 'static>(
        &self,
        key: &str,
    ) -> Result<Option<(T, DateTime<Utc>)>> {
        self.key_value_store.get(key).await
    }

    /// Retrieves the last_bot_message_at timestamp for a given chat.
    async fn get_last_bot_message_at(&self, local_chat_id: i32) -> Result<Option<DateTime<Utc>>> {
        let result = sqlx::query!(
            "SELECT last_bot_message_at FROM chats WHERE id = $1",
            local_chat_id
        )
        .fetch_one(&self.pool)
        .await
        .with_context(|| {
            format!("failed to fetch last_bot_message_at for chat_id {local_chat_id}")
        })?;
        Ok(result.last_bot_message_at)
    }

    /// Updates the last_bot_message_at timestamp for a given chat.
    async fn update_last_bot_message_at(
        &self,
        local_chat_id: i32,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE chats SET last_bot_message_at = $1, updated_at = now() WHERE id = $2",
            timestamp,
            local_chat_id
        )
        .execute(&self.pool)
        .await
        .with_context(|| {
            format!("failed to update last_bot_message_at for chat_id {local_chat_id}")
        })?;
        Ok(())
    }

    /// Fetches messages since a given timestamp, with sender information.
    /// Used to build context from missed messages between bot responses.
    async fn get_messages_since(
        &self,
        local_chat_id: i32,
        message_thread_id: Option<i64>,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<MessageWithSender>> {
        // Use different queries based on whether we're filtering by thread_id
        let messages = match message_thread_id {
            Some(thread_id) => {
                sqlx::query_as!(
                    MessageWithSender,
                    r#"
                    SELECT
                        m.text,
                        u.username as sender_username,
                        u.first_name as sender_first_name,
                        m.sender_id,
                        u.is_bot,
                        m.sent_at
                    FROM messages m
                    JOIN users u ON m.sender_id = u.id
                    WHERE m.chat_id = $1
                      AND m.message_thread_id = $2
                      AND m.sent_at > $3
                    ORDER BY m.sent_at ASC
                    LIMIT $4
                    "#,
                    local_chat_id,
                    thread_id,
                    since,
                    limit
                )
                .fetch_all(&self.pool)
                .await
                .with_context(|| {
                    format!(
                        "failed to fetch messages since {since} for chat_id {local_chat_id}, thread_id {thread_id}"
                    )
                })?
            }
            None => {
                sqlx::query_as!(
                    MessageWithSender,
                    r#"
                    SELECT
                        m.text,
                        u.username as sender_username,
                        u.first_name as sender_first_name,
                        m.sender_id,
                        u.is_bot,
                        m.sent_at
                    FROM messages m
                    JOIN users u ON m.sender_id = u.id
                    WHERE m.chat_id = $1
                      AND m.message_thread_id IS NULL
                      AND m.sent_at > $2
                    ORDER BY m.sent_at ASC
                    LIMIT $3
                    "#,
                    local_chat_id,
                    since,
                    limit
                )
                .fetch_all(&self.pool)
                .await
                .with_context(|| {
                    format!(
                        "failed to fetch messages since {since} for chat_id {local_chat_id} (no thread)"
                    )
                })?
            }
        };

        Ok(messages)
    }

    /// Fetches all enabled scheduled jobs from the database.
    async fn get_enabled_scheduled_jobs(&self) -> Result<Vec<ScheduledJob>> {
        let jobs = sqlx::query_as!(
            ScheduledJob,
            r#"
            SELECT id, name, cron_schedule, prompt, telegram_chat_id, message_thread_id, enabled, created_at, updated_at
            FROM scheduled_jobs
            WHERE enabled = true
            ORDER BY id ASC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .with_context(|| "failed to fetch enabled scheduled jobs")?;

        Ok(jobs)
    }

    /// Fetches all scheduled jobs for a specific chat/thread.
    async fn get_scheduled_jobs_for_chat(
        &self,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<Vec<ScheduledJob>> {
        let jobs = match message_thread_id {
            Some(thread_id) => {
                sqlx::query_as!(
                    ScheduledJob,
                    r#"
                    SELECT id, name, cron_schedule, prompt, telegram_chat_id, message_thread_id, enabled, created_at, updated_at
                    FROM scheduled_jobs
                    WHERE telegram_chat_id = $1 AND message_thread_id = $2
                    ORDER BY name ASC
                    "#,
                    telegram_chat_id,
                    thread_id
                )
                .fetch_all(&self.pool)
                .await
                .with_context(|| format!("failed to fetch scheduled jobs for chat {telegram_chat_id} thread {thread_id}"))?
            }
            None => {
                sqlx::query_as!(
                    ScheduledJob,
                    r#"
                    SELECT id, name, cron_schedule, prompt, telegram_chat_id, message_thread_id, enabled, created_at, updated_at
                    FROM scheduled_jobs
                    WHERE telegram_chat_id = $1 AND message_thread_id IS NULL
                    ORDER BY name ASC
                    "#,
                    telegram_chat_id
                )
                .fetch_all(&self.pool)
                .await
                .with_context(|| format!("failed to fetch scheduled jobs for chat {telegram_chat_id}"))?
            }
        };

        Ok(jobs)
    }

    /// Creates a new scheduled job.
    async fn create_scheduled_job(
        &self,
        name: &str,
        cron_schedule: &str,
        prompt: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<i32> {
        let result = sqlx::query!(
            r#"
            INSERT INTO scheduled_jobs (name, cron_schedule, prompt, telegram_chat_id, message_thread_id, enabled, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, true, NOW(), NOW())
            RETURNING id
            "#,
            name,
            cron_schedule,
            prompt,
            telegram_chat_id,
            message_thread_id
        )
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("failed to create scheduled job '{name}'"))?;

        Ok(result.id)
    }

    /// Deletes a scheduled job by name for a specific chat/thread.
    /// Returns true if a job was deleted, false if no matching job was found.
    async fn delete_scheduled_job_by_name(
        &self,
        name: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
    ) -> Result<bool> {
        let result = match message_thread_id {
            Some(thread_id) => sqlx::query!(
                r#"
                    DELETE FROM scheduled_jobs
                    WHERE name = $1 AND telegram_chat_id = $2 AND message_thread_id = $3
                    "#,
                name,
                telegram_chat_id,
                thread_id
            )
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to delete scheduled job '{name}'"))?,
            None => sqlx::query!(
                r#"
                    DELETE FROM scheduled_jobs
                    WHERE name = $1 AND telegram_chat_id = $2 AND message_thread_id IS NULL
                    "#,
                name,
                telegram_chat_id
            )
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to delete scheduled job '{name}'"))?,
        };

        Ok(result.rows_affected() > 0)
    }

    /// Sets the enabled status of a scheduled job by name for a specific chat/thread.
    /// Returns true if a job was updated, false if no matching job was found.
    async fn set_scheduled_job_enabled(
        &self,
        name: &str,
        telegram_chat_id: i64,
        message_thread_id: Option<i64>,
        enabled: bool,
    ) -> Result<bool> {
        let result = match message_thread_id {
            Some(thread_id) => sqlx::query!(
                r#"
                    UPDATE scheduled_jobs
                    SET enabled = $1, updated_at = NOW()
                    WHERE name = $2 AND telegram_chat_id = $3 AND message_thread_id = $4
                    "#,
                enabled,
                name,
                telegram_chat_id,
                thread_id
            )
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to update scheduled job '{name}'"))?,
            None => sqlx::query!(
                r#"
                    UPDATE scheduled_jobs
                    SET enabled = $1, updated_at = NOW()
                    WHERE name = $2 AND telegram_chat_id = $3 AND message_thread_id IS NULL
                    "#,
                enabled,
                name,
                telegram_chat_id
            )
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to update scheduled job '{name}'"))?,
        };

        Ok(result.rows_affected() > 0)
    }
}

// struct to represent a message from history for openai
#[derive(Debug)]
pub struct MessageFromHistory {
    pub sender_id: i32,
    pub text: Option<String>,
    // pub sent_at: DateTime<Utc>, // not directly needed for openai message construction yet
    // pub username: Option<String>, // potentially useful for the 'name' field in openai message
}

/// Message with sender information for building conversation context
#[derive(Debug, Clone)]
pub struct MessageWithSender {
    pub text: Option<String>,
    pub sender_username: Option<String>,
    pub sender_first_name: String,
    pub sender_id: i32,
    pub is_bot: bool,
    pub sent_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::types::{
        Chat as TelegramChat, Message as TelegramMessage, User as TelegramUser,
    };
    use chrono::Utc;
    use serde_json;
    use sqlx::PgPool;
    use sqlx::Row;

    // helper to create a mock telegram user
    fn mock_telegram_user(id: i64, username: &str) -> TelegramUser {
        TelegramUser {
            id,
            is_bot: false,
            first_name: "test_first".to_string(),
            last_name: Some("test_last".to_string()),
            username: Some(username.to_string()),
        }
    }

    // helper to create a mock telegram chat
    fn mock_telegram_chat(id: i64, chat_type: &str, title: &str) -> TelegramChat {
        TelegramChat {
            id,
            chat_type: chat_type.to_string(),
            title: Some(title.to_string()),
            username: Some(format!("{}username", title.to_lowercase())),
            first_name: None,
            last_name: None,
        }
    }

    // helper to create a mock telegram message
    fn mock_telegram_message(
        id: i64,
        text: &str,
        from_user: TelegramUser,
        chat: TelegramChat,
    ) -> TelegramMessage {
        TelegramMessage {
            message_id: id,
            message_thread_id: None,
            from: Some(from_user),
            date: Utc::now().timestamp(),
            chat,
            text: Some(text.to_string()),
            entities: None,
        }
    }

    #[sqlx::test]
    async fn test_upsert_user(_pool: PgPool) -> Result<()> {
        // _pool is no longer directly used, but sqlx::test requires it.
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };
        let user_data = mock_telegram_user(12345, "testuser");

        // first insert
        let user_id = db.upsert_user(&user_data).await?;
        assert!(user_id > 0);

        // try to insert again (should update)
        let updated_user_data = TelegramUser {
            first_name: "updated_first".to_string(),
            ..user_data.clone()
        };
        let updated_user_id = db.upsert_user(&updated_user_data).await?;
        assert_eq!(user_id, updated_user_id);

        // verify update
        let fetched_user = sqlx::query!("SELECT first_name FROM users WHERE id = $1", user_id)
            .fetch_one(&db.pool) // access pool via db instance for direct verification query
            .await?;
        assert_eq!(fetched_user.first_name, "updated_first");

        Ok(())
    }

    #[sqlx::test]
    async fn test_upsert_chat(_pool: PgPool) -> Result<()> {
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };
        let chat_data = mock_telegram_chat(67890, "private", "testchat");

        // first insert
        let chat_id = db.upsert_chat(&chat_data).await?;
        assert!(chat_id > 0);

        // try to insert again (should update)
        let updated_chat_data = TelegramChat {
            title: Some("updated_chat_title".to_string()),
            ..chat_data.clone()
        };
        let updated_chat_id = db.upsert_chat(&updated_chat_data).await?;
        assert_eq!(chat_id, updated_chat_id);

        // verify update
        let fetched_chat = sqlx::query!("SELECT title FROM chats WHERE id = $1", chat_id)
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(fetched_chat.title, Some("updated_chat_title".to_string()));

        Ok(())
    }

    #[sqlx::test]
    async fn test_insert_message(_pool: PgPool) -> Result<()> {
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };
        let user_data = mock_telegram_user(111, "msguser");
        let local_user_id = db.upsert_user(&user_data).await?;

        let chat_data = mock_telegram_chat(222, "group", "message_chat");
        let local_chat_id = db.upsert_chat(&chat_data).await?;

        let message_data =
            mock_telegram_message(333, "hello world", user_data.clone(), chat_data.clone());
        let raw_json = serde_json::to_string(&message_data)?;

        // first insert
        let message_id = db
            .insert_message(&message_data, local_chat_id, local_user_id, &raw_json)
            .await?;
        assert!(message_id > 0);

        // try to insert the same message again (should be ignored, but return existing id)
        let message_id_again = db
            .insert_message(&message_data, local_chat_id, local_user_id, &raw_json)
            .await?;
        assert_eq!(message_id, message_id_again);

        // verify insertion by fetching
        let fetched_message = sqlx::query!("SELECT text FROM messages WHERE id = $1", message_id)
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(fetched_message.text, Some("hello world".to_string()));

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_message_history(_pool: PgPool) -> Result<()> {
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };
        let user = mock_telegram_user(777, "histuser");
        let local_user_id = db.upsert_user(&user).await?;
        let chat = mock_telegram_chat(888, "private", "history_chat");
        let local_chat_id = db.upsert_chat(&chat).await?;

        // insert a few messages
        for i in 0..5 {
            let msg_text = format!("message {i}");
            // make sure sent_at is distinct and in order for testing
            let msg_tg = TelegramMessage {
                message_id: 1000 + i,
                message_thread_id: None,
                from: Some(user.clone()),
                chat: chat.clone(),
                date: Utc::now().timestamp() + i, // ensure distinct timestamps
                text: Some(msg_text.clone()),
                entities: None,
            };
            let raw_json = serde_json::to_string(&msg_tg)?;
            db.insert_message(&msg_tg, local_chat_id, local_user_id, &raw_json)
                .await?;
        }

        // get last 3 messages
        let history = db.get_message_history(local_chat_id, 3).await?;
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].text, Some("message 2".to_string()));
        assert_eq!(history[1].text, Some("message 3".to_string()));
        assert_eq!(history[2].text, Some("message 4".to_string()));
        assert_eq!(history[0].sender_id, local_user_id);

        // get more messages than exist
        let history_all = db.get_message_history(local_chat_id, 10).await?;
        assert_eq!(history_all.len(), 5);
        assert_eq!(history_all[0].text, Some("message 0".to_string()));

        Ok(())
    }

    #[sqlx::test]
    async fn test_openai_response_id_handling(_pool: PgPool) -> Result<()> {
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };
        let chat_data = mock_telegram_chat(999, "group", "openai_chat");
        let local_chat_id = db.upsert_chat(&chat_data).await?;

        // initially, should be none
        let initial_response_id = db.get_last_openai_response_id(local_chat_id).await?;
        assert!(initial_response_id.is_none());

        // update it
        let test_response_id = "resp_12345";
        db.update_last_openai_response_id(local_chat_id, test_response_id)
            .await?;

        // get it back and verify
        let fetched_response_id = db.get_last_openai_response_id(local_chat_id).await?;
        assert_eq!(fetched_response_id, Some(test_response_id.to_string()));

        // clear it
        db.clear_last_openai_response_id(local_chat_id).await?;

        // get it back and verify it's none
        let cleared_response_id = db.get_last_openai_response_id(local_chat_id).await?;
        assert!(cleared_response_id.is_none());

        Ok(())
    }

    #[sqlx::test]
    async fn test_postgres_db_new_and_migrations(_pool: PgPool) -> Result<()> {
        // we create an instance of postgresdb using the new method.
        // the new method itself contains the migration logic.
        let db = PostgresDb {
            pool: _pool.clone(),
            key_value_store: PostgresKeyValueStore::new(_pool.clone()),
        };

        // we can do a simple query to ensure the pool is usable.
        let row = sqlx::query("SELECT 1 as one").fetch_one(&db.pool).await?;
        // the actual type will depend on the db, for postgres this is i32 with sqlx::query_scalar!
        // but with query! it is a struct with a field `one`. here we just check it exists.
        let _one: Option<i32> = row.try_get("one").ok(); // example, adapt if needed for your db
        assert!(_one.is_some());
        assert_eq!(_one, Some(1));

        Ok(())
    }
}

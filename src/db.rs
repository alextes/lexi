use chrono::{DateTime, Utc};
use eyre::{Context, ContextCompat, Result};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr;

use crate::telegram::types::{
    Chat as TelegramChat, Message as TelegramMessage, User as TelegramUser,
};

// Initialize database connection and run migrations
pub async fn initialize_database(database_url: &str) -> Result<PgPool> {
    let options = PgConnectOptions::from_str(database_url)
        .wrap_err_with(|| format!("failed to parse database_url: '{}'", database_url))?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect_with(options)
        .await
        .wrap_err_with(|| {
            format!(
                "failed to connect to postgresql database at {}",
                database_url
            )
        })?;

    tracing::info!("running database migrations (postgresql)... ");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .wrap_err_with(|| "failed to run database migrations")?;
    tracing::info!("database migrations complete (postgresql).");

    Ok(pool)
}

// Upsert a user: insert if not exists (based on telegram_id), or update if exists.
// Returns the local database ID of the user.
pub async fn upsert_user(pool: &PgPool, user_data: &TelegramUser) -> Result<i32> {
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
    .fetch_one(pool)
    .await
    .wrap_err_with(|| format!("failed to upsert user with telegram_id {}", user_data.id))?;

    Ok(query_result.id)
}

// Upsert a chat: insert if not exists (based on telegram_id), or update if exists.
// Returns the local database ID of the chat.
pub async fn upsert_chat(pool: &PgPool, chat_data: &TelegramChat) -> Result<i32> {
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
    .fetch_one(pool)
    .await
    .wrap_err_with(|| format!("failed to upsert chat with telegram_id {}", chat_data.id))?;

    Ok(query_result.id)
}

// Insert a message. If it already exists (based on UNIQUE(chat_id, telegram_message_id)), ignore the insert.
// Returns the local database ID of the message (either newly inserted or existing if ignored, though a return of 0 from last_insert_rowid() after IGNORE typically means no new row was inserted).
// For simplicity, we will return the last_insert_rowid. If it's 0 after an IGNORE, the calling code should understand no new row was made.
// A more robust way would be to SELECT the ID after an INSERT OR IGNORE if a consistent ID is always needed.
pub async fn insert_message(
    pool: &PgPool,
    message_data: &TelegramMessage,
    local_chat_id: i32,
    local_user_id: i32,
    raw_message_json: &str,
) -> Result<i32> {
    let sent_at_datetime =
        DateTime::<Utc>::from_timestamp(message_data.date, 0).wrap_err_with(|| {
            format!(
                "failed to convert telegram message date {} to naivedatetime",
                message_data.date
            )
        })?;

    let insert_result = sqlx::query!(
        r#"
        INSERT INTO messages (telegram_message_id, chat_id, sender_id, text, sent_at, raw_message, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, now())
        ON CONFLICT (chat_id, telegram_message_id) DO NOTHING
        RETURNING id;
        "#,
        message_data.message_id,
        local_chat_id,
        local_user_id,
        message_data.text,
        sent_at_datetime,
        raw_message_json
    )
    .fetch_optional(pool)
    .await
    .wrap_err_with(|| format!("failed to execute insert_or_do_nothing for message with telegram_message_id {}", message_data.message_id))?;

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
        .fetch_one(pool)
        .await
        .wrap_err_with(|| format!("failed to fetch id for existing message (tg_id: {}, chat_id: {}) after insert_or_do_nothing", message_data.message_id, local_chat_id))?;
        Ok(existing_row.id)
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

// fetches the last N messages from a given chat for building conversation history.
// orders by sent_at ascending (oldest of the N to newest).
pub async fn get_message_history(
    pool: &PgPool,
    local_chat_id: i32,
    limit: i64,
) -> Result<Vec<MessageFromHistory>> {
    let messages = sqlx::query_as!(
        MessageFromHistory,
        r#"
        SELECT sender_id, text --, sent_at, u.username -- include username if joining with users table
        FROM messages m
        -- Optional: JOIN users u ON m.sender_id = u.id 
        WHERE m.chat_id = $1
        ORDER BY m.sent_at DESC -- get the latest N first
        LIMIT $2;
        "#,
        local_chat_id,
        limit
    )
    .fetch_all(pool)
    .await
    .wrap_err_with(|| {
        format!(
            "failed to fetch message history for chat_id {} with limit {}",
            local_chat_id,
            limit
        )
    })?;

    // The query gets latest N, but openai expects history oldest to newest.
    Ok(messages.into_iter().rev().collect())
}

// retrieves the last_openai_response_id for a given chat.
pub async fn get_last_openai_response_id(
    pool: &PgPool,
    local_chat_id: i32,
) -> Result<Option<String>> {
    let result = sqlx::query!(
        "SELECT last_openai_response_id FROM chats WHERE id = $1",
        local_chat_id
    )
    .fetch_one(pool)
    .await
    .wrap_err_with(|| {
        format!(
            "failed to fetch last_openai_response_id for chat_id {}",
            local_chat_id
        )
    })?;
    Ok(result.last_openai_response_id)
}

// updates the last_openai_response_id for a given chat.
pub async fn update_last_openai_response_id(
    pool: &PgPool,
    local_chat_id: i32,
    response_id: &str,
) -> Result<()> {
    sqlx::query!(
        "UPDATE chats SET last_openai_response_id = $1, updated_at = now() WHERE id = $2",
        response_id,
        local_chat_id
    )
    .execute(pool)
    .await
    .wrap_err_with(|| {
        format!(
            "failed to update last_openai_response_id for chat_id {} to {}",
            local_chat_id, response_id
        )
    })?;
    Ok(())
}

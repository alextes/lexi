use sqlx::sqlite::{SqlitePool, SqliteConnectOptions};
use sqlx::Row;
use std::str::FromStr;
use chrono::{DateTime, Utc};
use anyhow::{Context, Result}; // Added anyhow

use crate::types::{User as TelegramUser, Chat as TelegramChat, Message as TelegramMessage};

// Initialize database connection and run migrations
pub async fn initialize_database(database_url: &str) -> Result<SqlitePool> { 
    let connect_options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("failed to parse database_url: '{}'", database_url))? // Closure for lazy eval
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    let pool = SqlitePool::connect_with(connect_options).await
        .with_context(|| "failed to connect to database")?;

    tracing::info!("running database migrations from db.rs...");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .with_context(|| "failed to run database migrations")?;
    tracing::info!("database migrations complete (from db.rs).");

    Ok(pool)
}

// Upsert a user: insert if not exists (based on telegram_id), or update if exists.
// Returns the local database ID of the user.
pub async fn upsert_user(pool: &SqlitePool, user_data: &TelegramUser) -> Result<i64> { 
    let existing_user_id: Option<i64> = sqlx::query("SELECT id FROM users WHERE telegram_id = ?1")
        .bind(user_data.id)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to fetch user for upsert, telegram_id: {}", user_data.id))?
        .map(|row: sqlx::sqlite::SqliteRow| row.get::<'_, i64, _>("id")); // Explicit type for row and get

    if let Some(id) = existing_user_id {
        // User exists, update it
        sqlx::query(
            "UPDATE users SET username = ?1, first_name = ?2, last_name = ?3, is_bot = ?4, updated_at = CURRENT_TIMESTAMP WHERE id = ?5",
        )
        .bind(&user_data.username)
        .bind(&user_data.first_name)
        .bind(&user_data.last_name)
        .bind(user_data.is_bot)
        .bind(id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to update user with telegram_id {}", user_data.id))?;
        Ok(id)
    } else {
        // User does not exist, insert it
        let result = sqlx::query(
            "INSERT INTO users (telegram_id, username, first_name, last_name, is_bot) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(user_data.id)
        .bind(&user_data.username)
        .bind(&user_data.first_name)
        .bind(&user_data.last_name)
        .bind(user_data.is_bot)
        .execute(pool)
        .await
        .with_context(|| format!("failed to insert user with telegram_id {}", user_data.id))?;
        Ok(result.last_insert_rowid())
    }
}

// Upsert a chat: insert if not exists (based on telegram_id), or update if exists.
// Returns the local database ID of the chat.
pub async fn upsert_chat(pool: &SqlitePool, chat_data: &TelegramChat) -> Result<i64> { 
    let existing_chat_id: Option<i64> = sqlx::query("SELECT id FROM chats WHERE telegram_id = ?1")
        .bind(chat_data.id)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("failed to fetch chat for upsert, telegram_id: {}", chat_data.id))?
        .map(|row: sqlx::sqlite::SqliteRow| row.get::<'_, i64, _>("id")); // Explicit type for row and get

    if let Some(id) = existing_chat_id {
        // Chat exists, update it (already correctly omits first_name, last_name)
        sqlx::query(
            "UPDATE chats SET type = ?1, title = ?2, username = ?3, updated_at = CURRENT_TIMESTAMP WHERE id = ?4",
        )
        .bind(&chat_data.chat_type)
        .bind(&chat_data.title)
        .bind(&chat_data.username) 
        .bind(id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to update chat with telegram_id {}", chat_data.id))?;
        Ok(id)
    } else {
        // Chat does not exist, insert it (remove first_name, last_name)
        let result = sqlx::query(
            "INSERT INTO chats (telegram_id, type, title, username) VALUES (?1, ?2, ?3, ?4)", // Removed first_name, last_name
        )
        .bind(chat_data.id)
        .bind(&chat_data.chat_type)
        .bind(&chat_data.title)
        .bind(&chat_data.username)
        .execute(pool)
        .await
        .with_context(|| format!("failed to insert chat with telegram_id {}", chat_data.id))?;
        Ok(result.last_insert_rowid())
    }
}


// Insert a message. If it already exists (based on UNIQUE(chat_id, telegram_message_id)), ignore the insert.
// Returns the local database ID of the message (either newly inserted or existing if ignored, though a return of 0 from last_insert_rowid() after IGNORE typically means no new row was inserted).
// For simplicity, we will return the last_insert_rowid. If it's 0 after an IGNORE, the calling code should understand no new row was made.
// A more robust way would be to SELECT the ID after an INSERT OR IGNORE if a consistent ID is always needed.
pub async fn insert_message(
    pool: &SqlitePool, 
    message_data: &TelegramMessage, 
    local_chat_id: i64, 
    local_user_id: i64, 
    raw_message_json: &str
) -> Result<i64> { 
    let sent_at_datetime = DateTime::<Utc>::from_timestamp(message_data.date, 0)
        .with_context(|| format!("failed to convert telegram message date {} to NaiveDateTime", message_data.date))?;

    // Using INSERT OR IGNORE to handle potential duplicates based on UNIQUE(chat_id, telegram_message_id)
    let result = sqlx::query(
        "INSERT OR IGNORE INTO messages (telegram_message_id, chat_id, sender_id, text, sent_at, raw_message) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(message_data.message_id)
    .bind(local_chat_id)
    .bind(local_user_id)
    .bind(&message_data.text)
    .bind(sent_at_datetime)
    .bind(raw_message_json)
    .execute(pool)
    .await
    .with_context(|| format!("failed to execute insert_or_ignore for message with telegram_message_id {}", message_data.message_id))?;

    // If rows_affected is 0, it means the IGNORE clause was triggered because the row already existed.
    // last_insert_rowid() might return 0 in this case for SQLite, or the ID of the conflicting row.
    // If a new row was inserted, it returns the new ID.
    if result.rows_affected() > 0 {
        tracing::debug!(telegram_message_id = message_data.message_id, chat_id = local_chat_id, "new message inserted, row_id: {}", result.last_insert_rowid());
    } else {
        tracing::debug!(telegram_message_id = message_data.message_id, chat_id = local_chat_id, "message already existed, insert ignored.");
        // If we need the ID of the *existing* ignored row, we'd have to SELECT it here.
        // For now, returning last_insert_rowid() which might be 0 or the conflicting row's ID is acceptable if the handler knows this.
        // A more robust approach if ID is always needed: query for it after ignore.
        // Let's query it to be safe and consistent, so the handler always gets a valid local ID.
        let existing_or_new_id = sqlx::query_scalar(
            "SELECT id FROM messages WHERE telegram_message_id = ?1 AND chat_id = ?2"
        )
        .bind(message_data.message_id)
        .bind(local_chat_id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("Failed to fetch ID for message (tg_id: {}, chat_id: {}) after insert_or_ignore", message_data.message_id, local_chat_id))?;
        return Ok(existing_or_new_id);
    }

    Ok(result.last_insert_rowid())
} 
use chrono::{DateTime, Utc};

/// A scheduled job stored in the database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduledJob {
    pub id: i32,
    pub name: String,
    pub cron_schedule: String,
    pub prompt: String,
    pub telegram_chat_id: i64,
    pub message_thread_id: Option<i64>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

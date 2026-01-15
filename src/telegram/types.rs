use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    pub error_code: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    // We can add other update types later, like edited_message, channel_post, etc.
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Message {
    pub message_id: i64,
    pub message_thread_id: Option<i64>, // Forum topic thread ID (for supergroups with topics enabled)
    pub from: Option<User>, // Optional: sender, empty for messages sent to channels
    pub chat: Chat,
    pub date: i64, // This is a Unix timestamp from Telegram
    pub text: Option<String>,
    pub entities: Option<Vec<MessageEntity>>,
    // Many other fields can be added here: photo, audio, document, etc.
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct User {
    pub id: i64, // This is telegram_id
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Chat {
    pub id: i64, // This is telegram_id
    #[serde(rename = "type")]
    pub chat_type: String, // "private", "group", "supergroup" or "channel"
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub entity_type: String, // "mention", "hashtag", "cashtag", "bot_command", "url", "email", "phone_number", "bold", "italic", "underline", "strikethrough", "spoiler", "code", "pre", "text_link", "text_mention"
    pub offset: usize,
    pub length: usize,
    pub url: Option<String>, // For "text_link" only
    pub user: Option<User>,  // For "text_mention" only
}

//! Builds conversation context by including missed messages.
//! When the bot is tagged in a group chat, this module fetches all messages
//! since the last bot response and formats them as context for OpenAI.

use crate::db::{Db, MessageWithSender};
use anyhow::Result;
use chrono::{DateTime, Utc};

use super::conversation_context::{self, ConversationKey};

/// Maximum number of missed messages to include in context
const MAX_MISSED_MESSAGES: i64 = 50;

/// Builds a prompt that includes missed messages as context.
/// For group chats, fetches messages since the last bot response and prepends them to the prompt.
/// For private chats or first messages, returns the prompt unchanged.
pub async fn build_context_with_missed_messages<D: Db>(
    db: &D,
    conversation_key: &ConversationKey,
    current_prompt: &str,
    bot_db_id: i32,
) -> Result<String> {
    // Get the timestamp of the last bot message
    let last_bot_at = conversation_context::get_last_bot_message_at(db, conversation_key).await?;

    let Some(since) = last_bot_at else {
        // No previous bot message, return prompt as-is
        return Ok(current_prompt.to_string());
    };

    // Fetch messages since last bot response
    let messages = db
        .get_messages_since(
            conversation_key.local_chat_id,
            conversation_key.message_thread_id,
            since,
            MAX_MISSED_MESSAGES,
        )
        .await?;

    // Filter out Lexi's own messages and messages without text
    // Note: Other bots' messages ARE included in context
    let user_messages: Vec<&MessageWithSender> = messages
        .iter()
        .filter(|m| m.sender_id != bot_db_id && m.text.is_some())
        .collect();

    if user_messages.is_empty() {
        // No missed messages, return prompt as-is
        return Ok(current_prompt.to_string());
    }

    // Format the context
    let context_block = format_context_block(&user_messages);
    Ok(format!(
        "{context_block}\n\n[Your message:]\n{current_prompt}"
    ))
}

/// Formats missed messages into a context block for OpenAI.
fn format_context_block(messages: &[&MessageWithSender]) -> String {
    let mut lines = Vec::with_capacity(messages.len() + 1);
    lines.push("[Recent conversation in this chat:]".to_string());

    for msg in messages {
        let sender_name = get_sender_display_name(msg);
        let text = msg.text.as_deref().unwrap_or("");
        lines.push(format!("{sender_name}: {text}"));
    }

    lines.join("\n")
}

/// Gets a display name for a message sender.
/// Prefers username if available, otherwise uses first_name.
fn get_sender_display_name(msg: &MessageWithSender) -> String {
    msg.sender_username
        .as_ref()
        .map(|u| format!("@{u}"))
        .unwrap_or_else(|| msg.sender_first_name.clone())
}

/// Converts a Unix timestamp to DateTime<Utc>.
/// Returns None if the timestamp is invalid.
pub fn unix_to_datetime(timestamp: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(timestamp, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_message(
        text: &str,
        username: Option<&str>,
        first_name: &str,
        sender_id: i32,
        is_bot: bool,
    ) -> MessageWithSender {
        MessageWithSender {
            text: Some(text.to_string()),
            sender_username: username.map(String::from),
            sender_first_name: first_name.to_string(),
            sender_id,
            is_bot,
            sent_at: Utc::now(),
        }
    }

    #[test]
    fn test_get_sender_display_name_prefers_username() {
        let msg = mock_message("hello", Some("alice"), "Alice", 1, false);
        assert_eq!(get_sender_display_name(&msg), "@alice");
    }

    #[test]
    fn test_get_sender_display_name_falls_back_to_first_name() {
        let msg = mock_message("hello", None, "Bob", 2, false);
        assert_eq!(get_sender_display_name(&msg), "Bob");
    }

    #[test]
    fn test_format_context_block() {
        let msg1 = mock_message(
            "I think we should use Redis",
            Some("alice"),
            "Alice",
            1,
            false,
        );
        let msg2 = mock_message("What about Memcached?", None, "Bob", 2, false);
        let messages: Vec<&MessageWithSender> = vec![&msg1, &msg2];

        let block = format_context_block(&messages);

        assert!(block.contains("[Recent conversation in this chat:]"));
        assert!(block.contains("@alice: I think we should use Redis"));
        assert!(block.contains("Bob: What about Memcached?"));
    }

    #[test]
    fn test_unix_to_datetime_valid() {
        let dt = unix_to_datetime(1704067200); // 2024-01-01 00:00:00 UTC
        assert!(dt.is_some());
    }

    #[test]
    fn test_unix_to_datetime_invalid() {
        // Extremely far future timestamp that might overflow
        let dt = unix_to_datetime(i64::MAX);
        assert!(dt.is_none());
    }
}

//! Unified conversation context API that routes to either chat-level or topic-level storage.
//! This module provides a single interface for managing OpenAI response IDs,
//! abstracting whether the conversation is in a regular chat or a forum topic.

use crate::ai_interaction::topic_context;
use crate::db::Db;
use anyhow::Result;

/// Identifies a conversation context - either a chat or a specific topic within a chat.
#[derive(Debug, Clone)]
pub struct ConversationKey {
    pub local_chat_id: i32,
    pub message_thread_id: Option<i64>,
}

impl ConversationKey {
    pub fn new(local_chat_id: i32, message_thread_id: Option<i64>) -> Self {
        Self {
            local_chat_id,
            message_thread_id,
        }
    }

    /// Returns true if this conversation is in a forum topic.
    pub fn is_topic(&self) -> bool {
        self.message_thread_id.is_some()
    }
}

/// Get the last OpenAI response ID for a conversation.
/// Routes to topic-level storage if thread_id is present, otherwise uses chat-level storage.
pub async fn get_response_id<D: Db>(db: &D, key: &ConversationKey) -> Result<Option<String>> {
    match key.message_thread_id {
        Some(thread_id) => {
            topic_context::get_topic_response_id(db, key.local_chat_id, thread_id).await
        }
        None => db.get_last_openai_response_id(key.local_chat_id).await,
    }
}

/// Update the last OpenAI response ID for a conversation.
/// Routes to topic-level storage if thread_id is present, otherwise uses chat-level storage.
pub async fn update_response_id<D: Db>(
    db: &D,
    key: &ConversationKey,
    response_id: &str,
) -> Result<()> {
    match key.message_thread_id {
        Some(thread_id) => {
            topic_context::set_topic_response_id(db, key.local_chat_id, thread_id, response_id)
                .await
        }
        None => {
            db.update_last_openai_response_id(key.local_chat_id, response_id)
                .await
        }
    }
}

/// Clear the last OpenAI response ID for a conversation.
/// Routes to topic-level storage if thread_id is present, otherwise uses chat-level storage.
pub async fn clear_response_id<D: Db>(db: &D, key: &ConversationKey) -> Result<()> {
    match key.message_thread_id {
        Some(thread_id) => {
            topic_context::clear_topic_response_id(db, key.local_chat_id, thread_id).await
        }
        None => db.clear_last_openai_response_id(key.local_chat_id).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MockDb;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_conversation_key_is_topic() {
        let chat_key = ConversationKey::new(1, None);
        assert!(!chat_key.is_topic());

        let topic_key = ConversationKey::new(1, Some(123));
        assert!(topic_key.is_topic());
    }

    #[tokio::test]
    async fn test_get_response_id_routes_to_chat_when_no_thread() {
        let mut mock_db = MockDb::new();
        let key = ConversationKey::new(42, None);

        mock_db
            .expect_get_last_openai_response_id()
            .with(eq(42))
            .times(1)
            .returning(|_| Ok(Some("resp_chat".to_string())));

        let result = get_response_id(&mock_db, &key).await.unwrap();
        assert_eq!(result, Some("resp_chat".to_string()));
    }

    #[tokio::test]
    async fn test_update_response_id_routes_to_chat_when_no_thread() {
        let mut mock_db = MockDb::new();
        let key = ConversationKey::new(42, None);

        mock_db
            .expect_update_last_openai_response_id()
            .with(eq(42), eq("resp_new"))
            .times(1)
            .returning(|_, _| Ok(()));

        update_response_id(&mock_db, &key, "resp_new")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_clear_response_id_routes_to_chat_when_no_thread() {
        let mut mock_db = MockDb::new();
        let key = ConversationKey::new(42, None);

        mock_db
            .expect_clear_last_openai_response_id()
            .with(eq(42))
            .times(1)
            .returning(|_| Ok(()));

        clear_response_id(&mock_db, &key).await.unwrap();
    }
}

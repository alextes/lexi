//! Per-topic conversation context storage using the key-value store.
//! This module handles OpenAI response ID tracking for forum topics,
//! where each topic within a chat maintains its own separate conversation context.

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

const TOPIC_RESPONSE_KEY_PREFIX: &str = "topic_response_id:chat:";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TopicResponseState {
    pub last_openai_response_id: Option<String>,
}

fn topic_response_key(local_chat_id: i32, thread_id: i64) -> String {
    format!("{TOPIC_RESPONSE_KEY_PREFIX}{local_chat_id}:thread:{thread_id}")
}

/// Get the last OpenAI response ID for a specific topic.
pub async fn get_topic_response_id<D: Db>(
    db: &D,
    local_chat_id: i32,
    thread_id: i64,
) -> Result<Option<String>> {
    let key = topic_response_key(local_chat_id, thread_id);
    match db.get_kv::<TopicResponseState>(&key).await? {
        Some((state, _)) => Ok(state.last_openai_response_id),
        None => Ok(None),
    }
}

/// Set the last OpenAI response ID for a specific topic.
pub async fn set_topic_response_id<D: Db>(
    db: &D,
    local_chat_id: i32,
    thread_id: i64,
    response_id: &str,
) -> Result<()> {
    let key = topic_response_key(local_chat_id, thread_id);
    let state = TopicResponseState {
        last_openai_response_id: Some(response_id.to_string()),
    };
    db.set_kv(&key, &state).await
}

/// Clear the last OpenAI response ID for a specific topic.
pub async fn clear_topic_response_id<D: Db>(
    db: &D,
    local_chat_id: i32,
    thread_id: i64,
) -> Result<()> {
    let key = topic_response_key(local_chat_id, thread_id);
    let state = TopicResponseState {
        last_openai_response_id: None,
    };
    db.set_kv(&key, &state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MockDb;
    use chrono::Utc;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_get_topic_response_id_returns_value() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 42;
        let thread_id = 123i64;
        let key = topic_response_key(local_chat_id, thread_id);
        let state = TopicResponseState {
            last_openai_response_id: Some("resp_abc123".to_string()),
        };

        mock_db
            .expect_get_kv::<TopicResponseState>()
            .with(eq(key))
            .times(1)
            .returning(move |_| Ok(Some((state.clone(), Utc::now()))));

        let result = get_topic_response_id(&mock_db, local_chat_id, thread_id)
            .await
            .unwrap();

        assert_eq!(result, Some("resp_abc123".to_string()));
    }

    #[tokio::test]
    async fn test_get_topic_response_id_returns_none_when_not_set() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 42;
        let thread_id = 456i64;
        let key = topic_response_key(local_chat_id, thread_id);

        mock_db
            .expect_get_kv::<TopicResponseState>()
            .with(eq(key))
            .times(1)
            .returning(|_| Ok(None));

        let result = get_topic_response_id(&mock_db, local_chat_id, thread_id)
            .await
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_set_topic_response_id() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 7;
        let thread_id = 999i64;
        let key = topic_response_key(local_chat_id, thread_id);
        let response_id = "resp_xyz789";

        mock_db
            .expect_set_kv::<TopicResponseState>()
            .withf(move |k, v| {
                k == key && v.last_openai_response_id == Some(response_id.to_string())
            })
            .times(1)
            .returning(|_, _| Ok(()));

        set_topic_response_id(&mock_db, local_chat_id, thread_id, response_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_clear_topic_response_id() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 9;
        let thread_id = 111i64;
        let key = topic_response_key(local_chat_id, thread_id);

        mock_db
            .expect_set_kv::<TopicResponseState>()
            .withf(move |k, v| k == key && v.last_openai_response_id.is_none())
            .times(1)
            .returning(|_, _| Ok(()));

        clear_topic_response_id(&mock_db, local_chat_id, thread_id)
            .await
            .unwrap();
    }
}

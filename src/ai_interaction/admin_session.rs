use crate::db::Db;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const ADMIN_SESSION_KEY_PREFIX: &str = "admin_session:chat:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSessionState {
    pub active: bool,
    pub started_at: Option<DateTime<Utc>>,
    pub last_response_id_before_admin: Option<String>,
}

impl AdminSessionState {
    #[must_use]
    pub fn inactive() -> Self {
        Self {
            active: false,
            started_at: None,
            last_response_id_before_admin: None,
        }
    }
}

fn admin_session_key(local_chat_id: i32) -> String {
    format!("{ADMIN_SESSION_KEY_PREFIX}{local_chat_id}")
}

pub async fn get_admin_session_state<D: Db>(
    db: &D,
    local_chat_id: i32,
) -> Result<Option<AdminSessionState>> {
    let key = admin_session_key(local_chat_id);
    Ok(db.get_kv::<AdminSessionState>(&key).await?.map(|(v, _)| v))
}

pub async fn set_admin_session_state<D: Db>(
    db: &D,
    local_chat_id: i32,
    state: &AdminSessionState,
) -> Result<()> {
    let key = admin_session_key(local_chat_id);
    db.set_kv(&key, state).await
}

pub async fn start_admin_session<D: Db>(
    db: &D,
    local_chat_id: i32,
    last_response_id_before_admin: Option<String>,
) -> Result<AdminSessionState> {
    let state = AdminSessionState {
        active: true,
        started_at: Some(Utc::now()),
        last_response_id_before_admin,
    };
    set_admin_session_state(db, local_chat_id, &state).await?;
    Ok(state)
}

pub async fn end_admin_session<D: Db>(db: &D, local_chat_id: i32) -> Result<AdminSessionState> {
    let state = AdminSessionState::inactive();
    set_admin_session_state(db, local_chat_id, &state).await?;
    Ok(state)
}

pub async fn update_admin_session_last_response_id<D: Db>(
    db: &D,
    local_chat_id: i32,
    last_response_id_before_admin: Option<String>,
) -> Result<AdminSessionState> {
    let mut state = get_admin_session_state(db, local_chat_id)
        .await?
        .unwrap_or_else(AdminSessionState::inactive);
    state.last_response_id_before_admin = last_response_id_before_admin;
    set_admin_session_state(db, local_chat_id, &state).await?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MockDb;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_get_admin_session_state_returns_value() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 42;
        let key = admin_session_key(local_chat_id);
        let state = AdminSessionState {
            active: true,
            started_at: Some(Utc::now()),
            last_response_id_before_admin: Some("resp_123".to_string()),
        };
        let expected_state = state.clone();

        mock_db
            .expect_get_kv::<AdminSessionState>()
            .with(eq(key))
            .times(1)
            .returning(move |_| Ok(Some((state.clone(), Utc::now()))));

        let fetched = get_admin_session_state(&mock_db, local_chat_id)
            .await
            .unwrap();

        assert_eq!(fetched, Some(expected_state));
    }

    #[tokio::test]
    async fn test_start_admin_session_sets_state() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 7;
        let key = admin_session_key(local_chat_id);
        let last_response = Some("resp_before_admin".to_string());
        let expected_last_response = last_response.clone();

        mock_db
            .expect_set_kv::<AdminSessionState>()
            .withf(move |k, v| {
                k == key
                    && v.active
                    && v.started_at.is_some()
                    && v.last_response_id_before_admin == expected_last_response
            })
            .times(1)
            .returning(|_, _| Ok(()));

        let state = start_admin_session(&mock_db, local_chat_id, last_response)
            .await
            .unwrap();

        assert!(state.active);
        assert!(state.started_at.is_some());
    }

    #[tokio::test]
    async fn test_end_admin_session_clears_state() {
        let mut mock_db = MockDb::new();
        let local_chat_id = 9;
        let key = admin_session_key(local_chat_id);

        mock_db
            .expect_set_kv::<AdminSessionState>()
            .withf(move |k, v| {
                k == key && !v.active && v.started_at.is_none()
            })
            .times(1)
            .returning(|_, _| Ok(()));

        let state = end_admin_session(&mock_db, local_chat_id)
            .await
            .unwrap();

        assert!(!state.active);
        assert!(state.started_at.is_none());
    }
}

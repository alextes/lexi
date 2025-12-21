use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::PgPool;

#[allow(async_fn_in_trait)]
pub trait KeyValueStore {
    async fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()>;
    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<(T, DateTime<Utc>)>>;
}

#[derive(Debug, Clone)]
pub struct PostgresKeyValueStore {
    pool: PgPool,
}

impl PostgresKeyValueStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl KeyValueStore for PostgresKeyValueStore {
    async fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json_value = serde_json::to_value(value)
            .with_context(|| format!("failed to serialize value for key: {key}"))?;

        sqlx::query!(
            r#"
            INSERT INTO key_value_store (key, value, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (key) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW();
            "#,
            key,
            json_value
        )
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to set key: {key}"))?;

        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<(T, DateTime<Utc>)>> {
        let row = sqlx::query!(
            "SELECT value, updated_at FROM key_value_store WHERE key = $1",
            key
        )
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("failed to get key: {key}"))?;

        if let Some(row) = row {
            let value: T = serde_json::from_value(row.value)
                .with_context(|| format!("failed to deserialize value for key: {key}"))?;
            Ok(Some((value, row.updated_at)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use sqlx::PgPool;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
    struct TestStruct {
        field1: String,
        field2: i32,
    }

    async fn setup_kv_store(pool: &PgPool) -> PostgresKeyValueStore {
        // run migration for the key_value_store table
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .expect("failed to run migrations for tests");
        PostgresKeyValueStore::new(pool.clone())
    }

    #[sqlx::test]
    async fn test_set_and_get_value(pool: PgPool) {
        let kv_store = setup_kv_store(&pool).await;
        let key = "test_key";
        let value = TestStruct {
            field1: "hello".to_string(),
            field2: 42,
        };

        kv_store.set(key, &value).await.unwrap();

        let (retrieved_value, _): (TestStruct, _) = kv_store.get(key).await.unwrap().unwrap();
        assert_eq!(value, retrieved_value);
    }

    #[sqlx::test]
    async fn test_get_non_existent_key(pool: PgPool) {
        let kv_store = setup_kv_store(&pool).await;
        let key = "non_existent_key";

        let result: Option<(TestStruct, DateTime<Utc>)> = kv_store.get(key).await.unwrap();
        assert!(result.is_none());
    }

    #[sqlx::test]
    async fn test_update_value(pool: PgPool) {
        let kv_store = setup_kv_store(&pool).await;
        let key = "update_key";
        let initial_value = TestStruct {
            field1: "initial".to_string(),
            field2: 1,
        };
        let updated_value = TestStruct {
            field1: "updated".to_string(),
            field2: 2,
        };

        kv_store.set(key, &initial_value).await.unwrap();
        let (retrieved_initial, initial_timestamp) = kv_store.get(key).await.unwrap().unwrap();
        assert_eq!(initial_value, retrieved_initial);

        // we need to make sure the timestamp changes
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        kv_store.set(key, &updated_value).await.unwrap();
        let (retrieved_updated, updated_timestamp) = kv_store.get(key).await.unwrap().unwrap();
        assert_eq!(updated_value, retrieved_updated);
        assert!(updated_timestamp > initial_timestamp);
    }
}

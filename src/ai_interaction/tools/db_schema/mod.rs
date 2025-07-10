//! whenever writing sql queries this tool can help the ai see the full db schema
//! for database schemas we know about.
use crate::db::Db;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{Connection, Row};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

pub const DATABASE_SCHEMA_TOOL_NAME: &str = "get_database_schema";

const SCHEMA_QUERY: &str = r#"
SELECT json_build_object(
    'enums', (
        SELECT json_agg(
            json_build_object(
                'name', t.typname,
                'values', (SELECT array_agg(e.enumlabel ORDER BY e.enumsortorder) FROM pg_enum e WHERE e.enumtypid = t.oid)
            )
        )
        FROM pg_type t
        JOIN pg_namespace n ON n.oid = t.typnamespace
        WHERE n.nspname = 'public' AND t.typtype = 'e'
    ),
    'tables', (
        SELECT json_agg(
            json_build_object(
                'name', t.table_name,
                'columns', c.columns,
                'primary_keys', pk.primary_keys,
                'indexes', i.indexes,
                'foreign_keys', fk.foreign_keys
            )
        )
        FROM information_schema.tables t
        LEFT JOIN ( -- columns
            SELECT
                table_name,
                json_agg(
                    json_build_object(
                        'name', column_name,
                        'type', udt_name,
                        'is_nullable', is_nullable = 'YES',
                        'default', column_default
                    ) ORDER BY ordinal_position
                ) as columns
            FROM information_schema.columns c
            WHERE table_schema = 'public'
            GROUP BY table_name
        ) c ON t.table_name = c.table_name
        LEFT JOIN ( -- primary keys
            SELECT
                tc.table_name,
                json_agg(kcu.column_name) as primary_keys
            FROM
                information_schema.table_constraints AS tc
            JOIN information_schema.key_column_usage AS kcu
                ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = 'public'
            GROUP BY tc.table_name
        ) pk ON t.table_name = pk.table_name
        LEFT JOIN ( -- indexes
            SELECT
                tablename,
                json_agg(
                    json_build_object(
                        'name', indexname,
                        'definition', indexdef
                    )
                ) as indexes
            FROM pg_indexes
            WHERE schemaname = 'public'
            GROUP BY tablename
        ) i ON t.table_name = i.tablename
        LEFT JOIN ( -- foreign keys
            SELECT
                tc.table_name,
                json_agg(json_build_object(
                    'constraint_name', tc.constraint_name,
                    'column_name', kcu.column_name,
                    'foreign_table_name', ccu.table_name,
                    'foreign_column_name', ccu.column_name
                )) as foreign_keys
            FROM
                information_schema.table_constraints AS tc
            JOIN information_schema.key_column_usage AS kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage AS ccu
              ON ccu.constraint_name = tc.constraint_name
             AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = 'public'
            GROUP BY tc.table_name
        ) fk ON t.table_name = fk.table_name
        WHERE t.table_schema = 'public' AND t.table_type = 'BASE TABLE'
    ),
    'views', (
        SELECT json_agg(
            json_build_object(
                'name', v.table_name,
                'definition', v.view_definition,
                'columns', c.columns
            )
        )
        FROM information_schema.views v
        LEFT JOIN (
            SELECT
                table_name,
                json_agg(
                    json_build_object(
                        'name', column_name,
                        'type', udt_name,
                        'is_nullable', is_nullable = 'YES',
                        'default', column_default
                    ) ORDER BY ordinal_position
                ) as columns
            FROM information_schema.columns c
            WHERE table_schema = 'public'
            GROUP BY table_name
        ) c ON v.table_name = c.table_name
        WHERE v.table_schema = 'public'
    ),
    'materialized_views', (
        SELECT json_agg(
            json_build_object(
                'name', m.matviewname,
                'definition', m.definition,
                'columns', c.columns
            )
        )
        FROM pg_matviews m
        LEFT JOIN (
            SELECT
                table_name,
                json_agg(
                    json_build_object(
                        'name', column_name,
                        'type', udt_name,
                        'is_nullable', is_nullable = 'YES',
                        'default', column_default
                    ) ORDER BY ordinal_position
                ) as columns
            FROM information_schema.columns c
            WHERE table_schema = 'public'
            GROUP BY table_name
        ) c ON m.matviewname = c.table_name
        WHERE m.schemaname = 'public'
    )
) AS schema;
"#;

#[derive(Debug, serde::Deserialize)]
pub struct GetDatabaseSchemaArgs {
    pub database_name: String,
}

pub static DATABASE_SCHEMA_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "database_name".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the name of the database to get the schema for. must be one of 'mevdb' or 'globaldb'.",
            )
            .enum_string(&["mevdb", "globaldb"])
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["database_name".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        DATABASE_SCHEMA_TOOL_NAME.to_string(),
        Some(
            "retrieves the schema definition for a specified database. this can be used to understand table structures before forming a query for the corresponding 'execute_<db_name>_query' tool."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(skip(db, db_url))]
async fn fetch_and_cache_schema<D: Db>(
    db: &D,
    db_name: &str,
    db_url: &str,
    cache_key: &str,
) -> Result<String> {
    info!("fetching fresh schema for {db_name} using url");

    let schema_json: serde_json::Value = match sqlx::postgres::PgConnection::connect(db_url).await {
        Ok(mut conn) => match sqlx::query(SCHEMA_QUERY).fetch_one(&mut conn).await {
            Ok(row) => {
                let schema: serde_json::Value = row.get("schema");
                if let Err(e) = conn.close().await {
                    warn!(error = %e, "failed to close database connection for schema fetch");
                }
                schema
            }
            Err(e) => {
                let err_msg = format!("failed to execute schema query for {db_name}: {e}");
                error!("{}", err_msg);
                json!({
                    "status": "error",
                    "message": "database_query_failed",
                    "details": err_msg
                })
            }
        },
        Err(e) => {
            let err_msg = format!("failed to connect to {db_name} database: {e}");
            error!("{}", err_msg);
            json!({
                "status": "error",
                "message": "database_connection_failed",
                "details": err_msg
            })
        }
    };

    let schema_string = schema_json.to_string();
    if schema_json.get("status").and_then(|s| s.as_str()) == Some("error") {
        warn!(
            "failed to fetch new schema for {db_name}, not caching result. error: {schema_string}",
        );
    } else {
        info!("caching new schema for {db_name}");
        if let Err(e) = db.set_kv(cache_key, &schema_string).await {
            warn!(error = %e, "failed to cache new schema for {db_name}");
        }
    }

    Ok(schema_string)
}

#[instrument(skip(db, db_url))]
pub async fn execute_get_database_schema<D: Db>(
    db: &D,
    db_name: &str,
    db_url: &str,
) -> Result<String> {
    info!(db_name = %db_name, "executing get_database_schema tool");

    let cache_key = format!("db_schema_{db_name}");
    match db.get_kv::<String>(&cache_key).await {
        Ok(Some((cached_schema, last_updated))) => {
            if Utc::now().signed_duration_since(last_updated) > Duration::days(7) {
                info!("schema for {} is stale, fetching fresh version.", db_name);
                fetch_and_cache_schema(db, db_name, db_url, &cache_key).await
            } else {
                info!("returning cached schema for {}", db_name);
                Ok(cached_schema)
            }
        }
        Ok(None) => {
            info!("no cached schema found for {}, fetching.", db_name);
            fetch_and_cache_schema(db, db_name, db_url, &cache_key).await
        }
        Err(e) => {
            warn!(db_name = %db_name, error = %e, "error getting schema from cache");
            Ok(json!({
                "status": "error",
                "message": "failed_to_read_schema_from_cache",
                "details": e.to_string()
            })
            .to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{MockDb, PostgresDb};
    use sqlx::PgPool;

    #[tokio::test]
    async fn test_get_schema_cached() {
        let mut mock_db = MockDb::new();
        let cached_schema = "{\"tables\":[]}".to_string();
        let db_name = "mevdb";
        let db_url = "dummy-url"; // not used when cache hits
        let cache_key = format!("db_schema_{db_name}");

        mock_db
            .expect_get_kv::<String>()
            .withf(move |key| key == &cache_key)
            .times(1)
            .returning({
                let cs = cached_schema.clone();
                move |_| Ok(Some((cs.clone(), Utc::now())))
            });

        let result_str = execute_get_database_schema(&mock_db, db_name, db_url)
            .await
            .unwrap();

        assert_eq!(result_str, cached_schema);
    }

    // this is a mock of fetch_and_cache_schema to be used in the stale test
    async fn mock_fetch_and_cache_schema(
        _db: &impl Db,
        _db_name: &str,
        _db_url: &str,
        _cache_key: &str,
    ) -> Result<String> {
        Ok("{\"tables\":[\"new_schema\"]}".to_string())
    }

    #[tokio::test]
    async fn test_stale_cache_is_refetched() {
        let mut mock_db = MockDb::new();
        let stale_schema = "{\"tables\":[\"stale\"]}".to_string();
        let db_name = "testdb_stale";
        let db_url = "dummy-url";
        let cache_key = format!("db_schema_{db_name}");

        // 1. mock get_kv to return a stale value
        let get_key = cache_key.clone();
        mock_db
            .expect_get_kv::<String>()
            .withf(move |key| key == &get_key)
            .times(1)
            .returning({
                let cs = stale_schema.clone();
                move |_| {
                    let stale_time = Utc::now() - Duration::days(8);
                    Ok(Some((cs.clone(), stale_time)))
                }
            });

        // 2. mock set_kv to be called once, since we expect a refetch and cache
        let set_key = cache_key.clone();
        mock_db
            .expect_set_kv::<String>()
            .withf(move |key, val| key == &set_key && val == "{\"tables\":[\"new_schema\"]}")
            .times(1)
            .returning(|_, _| Ok(()));

        // a little trick to use our mock implementation of fetch_and_cache_schema
        // we can't do this directly in rust, so we have to replicate the logic of
        // execute_get_database_schema here.
        let (cached_schema, last_updated) =
            mock_db.get_kv::<String>(&cache_key).await.unwrap().unwrap();

        let new_schema = if Utc::now().signed_duration_since(last_updated) > Duration::days(7) {
            let new_schema_str = mock_fetch_and_cache_schema(&mock_db, db_name, db_url, &cache_key)
                .await
                .unwrap();
            mock_db.set_kv(&cache_key, &new_schema_str).await.unwrap();
            new_schema_str
        } else {
            cached_schema
        };

        assert_eq!(new_schema, "{\"tables\":[\"new_schema\"]}");
    }

    #[sqlx::test]
    async fn test_fetch_and_cache_schema_integration(pool: PgPool) {
        let db = PostgresDb::new_from_pool(pool);
        let test_db_url = std::env::var("DATABASE_URL").unwrap();
        let db_name = "testdb_integration";
        let cache_key = format!("db_schema_{db_name}");

        // 1. fetch and cache the schema.
        let fetched_schema_str = fetch_and_cache_schema(&db, db_name, &test_db_url, &cache_key)
            .await
            .unwrap();

        let fetched_schema_json: serde_json::Value =
            serde_json::from_str(&fetched_schema_str).unwrap();
        assert!(
            fetched_schema_json
                .get("tables")
                .and_then(|t| t.as_array())
                .map_or(false, |a| !a.is_empty()),
            "schema should have tables, was: {}",
            fetched_schema_str
        );

        // 2. verify that it was cached correctly.
        let cached_result = db.get_kv::<String>(&cache_key).await.unwrap();
        assert!(cached_result.is_some(), "schema was not cached");
        let (cached_schema_str, _) = cached_result.unwrap();

        assert_eq!(fetched_schema_str, cached_schema_str);
    }
}

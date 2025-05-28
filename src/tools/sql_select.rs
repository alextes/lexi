use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use tracing::{error, info, warn};

pub const SQL_TOOL_NAME: &str = "execute_sql_query";

// this function executes the AI-generated query. it expects the query to target existing tables like 'users'.
// for its own unit tests, queries will target 'tool_test_users'.
pub async fn execute_db_query(pool: &PgPool, query: &str) -> JsonValue {
    info!(query = %query, "(sql executor) attempting to execute db query");

    let trimmed_query = query.trim().to_lowercase();
    if !trimmed_query.starts_with("select") {
        warn!(query = %query, "(sql executor) rejected non-select query.");
        return json!({
            "status": "error",
            "message": "invalid query type. only select queries are permitted.",
            "details": format!("query was: {}", query)
        });
    }

    info!(query = %query, "(sql executor) executing select db query");
    match sqlx::query(query).fetch_all(pool).await {
        Ok(rows) => {
            if rows.is_empty() {
                return json!({
                    "status": "success",
                    "message": "query executed successfully. no rows returned.",
                    "results": []
                });
            }
            let mut results: Vec<JsonMap<String, JsonValue>> = Vec::new();
            for row in rows {
                let mut json_row = JsonMap::new();
                for (i, column) in row.columns().iter().enumerate() {
                    let value = match row.try_get_raw(i) {
                        Ok(raw_value) if !raw_value.is_null() => {
                            if let Ok(v_str) = row.try_get::<String, _>(i) {
                                json!(v_str)
                            } else if let Ok(v_i64) = row.try_get::<i64, _>(i) {
                                json!(v_i64)
                            } else if let Ok(v_i32) = row.try_get::<i32, _>(i) {
                                json!(v_i32)
                            } else if let Ok(v_f64) = row.try_get::<f64, _>(i) {
                                json!(v_f64)
                            } else if let Ok(v_bool) = row.try_get::<bool, _>(i) {
                                json!(v_bool)
                            } else if let Ok(v_time) =
                                row.try_get::<chrono::DateTime<chrono::Utc>, _>(i)
                            {
                                json!(v_time.to_rfc3339())
                            } else {
                                json!(null)
                            }
                        }
                        _ => json!(null),
                    };
                    json_row.insert(column.name().to_string(), value);
                }
                results.push(json_row);
            }
            json!({
                "status": "success",
                "message": "query executed successfully.",
                "results": results
            })
        }
        Err(e) => {
            error!(error = %e, query = %query, "(sql executor) failed to execute sql query");
            json!({
                "status": "error",
                "message": "failed to execute sql query.",
                "details": e.to_string()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::{Context, Result};
    use sqlx::PgPool;

    // this function prepares a uniquely named table specifically for unit testing the sql_select tool's db execution logic.
    // it does not and should not rely on the main bot's migrations or affect the main 'users' table.
    pub async fn prepare_tool_test_users_table(pool: &PgPool) -> Result<()> {
        let table_name = "tool_test_users";
        info!(table = %table_name, "(sql select tool tests) preparing dedicated test table...");

        sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
            .execute(pool)
            .await
            .context(format!(
                "failed to drop table {} (sql select tool tests)",
                table_name
            ))?;

        info!(table = %table_name, "(sql select tool tests) creating dedicated test table {}...", table_name);
        sqlx::query(&format!(
            "CREATE TABLE {} (
            id SERIAL PRIMARY KEY,
            username VARCHAR(255) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL UNIQUE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );",
            table_name
        ))
        .execute(pool)
        .await
        .context(format!(
            "failed to create table {} (sql select tool tests)",
            table_name
        ))?;

        info!(table = %table_name, "(sql select tool tests) inserting test data into {}...", table_name);
        let test_users = [
            ("simple_alpha", "alpha@simpletest.com"),
            ("simple_beta", "beta@simpletest.com"),
        ];
        for (username, email) in &test_users {
            sqlx::query(&format!(
            "INSERT INTO {} (username, email) VALUES ($1, $2) ON CONFLICT (username) DO NOTHING",
            table_name
        ))
            .bind(username)
            .bind(email)
            .execute(pool)
            .await
            .context(format!(
                "failed to insert test user {} into {} (sql select tool tests)",
                username, table_name
            ))?;
        }
        info!(table = %table_name, "(sql select tool tests) dedicated test table {} prepared.", table_name);
        Ok(())
    }

    #[sqlx::test]
    async fn test_execute_db_query_valid_select(pool: PgPool) -> Result<()> {
        prepare_tool_test_users_table(&pool)
            .await
            .expect("db schema prep for test_valid_select failed");

        let query = "SELECT username, email FROM tool_test_users WHERE username = 'simple_alpha' ORDER BY id;";
        let result = execute_db_query(&pool, query).await;

        assert_eq!(result["status"], "success");
        assert!(result["results"].is_array());
        let results_arr = result["results"].as_array().unwrap();
        assert_eq!(results_arr.len(), 1);
        assert_eq!(results_arr[0]["username"], "simple_alpha");
        assert_eq!(results_arr[0]["email"], "alpha@simpletest.com");
        Ok(())
    }

    #[sqlx::test]
    async fn test_execute_db_query_non_select_rejected(pool: PgPool) -> Result<()> {
        let query = "INSERT INTO tool_test_users (username, email) VALUES ('test_insert', 'insert@test.com');";
        let result = execute_db_query(&pool, query).await;

        assert_eq!(result["status"], "error");
        assert_eq!(
            result["message"],
            "invalid query type. only select queries are permitted."
        );
        Ok(())
    }

    #[sqlx::test]
    async fn test_execute_db_query_sql_error(pool: PgPool) -> Result<()> {
        prepare_tool_test_users_table(&pool)
            .await
            .expect("db schema prep for test_sql_error failed");

        let query = "SELECT non_existent_column FROM tool_test_users;";
        let result = execute_db_query(&pool, query).await;
        assert_eq!(result["status"], "error");
        assert_eq!(result["message"], "failed to execute sql query.");
        assert!(result["details"]
            .as_str()
            .unwrap()
            .contains("column \"non_existent_column\" does not exist"));
        Ok(())
    }
}

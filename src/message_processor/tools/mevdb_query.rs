use crate::env::ENV_CONFIG;
use crate::message_processor::HandlerContext;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use eyre::Result;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const MEVDB_TOOL_NAME: &str = "execute_mevdb_query";

pub static MEVDB_QUERY_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "sql_query".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the sql select query to execute against the mev-specific database. must start with 'select'. provide the full query."
            )
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["sql_query".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        MEVDB_TOOL_NAME.to_string(),
        Some(
            "executes a sql select query against a read-only mev (maximal extractable value) database. provide the complete sql query. important tables and columns will be described by the assistant if known."
                .to_string(),
        ),
        Some(tool_params),
    )
});

async fn execute_mevdb_db_query(query: &str) -> JsonValue {
    info!(query = %query, "(mevdb executor) attempting to execute db query");

    let db_url = if let Some(url) = &ENV_CONFIG.mevdb_database_url { url.clone() } else {
        warn!("(mevdb executor) MEVDB_DATABASE_URL not configured.");
        return json!({
            "status": "error",
            "message": "mevdb query tool is not configured (missing MEVDB_DATABASE_URL).",
            "details": "The MEVDB_DATABASE_URL environment variable must be set to use this tool."
        });
    };

    let pool_result = PgPool::connect(&db_url).await;
    let pool = match pool_result {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "(mevdb executor) failed to connect to mevdb");
            return json!({
                "status": "error",
                "message": "failed to connect to the mevdb database.",
                "details": e.to_string()
            });
        }
    };

    let trimmed_query = query.trim().to_lowercase();
    if !trimmed_query.starts_with("select") {
        warn!(query = %query, "(mevdb executor) rejected non-select query.");
        return json!({
            "status": "error",
            "message": "invalid query type. only select queries are permitted for mevdb.",
            "details": format!("query was: {}", query)
        });
    }

    info!(query = %query, "(mevdb executor) executing select db query");
    match sqlx::query(query).fetch_all(&pool).await {
        // Use the new pool
        Ok(rows) => {
            if rows.is_empty() {
                // Return a simpler message for no rows
                return json!({
                    "message": "mevdb query executed successfully. no rows returned."
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
                                json!(null) // Should ideally represent as string if unknown type
                            }
                        }
                        _ => json!(null),
                    };
                    json_row.insert(column.name().to_string(), value);
                }
                results.push(json_row);
            }
            // Return only the array of results directly for successful queries with data
            json!(results)
        }
        Err(e) => {
            error!(error = %e, query = %query, "(mevdb executor) failed to execute sql query");
            json!({
                "status": "error",
                "message": "failed to execute mevdb sql query.",
                "details": e.to_string()
            })
        }
    }
}

pub async fn execute_mevdb_query_tool(
    _ctx: &HandlerContext<'_>, // Context might be needed for future enhancements or if db pool is on ctx
    arguments_json_str: &str,  // The arguments string from OutputFunctionCall
) -> Result<String> {
    // Returns a JSON string (query results or error)
    info!(args = %arguments_json_str, "executing execute_mevdb_query tool");

    match serde_json::from_str::<HashMap<String, String>>(arguments_json_str) {
        Ok(args_map) => {
            if let Some(sql_query_from_ai) = args_map.get("sql_query") {
                info!(query = %sql_query_from_ai, "parsed sql_query from ai arguments");
                let result_json_value = execute_mevdb_db_query(sql_query_from_ai).await;
                Ok(result_json_value.to_string())
            } else {
                let err_msg = "argument 'sql_query' missing";
                warn!(args = %arguments_json_str, err_msg);
                Ok(json!({
                    "status": "error",
                    "message": err_msg,
                    "details": format!("expected json with a 'sql_query' key, got: {}", arguments_json_str)
                }).to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {e}");
            warn!(args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(json!({
                "status": "error",
                "message": "failed to parse tool arguments as json.",
                "details": err_msg
            })
            .to_string())
        }
    }
}

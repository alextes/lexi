use crate::env::ENV_CONFIG;
use crate::message_processor::HandlerContext;
use crate::openai_api::{ToolDefinition, ToolFunctionParameterProperty, ToolFunctionParameters};
use eyre::Result;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const MEVDB_TOOL_NAME: &str = "execute_mevdb_query";

// --- tool definition ---
pub static MEVDB_QUERY_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "sql_query".to_string(),
        ToolFunctionParameterProperty {
            r#type: "string".to_string(),
            description: Some(
                "the sql select query to execute against the mev-specific database. must start with 'select'. provide the full query."
                    .to_string(),
            ),
            r#enum: Vec::new(),
        },
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["sql_query".to_string()]),
        additional_properties: false,
    };
    ToolDefinition {
        r#type: "function".to_string(),
        name: MEVDB_TOOL_NAME.to_string(),
        description: Some(
            "executes a sql select query against a read-only mev (maximal extractable value) database. provide the complete sql query. important tables and columns will be described by the assistant if known."
                .to_string(),
        ),
        parameters: Some(tool_params),
        strict: Some(true),
    }
});

// --- tool execution logic for mevdb ---
async fn execute_mevdb_db_query(query: &str) -> JsonValue {
    info!(query = %query, "(mevdb executor) attempting to execute db query");

    let db_url = match &ENV_CONFIG.mevdb_database_url {
        Some(url) => url.clone(),
        None => {
            warn!("(mevdb executor) MEVDB_DATABASE_URL not configured.");
            return json!({
                "status": "error",
                "message": "mevdb query tool is not configured (missing MEVDB_DATABASE_URL).",
                "details": "The MEVDB_DATABASE_URL environment variable must be set to use this tool."
            });
        }
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

// --- new simplified tool execution function ---
pub async fn execute_mevdb_query_tool(
    _ctx: &HandlerContext<'_>, // Context might be needed for future enhancements or if db pool is on ctx
    telegram_chat_id: i64,     // For logging
    arguments_json_str: &str,  // The arguments string from OutputFunctionCall
) -> Result<String> {
    // Returns a JSON string (query results or error)
    info!(chat_id = telegram_chat_id, args = %arguments_json_str, "executing execute_mevdb_query tool");

    match serde_json::from_str::<HashMap<String, String>>(arguments_json_str) {
        Ok(args_map) => {
            if let Some(sql_query_from_ai) = args_map.get("sql_query") {
                info!(chat_id = telegram_chat_id, query = %sql_query_from_ai, "parsed sql_query from ai arguments");
                let result_json_value = execute_mevdb_db_query(sql_query_from_ai).await;
                Ok(result_json_value.to_string())
            } else {
                let err_msg = "argument 'sql_query' missing";
                warn!(chat_id = telegram_chat_id, args = %arguments_json_str, err_msg);
                Ok(json!({
                    "status": "error",
                    "message": err_msg,
                    "details": format!("expected json with a 'sql_query' key, got: {}", arguments_json_str)
                }).to_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {}", e);
            warn!(chat_id = telegram_chat_id, args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(json!({
                "status": "error",
                "message": "failed to parse tool arguments as json.",
                "details": err_msg
            })
            .to_string())
        }
    }
}

// --- OLD tool call handling (to be removed/replaced by logic in openai_chat.rs) ---
/*
pub async fn handle_mevdb_query_tool_call(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64,
    function_call: &OutputFunctionCall,
    original_input_items: Vec<InputItem>,
    initial_api_response_id: &str,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    info!(chat_id = telegram_chat_id, args = %function_call.arguments, "received call for {}", function_call.name);

    match serde_json::from_str::<HashMap<String, String>>(&function_call.arguments) {
        Ok(args_map) => {
            if let Some(sql_query_from_ai) = args_map.get("sql_query") {
                info!(query = %sql_query_from_ai, "executing mevdb query from ai");

                let result_json_value = execute_mevdb_db_query(sql_query_from_ai).await;
                let result_json_string = result_json_value.to_string();

                let step2_ctx = ToolStep2Context {
                    telegram_chat_id,
                    function_name: &function_call.name,
                    function_id: &function_call.id,
                    function_call_id: &function_call.call_id,
                    function_arguments: &function_call.arguments,
                    original_input_items,
                    initial_api_response_id,
                    available_tools: available_tools.clone(),
                    instructions,
                    tool_output_json_string: result_json_string,
                };

                match handle_tool_call_step2_openai_response(ctx, step2_ctx).await {
                    Ok((api_response_step2, updated_input_items)) => {
                        // Pass to the main OpenAI response processor
                        Box::pin(process_openai_response(
                            ctx,
                            telegram_chat_id,
                            api_response_step2,
                            updated_input_items,
                            available_tools,
                            instructions,
                        ))
                        .await
                    }
                    Err(e) => {
                        // Step 2 API call itself failed
                        error!(chat_id = telegram_chat_id, tool_name = function_call.name, error = %e, "step 2 api call failed for mevdb_query_tool");
                        let fallback_text = format!(
                            "i tried to use the {} tool, but the final step of consulting the ai failed: {}. you might want to try again.",
                            function_call.name,
                            e
                        );
                        Ok((fallback_text, initial_api_response_id.to_string()))
                    }
                }
            } else {
                warn!(
                    chat_id = telegram_chat_id,
                    "'sql_query' missing in {} args", function_call.name
                );
                Err(eyre!(
                    "argument 'sql_query' missing for mevdb tool {}",
                    function_call.name
                ))
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to parse args for mevdb tool {}", function_call.name);
            Err(e).context(format!(
                "failed to parse args for mevdb tool {}",
                function_call.name
            ))
        }
    }
}
*/

// note: tests for mevdb_query would require a separate test database setup or mocking.
// for now, the query execution logic is identical to execute_sql_query, which is tested.

// TODO: alex notes that mevdb_query execution is still failing in some scenarios (e.g. main bot loop).
// this needs further debugging and dedicated test coverage to ensure reliability,
// especially around database connection and error propagation through the OpenAI step 2 call.

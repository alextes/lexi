use crate::openai_api::{
    call_responses_api, InputItem, InputMessageObject, OutputFunctionCall, OutputItem, ToolDefinition,
    ToolFunctionParameterProperty, ToolFunctionParameters,
};
use crate::message_processor::HandlerContext;
use eyre::{eyre, Context, Result};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, warn};
use chrono;

pub const SQL_TOOL_NAME: &str = "execute_sql_query";

// --- tool definition ---
pub static SQL_QUERY_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "sql_query".to_string(),
        ToolFunctionParameterProperty {
            r#type: "string".to_string(),
            description: Some(
                "the sql select query to execute. example: SELECT * FROM users WHERE id = 1. must start with 'select'."
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
        name: SQL_TOOL_NAME.to_string(), // Use const defined in this file
        description: Some(
            "executes a sql select query against the postgresql database and returns the results. only select queries are permitted. attempts to use other query types will result in an error."
                .to_string(),
        ),
        parameters: Some(tool_params),
        strict: Some(true),
    }
});

// --- tool execution logic ---
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

// --- tool call handling (response processing) ---
pub async fn handle_execute_sql_query_tool_call(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64, 
    function_call: &OutputFunctionCall,
    original_input_items: Vec<InputItem>,
    initial_api_response_id: &str,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    info!(chat_id = telegram_chat_id, args = %function_call.arguments, "handler received call for {}", function_call.name);

    match serde_json::from_str::<HashMap<String, String>>(&function_call.arguments) {
        Ok(args_map) => {
            if let Some(sql_query_from_ai) = args_map.get("sql_query") {
                info!(query = %sql_query_from_ai, "executing sql query from ai");

                let sql_result_json = execute_db_query(ctx.pool, sql_query_from_ai).await; // Call local function

                let mut inputs_for_step2 = original_input_items;
                inputs_for_step2.push(InputItem::Message(InputMessageObject {
                    role: "assistant".to_string(),
                    content: format!(
                        "tool_call: name={}, id={}, call_id={}, args={}",
                        function_call.name,
                        function_call.id,
                        function_call.call_id,
                        function_call.arguments
                    ),
                }));
                inputs_for_step2.push(InputItem::FunctionCallOutput(
                    crate::openai_api::FunctionCallOutputItem {
                        r#type: "function_call_output".to_string(),
                        call_id: function_call.call_id.clone(),
                        output: sql_result_json.to_string(),
                    },
                ));

                info!("(handler) sending function call result back to /v1/responses api");
                let step2_api_args = crate::openai_api::CallResponsesApiOptionalArgs {
                    model_id: crate::message_processor::OPENAI_RESPONSES_MODEL_ID, // Access const from message_processor
                    previous_response_id: Some(initial_api_response_id),
                    tools: Some(available_tools),
                    tool_choice: None,
                    instructions: Some(instructions),
                    temperature: None,
                    store: None,
                };
                match call_responses_api(
                    ctx.http_client,
                    ctx.openai_api_key,
                    inputs_for_step2,
                    step2_api_args,
                )
                .await
                {
                    Ok(api_response_2) => {
                        let response_id_for_db = api_response_2.id.clone();
                        if let Some(OutputItem::Message(final_msg)) = api_response_2.output.first() {
                            if final_msg.role == "assistant" {
                                if let Some(content) = final_msg.content.first() {
                                    if content.r#type == "output_text" {
                                        return Ok((content.text.clone(), response_id_for_db));
                                    }
                                }
                            }
                        }
                        warn!(chat_id = telegram_chat_id, "tool call step 2 response did not contain expected assistant message text structure.");
                        Ok((
                            "i processed the database query but couldn't form a final summary in the expected format.".to_string(), 
                            response_id_for_db 
                        ))
                    }
                    Err(e) => {
                        error!(chat_id = telegram_chat_id, error = %e, "tool call step 2 api call failed");
                        Err(e).context("tool call step 2 api call failed")
                    }
                }
            } else {
                warn!(chat_id = telegram_chat_id, "'sql_query' missing in {} args", function_call.name);
                Err(eyre!(
                    "argument 'sql_query' missing for tool {}",
                    function_call.name
                ))
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to parse args for {}", function_call.name);
            Err(e).context(format!(
                "failed to parse args for tool {}",
                function_call.name
            ))
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    // Re-add eyre::Result for tests if it was removed from top-level uses, or keep if still used.
    // Assuming execute_db_query tests from sql_select.rs are moved here.

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
            "CREATE TABLE {} (\n            id SERIAL PRIMARY KEY,\n            username VARCHAR(255) NOT NULL UNIQUE,\n            email VARCHAR(255) NOT NULL UNIQUE,\n            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()\n        );",
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
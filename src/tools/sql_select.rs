use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestUserMessageArgs, ChatCompletionToolArgs, ChatCompletionToolType,
        CreateChatCompletionRequestArgs, FunctionObjectArgs,
    },
    Client as OpenAIClient,
};
use eyre::{eyre, Context, Result};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use tracing::{debug, error, info, warn};

// tool name can remain execute_sql_query for compatibility if the ai is already trained/prompted for it,
// but the description and implementation will make it select-only.
const SQL_TOOL_NAME: &str = "execute_sql_query";
const SQL_TOOL_DESCRIPTION: &str = "executes a sql select query against the postgresql database and returns the results. only select queries are permitted. attempts to use other query types will result in an error.";

fn get_sql_tool_parameters() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "sql_query": {
                "type": "string",
                "description": "the sql select query to execute. example: select * from users where id = 1. must start with 'select'."
            }
        },
        "required": ["sql_query"]
    })
}

// this function executes the AI-generated query. it expects the query to target existing tables like 'users'.
// for its own unit tests, queries will target 'tool_test_users'.
async fn execute_db_query(pool: &PgPool, query: &str) -> JsonValue {
    info!(query = %query, "(sql select tool) attempting to execute db query");

    let trimmed_query = query.trim().to_lowercase();
    if !trimmed_query.starts_with("select") {
        warn!(query = %query, "(sql select tool) rejected non-select query.");
        return json!({
            "status": "error",
            "message": "invalid query type. only select queries are permitted.",
            "details": format!("query was: {}", query)
        });
    }

    info!(query = %query, "(sql select tool) executing select db query");
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
            error!(error = %e, query = %query, "(sql select tool) failed to execute sql query");
            json!({
                "status": "error",
                "message": "failed to execute sql query.",
                "details": e.to_string()
            })
        }
    }
}

pub async fn process_instruction_with_sql_tool(
    openai_client: &OpenAIClient<OpenAIConfig>,
    pool: &PgPool, // this pool should connect to the db with the actual 'users' table
    instruction: String,
    model: &str,
) -> Result<String> {
    info!(instruction = %instruction, model = %model, "(sql select tool) processing instruction");

    let system_prompt = format!(
        "you are a helpful assistant. the user may refer to you as lexi. you have one tool available: '{}'. \
        use it when appropriate to answer user questions. \
        only issue select queries starting with 'select'. \
        the available tables are: \
        1. 'users' (stores telegram user information): \
           columns: id (serial primary key), telegram_id (bigint unique not null), username (text), first_name (text not null), last_name (text), is_bot (boolean not null default false), created_at (timestamptz not null default now()), updated_at (timestamptz not null default now()). \
        2. 'chats' (stores chat information): \
           columns: id (serial primary key), telegram_id (bigint unique not null), type (text not null - e.g., 'private', 'group'), title (text), username (text), created_at (timestamptz not null default now()), updated_at (timestamptz not null default now()). \
        3. 'messages' (stores messages from chats): \
           columns: id (serial primary key), telegram_message_id (bigint not null), chat_id (integer not null, references chats.id), sender_id (integer not null, references users.id), text (text), sent_at (timestamptz not null), raw_message (text), created_at (timestamptz not null default now()). \
        ensure your queries target these tables and their specified columns. \
        if you use the tool, summarize the results for the user upon receiving them.",
        SQL_TOOL_NAME
    );
    let system_message = ChatCompletionRequestSystemMessageArgs::default()
        .content(system_prompt)
        .build()?;

    let initial_user_message = ChatCompletionRequestUserMessageArgs::default()
        .content(instruction.clone())
        .build()?;

    let mut messages: Vec<ChatCompletionRequestMessage> =
        vec![system_message.into(), initial_user_message.into()];

    let tool_definition = ChatCompletionToolArgs::default()
        .r#type(ChatCompletionToolType::Function)
        .function(
            FunctionObjectArgs::default()
                .name(SQL_TOOL_NAME)
                .description(SQL_TOOL_DESCRIPTION)
                .parameters(get_sql_tool_parameters())
                .build()?,
        )
        .build()?;

    let first_request = CreateChatCompletionRequestArgs::default()
        .model(model)
        .messages(messages.clone())
        .tools(vec![tool_definition])
        .build()
        .context("(sql select tool) failed to build first openai request")?;

    info!("(sql select tool) sending first request to openai.");
    let first_response = openai_client.chat().create(first_request).await?;
    debug!(response_details = ?first_response, "(sql select tool) first openai response");

    if let Some(first_choice) = first_response.choices.first() {
        let assistant_message_from_api = first_choice.message.clone();
        let mut assistant_msg_builder = ChatCompletionRequestAssistantMessageArgs::default();
        if let Some(content) = assistant_message_from_api.content {
            assistant_msg_builder.content(content);
        }
        if let Some(tool_calls) = assistant_message_from_api.tool_calls {
            assistant_msg_builder.tool_calls(tool_calls);
        }
        messages.push(assistant_msg_builder.build()?.into());

        if let Some(tool_calls_from_response) = &first_choice.message.tool_calls {
            for tool_call in tool_calls_from_response {
                if tool_call.r#type == ChatCompletionToolType::Function
                    && tool_call.function.name == SQL_TOOL_NAME
                {
                    info!(tool_call_id = %tool_call.id, args = %tool_call.function.arguments, "(sql select tool) processing tool call.");
                    let parsed_args: Result<JsonValue, _> =
                        serde_json::from_str(&tool_call.function.arguments);
                    match parsed_args {
                        Ok(json_args) => {
                            if let Some(sql_query) =
                                json_args.get("sql_query").and_then(|v| v.as_str())
                            {
                                // AI-generated query is executed directly by execute_db_query.
                                // This query will target tables like 'users', not 'tool_test_users'.
                                let tool_response_data = execute_db_query(pool, sql_query).await;
                                let tool_response_msg =
                                    ChatCompletionRequestToolMessageArgs::default()
                                        .tool_call_id(tool_call.id.clone())
                                        .content(tool_response_data.to_string())
                                        .build()?;
                                messages.push(tool_response_msg.into());
                            } else {
                                let err_msg = "(sql select tool) 'sql_query' missing/invalid.";
                                error!(err_msg);
                                messages.push(
                                    ChatCompletionRequestToolMessageArgs::default()
                                        .tool_call_id(tool_call.id.clone())
                                        .content(
                                            json!({ "status": "error", "message": err_msg })
                                                .to_string(),
                                        )
                                        .build()?
                                        .into(),
                                );
                            }
                        }
                        Err(e) => {
                            let err_msg =
                                format!("(sql select tool) failed to parse tool args: {}", e);
                            error!(err_msg);
                            messages.push(
                                ChatCompletionRequestToolMessageArgs::default()
                                    .tool_call_id(tool_call.id.clone())
                                    .content(
                                        json!({ "status": "error", "message": err_msg })
                                            .to_string(),
                                    )
                                    .build()?
                                    .into(),
                            );
                        }
                    }
                } else {
                    warn!(tool_details = ?tool_call, "(sql select tool) ai called an unexpected tool.");
                }
            }

            info!("(sql select tool) sending second request to openai with tool response(s).");
            let second_request = CreateChatCompletionRequestArgs::default()
                .model(model)
                .messages(messages.clone())
                .build()
                .context("(sql select tool) failed to build second openai request")?;

            let second_response = openai_client.chat().create(second_request).await?;
            debug!(response_details = ?second_response, "(sql select tool) second openai response");

            if let Some(second_choice) = second_response.choices.first() {
                if let Some(final_content) = &second_choice.message.content {
                    info!(message = %final_content, "(sql select tool) received final ai response.");
                    return Ok(final_content.clone());
                } else {
                    warn!("(sql select tool) second ai response had no content.");
                    return Err(eyre!(
                        "(sql select tool) second ai response had no content."
                    ));
                }
            } else {
                error!("(sql select tool) no choices in second openai response.");
                return Err(eyre!(
                    "(sql select tool) no choices in second openai response."
                ));
            }
        } else if let Some(content) = &first_choice.message.content {
            info!(%content, "(sql select tool) ai responded directly without tool call.");
            return Ok(content.clone());
        } else {
            warn!("(sql select tool) first ai response had no content or tool calls.");
            return Err(eyre!(
                "(sql select tool) first ai response had no content or tool calls."
            ));
        }
    } else {
        error!("(sql select tool) no choices in first openai response.");
        Err(eyre!(
            "(sql select tool) no choices in first openai response."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    // this function prepares a uniquely named table specifically for unit testing the sql_select tool's db execution logic.
    // it does not and should not rely on the main bot's migrations or affect the main 'users' table.
    pub async fn prepare_tool_test_users_table(pool: &PgPool) -> Result<()> {
        let table_name = "tool_test_users";
        info!(table = %table_name, "(sql select tool) preparing dedicated test table...");

        // Use format! to safely inject table name into DDL - not ideal for parameters but okay for fixed internal table names.
        // For user-provided table names, this would be a SQL injection risk.
        sqlx::query(&format!("DROP TABLE IF EXISTS {}", table_name))
            .execute(pool)
            .await
            .context(format!(
                "failed to drop table {} (sql select tool tests)",
                table_name
            ))?;

        info!(table = %table_name, "(sql select tool) creating dedicated test table {}...", table_name);
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

        info!(table = %table_name, "(sql select tool) inserting test data into {}...", table_name);
        let test_users = [
            ("simple_alpha", "alpha@simpletest.com"),
            ("simple_beta", "beta@simpletest.com"),
        ];
        for (username, email) in &test_users {
            // Important: Use query builder or $1, $2 for values, not format! for values.
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
        info!(table = %table_name, "(sql select tool) dedicated test table {} prepared.", table_name);
        Ok(())
    }

    #[sqlx::test] // ensures test runs in a transaction, migrations = false is good practice here
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
        // No need to prepare schema if we are testing a non-SELECT query that should be rejected before DB interaction.
        // However, prepare_tool_test_users_table is harmless here if called, due to transaction rollback.
        // prepare_tool_test_users_table(&pool).await.expect("db schema prep for test_non_select_rejected failed");

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

        let query = "SELECT non_existent_column FROM tool_test_users;"; // Querying the test table
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

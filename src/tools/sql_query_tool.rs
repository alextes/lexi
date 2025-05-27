use anyhow::{anyhow, Context, Result};
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
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use sqlx::{Column, PgPool, Row, ValueRef};
use tracing::{debug, error, info, warn};

const SQL_TOOL_NAME: &str = "execute_sql_query";
const SQL_TOOL_DESCRIPTION: &str = "executes a sql query against the postgresql database and returns the results. use this to answer questions about data stored in the database. only select queries are permitted.";

fn get_sql_tool_parameters() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "sql_query": {
                "type": "string",
                "description": "the sql select query to execute. example: select * from users where id = 1"
            }
        },
        "required": ["sql_query"]
    })
}

pub async fn prepare_sql_tool_schema(pool: &PgPool) -> Result<()> {
    info!("(sql tool) preparing schema...");
    sqlx::query("DROP TABLE IF EXISTS users")
        .execute(pool)
        .await
        .context("failed to drop users table (sql tool)")?;

    sqlx::query(
        "CREATE TABLE users (
            id SERIAL PRIMARY KEY,
            username VARCHAR(255) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL UNIQUE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );",
    )
    .execute(pool)
    .await
    .context("failed to create users table (sql tool)")?;

    let test_users = [
        ("tool_user_alpha", "alpha@tooltest.com"),
        ("tool_user_beta", "beta@tooltest.com"),
    ];
    for (username, email) in &test_users {
        sqlx::query(
            "INSERT INTO users (username, email) VALUES ($1, $2) ON CONFLICT (username) DO NOTHING",
        )
        .bind(username)
        .bind(email)
        .execute(pool)
        .await
        .context(format!(
            "failed to insert test user: {} (sql tool)",
            username
        ))?;
    }
    info!("(sql tool) schema prepared with test data.");
    Ok(())
}

async fn execute_db_query(pool: &PgPool, query: &str) -> JsonValue {
    info!(actual_query = %query, "(sql tool) executing db query");
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
            error!(error = %e, query = %query, "(sql tool) failed to execute sql query");
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
    pool: &PgPool,
    instruction: String,
    model: &str,
) -> Result<String> {
    info!(instruction = %instruction, model = %model, "(sql tool) processing instruction");

    let system_prompt = format!("you are a helpful assistant. you have one tool available: '{}'. use it when appropriate to answer user questions. only issue select queries. if you use the tool, summarize the results for the user upon receiving them.", SQL_TOOL_NAME);
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
        .context("(sql tool) failed to build first openai request")?;

    info!("(sql tool) sending first request to openai.");
    let first_response = openai_client.chat().create(first_request).await?;
    debug!(response_details = ?first_response, "(sql tool) first openai response");

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
                    info!(tool_call_id = %tool_call.id, args = %tool_call.function.arguments, "(sql tool) processing tool call.");
                    let parsed_args: Result<JsonValue, _> =
                        serde_json::from_str(&tool_call.function.arguments);
                    match parsed_args {
                        Ok(json_args) => {
                            if let Some(sql_query) =
                                json_args.get("sql_query").and_then(|v| v.as_str())
                            {
                                let tool_response_data = execute_db_query(pool, sql_query).await;
                                let tool_response_msg =
                                    ChatCompletionRequestToolMessageArgs::default()
                                        .tool_call_id(tool_call.id.clone())
                                        .content(tool_response_data.to_string())
                                        .build()?;
                                messages.push(tool_response_msg.into());
                            } else {
                                let err_msg = "(sql tool) 'sql_query' missing/invalid.";
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
                            let err_msg = format!("(sql tool) failed to parse tool args: {}", e);
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
                    warn!(tool_details = ?tool_call, "(sql tool) ai called an unexpected tool.");
                }
            }

            info!("(sql tool) sending second request to openai with tool response(s).");
            let second_request = CreateChatCompletionRequestArgs::default()
                .model(model)
                .messages(messages.clone())
                .build()
                .context("(sql tool) failed to build second openai request")?;

            let second_response = openai_client.chat().create(second_request).await?;
            debug!(response_details = ?second_response, "(sql tool) second openai response");

            if let Some(second_choice) = second_response.choices.first() {
                if let Some(final_content) = &second_choice.message.content {
                    info!(message = %final_content, "(sql tool) received final ai response.");
                    return Ok(final_content.clone());
                } else {
                    warn!("(sql tool) second ai response had no content.");
                    return Err(anyhow!("(sql tool) second ai response had no content."));
                }
            } else {
                error!("(sql tool) no choices in second openai response.");
                return Err(anyhow!("(sql tool) no choices in second openai response."));
            }
        } else if let Some(content) = &first_choice.message.content {
            info!(%content, "(sql tool) ai responded directly without tool call.");
            return Ok(content.clone());
        } else {
            warn!("(sql tool) first ai response had no content or tool calls.");
            return Err(anyhow!(
                "(sql tool) first ai response had no content or tool calls."
            ));
        }
    } else {
        error!("(sql tool) no choices in first openai response.");
        Err(anyhow!("(sql tool) no choices in first openai response."))
    }
}

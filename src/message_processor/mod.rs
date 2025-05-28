use crate::openai_api::{
    call_responses_api, InputItem, InputMessageObject, OutputItem, ToolDefinition,

};
use crate::telegram::types::{Message as TelegramMessage, Update as TelegramUpdate};
use crate::{db, telegram};
use eyre::{Context, Result}; 
use reqwest::Client as ReqwestClient;
use serde_json::to_string as serde_json_to_string;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

pub mod tools;

const BOT_USERNAME: &str = "@lexi_alex_bot";
const OPENAI_RESPONSES_MODEL_ID: &str = "gpt-4.1-nano";

// Context struct to hold shared resources and configuration
pub struct HandlerContext<'a> {
    pub pool: &'a PgPool,
    pub http_client: &'a ReqwestClient,
    pub api_base_url: &'a str,
    pub bot_token: &'a str,
    pub bot_db_id: i32,
    pub openai_api_key: &'a str,
}

async fn process_message_content(
    ctx: &HandlerContext<'_>,
    incoming_message: &TelegramMessage,
    local_chat_id_for_conversation: i32,
) -> Result<()> {
    let should_trigger_ai_reply = incoming_message.chat.chat_type == "private"
        || mentions_bot(&incoming_message.text, &incoming_message.entities);

    if should_trigger_ai_reply {
        let prompt_text = if incoming_message.chat.chat_type == "private" {
            incoming_message.text.as_deref().unwrap_or("").to_string()
        } else {
            extract_prompt_from_mention(
                incoming_message.text.as_deref().unwrap_or(""),
                &incoming_message.entities,
            )
        };

        if prompt_text.is_empty() && incoming_message.text.is_some() {
            info!(
                chat_id = incoming_message.chat.id,
                "prompt is empty after processing, sending generic acknowledgement."
            );
            let acknowledgement = format!("Hi {}! You {} me, but your message seemed empty after processing. What can I help you with?",
                incoming_message.from.as_ref().map_or("there", |u| &u.first_name),
                if incoming_message.chat.chat_type == "private" { "messaged" } else { "mentioned" }
            );
            let sent_ack_message = telegram::send_message(
                ctx.http_client,
                ctx.api_base_url,
                ctx.bot_token,
                incoming_message.chat.id,
                &acknowledgement,
            )
            .await
            .wrap_err_with(|| "failed to send acknowledgement for empty prompt")?;

            let ack_raw_json = serde_json_to_string(&sent_ack_message)
                .context("failed to serialize bot acknowledgement message to JSON")?;
            db::insert_message(
                ctx.pool,
                &sent_ack_message,
                local_chat_id_for_conversation,
                ctx.bot_db_id,
                &ack_raw_json,
            )
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to insert bot acknowledgement (id: {}) into database",
                    sent_ack_message.message_id
                )
            })?;
            info!(
                chat_id = incoming_message.chat.id,
                sent_message_id = sent_ack_message.message_id,
                "saved bot acknowledgement to db"
            );
        } else if !prompt_text.is_empty() {
            generate_and_send_ai_reply(
                ctx,
                incoming_message,
                &prompt_text,
                local_chat_id_for_conversation,
            )
            .await?;
        } else {
            debug!(
                chat_id = incoming_message.chat.id,
                "message text is empty, not an error, just no text to process for AI reply."
            );
        }
    } else {
        log_other_mentions(incoming_message);
    }
    Ok(())
}

pub async fn process_update(
    pool: &PgPool,
    update: &TelegramUpdate,
    http_client: &ReqwestClient,
    api_base_url: &str,
    bot_token: &str,
    bot_db_id: i32,
    openai_api_key: &str,
) -> Result<()> {
    debug!(?update, "processing update in handler");

    if let Some(incoming_message) = &update.message {
        let sender_data = match &incoming_message.from {
            Some(user) => user,
            None => {
                warn!(
                    message_id = incoming_message.message_id,
                    chat_id = incoming_message.chat.id,
                    "message has no sender (e.g., channel post), skipping db insert."
                );
                log_other_mentions(incoming_message);
                return Ok(());
            }
        };

        let local_user_id = db::upsert_user(pool, sender_data)
            .await
            .wrap_err_with(|| format!("upserting user (telegram_id: {}) failed", sender_data.id))?;

        let chat_data = &incoming_message.chat;
        let local_chat_id_for_conversation = db::upsert_chat(pool, chat_data)
            .await
            .wrap_err_with(|| format!("upserting chat (telegram_id: {}) failed", chat_data.id))?;

        let raw_message_json = serde_json_to_string(incoming_message).wrap_err_with(|| {
            format!(
                "serializing message (id: {}) to json failed",
                incoming_message.message_id
            )
        })?;

        db::insert_message(
            pool,
            incoming_message,
            local_chat_id_for_conversation,
            local_user_id,
            &raw_message_json,
        )
        .await
        .wrap_err_with(|| {
            format!(
                "inserting incoming message (id: {}) failed",
                incoming_message.message_id
            )
        })?;

        info!(
            telegram_message_id = incoming_message.message_id,
            local_db_user_id = local_user_id,
            local_db_chat_id = local_chat_id_for_conversation,
            "successfully inserted incoming message"
        );

        // Create context for handler functions
        let ctx = HandlerContext {
            pool,
            http_client,
            api_base_url,
            bot_token,
            bot_db_id,
            openai_api_key,
        };

        process_message_content(&ctx, incoming_message, local_chat_id_for_conversation).await?;
    }
    Ok(())
}

fn mentions_bot(
    text_option: &Option<String>,
    entities_option: &Option<Vec<crate::telegram::types::MessageEntity>>,
) -> bool {
    if let (Some(text), Some(entities)) = (text_option, entities_option) {
        entities.iter().any(|entity| {
            if entity.entity_type == "mention" {
                let mention_text = &text[entity.offset..entity.offset + entity.length];
                mention_text.eq_ignore_ascii_case(BOT_USERNAME)
            } else {
                false
            }
        })
    } else {
        false
    }
}

fn extract_prompt_from_mention(
    text: &str,
    entities_option: &Option<Vec<crate::telegram::types::MessageEntity>>,
) -> String {
    if let Some(entities) = entities_option {
        for entity in entities {
            if entity.entity_type == "mention" {
                let mention_text = &text[entity.offset..entity.offset + entity.length];
                if mention_text.eq_ignore_ascii_case(BOT_USERNAME) {
                    return text.replace(mention_text, "").trim().to_string();
                }
            }
        }
    }
    text.to_string()
}

async fn generate_and_send_ai_reply(
    ctx: &HandlerContext<'_>,
    incoming_message: &TelegramMessage,
    prompt_text: &str,
    local_chat_id_for_conversation: i32,
) -> Result<()> {
    info!(
        chat_id = incoming_message.chat.id,
        message_id = incoming_message.message_id,
        prompt = prompt_text,
        "generating ai reply for user: '{}'", prompt_text
    );

    let previous_response_id_opt = match db::get_last_openai_response_id(ctx.pool, local_chat_id_for_conversation).await {
        Ok(id_opt) => id_opt,
        Err(e) => {
            warn!(chat_id = incoming_message.chat.id, error = %e, "failed to fetch last_openai_response_id, proceeding without it.");
            None
        }
    };

    let input_items = vec![InputItem::Message(InputMessageObject {
        role: "user".to_string(),
        content: prompt_text.to_string(),
    })];

    let available_tools = vec![tools::execute_sql_query::SQL_QUERY_TOOL.clone()];
    let instructions = format!(
        "you are a helpful ai assistant named lexi. use tools if appropriate. \
        you have one tool available: '{}'. \
        its purpose is to execute a sql select query you provide against the database. \
        you must formulate the sql query yourself. only select queries are permitted. \
        the available tables are (focus on querying 'users', 'chats', 'messages'): \
        1. 'users' (stores telegram user information): \
           columns: id (serial primary key), telegram_id (bigint unique not null), username (text), first_name (text not null), last_name (text), is_bot (boolean not null default false), created_at (timestamptz not null default now()), updated_at (timestamptz not null default now()). \
        2. 'chats' (stores chat information): \
           columns: id (serial primary key), telegram_id (bigint unique not null), type (text not null - e.g., 'private', 'group'), title (text), username (text), created_at (timestamptz not null default now()), updated_at (timestamptz not null default now()). \
        3. 'messages' (stores messages from chats): \
           columns: id (serial primary key), telegram_message_id (bigint not null), chat_id (integer not null, references chats.id), sender_id (integer not null, references users.id), text (text), sent_at (timestamptz not null), raw_message (text), created_at (timestamptz not null default now()). \
        ensure your queries target these tables and their specified columns correctly. if you use the tool, you will provide the exact sql query to execute.",
        tools::execute_sql_query::SQL_TOOL_NAME
    );

    let initial_api_args = crate::openai_api::CallResponsesApiOptionalArgs {
        model_id: OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: previous_response_id_opt.as_deref(),
        tools: Some(available_tools.clone()),
        tool_choice: None,
        instructions: Some(&instructions),
        temperature: None,
        store: None,
    };

    match call_responses_api(
        ctx.http_client,
        ctx.openai_api_key,
        input_items.clone(),
        initial_api_args,
    ).await {
        Ok(api_response_1) => {
            match process_openai_response(
                ctx, 
                incoming_message.chat.id,
                api_response_1, 
                input_items, 
                available_tools, // This will be passed to the tool handler
                &instructions
            ).await {
                Ok((final_text, response_id_to_store)) => {
                    send_reply_and_update_state(
                        ctx,
                        incoming_message.chat.id,
                        local_chat_id_for_conversation,
                        &final_text,
                        &response_id_to_store,
                    ).await?;
                }
                Err(e) => {
                    error!(chat_id = incoming_message.chat.id, error = %e, "error processing openai response or tool call");
                    let fallback_text = format!("sorry, an error occurred while processing your request with ai tools: {}", e);
                    let _ = send_reply_and_update_state(
                        ctx, 
                        incoming_message.chat.id, 
                        local_chat_id_for_conversation, 
                        &fallback_text, 
                        previous_response_id_opt.as_deref().unwrap_or("error_no_id_processing")
                    ).await.map_err(|send_err| {
                        error!(chat_id = incoming_message.chat.id, error = %send_err, "failed to send even the error fallback message.");
                    });
                    return Err(e);
                }
            }
        }
        Err(e) => {
            error!(chat_id = incoming_message.chat.id, error = %e, "initial /v1/responses api call failed");
            let fallback_message_text =
                "sorry, i encountered an issue calling the ai service.";
            let _ = send_reply_and_update_state(
                ctx, 
                incoming_message.chat.id, 
                local_chat_id_for_conversation, 
                fallback_message_text, 
                previous_response_id_opt.as_deref().unwrap_or("error_no_id_initial_call")
            ).await.map_err(|send_err| {
                error!(chat_id = incoming_message.chat.id, error = %send_err, "failed to send even the initial api call error fallback message.");
            });
            return Err(e);
        }
    }
    Ok(())
}

fn log_other_mentions(message: &TelegramMessage) {
    if let (Some(text), Some(entities)) = (message.text.as_ref(), message.entities.as_ref()) {
        for entity in entities {
            if entity.entity_type == "mention" {
                let mention_text = &text[entity.offset..entity.offset + entity.length];
                if !mention_text.eq_ignore_ascii_case(BOT_USERNAME) {
                    info!(
                        chat_id = message.chat.id,
                        user = message.from.as_ref().map_or_else(
                            || "n/a".to_string(),
                            |u| u.username.clone().unwrap_or_else(|| u.first_name.clone())
                        ),
                        mention = mention_text,
                        "(handler) received other mention (not bot)"
                    );
                }
            }
        }
    }
}

// Helper function to send reply and update database state
async fn send_reply_and_update_state(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64,     // Telegram's chat ID for sending the message
    local_chat_id_for_db: i32, // Our local DB chat ID
    reply_text: &str,
    response_id_to_store: &str, // The OpenAI response ID to store for conversation context
) -> Result<()> {
    info!(
        chat_id = telegram_chat_id,
        "sending final reply: '{}'", reply_text
    );
    let sent_bot_message = telegram::send_message(
        ctx.http_client,
        ctx.api_base_url,
        ctx.bot_token,
        telegram_chat_id, // Use the specific telegram chat id
        reply_text,
    )
    .await
    .wrap_err_with(|| format!("failed to send final reply to chat_id {}", telegram_chat_id))?;

    let bot_reply_raw_json = serde_json_to_string(&sent_bot_message)
        .context("failed to serialize bot reply message to json")?;
    db::insert_message(
        ctx.pool,
        &sent_bot_message,
        local_chat_id_for_db, // Use the local db chat id
        ctx.bot_db_id,
        &bot_reply_raw_json,
    )
    .await
    .wrap_err_with(|| {
        format!(
            "failed to insert bot final reply (id: {}) into database",
            sent_bot_message.message_id
        )
    })?;
    info!(
        chat_id = telegram_chat_id,
        sent_message_id = sent_bot_message.message_id,
        "saved bot final reply to db"
    );

    if let Err(e) = db::update_last_openai_response_id(
        ctx.pool,
        local_chat_id_for_db, // Use the local db chat id
        response_id_to_store,
    )
    .await
    {
        warn!(chat_id = telegram_chat_id, response_id = response_id_to_store, error = %e, "failed to update last_openai_response_id for chat.");
        // Not returning an error here, as sending the message was successful.
        // Logging the failure to update state is important, though.
    }
    Ok(())
}

// Helper function to process the initial API response and decide next steps
async fn process_openai_response(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64, 
    api_response_1: crate::openai_api::OpenAiApiResponse, 
    original_input_items: Vec<InputItem>,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    // Returns (final_text_to_send_to_user, response_id_to_store_for_chat_state)

    let response_1_id = api_response_1.id.clone(); // ID of the first response

    if let Some(output_item) = api_response_1.output.first() {
        match output_item {
            OutputItem::Message(msg) => {
                if msg.role == "assistant" {
                    if let Some(text_content) = msg.content.first() {
                        if text_content.r#type == "output_text" {
                            // Direct reply from AI, no tool used
                            return Ok((text_content.text.clone(), response_1_id));
                        }
                    }
                }
                // If assistant message is not found or not in expected format
                warn!(chat_id = telegram_chat_id, "no direct assistant text in first response: {:?}", msg);
                Ok(("i received a response from the ai, but couldn't understand it fully.".to_string(), response_1_id))
            }
            OutputItem::FunctionCall(fc) => {
                if fc.name == tools::execute_sql_query::SQL_TOOL_NAME {
                    // Delegate to the SQL tool handler
                    return tools::execute_sql_query::handle_execute_sql_query_tool_call(
                        ctx,
                        telegram_chat_id, 
                        fc, 
                        original_input_items,
                        &response_1_id, 
                        available_tools,
                        instructions,
                    ).await;
                } else {
                    warn!(chat_id = telegram_chat_id, function_call = ?fc, "main handler received unexpected function call name");
                    Ok((
                        format!(
                            "i tried to use an unexpected tool: {}. something went wrong.",
                            fc.name
                        ),
                        response_1_id,
                    ))
                }
            }
        }
    } else {
        warn!(chat_id = telegram_chat_id, "api response output was empty. response_id: {}", response_1_id);
        Ok((
            "i received an empty response from the ai.".to_string(),
            response_1_id,
        ))
    }
} 
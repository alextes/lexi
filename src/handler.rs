use crate::telegram::types::{Message as TelegramMessage, Update as TelegramUpdate};
use crate::{db, telegram};
use anyhow::{anyhow, Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
    Client as OpenAIClient,
};
use reqwest::Client as ReqwestClient;
use serde_json::to_string as serde_json_to_string;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

const BOT_USERNAME: &str = "@lexi_alex_bot";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4.1";

// Context struct to hold shared resources and configuration
struct HandlerContext<'a> {
    pool: &'a PgPool,
    openai_client: &'a OpenAIClient<OpenAIConfig>,
    http_client: &'a ReqwestClient,
    api_base_url: &'a str,
    bot_token: &'a str,
    bot_db_id: i64,
}

async fn process_message_content(
    ctx: &HandlerContext<'_>,
    incoming_message: &TelegramMessage,
    local_chat_id_for_conversation: i64,
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
            .with_context(|| "failed to send acknowledgement for empty prompt")?;

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
            .with_context(|| {
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
    openai_client: &OpenAIClient<OpenAIConfig>,
    http_client: &ReqwestClient,
    api_base_url: &str,
    bot_token: &str,
    bot_db_id: i64,
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
            .with_context(|| format!("upserting user (telegram_id: {}) failed", sender_data.id))?;

        let chat_data = &incoming_message.chat;
        let local_chat_id_for_conversation = db::upsert_chat(pool, chat_data)
            .await
            .with_context(|| format!("upserting chat (telegram_id: {}) failed", chat_data.id))?;

        let raw_message_json = serde_json_to_string(incoming_message).with_context(|| {
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
        .with_context(|| {
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
            openai_client,
            http_client,
            api_base_url,
            bot_token,
            bot_db_id,
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
    local_chat_id_for_conversation: i64,
) -> Result<()> {
    info!(
        chat_id = incoming_message.chat.id,
        message_id = incoming_message.message_id,
        prompt = prompt_text,
        "Generating AI reply for {}",
        if incoming_message.chat.chat_type == "private" {
            "direct message"
        } else {
            "mention"
        }
    );

    let mut user_message_builder = ChatCompletionRequestUserMessageArgs::default();
    user_message_builder.content(prompt_text);
    if let Some(name_str) = incoming_message
        .from
        .as_ref()
        .and_then(|u| u.username.clone())
    {
        user_message_builder.name(name_str);
    }
    let user_chat_message = user_message_builder
        .build()
        .with_context(|| "Failed to build user message for OpenAI")?
        .into();

    let system_chat_message = ChatCompletionRequestSystemMessageArgs::default()
        .content("You are a helpful AI assistant named Lexi.")
        .build()
        .with_context(|| "Failed to build system message for OpenAI")?
        .into();

    let request = CreateChatCompletionRequestArgs::default()
        .model(DEFAULT_OPENAI_MODEL)
        .messages(vec![system_chat_message, user_chat_message])
        .build()
        .with_context(|| "Failed to build OpenAI chat completion request")?;

    match ctx.openai_client.chat().create(request).await {
        Ok(completion_response) => {
            if let Some(choice) = completion_response.choices.first() {
                if let Some(ai_reply_content) = &choice.message.content {
                    info!(
                        chat_id = incoming_message.chat.id,
                        "Received reply from OpenAI: '{}'", ai_reply_content
                    );
                    let sent_bot_message = telegram::send_message(
                        ctx.http_client,
                        ctx.api_base_url,
                        ctx.bot_token,
                        incoming_message.chat.id,
                        ai_reply_content,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to send OpenAI reply to chat_id {}",
                            incoming_message.chat.id
                        )
                    })?;

                    let bot_reply_raw_json = serde_json_to_string(&sent_bot_message)
                        .context("failed to serialize bot reply message to JSON")?;
                    db::insert_message(
                        ctx.pool,
                        &sent_bot_message,
                        local_chat_id_for_conversation,
                        ctx.bot_db_id,
                        &bot_reply_raw_json,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to insert bot reply (id: {}) into database",
                            sent_bot_message.message_id
                        )
                    })?;
                    info!(
                        chat_id = incoming_message.chat.id,
                        sent_message_id = sent_bot_message.message_id,
                        "saved bot reply to db"
                    );
                } else {
                    warn!(
                        chat_id = incoming_message.chat.id,
                        "OpenAI response choice did not contain content."
                    );
                    return Err(anyhow!("OpenAI response choice missing content"));
                }
            } else {
                warn!(
                    chat_id = incoming_message.chat.id,
                    "OpenAI response did not contain any choices."
                );
                return Err(anyhow!("OpenAI response missing choices"));
            }
        }
        Err(e) => {
            error!(chat_id = incoming_message.chat.id, error = %e, "OpenAI API call failed");
            let fallback_message_text =
                "Sorry, I encountered an issue trying to process your request with the AI.";
            match telegram::send_message(
                ctx.http_client,
                ctx.api_base_url,
                ctx.bot_token,
                incoming_message.chat.id,
                fallback_message_text,
            )
            .await
            {
                Ok(sent_fallback_message) => {
                    warn!(
                        chat_id = incoming_message.chat.id,
                        "Sent fallback message after OpenAI error."
                    );
                    let fallback_raw_json = serde_json_to_string(&sent_fallback_message)
                        .context("failed to serialize bot fallback message to JSON")?;
                    if let Err(db_err) = db::insert_message(
                        ctx.pool,
                        &sent_fallback_message,
                        local_chat_id_for_conversation,
                        ctx.bot_db_id,
                        &fallback_raw_json,
                    )
                    .await
                    {
                        warn!(error = %db_err, "Failed to save bot's fallback message to DB.");
                    }
                }
                Err(send_err) => {
                    warn!(chat_id = incoming_message.chat.id, error = %send_err, "Failed to send fallback message after OpenAI error (send failed).");
                }
            }
            return Err(e.into());
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

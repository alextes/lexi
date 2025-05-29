//! this module handles the core logic for the telegram bot's operation.
//! it acts as an intermediary between raw telegram updates (received by `bot::r#loop`)
//! and the ai message processing logic (handled by `message_processor`).
//!
//! responsibilities include:
//! - parsing incoming telegram messages.
//! - determining if and how the bot should respond (e.g., based on mentions or private chat).
//! - extracting the prompt for the ai.
//! - managing database interactions for telegram-specific entities (users, chats, messages).
//! - orchestrating the call to the `message_processor` to get an ai-generated reply.
//! - sending the final reply back to the telegram user.
//! - maintaining conversation context by storing relevant openai response ids in the database.

pub mod r#loop; // declares the loop.rs submodule

use crate::db;
use crate::message_processor; // will call message_processor::drive_ai_conversation
use crate::telegram;
use crate::telegram::types::{Message as TelegramMessage, MessageEntity, Update as TelegramUpdate};
use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::to_string as serde_json_to_string;
use serde_json::Value;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};

// context struct required by bot logic to interact with various services.
// Renamed from BotLogicContext to BotContext as it's now in the `bot` module.
#[derive(Clone)]
pub struct BotContext<'a> {
    pub pool: &'a PgPool,
    pub http_client: &'a ReqwestClient,
    pub api_base_url: &'a str, // telegram api base
    pub bot_token: &'a str,
    pub bot_db_id: i32, // bot's own id in the users table
    pub openai_api_key: &'a str,
}

const BOT_USERNAME: &str = "@lexi_alex_bot";

pub fn mentions_bot(
    text_option: &Option<String>,
    entities_option: &Option<Vec<MessageEntity>>,
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

#[must_use]
pub fn extract_prompt_from_mention(
    text: &str,
    entities_option: &Option<Vec<MessageEntity>>,
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

pub fn log_other_mentions(message: &TelegramMessage) {
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
                        "received other mention (not bot)"
                    );
                }
            }
        }
    }
}

pub async fn send_reply_and_update_state(
    ctx: &BotContext<'_>,
    telegram_chat_id: i64,
    local_chat_id_for_db: i32,
    reply_text: &str,
    response_id_to_store: Option<&str>,
) -> Result<()> {
    info!(
        chat_id = telegram_chat_id,
        "sending final reply: '{}'",
        reply_text.chars().take(32).collect::<String>()
    );
    let sent_bot_message = telegram::send_message(
        ctx.http_client,
        ctx.api_base_url,
        ctx.bot_token,
        telegram_chat_id,
        reply_text,
    )
    .await
    .wrap_err_with(|| format!("failed to send final reply to chat_id {telegram_chat_id}"))?;

    let bot_reply_raw_json = serde_json_to_string(&sent_bot_message)
        .context("failed to serialize bot reply message to json")?;
    db::insert_message(
        ctx.pool,
        &sent_bot_message,
        local_chat_id_for_db,
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

    match response_id_to_store {
        Some(id_to_store) if !id_to_store.starts_with("error_no_id") => {
            if let Err(e) =
                db::update_last_openai_response_id(ctx.pool, local_chat_id_for_db, id_to_store)
                    .await
            {
                warn!(chat_id = telegram_chat_id, response_id = id_to_store, error = %e, "failed to update last_openai_response_id for chat.");
            }
        }
        _ => {
            warn!(
                chat_id = telegram_chat_id,
                "no valid response_id provided or an error placeholder was given; clearing last_openai_response_id for chat."
            );
            if let Err(e) = db::clear_last_openai_response_id(ctx.pool, local_chat_id_for_db).await
            {
                error!(chat_id = telegram_chat_id, error = %e, "failed to clear last_openai_response_id for chat after an issue.");
            }
        }
    }
    Ok(())
}

pub async fn handle_telegram_update(ctx: &BotContext<'_>, update: &TelegramUpdate) -> Result<()> {
    debug!(?update, "processing update");

    if let Some(incoming_message) = &update.message {
        let sender_data = if let Some(user) = &incoming_message.from {
            user
        } else {
            warn!(
                message_id = incoming_message.message_id,
                chat_id = incoming_message.chat.id,
                "message has no sender (e.g., channel post), skipping."
            );
            log_other_mentions(incoming_message);
            return Ok(());
        };

        let local_user_id = db::upsert_user(ctx.pool, sender_data)
            .await
            .wrap_err_with(|| format!("upserting user (telegram_id: {}) failed", sender_data.id))?;

        let chat_data = &incoming_message.chat;
        let local_chat_id_for_conversation = db::upsert_chat(ctx.pool, chat_data)
            .await
            .wrap_err_with(|| format!("upserting chat (telegram_id: {}) failed", chat_data.id))?;

        let raw_message_json = serde_json_to_string(incoming_message).wrap_err_with(|| {
            format!(
                "serializing message (id: {}) to json failed",
                incoming_message.message_id
            )
        })?;

        db::insert_message(
            ctx.pool,
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
                let acknowledgement = format!("hi {}! you {} me, but your message seemed empty after processing. what can i help you with?",
                    incoming_message.from.as_ref().map_or("there", |u| &u.first_name),
                    if incoming_message.chat.chat_type == "private" { "messaged" } else { "mentioned" }
                );
                send_reply_and_update_state(
                    ctx,
                    incoming_message.chat.id,
                    local_chat_id_for_conversation,
                    &acknowledgement,
                    None,
                )
                .await
                .wrap_err("failed to send/store acknowledgement for empty prompt")?;
            } else if !prompt_text.is_empty() {
                let previous_response_id_opt_string = match db::get_last_openai_response_id(
                    ctx.pool,
                    local_chat_id_for_conversation,
                )
                .await
                {
                    Ok(id_opt) => id_opt,
                    Err(e) => {
                        warn!(chat_id = incoming_message.chat.id, error = %e, "failed to fetch last_openai_response_id, proceeding without it.");
                        None
                    }
                };

                let mp_ctx = message_processor::HandlerContext {
                    pool: ctx.pool,
                    http_client: ctx.http_client,
                    bot_db_id: ctx.bot_db_id,
                    openai_api_key: ctx.openai_api_key,
                };

                match message_processor::drive_ai_conversation(
                    &mp_ctx,
                    &prompt_text,
                    incoming_message.chat.id,
                    previous_response_id_opt_string.as_deref(),
                )
                .await
                {
                    Ok(ai_outcome) => match ai_outcome {
                        message_processor::AiConversationOutcome::TextMessage(
                            final_text,
                            response_id_to_store,
                        ) => {
                            send_reply_and_update_state(
                                ctx,
                                incoming_message.chat.id,
                                local_chat_id_for_conversation,
                                &final_text,
                                Some(&response_id_to_store),
                            )
                            .await?;
                        }
                        message_processor::AiConversationOutcome::ResetConversation(
                            confirmation_json_str,
                            _response_id,
                        ) => {
                            let final_message_to_send = match serde_json::from_str::<Value>(
                                &confirmation_json_str,
                            ) {
                                Ok(json_val) => {
                                    if let Some(msg_content) =
                                        json_val.get("message").and_then(|v| v.as_str())
                                    {
                                        format!("system message: {}", msg_content)
                                    } else {
                                        warn!(chat_id = incoming_message.chat.id, json_payload = %confirmation_json_str, "resetconversation json did not contain a 'message' field. sending raw json.");
                                        confirmation_json_str.to_string()
                                    }
                                }
                                Err(e) => {
                                    warn!(chat_id = incoming_message.chat.id, error = %e, raw_payload = %confirmation_json_str, "failed to parse resetconversation json. sending raw string.");
                                    confirmation_json_str.to_string()
                                }
                            };

                            telegram::send_message(
                                ctx.http_client,
                                ctx.api_base_url,
                                ctx.bot_token,
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .wrap_err("failed to send conversation reset confirmation message")?;
                            if let Err(e) = db::clear_last_openai_response_id(
                                ctx.pool,
                                local_chat_id_for_conversation,
                            )
                            .await
                            {
                                error!(chat_id = incoming_message.chat.id, error = %e, "failed to clear last_openai_response_id for chat after reset command.");
                            }
                            info!(
                                chat_id = incoming_message.chat.id,
                                "conversation reset for chat."
                            );
                        }
                    },
                    Err(e) => {
                        error!(chat_id = incoming_message.chat.id, error = %e, "error from drive_ai_conversation");
                        let fallback_text =
                            format!("sorry, an error occurred while generating the ai reply: {e}");
                        let _ = send_reply_and_update_state(
                            ctx,
                            incoming_message.chat.id,
                            local_chat_id_for_conversation,
                            &fallback_text,
                            None,
                        )
                        .await
                        .map_err(|send_err| {
                            error!(chat_id = incoming_message.chat.id, error = %send_err, "failed to send even the error fallback message after content gen failure.");
                        });
                        return Err(e);
                    }
                }
            } else {
                debug!(
                    chat_id = incoming_message.chat.id,
                    "message text is empty, not an error, just no text to process for ai reply."
                );
            }
        } else {
            log_other_mentions(incoming_message);
        }
    } else {
        debug!("received update without a message (e.g., edited message, callback query), skipping direct reply logic.");
    }
    Ok(())
}

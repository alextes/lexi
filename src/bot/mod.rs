//! this module handles the core logic for the telegram bot's operation.
//! it acts as an intermediary between raw telegram updates (received by `bot::r#loop`)
//! and the ai interaction logic (handled by `ai_interaction`).
//!
//! responsibilities include:
//! - parsing incoming telegram messages.
//! - determining if and how the bot should respond (e.g., based on mentions or private chat).
//! - extracting the prompt for the ai.
//! - managing database interactions for telegram-specific entities (users, chats, messages).
//! - orchestrating the call to the `ai_interaction` to get an ai-generated reply.
//! - sending the final reply back to the telegram user.
//! - maintaining conversation context by storing relevant openai response ids in the database.

pub mod r#loop;
mod proactive;

use crate::ai_interaction::tools::beacon_slot_check::BeaconNodeHttp;
use crate::ai_interaction::tools::db_schema::LiveSchemaFetcher;
use crate::ai_interaction::tools::relay_circuit_breaker::RelayCircuitBreaker;
use crate::ai_interaction::{self, admin_session, AiConversationOutcome};
use crate::db::Db;
use crate::env::ENV_CONFIG;
use crate::telegram;
use crate::telegram::types::{Message as TelegramMessage, MessageEntity, Update as TelegramUpdate};
use anyhow::{Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::to_string as serde_json_to_string;
use serde_json::Value;
use tracing::{debug, error, info, warn};

// context struct required by bot logic to interact with various services.
#[derive(Clone)]
pub struct BotContext<D: Db> {
    pub db: D,
    pub http_client: ReqwestClient,
    pub api_base_url: String, // telegram api base
    pub bot_token: String,
    pub bot_db_id: i32, // bot's own id in the users table
    pub openai_api_key: String,
    pub schema_fetcher: LiveSchemaFetcher,
    pub beacon_node: BeaconNodeHttp,
    pub relay_circuit_breaker: RelayCircuitBreaker,
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
                    let temp_text = text.replace(mention_text, "");
                    return temp_text.split_whitespace().collect::<Vec<_>>().join(" ");
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

fn strip_admin_code(prompt_text: &str, bot_admin_code: Option<&str>) -> (bool, String) {
    let Some(expected_admin_code) = bot_admin_code else {
        return (false, prompt_text.to_string());
    };

    let admin_trigger_phrase = format!("admin code {expected_admin_code}");
    if prompt_text.contains(&admin_trigger_phrase) {
        let cleaned = prompt_text.replace(&admin_trigger_phrase, "");
        let normalized = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        (true, normalized)
    } else {
        (false, prompt_text.to_string())
    }
}

async fn admin_session_is_active<D: Db>(db: &D, local_chat_id: i32) -> bool {
    match admin_session::get_admin_session_state(db, local_chat_id).await {
        Ok(Some(state)) => state.active,
        Ok(None) => false,
        Err(e) => {
            warn!(local_chat_id, error = %e, "failed to read admin session state");
            false
        }
    }
}

pub async fn send_reply_and_update_state<D: Db>(
    ctx: &BotContext<D>,
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
        &ctx.http_client,
        &ctx.api_base_url,
        &ctx.bot_token,
        telegram_chat_id,
        reply_text,
    )
    .await
    .with_context(|| format!("failed to send final reply to chat_id {telegram_chat_id}"))?;

    let bot_reply_raw_json = serde_json_to_string(&sent_bot_message)
        .context("failed to serialize bot reply message to json")?;
    ctx.db
        .insert_message(
            &sent_bot_message,
            local_chat_id_for_db,
            ctx.bot_db_id,
            &bot_reply_raw_json,
        )
        .await
        .with_context(|| {
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
            if let Err(e) = ctx
                .db
                .update_last_openai_response_id(local_chat_id_for_db, id_to_store)
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
            if let Err(e) = ctx
                .db
                .clear_last_openai_response_id(local_chat_id_for_db)
                .await
            {
                error!(chat_id = telegram_chat_id, error = %e, "failed to clear last_openai_response_id for chat after an issue.");
            }
        }
    }
    Ok(())
}

pub async fn handle_telegram_update<D: Db + Clone>(
    ctx: &BotContext<D>,
    update: &TelegramUpdate,
) -> Result<()> {
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

        let local_user_id =
            ctx.db.upsert_user(sender_data).await.with_context(|| {
                format!("upserting user (telegram_id: {}) failed", sender_data.id)
            })?;

        let chat_data = &incoming_message.chat;
        let local_chat_id_for_conversation =
            ctx.db.upsert_chat(chat_data).await.with_context(|| {
                format!("upserting chat (telegram_id: {}) failed", chat_data.id)
            })?;

        let raw_message_json = serde_json_to_string(incoming_message).with_context(|| {
            format!(
                "serializing message (id: {}) to json failed",
                incoming_message.message_id
            )
        })?;

        ctx.db
            .insert_message(
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

        if let Some(txt) = &incoming_message.text {
            proactive::maybe_schedule_proactive_slot_check(ctx, incoming_message.chat.id, txt);
        }

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

            let previous_response_id_opt_string = match ctx
                .db
                .get_last_openai_response_id(local_chat_id_for_conversation)
                .await
            {
                Ok(id_opt) => id_opt,
                Err(e) => {
                    warn!(chat_id = incoming_message.chat.id, error = %e, "failed to fetch last_openai_response_id, proceeding without it.");
                    None
                }
            };

            let (admin_code_detected, sanitized_prompt) =
                strip_admin_code(&prompt_text, ENV_CONFIG.bot_admin_code.as_deref());
            let mut admin_session_active =
                admin_session_is_active(&ctx.db, local_chat_id_for_conversation).await;
            if !admin_session_active && admin_code_detected {
                if let Err(e) = admin_session::start_admin_session(
                    &ctx.db,
                    local_chat_id_for_conversation,
                    previous_response_id_opt_string.clone(),
                )
                .await
                {
                    warn!(chat_id = incoming_message.chat.id, error = %e, "failed to start admin session");
                } else {
                    admin_session_active = true;
                }
            }

            if sanitized_prompt.is_empty() && incoming_message.text.is_some() {
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
                .context("failed to send/store acknowledgement for empty prompt")?;
            } else if !sanitized_prompt.is_empty() {
                let current_model_id_for_ai_call = ai_interaction::get_current_model_id().await;

                let mp_ctx = ai_interaction::HandlerContext {
                    db: ctx.db.clone(),
                    http_client: ctx.http_client.clone(),
                    bot_db_id: ctx.bot_db_id,
                    openai_api_key: ctx.openai_api_key.clone(),
                    beacon_node: ctx.beacon_node.clone(),
                    relay_circuit_breaker: ctx.relay_circuit_breaker.clone(),
                    schema_fetcher: ctx.schema_fetcher.clone(),
                };

                match ai_interaction::drive_ai_conversation(
                    &mp_ctx,
                    &sanitized_prompt,
                    previous_response_id_opt_string.as_deref(),
                    &current_model_id_for_ai_call,
                    admin_session_active,
                )
                .await
                {
                    Ok(ai_outcome) => match ai_outcome {
                        AiConversationOutcome::TextMessage(final_text, response_id_to_store) => {
                            send_reply_and_update_state(
                                ctx,
                                incoming_message.chat.id,
                                local_chat_id_for_conversation,
                                &final_text,
                                Some(&response_id_to_store),
                            )
                            .await?;
                        }
                        AiConversationOutcome::ResetConversation(
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
                                        format!("system message: {msg_content}")
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
                                &ctx.http_client,
                                ctx.api_base_url.as_str(),
                                ctx.bot_token.as_str(),
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .context("failed to send conversation reset confirmation message")?;

                            if let Err(e) = ctx
                                .db
                                .clear_last_openai_response_id(local_chat_id_for_conversation)
                                .await
                            {
                                error!(
                                    chat_id = incoming_message.chat.id,
                                    local_chat_id = local_chat_id_for_conversation,
                                    error = %e,
                                    "critical: failed to clear last_openai_response_id in db for chat after reset."
                                );
                                // we'll continue, but the next message might be in the wrong context.
                            }

                            info!(
                                chat_id = incoming_message.chat.id,
                                "conversation reset for chat."
                            );
                        }
                        AiConversationOutcome::ChangeModel(
                            confirmation_json_str,
                            _response_id,
                            new_model_id,
                        ) => {
                            info!(chat_id = incoming_message.chat.id, %new_model_id, "openai model change requested by admin tool. updating global model id.");
                            ai_interaction::set_current_model_id(new_model_id.clone()).await;
                            let final_message_to_send = match serde_json::from_str::<Value>(
                                &confirmation_json_str,
                            ) {
                                Ok(json_val) => {
                                    if let Some(msg_content) =
                                        json_val.get("message").and_then(|v| v.as_str())
                                    {
                                        format!("system message: {msg_content}")
                                    } else {
                                        warn!(chat_id = incoming_message.chat.id, json_payload = %confirmation_json_str, "changemodel json did not contain a 'message' field. sending raw json.");
                                        confirmation_json_str.to_string()
                                    }
                                }
                                Err(e) => {
                                    warn!(chat_id = incoming_message.chat.id, error = %e, raw_payload = %confirmation_json_str, "failed to parse changemodel json. sending raw string.");
                                    confirmation_json_str.to_string()
                                }
                            };
                            telegram::send_message(
                                &ctx.http_client,
                                ctx.api_base_url.as_str(),
                                ctx.bot_token.as_str(),
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .context("failed to send model change confirmation message")?;
                        }
                        AiConversationOutcome::ChangeVerbosity(
                            confirmation_json_str,
                            _response_id,
                            new_verbosity,
                        ) => {
                            info!(chat_id = incoming_message.chat.id, %new_verbosity, "openai verbosity change requested by admin tool. updating global verbosity.");
                            ai_interaction::set_current_verbosity(Some(new_verbosity.clone()))
                                .await;
                            let final_message_to_send = match serde_json::from_str::<Value>(
                                &confirmation_json_str,
                            ) {
                                Ok(json_val) => {
                                    if let Some(msg_content) =
                                        json_val.get("message").and_then(|v| v.as_str())
                                    {
                                        format!("system message: {msg_content}")
                                    } else {
                                        warn!(chat_id = incoming_message.chat.id, json_payload = %confirmation_json_str, "changeverbosity json did not contain a 'message' field. sending raw json.");
                                        confirmation_json_str.to_string()
                                    }
                                }
                                Err(e) => {
                                    warn!(chat_id = incoming_message.chat.id, error = %e, raw_payload = %confirmation_json_str, "failed to parse changeverbosity json. sending raw string.");
                                    confirmation_json_str.to_string()
                                }
                            };

                            telegram::send_message(
                                &ctx.http_client,
                                ctx.api_base_url.as_str(),
                                ctx.bot_token.as_str(),
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .context("failed to send verbosity change confirmation message")?;
                        }
                        AiConversationOutcome::ChangeReasoningEffort(
                            confirmation_json_str,
                            _response_id,
                            new_effort,
                        ) => {
                            info!(chat_id = incoming_message.chat.id, %new_effort, "openai reasoning effort change requested by admin tool. updating global reasoning effort.");
                            ai_interaction::set_current_reasoning_effort(Some(new_effort.clone()))
                                .await;
                            let final_message_to_send = match serde_json::from_str::<Value>(
                                &confirmation_json_str,
                            ) {
                                Ok(json_val) => {
                                    if let Some(msg_content) =
                                        json_val.get("message").and_then(|v| v.as_str())
                                    {
                                        format!("system message: {msg_content}")
                                    } else {
                                        warn!(chat_id = incoming_message.chat.id, json_payload = %confirmation_json_str, "changereasoningeffort json did not contain a 'message' field. sending raw json.");
                                        confirmation_json_str.to_string()
                                    }
                                }
                                Err(e) => {
                                    warn!(chat_id = incoming_message.chat.id, error = %e, raw_payload = %confirmation_json_str, "failed to parse changereasoningeffort json. sending raw string.");
                                    confirmation_json_str.to_string()
                                }
                            };

                            telegram::send_message(
                                &ctx.http_client,
                                ctx.api_base_url.as_str(),
                                ctx.bot_token.as_str(),
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .context(
                                "failed to send reasoning effort change confirmation message",
                            )?;
                        }
                        AiConversationOutcome::EndAdminSession(
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
                                        format!("system message: {msg_content}")
                                    } else {
                                        warn!(chat_id = incoming_message.chat.id, json_payload = %confirmation_json_str, "end admin session json did not contain a 'message' field. sending raw json.");
                                        confirmation_json_str.to_string()
                                    }
                                }
                                Err(e) => {
                                    warn!(chat_id = incoming_message.chat.id, error = %e, raw_payload = %confirmation_json_str, "failed to parse end admin session json. sending raw string.");
                                    confirmation_json_str.to_string()
                                }
                            };

                            if let Ok(state_opt) = admin_session::get_admin_session_state(
                                &ctx.db,
                                local_chat_id_for_conversation,
                            )
                            .await
                            {
                                if let Some(state) = state_opt {
                                    match state.last_response_id_before_admin.as_deref() {
                                        Some(resp_id) => {
                                            if let Err(e) = ctx
                                                .db
                                                .update_last_openai_response_id(
                                                    local_chat_id_for_conversation,
                                                    resp_id,
                                                )
                                                .await
                                            {
                                                warn!(chat_id = incoming_message.chat.id, response_id = resp_id, error = %e, "failed to restore last_openai_response_id after admin session end.");
                                            }
                                        }
                                        None => {
                                            if let Err(e) = ctx
                                                .db
                                                .clear_last_openai_response_id(
                                                    local_chat_id_for_conversation,
                                                )
                                                .await
                                            {
                                                warn!(chat_id = incoming_message.chat.id, error = %e, "failed to clear last_openai_response_id after admin session end.");
                                            }
                                        }
                                    }
                                }
                            } else {
                                warn!(
                                    chat_id = incoming_message.chat.id,
                                    "failed to read admin session state for end admin session."
                                );
                            }

                            if let Err(e) = admin_session::end_admin_session(
                                &ctx.db,
                                local_chat_id_for_conversation,
                            )
                            .await
                            {
                                warn!(chat_id = incoming_message.chat.id, error = %e, "failed to clear admin session state after end.");
                            }

                            telegram::send_message(
                                &ctx.http_client,
                                ctx.api_base_url.as_str(),
                                ctx.bot_token.as_str(),
                                incoming_message.chat.id,
                                &final_message_to_send,
                            )
                            .await
                            .context("failed to send admin session end confirmation message")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::types::MessageEntity; // For creating MessageEntity instances

    // tests for mentions_bot
    #[test]
    fn test_mentions_bot_true_when_mentioned() {
        let text = Some(format!("hello {BOT_USERNAME} how are you"));
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 6, // Start of "@lexi_alex_bot"
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert!(mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_when_not_mentioned() {
        let text = Some("hello regular user".to_string());
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 6,
            length: 12, // "regular user"
            url: None,
            user: None,
        }]);
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_with_text_no_entities() {
        let text = Some(format!("hello {BOT_USERNAME}"));
        let entities: Option<Vec<MessageEntity>> = None;
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_with_entities_no_text() {
        let text: Option<String> = None;
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 0,
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_with_no_text_no_entities() {
        let text: Option<String> = None;
        let entities: Option<Vec<MessageEntity>> = None;
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_with_empty_entities_list() {
        let text = Some(format!("hello {BOT_USERNAME}"));
        let entities = Some(Vec::new());
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_false_for_other_mention() {
        let text = Some("hello @other_bot how are you".to_string());
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 6,
            length: 10, // "@other_bot"
            url: None,
            user: None,
        }]);
        assert!(!mentions_bot(&text, &entities));
    }

    #[test]
    fn test_mentions_bot_true_case_insensitive() {
        let text = Some(format!("hello {}", BOT_USERNAME.to_uppercase()));
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 6,
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        // Assuming BOT_USERNAME is lowercase, the mention in text is uppercase
        // The comparison in mentions_bot is case-insensitive.
        assert!(mentions_bot(&text, &entities));
    }

    // tests for extract_prompt_from_mention
    #[test]
    fn test_extract_prompt_mention_at_start() {
        let text = format!("{BOT_USERNAME} what is the weather?");
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 0,
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert_eq!(
            extract_prompt_from_mention(&text, &entities),
            "what is the weather?"
        );
    }

    #[test]
    fn test_extract_prompt_mention_in_middle() {
        let text = format!("hey {BOT_USERNAME} tell me a joke");
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 4, // After "hey "
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert_eq!(
            extract_prompt_from_mention(&text, &entities),
            "hey tell me a joke"
        );
    }

    #[test]
    fn test_extract_prompt_mention_at_end() {
        let text_prefix = "summarize this for me ";
        let bot_mention_segment = BOT_USERNAME;
        let text = format!("{text_prefix}{bot_mention_segment}");

        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: text_prefix.len(),
            length: bot_mention_segment.len(),
            url: None,
            user: None,
        }]);

        println!(
            "test_extract_prompt_mention_at_end: text = '{}' (len: {} bytes, {} chars)",
            text,
            text.len(),
            text.chars().count()
        );
        println!(
            "test_extract_prompt_mention_at_end: text_prefix = '{}' (len: {})",
            text_prefix,
            text_prefix.len()
        );
        println!(
            "test_extract_prompt_mention_at_end: bot_mention_segment = '{}' (len: {})",
            bot_mention_segment,
            bot_mention_segment.len()
        );

        if let Some(ents) = &entities {
            if !ents.is_empty() {
                let entity = &ents[0];
                println!(
                    "test_extract_prompt_mention_at_end: entity.offset = {}, entity.length = {}",
                    entity.offset, entity.length
                );
                let end_slice = entity.offset + entity.length;
                println!("test_extract_prompt_mention_at_end: calculated slice end = {end_slice}");
            }
        }

        assert_eq!(
            extract_prompt_from_mention(&text, &entities),
            text_prefix.trim_end().to_string() // Ensure it's a String for comparison
        );
    }

    #[test]
    fn test_extract_prompt_no_mention_in_text() {
        let text = "just a regular message".to_string();
        let entities: Option<Vec<MessageEntity>> = None;
        assert_eq!(extract_prompt_from_mention(&text, &entities), text);
    }

    #[test]
    fn test_extract_prompt_mention_text_no_entities() {
        let text = format!("{BOT_USERNAME} what if entities are missing?");
        let entities: Option<Vec<MessageEntity>> = None;
        // Should return original text if entities are None, even if text contains mention string
        assert_eq!(extract_prompt_from_mention(&text, &entities), text);
    }

    #[test]
    fn test_extract_prompt_other_mention() {
        let text = "hey @other_bot what is up".to_string();
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 4,
            length: 10, // "@other_bot"
            url: None,
            user: None,
        }]);
        assert_eq!(extract_prompt_from_mention(&text, &entities), text);
    }

    #[test]
    fn test_extract_prompt_case_insensitive_mention() {
        let text = format!("{} HELP ME", BOT_USERNAME.to_uppercase());
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 0,
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert_eq!(extract_prompt_from_mention(&text, &entities), "HELP ME");
    }

    #[test]
    fn test_extract_prompt_empty_text() {
        let text = "";
        let entities: Option<Vec<MessageEntity>> = None;
        assert_eq!(extract_prompt_from_mention(text, &entities), "");
    }

    #[test]
    fn test_extract_prompt_only_mention() {
        let text = BOT_USERNAME.to_string();
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 0,
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        assert_eq!(extract_prompt_from_mention(&text, &entities), "");
    }

    #[test]
    fn test_extract_prompt_mention_with_leading_trailing_spaces_in_text() {
        let text = format!("  {BOT_USERNAME}  what now?  ");
        let entities = Some(vec![MessageEntity {
            entity_type: "mention".to_string(),
            offset: 2, // after leading spaces
            length: BOT_USERNAME.len(),
            url: None,
            user: None,
        }]);
        // .trim() in the function should handle the spaces around the extracted prompt
        assert_eq!(extract_prompt_from_mention(&text, &entities), "what now?");
    }
}

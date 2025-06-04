//! core ai conversation logic module.
//! it takes a user prompt and an optional previous openai response id, then drives the interaction
//! with the openai api, including any necessary tool calls, until a final text response is generated.
//! it is designed to be independent of the calling context (e.g., telegram, cli) and focuses solely
//! on the ai interaction flow, returning the final ai message and the last openai response id.

use crate::{ai_interaction::tools::beacon_slot_check::BeaconNode, db::Db}; // import the db trait
use anyhow::{Context, Result};
use reqwest::Client as ReqwestClient;
use tracing::{error, info, instrument};

use crate::openai_api::{
    call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
};
use openai_chat::{OPENAI_CALL_CONFIG, OPENAI_RESPONSES_MODEL_ID};

pub mod openai_chat;
pub mod tools;

/// represents the outcome of an ai conversation cycle.
pub enum AiConversationOutcome {
    /// the ai returned a text message.
    /// contains (message_content, response_id).
    TextMessage(String, String),
    /// the ai (or a tool called by the ai) requested the conversation to be reset.
    /// contains (confirmation_message_for_user, response_id_triggering_reset).
    ResetConversation(String, String),
}

pub struct HandlerContext<'a, D: Db, B: BeaconNode> {
    pub db: D,
    pub http_client: &'a ReqwestClient,
    pub bot_db_id: i32, // kept as some tools/logging might still reference it via context
    pub openai_api_key: &'a str,
    pub beacon_base_url: String,      // Added for beacon node base URL
    pub relay_admin_base_url: String, // Added for relay admin base URL
    pub beacon_node: B,
}

#[instrument(skip(ctx, prompt_text, previous_response_id), fields(logging_chat_id = %logging_chat_id))]
pub async fn drive_ai_conversation<D: Db, B: BeaconNode>(
    ctx: &HandlerContext<'_, D, B>,
    prompt_text: &str,
    logging_chat_id: i64, // This is used for logging within openai_chat and its tools
    previous_response_id: Option<&str>,
) -> Result<AiConversationOutcome> {
    info!(
        // logging_id field is now part of the span
        prompt = prompt_text,
        has_previous_id = previous_response_id.is_some(),
        "driving conversation for prompt: '{}'",
        prompt_text
    );

    let input_items = vec![InputItem::Message(InputMessageObject {
        role: "user".to_string(),
        content: prompt_text.to_string(),
    })];

    let initial_api_args = CallResponsesApiOptionalArgs {
        model_id: OPENAI_RESPONSES_MODEL_ID,
        previous_response_id,
        tools: Some(OPENAI_CALL_CONFIG.available_tools.clone()),
        tool_choice: None,
        instructions: Some(&OPENAI_CALL_CONFIG.instructions),
        temperature: None,
        store: Some(true),
    };

    match call_responses_api(
        ctx.http_client,
        ctx.openai_api_key,
        input_items.clone(),
        initial_api_args,
    )
    .await
    {
        Ok(api_response_1) => {
            openai_chat::start_ai_processing_loop(ctx, api_response_1, input_items)
                .await
                .context("core ai conversation processing loop failed")
        }
        Err(e) => {
            error!(error = %e, "initial /v1/responses api call failed");
            Err(e)
        }
    }
}

#[instrument(skip(ctx, prompt_text), fields(telegram_chat_id = %telegram_chat_id))]
pub async fn process_single_prompt_for_cli<D: Db, B: BeaconNode>(
    ctx: &HandlerContext<'_, D, B>,
    prompt_text: &str,
    telegram_chat_id: i64,
) -> Result<(String, String)> {
    info!(
        prompt = prompt_text,
        "processing single prompt via core ai driver"
    );
    match drive_ai_conversation(ctx, prompt_text, telegram_chat_id, None).await {
        Ok(outcome) => match outcome {
            AiConversationOutcome::TextMessage(message, response_id) => Ok((message, response_id)),
            AiConversationOutcome::ResetConversation(message, response_id) => {
                // For CLI, a reset doesn't mean much, so we'll just return the message and original ID.
                // The user can be informed that a reset was requested.
                info!(response_id = %response_id, "conversation reset was requested by ai/tool.");
                Ok((
                    format!(
                        "conversation reset requested by ai/tool, message: \"{}\"",
                        message
                    ),
                    response_id,
                ))
            }
        },
        Err(e) => Err(e),
    }
}

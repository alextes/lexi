//! core ai conversation logic module.
//! it takes a user prompt and an optional previous openai response id, then drives the interaction
//! with the openai api, including any necessary tool calls, until a final text response is generated.
//! it is designed to be independent of the calling context (e.g., telegram, cli) and focuses solely
//! on the ai interaction flow, returning the final ai message and the last openai response id.

use crate::{
    ai_interaction::tools::{
        beacon_slot_check::BeaconNode, relay_circuit_breaker::RelayCircuitBreaker,
    },
    db::Db,
}; // import the db trait
use anyhow::{Context, Result};
use reqwest::Client as ReqwestClient;
use tracing::{error, info, instrument};

use crate::env::ENV_CONFIG;
use crate::openai_api::{
    call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
};
use openai_chat::DEFAULT_OPENAI_MODEL_ID;
use openai_chat::OPENAI_CALL_CONFIG; // import the default model ID // Added import for ENV_CONFIG

// added imports for global model id store
use std::sync::LazyLock;
use tokio::sync::RwLock;
// end added imports

pub mod openai_chat;
pub mod tools;

// global model id store
pub static GLOBAL_MODEL_ID: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(DEFAULT_OPENAI_MODEL_ID.to_string()));

pub async fn get_current_model_id() -> String {
    GLOBAL_MODEL_ID.read().await.clone()
}

pub async fn set_current_model_id(new_model_id: String) {
    let mut model_id_guard = GLOBAL_MODEL_ID.write().await;
    *model_id_guard = new_model_id.clone(); // clone new_model_id for the log
    tracing::info!(new_global_model_id = %new_model_id, "global openai model id updated by admin tool");
}
// end global model id store

/// represents the outcome of an ai conversation cycle.
pub enum AiConversationOutcome {
    /// the ai returned a text message.
    /// contains (message_content, response_id).
    TextMessage(String, String),
    /// the ai (or a tool called by the ai) requested the conversation to be reset.
    /// contains (confirmation_message_for_user, response_id_triggering_reset).
    ResetConversation(String, String),
    /// the ai (or a tool called by the ai) requested the openai model to be changed for future turns.
    /// contains (confirmation_message_for_user, response_id_triggering_change, new_model_id).
    ChangeModel(String, String, String),
}

pub struct HandlerContext<'a, D: Db, B: BeaconNode> {
    pub db: D,
    pub http_client: ReqwestClient,
    pub bot_db_id: i32, // kept as some tools/logging might still reference it via context
    pub openai_api_key: &'a str,
    pub beacon_node: B,
    pub relay_circuit_breaker: RelayCircuitBreaker,
}

#[instrument(skip(ctx, prompt_text, previous_response_id))]
pub async fn drive_ai_conversation<D: Db, B: BeaconNode>(
    ctx: &HandlerContext<'_, D, B>,
    prompt_text: &str,
    previous_response_id: Option<&str>,
    current_model_id: &str,
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

    // determine tools *before* the first API call
    let turn_specific_tools = openai_chat::determine_turn_tools(
        &input_items,
        &OPENAI_CALL_CONFIG.available_tools,
        ENV_CONFIG.bot_admin_code.as_deref(),
        current_model_id,
    );

    let initial_api_args = CallResponsesApiOptionalArgs {
        model_id: current_model_id,
        previous_response_id,
        tools: Some(turn_specific_tools.clone()), // use the determined tools
        tool_choice: None,
        instructions: Some(&OPENAI_CALL_CONFIG.instructions),
        temperature: None,
        store: Some(true),
    };

    match call_responses_api(
        ctx.http_client.clone(),
        ctx.openai_api_key,
        input_items.clone(),
        initial_api_args,
    )
    .await
    {
        Ok(api_response_1) => openai_chat::start_ai_processing_loop(
            ctx,
            api_response_1,
            input_items,
            current_model_id.to_string(),
            turn_specific_tools, // pass the determined tools
        )
        .await
        .context("core ai conversation processing loop failed"),
        Err(e) => {
            error!(error = %e, "initial /v1/responses api call failed");
            Err(e)
        }
    }
}

#[instrument(skip(ctx, prompt_text))]
pub async fn process_single_prompt_for_cli<D: Db, B: BeaconNode>(
    ctx: &HandlerContext<'_, D, B>,
    prompt_text: &str,
) -> Result<(String, String)> {
    info!(
        prompt = prompt_text,
        "processing single prompt via core ai driver"
    );
    match drive_ai_conversation(ctx, prompt_text, None, DEFAULT_OPENAI_MODEL_ID).await {
        Ok(outcome) => match outcome {
            AiConversationOutcome::TextMessage(message, response_id) => Ok((message, response_id)),
            AiConversationOutcome::ResetConversation(message, response_id) => {
                // For CLI, a reset doesn't mean much, so we'll just return the message and original ID.
                // The user can be informed that a reset was requested.
                info!(response_id = %response_id, "conversation reset was requested by ai/tool.");
                Ok((
                    format!("conversation reset requested by ai/tool, message: \"{message}\""),
                    response_id,
                ))
            }
            AiConversationOutcome::ChangeModel(message, response_id, new_model_id) => {
                // For CLI, inform about model change.
                info!(response_id = %response_id, %new_model_id, "openai model change requested by ai/tool.");
                Ok((
                    format!(
                        "openai model change to '{new_model_id}' requested by ai/tool. confirmation: \"{message}\""
                    ),
                    response_id, // return original response_id that triggered change
                ))
            }
        },
        Err(e) => Err(e),
    }
}

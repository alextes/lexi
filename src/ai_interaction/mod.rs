//! core ai conversation logic module.
//! it takes a user prompt and an optional previous openai response id, then drives the interaction
//! with the openai api, including any necessary tool calls, until a final text response is generated.
//! it is designed to be independent of the calling context (e.g., telegram, cli) and focuses solely
//! on the ai interaction flow, returning the final ai message and the last openai response id.

use crate::{
    ai_interaction::tools::{
        beacon_slot_check::BeaconNode, db_schema::LiveSchemaFetcher,
        relay_circuit_breaker::RelayCircuitBreaker,
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
use openai_chat::OPENAI_CALL_CONFIG;

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

// global verbosity and reasoning effort stores
pub static GLOBAL_VERBOSITY: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

pub static GLOBAL_REASONING_EFFORT: LazyLock<RwLock<Option<String>>> =
    LazyLock::new(|| RwLock::new(None));

pub async fn get_current_verbosity() -> Option<String> {
    GLOBAL_VERBOSITY.read().await.clone()
}

pub async fn set_current_verbosity(new_verbosity: Option<String>) {
    let mut guard = GLOBAL_VERBOSITY.write().await;
    *guard = new_verbosity.clone();
    if let Some(v) = new_verbosity {
        tracing::info!(new_global_verbosity = %v, "global openai verbosity updated by admin tool");
    } else {
        tracing::info!("global openai verbosity cleared by admin tool");
    }
}

pub async fn get_current_reasoning_effort() -> Option<String> {
    GLOBAL_REASONING_EFFORT.read().await.clone()
}

pub async fn set_current_reasoning_effort(new_effort: Option<String>) {
    let mut guard = GLOBAL_REASONING_EFFORT.write().await;
    *guard = new_effort.clone();
    if let Some(e) = new_effort {
        tracing::info!(new_global_reasoning_effort = %e, "global openai reasoning effort updated by admin tool");
    } else {
        tracing::info!("global openai reasoning effort cleared by admin tool");
    }
}
// end global verbosity and reasoning effort stores

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
    /// the ai (or a tool) requested the verbosity to be changed for future turns.
    /// contains (confirmation_message_for_user, response_id_triggering_change, new_verbosity).
    ChangeVerbosity(String, String, String),
    /// the ai (or a tool) requested the reasoning effort to be changed for future turns.
    /// contains (confirmation_message_for_user, response_id_triggering_change, new_reasoning_effort).
    ChangeReasoningEffort(String, String, String),
}

pub struct HandlerContext<D: Db, B: BeaconNode> {
    pub db: D,
    pub http_client: ReqwestClient,
    pub bot_db_id: i32, // kept as some tools/logging might still reference it via context
    pub openai_api_key: String,
    pub beacon_node: B,
    pub relay_circuit_breaker: RelayCircuitBreaker,
    pub schema_fetcher: LiveSchemaFetcher,
}

#[instrument(skip(ctx, prompt_text, previous_response_id))]
pub async fn drive_ai_conversation<D, B>(
    ctx: &HandlerContext<D, B>,
    prompt_text: &str,
    previous_response_id: Option<&str>,
    current_model_id: &str,
) -> Result<AiConversationOutcome>
where
    D: Db + Send + Sync + 'static,
    B: BeaconNode + Send + Sync + 'static,
{
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

    let current_verbosity_owned = get_current_verbosity().await;
    let current_reasoning_effort_owned = get_current_reasoning_effort().await;

    let initial_api_args = CallResponsesApiOptionalArgs {
        model_id: current_model_id,
        previous_response_id,
        tools: Some(turn_specific_tools.clone()), // use the determined tools
        tool_choice: None,
        instructions: Some(&OPENAI_CALL_CONFIG.instructions),
        temperature: None,
        store: Some(true),
        verbosity: current_verbosity_owned.as_deref(),
        reasoning_effort: current_reasoning_effort_owned.as_deref(),
    };

    match call_responses_api(
        ctx.http_client.clone(),
        &ctx.openai_api_key,
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
            // log both display and debug forms so the full error chain is visible
            // (including transport-level issues from reqwest) while preserving it
            // in the returned error.
            error!(
                error = %e,
                error_debug = ?e,
                "initial /v1/responses api call failed"
            );
            Err(anyhow::Error::from(e).context("initial /v1/responses api call failed"))
        }
    }
}

#[instrument(skip(ctx, prompt_text))]
pub async fn process_single_prompt_for_cli<D: Db + Send + Sync + 'static, B: BeaconNode + Send + Sync + 'static>(
    ctx: &HandlerContext<D, B>,
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
            AiConversationOutcome::ChangeVerbosity(message, response_id, new_verbosity) => {
                info!(response_id = %response_id, %new_verbosity, "openai verbosity change requested by ai/tool.");
                set_current_verbosity(Some(new_verbosity.clone())).await;
                Ok((
                    format!(
                        "openai verbosity change to '{new_verbosity}' requested by ai/tool. confirmation: \"{message}\""
                    ),
                    response_id,
                ))
            }
            AiConversationOutcome::ChangeReasoningEffort(message, response_id, new_effort) => {
                info!(response_id = %response_id, %new_effort, "openai reasoning effort change requested by ai/tool.");
                set_current_reasoning_effort(Some(new_effort.clone())).await;
                Ok((
                    format!(
                        "openai reasoning effort change to '{new_effort}' requested by ai/tool. confirmation: \"{message}\""
                    ),
                    response_id,
                ))
            }
        },
        Err(e) => Err(e),
    }
}

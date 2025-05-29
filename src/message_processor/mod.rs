//! this module is responsible for the core ai conversation logic.
//! it takes a user prompt and an optional previous openai response id, then drives the interaction
//! with the openai api, including any necessary tool calls, until a final text response is generated.
//! it is designed to be independent of the calling context (e.g., telegram, cli) and focuses solely
//! on the ai interaction flow, returning the final ai message and the last openai response id.

use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use sqlx::PgPool; // still needed for HandlerContext as tools might use its pool field
use tracing::{error, info};

use crate::openai_api::{
    call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
};
use openai_chat::{OPENAI_CALL_CONFIG, OPENAI_RESPONSES_MODEL_ID};

pub mod openai_chat;
pub mod tools;

#[derive(Clone)]
pub struct HandlerContext<'a> {
    pub pool: &'a PgPool,
    pub http_client: &'a ReqwestClient,
    pub bot_db_id: i32, // Kept as some tools/logging might still reference it via context
    pub openai_api_key: &'a str,
}

pub async fn drive_ai_conversation(
    ctx: &HandlerContext<'_>,
    prompt_text: &str,
    logging_chat_id: i64, // This is used for logging within openai_chat and its tools
    previous_response_id: Option<&str>,
) -> Result<(String, String)> {
    info!(
        logging_id = logging_chat_id, // Changed from chat_id to logging_id for clarity
        prompt = prompt_text,
        has_previous_id = previous_response_id.is_some(),
        "(core ai) driving conversation for prompt: '{}'",
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
            openai_chat::start_ai_processing_loop(ctx, logging_chat_id, api_response_1, input_items)
                .await
                .wrap_err("core ai conversation processing loop failed")
        }
        Err(e) => {
            error!(logging_id = logging_chat_id, error = %e, "(core_ai) initial /v1/responses api call failed"); // Changed from chat_id
            Err(e)
        }
    }
}

pub async fn process_single_prompt_for_cli(
    ctx: &HandlerContext<'_>,
    prompt_text: &str,
    logging_chat_id: i64, // Renamed from telegram_chat_id as it's used as logging_chat_id
) -> Result<(String, String)> {
    info!(
        logging_id = logging_chat_id, // Changed from chat_id
        prompt = prompt_text,
        "(cli_wrapper) processing single prompt via core ai driver"
    );
    drive_ai_conversation(ctx, prompt_text, logging_chat_id, None).await
}

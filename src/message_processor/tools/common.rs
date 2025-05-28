use crate::message_processor::HandlerContext;
use crate::openai_api::{
    call_responses_api, InputItem, InputMessageObject, OutputItem, ToolDefinition,
};
use eyre::{Context, Result};
use tracing::{error, info, warn};

// struct to group arguments for the step2 handler
pub struct ToolStep2Context<'a> {
    pub telegram_chat_id: i64,
    pub function_name: &'a str,
    pub function_id: &'a str,
    pub function_call_id: &'a str,
    pub function_arguments: &'a str,
    pub original_input_items: Vec<InputItem>,
    pub initial_api_response_id: &'a str,
    pub available_tools: Vec<ToolDefinition>,
    pub instructions: &'a str,
    pub tool_output_json_string: String,
}

// common helper to process the second step of a tool call (sending result to openai)
// Renamed from handle_tool_call_step2_openai_response_with_struct
// Removed #[allow(clippy::too_many_arguments)] as the struct resolves it
pub async fn handle_tool_call_step2_openai_response(
    ctx: &HandlerContext<'_>,
    step2_ctx: ToolStep2Context<'_>,
) -> Result<(String, String)> {
    let mut inputs_for_step2 = step2_ctx.original_input_items;
    inputs_for_step2.push(InputItem::Message(InputMessageObject {
        role: "assistant".to_string(),
        content: format!(
            "tool_call: name={}, id={}, call_id={}, args={}",
            step2_ctx.function_name,
            step2_ctx.function_id,
            step2_ctx.function_call_id,
            step2_ctx.function_arguments
        ),
    }));
    inputs_for_step2.push(InputItem::FunctionCallOutput(
        crate::openai_api::FunctionCallOutputItem {
            r#type: "function_call_output".to_string(),
            call_id: step2_ctx.function_call_id.to_string(),
            output: step2_ctx.tool_output_json_string,
        },
    ));

    info!(
        chat_id = step2_ctx.telegram_chat_id,
        tool_name = step2_ctx.function_name,
        "sending tool call result back to /v1/responses api"
    );
    let step2_api_args = crate::openai_api::CallResponsesApiOptionalArgs {
        model_id: crate::message_processor::OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: Some(step2_ctx.initial_api_response_id),
        tools: Some(step2_ctx.available_tools),
        tool_choice: None,
        instructions: Some(step2_ctx.instructions),
        temperature: None,
        store: None,
    };

    match call_responses_api(
        ctx.http_client,
        ctx.openai_api_key,
        inputs_for_step2,
        step2_api_args,
    )
    .await
    {
        Ok(api_response_2) => {
            let response_id_for_db = api_response_2.id.clone();
            if let Some(OutputItem::Message(final_msg)) = api_response_2.output.first() {
                if final_msg.role == "assistant" {
                    if let Some(content) = final_msg.content.first() {
                        if content.r#type == "output_text" {
                            return Ok((content.text.clone(), response_id_for_db));
                        }
                    }
                }
            }
            warn!(
                chat_id = step2_ctx.telegram_chat_id,
                tool_name = step2_ctx.function_name,
                "step 2 response did not contain expected assistant message text structure."
            );
            Ok((
                format!(
                    "i used the {} tool, but couldn't form a final summary in the expected format.",
                    step2_ctx.function_name
                ),
                response_id_for_db,
            ))
        }
        Err(e) => {
            error!(
                chat_id = step2_ctx.telegram_chat_id,
                tool_name = step2_ctx.function_name,
                error = %e,
                "step 2 api call failed for tool"
            );
            Err(e).context(format!(
                "step 2 api call failed for tool {}",
                step2_ctx.function_name
            ))
        }
    }
}

// Old function with 11 arguments is now removed.

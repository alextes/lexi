use crate::message_processor::HandlerContext;
use crate::openai_api::{
    call_responses_api, InputItem, OpenAiApiResponse, OutputFunctionCall, ToolDefinition,
};
use eyre::{Context, Result};
use tracing::{error, info};

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
) -> Result<(OpenAiApiResponse, Vec<InputItem>)> {
    let mut inputs_for_step2 = step2_ctx.original_input_items;

    // Re-add FunctionCallEcho
    inputs_for_step2.push(InputItem::FunctionCallEcho(
        crate::openai_api::FunctionCallEchoItem {
            r#type: "function_call".to_string(),
            call_id: step2_ctx.function_call_id.to_string(),
            name: step2_ctx.function_name.to_string(),
            arguments: step2_ctx.function_arguments.to_string(),
        },
    ));

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
        call_id_being_sent = step2_ctx.function_call_id,
        "sending tool call result back to /v1/responses api"
    );
    let step2_api_args = crate::openai_api::CallResponsesApiOptionalArgs {
        model_id: crate::message_processor::openai_chat::OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: Some(step2_ctx.initial_api_response_id),
        tools: Some(step2_ctx.available_tools),
        tool_choice: None,
        instructions: Some(step2_ctx.instructions),
        temperature: None,
        store: Some(true),
    };

    match call_responses_api(
        ctx.http_client,
        ctx.openai_api_key,
        inputs_for_step2.clone(),
        step2_api_args,
    )
    .await
    {
        Ok(api_response_2) => Ok((api_response_2, inputs_for_step2)),
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

// New struct to hold a processed tool call (original call + its output string)
pub struct ProcessedToolCall {
    pub original_fc: OutputFunctionCall, // Contains id, call_id, name, arguments
    pub output_json_string: String,      // The JSON string result from executing the tool
}

// New common function to handle step 2 API call with potentially multiple tool results
pub async fn execute_step2_api_call_with_multiple_tool_results(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64,                        // For logging
    initial_api_response_id: &str, // ID of the OpenAI response that requested these tools
    mut original_input_items: Vec<InputItem>, // The input items that led to the initial_api_response_id
    processed_tool_calls: Vec<ProcessedToolCall>, // All tool calls from that response, now with their outputs
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(OpenAiApiResponse, Vec<InputItem>)> {
    // Returns the new API response and the inputs used for it

    if processed_tool_calls.is_empty() {
        // This case should ideally not be reached if this function is called after tool execution.
        // However, as a safeguard, return an error or handle appropriately.
        error!(chat_id = telegram_chat_id, initial_api_response_id, "execute_step2_api_call_with_multiple_tool_results called with no processed tool calls.");
        return Err(eyre::eyre!(
            "no processed tool calls provided for step 2 api call"
        ));
    }

    info!(
        chat_id = telegram_chat_id,
        initial_response_id = initial_api_response_id,
        num_tool_results = processed_tool_calls.len(),
        "constructing step 2 API call with multiple tool results."
    );

    // Add all FunctionCallEcho and FunctionCallOutput items for the processed tools
    for tool_call_result in processed_tool_calls {
        info!(
            chat_id = telegram_chat_id,
            tool_name = %tool_call_result.original_fc.name,
            call_id = %tool_call_result.original_fc.call_id,
            "adding echo and output for tool call to step 2 inputs."
        );
        original_input_items.push(InputItem::FunctionCallEcho(
            crate::openai_api::FunctionCallEchoItem {
                r#type: "function_call".to_string(),
                call_id: tool_call_result.original_fc.call_id.clone(),
                name: tool_call_result.original_fc.name.clone(),
                arguments: tool_call_result.original_fc.arguments.clone(),
            },
        ));
        original_input_items.push(InputItem::FunctionCallOutput(
            crate::openai_api::FunctionCallOutputItem {
                r#type: "function_call_output".to_string(),
                call_id: tool_call_result.original_fc.call_id.clone(), // Use the same call_id
                output: tool_call_result.output_json_string,
            },
        ));
    }

    let step2_api_args = crate::openai_api::CallResponsesApiOptionalArgs {
        model_id: crate::message_processor::openai_chat::OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: Some(initial_api_response_id),
        tools: Some(available_tools),
        tool_choice: None, // Let the model decide next steps after tool execution
        instructions: Some(instructions),
        temperature: None,
        store: Some(true), // Ensure conversation history is stored
    };

    // Call the OpenAI /v1/responses API
    match call_responses_api(
        ctx.http_client,
        ctx.openai_api_key,
        original_input_items.clone(), // Clone because we return them
        step2_api_args,
    )
    .await
    {
        Ok(api_response_step2) => Ok((api_response_step2, original_input_items)),
        Err(e) => {
            error!(
                chat_id = telegram_chat_id,
                initial_response_id = initial_api_response_id,
                error = %e,
                "step 2 api call with multiple tool results failed."
            );
            Err(e).context("step 2 api call with multiple tool results failed")
        }
    }
}

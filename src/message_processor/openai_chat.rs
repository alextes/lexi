//! this module encapsulates all direct interactions with the openai api for chat completions.
//! it defines the tools available to the ai, prepares api requests, and processes responses,
//! including handling multi-step interactions involving tool calls and subsequent api calls.
//! it is designed to be state-agnostic regarding the broader conversation context (e.g., telegram chat history),
//! receiving necessary information like previous response ids or initial prompts as parameters.
//! this allows it to be used by different entry points, such as the main telegram bot loop
//! or a command-line interface for testing.

use crate::openai_api::{
    call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
    OpenAiApiResponse, OutputFunctionCall, OutputItem, ToolDefinition,
};
use eyre::Result;
use futures::future::join_all;
use tracing::{debug, error, info, warn};

pub const OPENAI_RESPONSES_MODEL_ID: &str = "gpt-4.1";

use std::sync::LazyLock;

// struct to hold prepared configuration for openai api calls
pub struct OpenAiCallConfig {
    pub available_tools: Vec<ToolDefinition>,
    pub instructions: String,
}

pub static OPENAI_CALL_CONFIG: LazyLock<OpenAiCallConfig> = LazyLock::new(|| {
    let available_tools = vec![
        super::tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL.clone(),
        super::tools::database_schema::DATABASE_SCHEMA_TOOL.clone(),
        super::tools::mevdb_query::MEVDB_QUERY_TOOL.clone(),
        super::tools::globaldb_query::GLOBALDB_QUERY_TOOL.clone(),
        super::tools::conversation_admin::CONVERSATION_ADMIN_TOOL.clone(),
    ];
    let instructions = "you are a helpful ai assistant named lexi.".to_string();
    OpenAiCallConfig {
        available_tools,
        instructions,
    }
});

fn summarize_conversation_history(history: &[InputItem]) -> Vec<String> {
    history
        .iter()
        .map(|item| match item {
            InputItem::Message(msg) => match msg.role.as_str() {
                "user" => "user_message".to_string(),
                "assistant" => "assistant_message".to_string(),
                "tool" => "tool_message_generic".to_string(), // Should ideally not happen with current logic
                _ => format!("message_role_{}", msg.role),
            },
            InputItem::FunctionCallOutput(_) => "tool_call_output".to_string(),
            InputItem::FunctionCallEcho(_) => "tool_call_echo".to_string(), // Should not happen with current logic
            InputItem::Text(_) => "text_input_item".to_string(), // Should not happen with current logic
        })
        .collect()
}

// processes the response from the openai api, handling direct messages or dispatching to tool handlers.
// this is the core loop that handles sequences of api calls if tools are involved.
pub(super) async fn process_openai_response_loop(
    ctx: &super::HandlerContext<'_>,
    logging_chat_id: i64, // keep this for context specific logging if needed, or rename/remove if truly generic
    mut api_response: OpenAiApiResponse,
    mut conversation_history: Vec<InputItem>,
    available_tools: Vec<ToolDefinition>, // pass directly, from OPENAI_CALL_CONFIG
    system_instructions: &str,            // pass directly, from OPENAI_CALL_CONFIG
) -> Result<(String, String)> {
    let mut first_iteration = true; // true for the first pass through this loop with the given api_response
    loop {
        let current_response_id = api_response.id.clone();
        let mut function_calls_to_execute: Vec<OutputFunctionCall> = Vec::new();
        let mut assistant_messages_in_current_turn: Vec<String> = Vec::new();
        let mut assistant_text_content_this_turn: Option<String> = None;

        let output_items = std::mem::take(&mut api_response.output);

        for output_item in output_items {
            match output_item {
                OutputItem::FunctionCall(fc) => {
                    function_calls_to_execute.push(fc);
                }
                OutputItem::Message(msg) => {
                    if msg.role == "assistant" {
                        if let Some(text_content) = msg.content.first() {
                            if text_content.r#type == "output_text" {
                                let text = text_content.text.clone();
                                assistant_messages_in_current_turn.push(text.clone());
                                assistant_text_content_this_turn = Some(text);
                            }
                        }
                    }
                }
            }
        }

        // add assistant's message from this turn to history.
        // if tool calls were also made, this message is the one that led to them.
        if let Some(text) = assistant_text_content_this_turn {
            conversation_history.push(InputItem::Message(InputMessageObject {
                role: "assistant".to_string(),
                content: text,
            }));
        } else if !function_calls_to_execute.is_empty() {
            // if there are tool calls but no assistant text, add an empty assistant message.
            // this marks the assistant's turn in the history.
            conversation_history.push(InputItem::Message(InputMessageObject {
                role: "assistant".to_string(),
                content: String::new(),
            }));
        }

        if !function_calls_to_execute.is_empty() {
            let num_fc_this_turn = function_calls_to_execute.len();
            info!(
                chat_id = logging_chat_id, // keep using logging_chat_id for this specific log
                response_id = %current_response_id,
                num_function_calls = num_fc_this_turn,
                "processing response with {} function call(s).", num_fc_this_turn
            );

            let tool_execution_futures: Vec<_> = function_calls_to_execute.iter().map(async |fc_request| {
                    let tool_output_result = if fc_request.name
                        == super::tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME
                    {
                        super::tools::beacon_slot_check::execute_beacon_slot_check(
                            ctx,
                            &fc_request.arguments,
                        )
                        .await
                    } else if fc_request.name == super::tools::mevdb_query::MEVDB_TOOL_NAME {
                        super::tools::mevdb_query::execute_mevdb_query_tool(
                            ctx,
                            &fc_request.arguments,
                        )
                        .await
                    } else if fc_request.name == super::tools::database_schema::DATABASE_SCHEMA_TOOL_NAME {
                        super::tools::database_schema::execute_get_database_schema(
                            ctx,
                            &fc_request.arguments,
                        )
                        .await
                    } else if fc_request.name == super::tools::conversation_admin::CONVERSATION_ADMIN_TOOL_NAME {
                        super::tools::conversation_admin::execute_conversation_admin_command(
                            ctx,
                            &fc_request.arguments,
                        )
                        .await
                    } else if fc_request.name == super::tools::globaldb_query::GLOBALDB_TOOL_NAME {
                        super::tools::globaldb_query::execute_globaldb_query_tool(
                            ctx,
                            &fc_request.arguments,
                        )
                        .await
                    } else {
                        warn!(
                            chat_id = logging_chat_id, // keep using logging_chat_id
                            function_call = ?fc_request,
                            "openai_chat module received unexpected function call name during parallel execution planning"
                        );
                        Ok(format!(
                            "{{\"error\": \"unexpected_tool_name\", \"tool_name\": \"{}\"}}",
                            fc_request.name
                        ))
                    };
                    (fc_request, tool_output_result)
            }).collect();
            let execution_results = join_all(tool_execution_futures).await;

            let mut any_tool_execution_failed = false;

            for (original_fc, result_from_tool_handler) in execution_results {
                let tool_output_json_string = match result_from_tool_handler {
                    Ok(output_str) => output_str,
                    Err(e) => {
                        any_tool_execution_failed = true;
                        error!(chat_id = logging_chat_id, tool_name = %original_fc.name, error = %e, "tool execution failed");
                        format!(
                            "{{\"error\": \"tool_execution_failed\", \"tool_name\": \"{}\", \"details\": \"{}\"}}",
                            original_fc.name,
                            e.to_string().replace('"',"\\\"") // ensure json string is valid
                        )
                    }
                };

                // add tool result to conversation history using FunctionCallOutput
                conversation_history.push(InputItem::FunctionCallOutput(
                    crate::openai_api::FunctionCallOutputItem {
                        r#type: "function_call_output".to_string(), // As defined in your types
                        call_id: original_fc.call_id.clone(),       // From the OutputFunctionCall
                        output: tool_output_json_string,
                    },
                ));
            }

            if any_tool_execution_failed {
                warn!(chat_id = logging_chat_id, "one or more tool executions failed. results (including errors) will be sent to openai.");
            }

            // now, call the api again with the accumulated history including tool results.
            debug!(
                chat_id = logging_chat_id, // keep logging_chat_id
                response_id = %current_response_id,
                history_summary = ?summarize_conversation_history(&conversation_history),
                "sending updated conversation history to api (after tool results)"
            );
            let next_api_args = CallResponsesApiOptionalArgs {
                model_id: OPENAI_RESPONSES_MODEL_ID,
                previous_response_id: Some(&current_response_id), // use id of response that requested tools
                tools: Some(available_tools.clone()),
                tool_choice: None, // let the model decide next action (respond or call more tools)
                instructions: if first_iteration {
                    Some(system_instructions)
                } else {
                    None
                },
                temperature: None,
                store: Some(true), // continue storing context on the backend if supported
            };
            first_iteration = false; // Subsequent iterations will not send instructions

            match call_responses_api(
                ctx.http_client,
                ctx.openai_api_key,
                conversation_history.clone(), // pass the full updated history
                next_api_args,
            )
            .await
            {
                Ok(next_api_response) => {
                    api_response = next_api_response; // continue loop with new response
                                                      // conversation_history is already updated for the next iteration
                }
                Err(e) => {
                    error!(chat_id = logging_chat_id, response_id = %current_response_id, error= %e, "api call after tool results failed. cannot continue processing this interaction.");
                    return Ok((
                        format!("a critical error occurred while trying to process tool results with the ai: {e}. please try again."),
                        current_response_id // return the id of the last successful response before this error
                    ));
                }
            }
        } else if !assistant_messages_in_current_turn.is_empty() {
            // no function calls, and we have assistant message(s) -> this is the final response for this turn.
            let final_text = assistant_messages_in_current_turn.join("\n"); // join if multiple message bubbles, though typically one.
            info!(
                chat_id = logging_chat_id, response_id = %current_response_id,
                "received final assistant message"
            );
            debug!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                final_history_summary = ?summarize_conversation_history(&conversation_history),
                "final conversation history state"
            );
            return Ok((final_text, current_response_id));
        } else {
            // no function calls and no assistant messages this turn.
            warn!(
                chat_id = logging_chat_id, response_id = %current_response_id,
                "openai api response output was empty or contained no actionable items (no text, no tools)."
            );
            debug!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                empty_turn_history_summary = ?summarize_conversation_history(&conversation_history),
                "conversation history state on empty/unhandled turn"
            );
            return Ok((
                "i received an empty or unhandled response from the ai.".to_string(),
                current_response_id,
            ));
        }
    }
}

// old generate_ai_reply_content, to be mostly inlined into message_processor::drive_ai_conversation
// its core responsibility was the first api call and then handing off to process_openai_response_loop.
// we will rename it for now, then its body will be moved.
pub async fn start_ai_processing_loop(
    ctx: &super::HandlerContext<'_>,
    logging_chat_id: i64, // keep this, as it's used by process_openai_response_loop
    initial_api_response: OpenAiApiResponse, // This will be the response from the first call made by drive_ai_conversation
    initial_input_items: Vec<InputItem>,     // The input items that led to initial_api_response
) -> Result<(String, String)> {
    // now pass to the loop for potential tool use and further calls
    process_openai_response_loop(
        ctx,
        logging_chat_id, // pass this through
        initial_api_response,
        initial_input_items,
        OPENAI_CALL_CONFIG.available_tools.clone(),
        &OPENAI_CALL_CONFIG.instructions,
    )
    .await
}

// process_single_prompt_for_cli is removed from this module.
// its logic is covered by message_processor::process_single_prompt_for_cli
// calling message_processor::drive_ai_conversation.

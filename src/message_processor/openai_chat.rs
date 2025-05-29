//! this module encapsulates all direct interactions with the openai api for chat completions.
//! it defines the tools available to the ai, prepares api requests, and processes responses,
//! including handling multi-step interactions involving tool calls and subsequent api calls.
//! it is designed to be state-agnostic regarding the broader conversation context (e.g., telegram chat history),
//! receiving necessary information like previous response ids or initial prompts as parameters.
//! this allows it to be used by different entry points, such as the main telegram bot loop
//! or a command-line interface for testing.

use super::AiConversationOutcome;
use crate::openai_api::{
    self, call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
    OpenAiApiResponse, OutputFunctionCall, OutputItem, ToolDefinition,
};
use eyre::Result;
use serde_json::Value as JsonValue;
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
                "tool" => "tool_message_generic".to_string(),
                _ => format!("message_role_{}", msg.role),
            },
            InputItem::FunctionCallOutput(_) => "tool_call_output".to_string(),
            InputItem::FunctionCallEcho(_) => "tool_call_echo".to_string(),
            InputItem::Text(_) => "text_input_item".to_string(),
        })
        .collect()
}

/// parses the output items from an openai api response.
fn parse_api_response_output(
    output_items: Vec<OutputItem>,
) -> (Vec<OutputFunctionCall>, Option<String>) {
    let mut function_calls_to_execute: Vec<OutputFunctionCall> = Vec::new();
    let mut assistant_text_content_this_turn: Option<String> = None;

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
                            // If there are multiple assistant messages, concatenate them.
                            // Typically, there's one, or text followed by tool calls.
                            assistant_text_content_this_turn = Some(
                                assistant_text_content_this_turn
                                    .map_or(text.clone(), |prev| prev + "\n" + &text),
                            );
                        }
                    }
                }
            }
        }
    }
    (function_calls_to_execute, assistant_text_content_this_turn)
}

/// executes a single tool function call based on its name.
async fn execute_tool_call(
    ctx: &super::HandlerContext<'_>,
    fc_request: &OutputFunctionCall,
    logging_chat_id: i64, // Kept for specific tool logging if any tool still uses it internally, though most were removed
) -> Result<String> {
    use super::tools::*;

    let tool_name = &fc_request.name;
    let arguments = &fc_request.arguments;

    if tool_name == beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME {
        beacon_slot_check::execute_beacon_slot_check(ctx, arguments).await
    } else if tool_name == database_schema::DATABASE_SCHEMA_TOOL_NAME {
        database_schema::execute_get_database_schema(ctx, arguments).await
    } else if tool_name == mevdb_query::MEVDB_TOOL_NAME {
        mevdb_query::execute_mevdb_query_tool(ctx, arguments).await
    } else if tool_name == globaldb_query::GLOBALDB_TOOL_NAME {
        globaldb_query::execute_globaldb_query_tool(ctx, arguments).await
    } else if tool_name == conversation_admin::CONVERSATION_ADMIN_TOOL_NAME {
        conversation_admin::execute_conversation_admin_command(ctx, arguments).await
    } else {
        warn!(
            chat_id = logging_chat_id,
            tool_name = tool_name,
            "received unexpected tool name for execution"
        );
        Ok(format!(
            "{{\"error\": \"unexpected_tool_name\", \"tool_name\": \"{}\"}}",
            tool_name
        ))
    }
}

// processes the response from the openai api, handling direct messages or dispatching to tool handlers.
// this is the core loop that handles sequences of api calls if tools are involved.
pub(super) async fn process_openai_response_loop(
    ctx: &super::HandlerContext<'_>,
    logging_chat_id: i64,
    mut api_response: OpenAiApiResponse,
    mut conversation_history: Vec<InputItem>,
    available_tools: Vec<ToolDefinition>,
    system_instructions: &str,
) -> Result<AiConversationOutcome> {
    let mut first_iteration = true;
    loop {
        let current_response_id = api_response.id.clone();
        let (function_calls_to_execute, assistant_text_content_this_turn) =
            parse_api_response_output(std::mem::take(&mut api_response.output));

        if let Some(text) = &assistant_text_content_this_turn {
            conversation_history.push(InputItem::Message(InputMessageObject {
                role: "assistant".to_string(),
                content: text.clone(), // Clone here as it's also used for final output
            }));
        } else if !function_calls_to_execute.is_empty() {
            // if there are tool calls but no assistant text, add an empty assistant message.
            conversation_history.push(InputItem::Message(InputMessageObject {
                role: "assistant".to_string(),
                content: String::new(),
            }));
        }

        if !function_calls_to_execute.is_empty() {
            info!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                num_function_calls = function_calls_to_execute.len(),
                "processing response with {} function call(s).", function_calls_to_execute.len()
            );

            for fc_request in function_calls_to_execute {
                match execute_tool_call(ctx, &fc_request, logging_chat_id).await {
                    Ok(tool_output_json_string) => {
                        if fc_request.name
                            == super::tools::conversation_admin::CONVERSATION_ADMIN_TOOL_NAME
                        {
                            if let Ok(json_val) =
                                serde_json::from_str::<JsonValue>(&tool_output_json_string)
                            {
                                if json_val.get("action").and_then(|v| v.as_str())
                                    == Some("reset_conversation")
                                {
                                    info!(chat_id = logging_chat_id, response_id = %current_response_id, "conversation reset triggered by admin tool.");
                                    return Ok(AiConversationOutcome::ResetConversation(
                                        tool_output_json_string,
                                        current_response_id,
                                    ));
                                }
                            }
                        }
                        conversation_history.push(InputItem::FunctionCallOutput(
                            openai_api::FunctionCallOutputItem {
                                r#type: "function_call_output".to_string(),
                                call_id: fc_request.call_id.clone(),
                                output: tool_output_json_string,
                            },
                        ));
                    }
                    Err(e) => {
                        error!(chat_id = logging_chat_id, tool_name = %fc_request.name, error = %e, "tool execution failed");
                        let error_output = format!(
                            "{{\"error\": \"tool_execution_failed\", \"tool_name\": \"{}\", \"details\": \"{}\"}}",
                            fc_request.name,
                            e.to_string().replace('"',"\\\"")
                        );
                        conversation_history.push(InputItem::FunctionCallOutput(
                            openai_api::FunctionCallOutputItem {
                                r#type: "function_call_output".to_string(),
                                call_id: fc_request.call_id.clone(),
                                output: error_output,
                            },
                        ));
                    }
                }
            }

            // now, call the api again with the accumulated history including tool results.
            debug!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                history_summary = ?summarize_conversation_history(&conversation_history),
                "sending updated conversation history to api (after tool results)"
            );
            let next_api_args = CallResponsesApiOptionalArgs {
                model_id: OPENAI_RESPONSES_MODEL_ID,
                previous_response_id: Some(&current_response_id),
                tools: Some(available_tools.clone()),
                tool_choice: None,
                instructions: if first_iteration {
                    Some(system_instructions)
                } else {
                    None
                },
                temperature: None,
                store: Some(true),
            };
            first_iteration = false;

            match openai_api::call_responses_api(
                ctx.http_client,
                ctx.openai_api_key,
                conversation_history.clone(),
                next_api_args,
            )
            .await
            {
                Ok(next_api_response) => {
                    api_response = next_api_response;
                }
                Err(e) => {
                    error!(chat_id = logging_chat_id, response_id = %current_response_id, error= %e, "api call after tool results failed.");
                    return Err(eyre::eyre!("api call after tool results failed: {}", e));
                }
            }
        } else if let Some(final_text) = assistant_text_content_this_turn {
            // Use the text we parsed earlier if it exists and no tool calls were made.
            info!(chat_id = logging_chat_id, response_id = %current_response_id, "received final assistant message");
            debug!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                final_history_summary = ?summarize_conversation_history(&conversation_history),
                "final conversation history state"
            );
            return Ok(AiConversationOutcome::TextMessage(
                final_text,
                current_response_id,
            ));
        } else {
            warn!(chat_id = logging_chat_id, response_id = %current_response_id, "openai api response output was empty or contained no actionable items.");
            debug!(
                chat_id = logging_chat_id,
                response_id = %current_response_id,
                empty_turn_history_summary = ?summarize_conversation_history(&conversation_history),
                "conversation history state on empty/unhandled turn"
            );
            return Ok(AiConversationOutcome::TextMessage(
                String::new(),
                current_response_id,
            ));
        }
    }
}

pub async fn start_ai_processing_loop(
    ctx: &super::HandlerContext<'_>,
    logging_chat_id: i64,
    initial_api_response: OpenAiApiResponse,
    initial_input_items: Vec<InputItem>,
) -> Result<AiConversationOutcome> {
    process_openai_response_loop(
        ctx,
        logging_chat_id,
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

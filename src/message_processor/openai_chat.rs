//! this module encapsulates all direct interactions with the openai api for chat completions.
//! it defines the tools available to the ai, prepares api requests, and processes responses,
//! including handling multi-step interactions involving tool calls and subsequent api calls.
//! it is designed to be state-agnostic regarding the broader conversation context (e.g., telegram chat history),
//! receiving necessary information like previous response ids or initial prompts as parameters.
//! this allows it to be used by different entry points, such as the main telegram bot loop
//! or a command-line interface for testing.

use super::AiConversationOutcome;
use crate::{
    db::Db,
    openai_api::{
        self, CallResponsesApiOptionalArgs, InputItem, InputMessageObject, OpenAiApiResponse,
        OutputFunctionCall, OutputItem, ToolDefinition,
    },
};
use eyre::Result;
use serde_json::Value as JsonValue;
use tracing::{debug, error, info, instrument, warn};

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
        super::tools::db_schema::DATABASE_SCHEMA_TOOL.clone(),
        super::tools::db_query::MEVDB_QUERY_TOOL.clone(),
        super::tools::db_query::GLOBALDB_QUERY_TOOL.clone(),
        super::tools::conversation_admin::CONVERSATION_ADMIN_TOOL.clone(),
        super::tools::retrieve_manual::RETRIEVE_MANUAL_TOOL.clone(),
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
#[instrument(skip(ctx, fc_request), fields(tool_name = %fc_request.name))]
async fn execute_tool_call<D: Db>(
    ctx: &super::HandlerContext<'_, D>,
    fc_request: &OutputFunctionCall,
) -> Result<String> {
    use super::tools::*;

    let tool_name = &fc_request.name;
    let arguments = &fc_request.arguments;

    if tool_name == beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME {
        beacon_slot_check::execute_beacon_slot_check(ctx, arguments).await
    } else if tool_name == db_schema::DATABASE_SCHEMA_TOOL_NAME {
        db_schema::execute_get_database_schema(arguments).await
    } else if tool_name == db_query::MEVDB_TOOL_NAME {
        db_query::execute_mevdb_query_tool(arguments).await
    } else if tool_name == db_query::GLOBALDB_TOOL_NAME {
        db_query::execute_globaldb_query_tool(arguments).await
    } else if tool_name == conversation_admin::CONVERSATION_ADMIN_TOOL_NAME {
        conversation_admin::execute_conversation_admin_command(arguments).await
    } else if tool_name == retrieve_manual::RETRIEVE_MANUAL_TOOL_NAME {
        retrieve_manual::execute_retrieve_manual(arguments).await
    } else {
        warn!("received unexpected tool name for execution");
        Ok(format!(
            "{{\"error\": \"unexpected_tool_name\", \"tool_name\": \"{}\"}}",
            tool_name
        ))
    }
}

// processes the response from the openai api, handling direct messages or dispatching to tool handlers.
// this is the core loop that handles sequences of api calls if tools are involved.
#[instrument(skip_all)]
pub(super) async fn process_openai_response_loop<D: Db>(
    ctx: &super::HandlerContext<'_, D>,
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
                response_id = %current_response_id,
                num_function_calls = function_calls_to_execute.len(),
                "processing response with {} function call(s).", function_calls_to_execute.len()
            );

            for fc_request in function_calls_to_execute {
                match execute_tool_call(ctx, &fc_request).await {
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
                                    info!(response_id = %current_response_id, "conversation reset triggered by admin tool.");
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
                        error!(tool_name = %fc_request.name, error = %e, "tool execution failed");
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
                    error!(response_id = %current_response_id, error= %e, "api call after tool results failed.");
                    return Err(eyre::eyre!("api call after tool results failed: {}", e));
                }
            }
        } else if let Some(final_text) = assistant_text_content_this_turn {
            // Use the text we parsed earlier if it exists and no tool calls were made.
            info!(response_id = %current_response_id, "received final assistant message");
            debug!(
                response_id = %current_response_id,
                final_history_summary = ?summarize_conversation_history(&conversation_history),
                "final conversation history state"
            );
            return Ok(AiConversationOutcome::TextMessage(
                final_text,
                current_response_id,
            ));
        } else {
            warn!(response_id = %current_response_id, "openai api response output was empty or contained no actionable items.");
            debug!(
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

pub async fn start_ai_processing_loop<D: Db>(
    ctx: &super::HandlerContext<'_, D>,
    initial_api_response: OpenAiApiResponse,
    initial_input_items: Vec<InputItem>,
) -> Result<AiConversationOutcome> {
    process_openai_response_loop(
        ctx,
        initial_api_response,
        initial_input_items,
        OPENAI_CALL_CONFIG.available_tools.clone(),
        &OPENAI_CALL_CONFIG.instructions,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*; // imports functions from openai_chat.rs
    use crate::openai_api::{
        // imports types for constructing test data
        FunctionCallOutputItem,
        InputMessageObject,
        OutputFunctionCall,
        OutputItem,
        OutputMessage,
        OutputTextContent,
    };

    // --- tests for summarize_conversation_history ---
    #[test]
    fn test_summarize_empty_history() {
        let history: Vec<InputItem> = Vec::new();
        assert_eq!(
            summarize_conversation_history(&history),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_summarize_user_message() {
        let history = vec![InputItem::Message(InputMessageObject {
            role: "user".to_string(),
            content: "hello".to_string(),
        })];
        assert_eq!(
            summarize_conversation_history(&history),
            vec!["user_message"]
        );
    }

    #[test]
    fn test_summarize_assistant_message() {
        let history = vec![InputItem::Message(InputMessageObject {
            role: "assistant".to_string(),
            content: "world".to_string(),
        })];
        assert_eq!(
            summarize_conversation_history(&history),
            vec!["assistant_message"]
        );
    }

    #[test]
    fn test_summarize_tool_message() {
        // Role "tool" for InputMessageObject is not typical for input, but testing completeness
        let history = vec![InputItem::Message(InputMessageObject {
            role: "tool".to_string(),
            content: "tool stuff".to_string(),
        })];
        assert_eq!(
            summarize_conversation_history(&history),
            vec!["tool_message_generic"]
        );
    }

    #[test]
    fn test_summarize_function_call_output() {
        let history = vec![InputItem::FunctionCallOutput(FunctionCallOutputItem {
            r#type: "function_call_output".to_string(),
            call_id: "call_123".to_string(),
            output: "{{\"result\": \"ok\"}}".to_string(),
        })];
        assert_eq!(
            summarize_conversation_history(&history),
            vec!["tool_call_output"]
        );
    }

    #[test]
    fn test_summarize_mixed_history() {
        let history = vec![
            InputItem::Message(InputMessageObject {
                role: "user".to_string(),
                content: "first user message".to_string(),
            }),
            InputItem::Message(InputMessageObject {
                role: "assistant".to_string(),
                content: "first assistant reply".to_string(),
            }),
            InputItem::FunctionCallOutput(FunctionCallOutputItem {
                r#type: "function_call_output".to_string(),
                call_id: "call_abc".to_string(),
                output: "tool output data".to_string(),
            }),
            InputItem::Message(InputMessageObject {
                role: "user".to_string(),
                content: "second user message".to_string(),
            }),
        ];
        assert_eq!(
            summarize_conversation_history(&history),
            vec![
                "user_message",
                "assistant_message",
                "tool_call_output",
                "user_message"
            ]
        );
    }

    // --- tests for parse_api_response_output ---
    #[test]
    fn test_parse_empty_output_items() {
        let items: Vec<OutputItem> = Vec::new();
        let (fcs, text) = parse_api_response_output(items);
        assert!(fcs.is_empty());
        assert!(text.is_none());
    }

    #[test]
    fn test_parse_assistant_text_message() {
        let items = vec![OutputItem::Message(OutputMessage {
            id: "msg_1".to_string(),
            r#type: "message".to_string(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputTextContent {
                r#type: "output_text".to_string(),
                text: "hello from assistant".to_string(),
            }],
        })];
        let (fcs, text) = parse_api_response_output(items);
        assert!(fcs.is_empty());
        assert_eq!(text, Some("hello from assistant".to_string()));
    }

    #[test]
    fn test_parse_single_function_call() {
        let items = vec![OutputItem::FunctionCall(OutputFunctionCall {
            r#type: "function_call".to_string(),
            id: "fc_1".to_string(),
            call_id: "call_id_1".to_string(),
            name: "test_tool".to_string(),
            arguments: "{{\"arg1\": \"val1\"}}".to_string(),
        })];
        let (fcs, text) = parse_api_response_output(items);
        assert_eq!(fcs.len(), 1);
        assert_eq!(fcs[0].name, "test_tool");
        assert_eq!(fcs[0].arguments, "{{\"arg1\": \"val1\"}}");
        assert!(text.is_none());
    }

    #[test]
    fn test_parse_assistant_message_then_function_call() {
        let items = vec![
            OutputItem::Message(OutputMessage {
                id: "msg_leading".to_string(),
                r#type: "message".to_string(),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputTextContent {
                    r#type: "output_text".to_string(),
                    text: "okay, i will call a tool.".to_string(),
                }],
            }),
            OutputItem::FunctionCall(OutputFunctionCall {
                r#type: "function_call".to_string(),
                id: "fc_2".to_string(),
                call_id: "call_id_2".to_string(),
                name: "another_tool".to_string(),
                arguments: "{}".to_string(),
            }),
        ];
        let (fcs, text) = parse_api_response_output(items);
        assert_eq!(fcs.len(), 1);
        assert_eq!(fcs[0].name, "another_tool");
        assert_eq!(text, Some("okay, i will call a tool.".to_string()));
    }

    #[test]
    fn test_parse_multiple_function_calls() {
        let items = vec![
            OutputItem::FunctionCall(OutputFunctionCall {
                r#type: "function_call".to_string(),
                id: "fc_a".to_string(),
                call_id: "call_id_a".to_string(),
                name: "tool_one".to_string(),
                arguments: "{\"a\":1}".to_string(),
            }),
            OutputItem::FunctionCall(OutputFunctionCall {
                r#type: "function_call".to_string(),
                id: "fc_b".to_string(),
                call_id: "call_id_b".to_string(),
                name: "tool_two".to_string(),
                arguments: "{\"b\":2}".to_string(),
            }),
        ];
        let (fcs, text) = parse_api_response_output(items);
        assert_eq!(fcs.len(), 2);
        assert_eq!(fcs[0].name, "tool_one");
        assert_eq!(fcs[1].name, "tool_two");
        assert!(text.is_none());
    }

    #[test]
    fn test_parse_assistant_message_non_output_text() {
        let items = vec![OutputItem::Message(OutputMessage {
            id: "msg_other".to_string(),
            r#type: "message".to_string(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputTextContent {
                r#type: "other_type".to_string(), // Not output_text
                text: "this should be ignored".to_string(),
            }],
        })];
        let (fcs, text) = parse_api_response_output(items);
        assert!(fcs.is_empty());
        assert!(text.is_none()); // No output_text content should result in None
    }

    #[test]
    fn test_parse_multiple_assistant_messages_concatenated() {
        let items = vec![
            OutputItem::Message(OutputMessage {
                id: "msg_part1".to_string(),
                r#type: "message".to_string(),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputTextContent {
                    r#type: "output_text".to_string(),
                    text: "part one.".to_string(),
                }],
            }),
            OutputItem::Message(OutputMessage {
                id: "msg_part2".to_string(),
                r#type: "message".to_string(),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputTextContent {
                    r#type: "output_text".to_string(),
                    text: "part two.".to_string(),
                }],
            }),
        ];
        let (fcs, text) = parse_api_response_output(items);
        assert!(fcs.is_empty());
        assert_eq!(text, Some("part one.\npart two.".to_string()));
    }
}

//! this module encapsulates all direct interactions with the openai api for chat completions.
//! it defines the tools available to the ai, prepares api requests, and processes responses,
//! including handling multi-step interactions involving tool calls and subsequent api calls.
//! it is designed to be state-agnostic regarding the broader conversation context (e.g., telegram chat history),
//! receiving necessary information like previous response ids or initial prompts as parameters.
//! this allows it to be used by different entry points, such as the main telegram bot loop
//! or a command-line interface for testing.

use super::{AiConversationOutcome, BeaconNode};
use crate::{
    db::Db,
    openai_api::{
        self, ApiToolType, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
        OpenAiApiResponse, OutputFunctionCall, OutputItem, WebSearchToolConfig,
    },
};
use anyhow::{Context, Result};
use indoc::indoc;
use serde_json::Value as JsonValue;
use tracing::{debug, error, info, instrument, warn};

// add these imports
use crate::ai_interaction::tools;

pub const DEFAULT_OPENAI_MODEL_ID: &str = "gpt-5.2";

use std::sync::LazyLock;

// struct to hold prepared configuration for openai api calls
pub struct OpenAiCallConfig {
    pub available_tools: Vec<ApiToolType>,
    pub instructions: String,
}

pub static OPENAI_CALL_CONFIG: LazyLock<OpenAiCallConfig> = LazyLock::new(|| {
    let available_tools = vec![
        ApiToolType::Function(tools::admin_session_end::ADMIN_SESSION_END_TOOL.clone()),
        ApiToolType::Function(tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL.clone()),
        ApiToolType::Function(tools::db_schema::DATABASE_SCHEMA_TOOL.clone()),
        ApiToolType::Function(tools::db_query::MEVDB_QUERY_TOOL.clone()),
        ApiToolType::Function(tools::db_query::GLOBALDB_QUERY_TOOL.clone()),
        ApiToolType::Function(tools::retrieve_manual::RETRIEVE_MANUAL_TOOL.clone()),
        ApiToolType::Function(tools::relay_circuit_breaker::RELAY_CIRCUIT_BREAKER_TOOL.clone()),
        ApiToolType::Function(tools::conversation_admin::CONVERSATION_ADMIN_TOOL.clone()),
        ApiToolType::Function(tools::scheduled_jobs::SCHEDULED_JOBS_TOOL.clone()),
        ApiToolType::WebSearch(WebSearchToolConfig::new()),
    ];
    let instructions = indoc! {"
        you are a helpful ai assistant named lexi.
        your main mode of communication is through telegram, responses will automatically be sent to the telegram chat.
        format your responses using markdown (bold, italic, code blocks, lists) for better readability.
        you are part of the team running the ultra sound relay, and in this context may be asked to lend a hand.
        whenever functioning as the ultra sound relay assistant, the manual `relay_context` is available to provide more context.
    "}
    .to_string();
    OpenAiCallConfig {
        available_tools,
        instructions,
    }
});

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
            OutputItem::Reasoning(reasoning_item) => {
                debug!(reasoning_item_id = %reasoning_item.id, reasoning_summary = ?reasoning_item.summary, "received reasoning item, ignoring for now.");
                // currently, we just log and ignore reasoning items.
            }
            OutputItem::WebSearchCall(web_search_call_item) => {
                debug!(web_search_call_id = %web_search_call_item.id, status = %web_search_call_item.status, "received web_search_call item, ignoring for now.");
                // The actual content from web search comes in a subsequent Message item with annotations.
                // We log and ignore this specific item type for now in this parsing function.
            }
        }
    }
    (function_calls_to_execute, assistant_text_content_this_turn)
}

/// executes a single tool function call based on its name.
#[instrument(skip(ctx, fc_request), fields(tool_name = %fc_request.name))]
async fn execute_tool_call<D: Db, B: BeaconNode>(
    ctx: &super::HandlerContext<D, B>,
    fc_request: &OutputFunctionCall,
) -> Result<String> {
    use super::tools::*;

    let tool_name = &fc_request.name;
    let arguments = &fc_request.arguments;

    if tool_name == beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME {
        beacon_slot_check::execute_beacon_slot_check(&ctx.beacon_node, arguments).await
    } else if tool_name == db_schema::DATABASE_SCHEMA_TOOL_NAME {
        let args: db_schema::GetDatabaseSchemaArgs = serde_json::from_str(arguments)
            .with_context(|| format!("failed to parse args for {tool_name}"))?;
        let db_name = args.database_name;

        db_schema::execute_get_database_schema(&ctx.db, &db_name, &ctx.schema_fetcher).await
    } else if tool_name == db_query::MEVDB_TOOL_NAME {
        db_query::execute_mevdb_query_tool(arguments).await
    } else if tool_name == db_query::GLOBALDB_TOOL_NAME {
        db_query::execute_globaldb_query_tool(arguments).await
    } else if tool_name == admin_session_end::ADMIN_SESSION_END_TOOL_NAME {
        admin_session_end::execute_admin_session_end_tool(arguments).await
    } else if tool_name == conversation_admin::CONVERSATION_ADMIN_TOOL_NAME {
        conversation_admin::execute_conversation_admin_command(arguments).await
    } else if tool_name == retrieve_manual::RETRIEVE_MANUAL_TOOL_NAME {
        retrieve_manual::execute_retrieve_manual(arguments).await
    } else if tool_name == relay_circuit_breaker::RELAY_CIRCUIT_BREAKER_TOOL_NAME {
        relay_circuit_breaker::execute_relay_circuit_breaker_tool(
            &ctx.relay_circuit_breaker,
            arguments,
        )
        .await
    } else if tool_name == scheduled_jobs::SCHEDULED_JOBS_TOOL_NAME {
        scheduled_jobs::execute_scheduled_jobs(
            &ctx.db,
            ctx.current_telegram_chat_id,
            ctx.current_message_thread_id,
            arguments,
        )
        .await
    } else {
        warn!("received unexpected tool name for execution");
        Ok(format!(
            "{{\"error\": \"unexpected_tool_name\", \"tool_name\": \"{tool_name}\"}}"
        ))
    }
}

// processes the response from the openai api, handling direct messages or dispatching to tool handlers.
// this is the core loop that handles sequences of api calls if tools are involved.
#[instrument(skip_all)]
pub(super) async fn process_openai_response_loop<D: Db, B: BeaconNode>(
    ctx: &super::HandlerContext<D, B>,
    mut api_response: OpenAiApiResponse,
    mut conversation_history: Vec<InputItem>,
    available_tools: Vec<ApiToolType>,
    system_instructions: &str,
    current_model_id: String,
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
                            || fc_request.name
                                == super::tools::admin_session_end::ADMIN_SESSION_END_TOOL_NAME
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
                                if json_val.get("action").and_then(|v| v.as_str())
                                    == Some("model_updated")
                                {
                                    if let Some(new_model) =
                                        json_val.get("new_model_id").and_then(|v| v.as_str())
                                    {
                                        info!(response_id = %current_response_id, new_model_id = %new_model, "openai model preference update triggered by admin tool.");
                                        return Ok(AiConversationOutcome::ChangeModel(
                                            tool_output_json_string,
                                            current_response_id,
                                            new_model.to_string(),
                                        ));
                                    }
                                }
                                if json_val.get("action").and_then(|v| v.as_str())
                                    == Some("verbosity_updated")
                                {
                                    if let Some(new_verbosity) =
                                        json_val.get("new_verbosity").and_then(|v| v.as_str())
                                    {
                                        info!(response_id = %current_response_id, new_verbosity = %new_verbosity, "openai verbosity update triggered by admin tool.");
                                        return Ok(AiConversationOutcome::ChangeVerbosity(
                                            tool_output_json_string,
                                            current_response_id,
                                            new_verbosity.to_string(),
                                        ));
                                    }
                                }
                                if json_val.get("action").and_then(|v| v.as_str())
                                    == Some("reasoning_effort_updated")
                                {
                                    if let Some(new_effort) = json_val
                                        .get("new_reasoning_effort")
                                        .and_then(|v| v.as_str())
                                    {
                                        info!(response_id = %current_response_id, new_reasoning_effort = %new_effort, "openai reasoning effort update triggered by admin tool.");
                                        return Ok(AiConversationOutcome::ChangeReasoningEffort(
                                            tool_output_json_string,
                                            current_response_id,
                                            new_effort.to_string(),
                                        ));
                                    }
                                }
                                if json_val.get("action").and_then(|v| v.as_str())
                                    == Some("end_admin_session")
                                {
                                    info!(response_id = %current_response_id, "admin session end triggered by tool.");
                                    return Ok(AiConversationOutcome::EndAdminSession(
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
                convo_items = conversation_history.len(),
                tools_available = available_tools.len(),
                "sending updated conversation history to api (after tool results)"
            );
            // fetch current knobs for follow-up calls
            let current_verbosity_owned = super::get_current_verbosity().await;
            let current_reasoning_effort_owned = super::get_current_reasoning_effort().await;

            let next_api_args = CallResponsesApiOptionalArgs {
                model_id: &current_model_id,
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
                verbosity: current_verbosity_owned.as_deref(),
                reasoning_effort: current_reasoning_effort_owned.as_deref(),
            };
            first_iteration = false;

            match openai_api::call_responses_api(
                ctx.http_client.clone(),
                &ctx.openai_api_key,
                conversation_history.clone(),
                next_api_args,
            )
            .await
            {
                Ok(next_api_response) => {
                    api_response = next_api_response;
                }
                Err(e) => {
                    // include debug formatting so the full anyhow error chain (including
                    // underlying reqwest details) is visible in logs.
                    error!(
                        response_id = %current_response_id,
                        error = %e,
                        error_debug = ?e,
                        convo_items = conversation_history.len(),
                        "api call after tool results failed."
                    );
                    return Err(e.context("api call after tool results failed"));
                }
            }
        } else if let Some(final_text) = assistant_text_content_this_turn {
            // Use the text we parsed earlier if it exists and no tool calls were made.
            info!(
                response_id = %current_response_id,
                convo_items = conversation_history.len(),
                "received final assistant message"
            );
            debug!(
                response_id = %current_response_id,
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
                "conversation history state on empty/unhandled turn"
            );
            return Ok(AiConversationOutcome::TextMessage(
                String::new(),
                current_response_id,
            ));
        }
    }
}

/// determines the specific set of tools available for the current turn.
/// it starts with a base set of tools and filters admin-only tools unless an admin session is active.
pub(crate) fn determine_turn_tools(
    base_available_tools: &[ApiToolType],
    admin_session_active: bool,
    current_model_id: &str,
) -> Vec<ApiToolType> {
    let mut turn_specific_available_tools = base_available_tools.to_vec();

    if !admin_session_active {
        turn_specific_available_tools.retain(|tool| !tools::is_admin_only_tool(tool));
    }

    // support websearch for gpt-5.2 and gpt-5-mini; remove for others
    let websearch_supported =
        current_model_id == DEFAULT_OPENAI_MODEL_ID || current_model_id == "gpt-5-mini";
    if websearch_supported {
        info!(
            model_id = %current_model_id,
            "websearch tool is supported for this model and will be included if present in base tools."
        );
    } else {
        info!(
            model_id = %current_model_id,
            "websearch tool is not supported for this model and will be removed."
        );
        turn_specific_available_tools.retain(|tool| !matches!(tool, ApiToolType::WebSearch(_)));
    }

    turn_specific_available_tools
}

pub async fn start_ai_processing_loop<D: Db, B: BeaconNode>(
    ctx: &super::HandlerContext<D, B>,
    initial_api_response: OpenAiApiResponse,
    initial_input_items: Vec<InputItem>,
    current_model_id: String,
    turn_specific_available_tools: Vec<ApiToolType>,
) -> Result<AiConversationOutcome> {
    process_openai_response_loop(
        ctx,
        initial_api_response,
        initial_input_items,
        turn_specific_available_tools,
        &OPENAI_CALL_CONFIG.instructions,
        current_model_id,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_interaction::tools::conversation_admin;
    use crate::openai_api::{
        ApiToolType, OutputFunctionCall, OutputItem, OutputMessage, OutputTextContent,
        ToolDefinition, WebSearchToolConfig,
    };

    const TEST_GPT_5_MODEL_ID: &str = "gpt-5.2";
    const TEST_GPT_5_MINI_MODEL_ID: &str = "gpt-5-mini";

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

    // --- tests for determine_turn_tools ---
    // helper to create a dummy tool definition
    fn dummy_tool(name: &str) -> ApiToolType {
        ApiToolType::Function(ToolDefinition::new(
            name.to_string(),
            Some("description".to_string()),
            None,
        ))
    }

    #[test]
    fn dtt_admin_tools_excluded_when_session_inactive() {
        let base_tools = vec![
            dummy_tool("base_tool_1"),
            ApiToolType::Function(conversation_admin::CONVERSATION_ADMIN_TOOL.clone()),
        ];

        let determined_tools = determine_turn_tools(&base_tools, false, TEST_GPT_5_MODEL_ID);
        assert_eq!(determined_tools.len(), 1);
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == "base_tool_1",
            _ => false,
        }));
        assert!(!determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == conversation_admin::CONVERSATION_ADMIN_TOOL_NAME,
            _ => false,
        }));
    }

    #[test]
    fn dtt_admin_tools_included_when_session_active() {
        let base_tools = vec![
            dummy_tool("base_tool_2"),
            ApiToolType::Function(conversation_admin::CONVERSATION_ADMIN_TOOL.clone()),
        ];

        let determined_tools = determine_turn_tools(&base_tools, true, TEST_GPT_5_MODEL_ID);
        assert_eq!(
            determined_tools.len(),
            2,
            "expected base_tool_2 and admin_tool"
        );
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == "base_tool_2",
            _ => false,
        }));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == conversation_admin::CONVERSATION_ADMIN_TOOL_NAME,
            _ => false,
        }));
    }

    // --- new tests for websearch tool based on model_id ---
    fn web_search_tool() -> ApiToolType {
        ApiToolType::WebSearch(WebSearchToolConfig::new())
    }

    #[test]
    fn dtt_web_search_included_for_gpt_5_if_in_base() {
        let base_tools = vec![dummy_tool("base_tool"), web_search_tool()];
        let determined_tools = determine_turn_tools(&base_tools, false, DEFAULT_OPENAI_MODEL_ID);
        assert_eq!(determined_tools.len(), 2);
        assert!(determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == "base_tool",
            _ => false,
        }));
    }

    #[test]
    fn dtt_web_search_included_for_gpt_5_mini_if_in_base() {
        let base_tools = vec![dummy_tool("base_tool"), web_search_tool()];
        let determined_tools = determine_turn_tools(&base_tools, false, TEST_GPT_5_MINI_MODEL_ID);
        assert_eq!(determined_tools.len(), 2);
        assert!(determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == "base_tool",
            _ => false,
        }));
    }

    #[test]
    fn dtt_web_search_excluded_for_other_model_even_if_in_base() {
        let base_tools = vec![dummy_tool("base_tool"), web_search_tool()];
        let determined_tools = determine_turn_tools(&base_tools, false, "some-other-model-id");
        assert_eq!(
            determined_tools.len(),
            1,
            "websearch should be removed for other models"
        );
        assert!(!determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) => f.name == "base_tool",
            _ => false,
        }));
    }

    #[test]
    fn dtt_web_search_not_present_in_base_tools_gpt_5() {
        let base_tools = vec![dummy_tool("base_tool")]; // websearch not in base
        let determined_tools = determine_turn_tools(&base_tools, false, DEFAULT_OPENAI_MODEL_ID);
        assert_eq!(determined_tools.len(), 1);
        assert!(!determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
    }

    #[test]
    fn dtt_web_search_with_admin_tool_gpt_5() {
        let base_tools = vec![
            dummy_tool("base_tool"),
            web_search_tool(),
            ApiToolType::Function(conversation_admin::CONVERSATION_ADMIN_TOOL.clone()),
        ];
        let determined_tools = determine_turn_tools(&base_tools, true, DEFAULT_OPENAI_MODEL_ID);
        assert_eq!(
            determined_tools.len(),
            3,
            "should have base_tool, websearch, and admin_tool"
        );
        assert!(determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) =>
                f.name == tools::conversation_admin::CONVERSATION_ADMIN_TOOL_NAME,
            _ => false,
        }));
    }

    #[test]
    fn dtt_web_search_with_admin_tool_gpt_5_mini() {
        let base_tools = vec![
            dummy_tool("base_tool"),
            web_search_tool(),
            ApiToolType::Function(conversation_admin::CONVERSATION_ADMIN_TOOL.clone()),
        ];
        let determined_tools = determine_turn_tools(&base_tools, true, TEST_GPT_5_MINI_MODEL_ID);
        assert_eq!(
            determined_tools.len(),
            3,
            "should have base_tool, websearch, and admin_tool"
        );
        assert!(determined_tools
            .iter()
            .any(|t| matches!(t, ApiToolType::WebSearch(_))));
        assert!(determined_tools.iter().any(|t| match t {
            ApiToolType::Function(f) =>
                f.name == tools::conversation_admin::CONVERSATION_ADMIN_TOOL_NAME,
            _ => false,
        }));
    }
}

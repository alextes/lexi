use crate::db;
use crate::message_processor::tools::common::ProcessedToolCall;
use crate::openai_api::{
    call_responses_api, CallResponsesApiOptionalArgs, InputItem, InputMessageObject,
    OpenAiApiResponse, OutputFunctionCall, OutputItem, ToolDefinition,
};
use crate::telegram::types::Message as TelegramMessage;
use eyre::Result;
use futures::future::join_all;
use tracing::{error, info, warn};

// module-private constant
pub const OPENAI_RESPONSES_MODEL_ID: &str = "gpt-4.1";

// struct to hold prepared configuration for openai api calls
struct OpenAiCallConfig {
    available_tools: Vec<ToolDefinition>,
    instructions: String,
}

// prepares the tool definitions and system instructions for the openai api.
fn prepare_openai_call_config() -> OpenAiCallConfig {
    let available_tools = vec![
        super::tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL.clone(),
        super::tools::mevdb_query::MEVDB_QUERY_TOOL.clone(),
        super::tools::mevdb_schema::MEVDB_SCHEMA_TOOL.clone(),
    ];
    let instructions = format!(
        "you are a helpful ai assistant named lexi. use tools if appropriate. \
        you have the following tools available: \
        1. '{}': checks if an ethereum beacon chain slot was missed. params: 'slot_number' (integer). \
        2. '{}': executes a sql select query against a read-only mev database. params: 'sql_query' (string). \
        3. '{}': retrieves the schema for the mev database. use this if you need to understand table structures before using the '{}' tool. this tool takes no parameters. \
        for example, to query the mev database, you might first call '{}' to get the schema, and then use that information to formulate a query for '{}'. \
        ensure your queries/parameters target these tools and their specified inputs correctly.",
        super::tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME,
        super::tools::mevdb_query::MEVDB_TOOL_NAME,
        super::tools::mevdb_schema::MEVDB_SCHEMA_TOOL_NAME,
        super::tools::mevdb_query::MEVDB_TOOL_NAME,
        super::tools::mevdb_schema::MEVDB_SCHEMA_TOOL_NAME,
        super::tools::mevdb_query::MEVDB_TOOL_NAME
    );
    OpenAiCallConfig {
        available_tools,
        instructions,
    }
}

// processes the response from the openai api, handling direct messages or dispatching to tool handlers.
pub async fn process_openai_response(
    ctx: &super::HandlerContext<'_>,
    telegram_chat_id: i64,
    mut api_response: OpenAiApiResponse,
    mut original_input_items: Vec<InputItem>,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    loop {
        let current_response_id = api_response.id.clone();
        let mut function_calls_to_execute: Vec<OutputFunctionCall> = Vec::new();
        let mut assistant_messages_in_current_turn: Vec<String> = Vec::new();

        let current_output_items = std::mem::take(&mut api_response.output);

        for output_item in current_output_items {
            match output_item {
                OutputItem::FunctionCall(fc) => {
                    function_calls_to_execute.push(fc);
                }
                OutputItem::Message(msg) => {
                    if msg.role == "assistant" {
                        if let Some(text_content) = msg.content.first() {
                            if text_content.r#type == "output_text" {
                                assistant_messages_in_current_turn.push(text_content.text.clone());
                            }
                        }
                    }
                }
            }
        }

        if !function_calls_to_execute.is_empty() {
            let num_fc_this_turn = function_calls_to_execute.len();
            info!(
                chat_id = telegram_chat_id,
                response_id = %current_response_id,
                num_function_calls = num_fc_this_turn,
                "processing response with {} function call(s).", num_fc_this_turn
            );

            let mut tool_execution_futures = Vec::new();

            for fc in function_calls_to_execute {
                let fc_clone = fc.clone();
                let ctx_clone = ctx.clone();

                let future = async move {
                    let tool_output_result = if fc_clone.name
                        == super::tools::beacon_slot_check::BEACON_SLOT_CHECK_TOOL_NAME
                    {
                        super::tools::beacon_slot_check::execute_beacon_slot_check(
                            &ctx_clone,
                            telegram_chat_id,
                            &fc_clone.arguments,
                        )
                        .await
                    } else if fc_clone.name == super::tools::mevdb_query::MEVDB_TOOL_NAME {
                        super::tools::mevdb_query::execute_mevdb_query_tool(
                            &ctx_clone,
                            telegram_chat_id,
                            &fc_clone.arguments,
                        )
                        .await
                    } else if fc_clone.name == super::tools::mevdb_schema::MEVDB_SCHEMA_TOOL_NAME {
                        super::tools::mevdb_schema::execute_get_mevdb_schema(
                            &ctx_clone,
                            telegram_chat_id,
                        )
                        .await
                    } else {
                        warn!(
                            chat_id = telegram_chat_id,
                            function_call = ?fc_clone,
                            "openai_chat module received unexpected function call name during parallel execution planning"
                        );
                        Ok(format!(
                            "{{\"error\": \"unexpected_tool_name\", \"tool_name\": \"{}\"}}",
                            fc_clone.name
                        ))
                    };
                    (fc, tool_output_result)
                };
                tool_execution_futures.push(future);
            }

            let execution_results = join_all(tool_execution_futures).await;

            let mut processed_tool_calls_for_api: Vec<ProcessedToolCall> = Vec::new();
            let mut any_tool_execution_failed = false;

            for (original_fc, result_from_tool_handler) in execution_results {
                match result_from_tool_handler {
                    Ok(output_json_string) => {
                        processed_tool_calls_for_api.push(ProcessedToolCall {
                            original_fc: original_fc.clone(),
                            output_json_string,
                        });
                    }
                    Err(e) => {
                        any_tool_execution_failed = true;
                        error!(chat_id = telegram_chat_id, tool_name = %original_fc.name, error = %e, "tool execution failed");
                        processed_tool_calls_for_api.push(ProcessedToolCall {
                            original_fc: original_fc.clone(),
                            output_json_string: format!(
                                "{{\"error\": \"tool_execution_failed\", \"tool_name\": \"{}\", \"details\": \"{}\"}}",
                                original_fc.name,
                                e.to_string().replace('"',"\\\"")
                            ),
                        });
                    }
                }
            }

            if any_tool_execution_failed {
                warn!(chat_id = telegram_chat_id, "one or more tool executions failed. results (including errors) will be sent to openai.");
            }

            match super::tools::common::execute_step2_api_call_with_multiple_tool_results(
                ctx,
                telegram_chat_id,
                &current_response_id,
                original_input_items.clone(),
                processed_tool_calls_for_api,
                available_tools.clone(),
                instructions,
            )
            .await
            {
                Ok((next_api_response, next_original_inputs)) => {
                    api_response = next_api_response;
                    original_input_items = next_original_inputs;
                }
                Err(e) => {
                    error!(chat_id = telegram_chat_id, response_id = %current_response_id, error= %e, "step 2 api call with multiple tool results failed. cannot continue processing this interaction.");
                    return Ok((
                        format!("a critical error occurred while trying to process tool results with the ai: {}. please try again.", e),
                        current_response_id
                    ));
                }
            }
        } else if !assistant_messages_in_current_turn.is_empty() {
            let final_text = assistant_messages_in_current_turn.remove(0);
            info!(
                chat_id = telegram_chat_id, response_id = %current_response_id,
                "received final assistant message: {}", final_text
            );
            return Ok((final_text, current_response_id));
        } else {
            warn!(
                chat_id = telegram_chat_id, response_id = %current_response_id,
                "openai api response output was empty or contained no actionable items."
            );
            return Ok((
                "i received an empty or unhandled response from the ai.".to_string(),
                current_response_id,
            ));
        }
    }
}

// generates an ai reply by calling the openai api and processing its response, including tool usage.
// returns the final text content and the openai response id to be stored for conversation context.
pub async fn generate_ai_reply_content(
    ctx: &super::HandlerContext<'_>,
    incoming_message: &TelegramMessage,
    prompt_text: &str,
    local_chat_id_for_conversation: i32,
) -> Result<(String, String)> {
    info!(
        chat_id = incoming_message.chat.id,
        message_id = incoming_message.message_id,
        prompt = prompt_text,
        "generating ai reply content for user: '{}'",
        prompt_text
    );

    let previous_response_id_opt = match db::get_last_openai_response_id(
        ctx.pool,
        local_chat_id_for_conversation,
    )
    .await
    {
        Ok(id_opt) => id_opt,
        Err(e) => {
            warn!(chat_id = incoming_message.chat.id, error = %e, "failed to fetch last_openai_response_id, proceeding without it.");
            None
        }
    };

    let input_items = vec![InputItem::Message(InputMessageObject {
        role: "user".to_string(),
        content: prompt_text.to_string(),
    })];

    let call_config = prepare_openai_call_config();

    let initial_api_args = CallResponsesApiOptionalArgs {
        model_id: OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: previous_response_id_opt.as_deref(),
        tools: Some(call_config.available_tools.clone()),
        tool_choice: None,
        instructions: Some(&call_config.instructions),
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
            process_openai_response(
                ctx,
                incoming_message.chat.id,
                api_response_1,
                input_items,
                call_config.available_tools,
                &call_config.instructions,
            )
            .await
        }
        Err(e) => {
            error!(chat_id = incoming_message.chat.id, error = %e, "initial /v1/responses api call failed in openai_chat module");
            Err(e)
        }
    }
}

// processes a single prompt for cli testing, bypassing database lookups for conversation context.
pub async fn process_single_prompt_for_cli(
    ctx: &super::HandlerContext<'_>,
    prompt_text: &str,
    telegram_chat_id: i64,
) -> Result<(String, String)> {
    info!(
        chat_id = telegram_chat_id,
        prompt = prompt_text,
        "(cli_test) processing single prompt directly in openai_chat module"
    );

    let input_items = vec![InputItem::Message(InputMessageObject {
        role: "user".to_string(),
        content: prompt_text.to_string(),
    })];

    let call_config = prepare_openai_call_config();

    let initial_api_args = CallResponsesApiOptionalArgs {
        model_id: OPENAI_RESPONSES_MODEL_ID,
        previous_response_id: None,
        tools: Some(call_config.available_tools.clone()),
        tool_choice: None,
        instructions: Some(&call_config.instructions),
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
            process_openai_response(
                ctx,
                telegram_chat_id,
                api_response_1,
                input_items,
                call_config.available_tools,
                &call_config.instructions,
            )
            .await
        }
        Err(e) => {
            error!(chat_id = telegram_chat_id, error = %e, "(cli_test) initial /v1/responses api call failed in openai_chat module");
            Err(e)
        }
    }
}

use eyre::{Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::{json, Map as SerdeJsonMap, Value as JsonValue};
use tracing::{debug, error, info};

pub mod types;
pub use types::*;

const OPENAI_RESPONSES_API_URL: &str = "https://api.openai.com/v1/responses";

pub async fn call_responses_api<'a>(
    http_client: &ReqwestClient,
    api_key: &str,
    input_items: Vec<InputItem>,
    args: CallResponsesApiOptionalArgs<'a>,
) -> Result<OpenAiApiResponse> {
    info!(
        url = OPENAI_RESPONSES_API_URL,
        model = args.model_id,
        previous_id = args.previous_response_id,
        input_items_count = input_items.len(),
        tools_count = args.tools.as_ref().map_or(0, |t| t.len()),
        "attempting to call openai /v1/responses endpoint"
    );

    let mut payload_map = SerdeJsonMap::new();
    payload_map.insert("input".to_string(), json!(input_items));
    payload_map.insert("model".to_string(), json!(args.model_id));

    if let Some(prev_id) = args.previous_response_id {
        payload_map.insert("previous_response_id".to_string(), json!(prev_id));
    }
    if let Some(tls) = args.tools {
        payload_map.insert("tools".to_string(), json!(tls));
    }
    if let Some(tc) = args.tool_choice {
        payload_map.insert("tool_choice".to_string(), tc);
    }
    if let Some(instr) = args.instructions {
        payload_map.insert("instructions".to_string(), json!(instr));
    }
    if let Some(temp) = args.temperature {
        payload_map.insert("temperature".to_string(), json!(temp));
    }
    if let Some(s) = args.store {
        payload_map.insert("store".to_string(), json!(s));
    }

    let request_payload = JsonValue::Object(payload_map);

    debug!(payload = ?request_payload, "sending payload to /v1/responses api");

    let response = http_client
        .post(OPENAI_RESPONSES_API_URL)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&request_payload)
        .send()
        .await
        .context("failed to send request to openai /v1/responses endpoint")?;

    let status = response.status();
    let response_text = response
        .text()
        .await
        .context("failed to read response text from openai /v1/responses endpoint")?;

    if status.is_success() {
        info!(
            status = %status,
            raw_response_preview = &response_text[..std::cmp::min(response_text.len(), 500)],
            "successfully received response from /v1/responses"
        );
        let parsed_response: OpenAiApiResponse = serde_json::from_str(&response_text)
            .wrap_err_with(|| format!("failed to deserialize openai /v1/responses json. response text: {}\nensure structs match api response.", response_text))?;
        Ok(parsed_response)
    } else {
        error!(
            status = %status,
            response_body = response_text,
            "error response from openai /v1/responses endpoint"
        );
        Err(eyre::eyre!(
            "openai /v1/responses api call failed with status {}: {}. response: {}",
            status,
            status.canonical_reason().unwrap_or("unknown error"),
            response_text
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client as ReqwestClient;
    use std::collections::HashMap;
    use std::env;

    #[tokio::test]
    async fn test_call_responses_api_with_input_parameter() {
        let _ = color_eyre::install();

        let api_key = match env::var("OPENAI_API_KEY") {
            Ok(key) => key,
            Err(_) => {
                eprintln!(
                    "test_call_responses_api_with_input_parameter skipped: OPENAI_API_KEY not set."
                );
                return;
            }
        };

        let http_client = ReqwestClient::new();
        let current_user_input = "tell me a three sentence bedtime story about a unicorn.";
        let model_id_val = "gpt-4.1";

        let input_items_val = vec![InputItem::Message(InputMessageObject {
            role: "user".to_string(),
            content: current_user_input.to_string(),
        })];

        let api_args = CallResponsesApiOptionalArgs {
            model_id: model_id_val,
            previous_response_id: None,
            tools: None,
            tool_choice: None,
            instructions: None,
            temperature: None,
            store: None,
        };

        println!("attempting test call to call_responses_api with 'input' parameter...");
        let result = call_responses_api(&http_client, &api_key, input_items_val, api_args).await;

        match result {
            Ok(parsed_response) => {
                println!(
                    "test_call_responses_api_with_input_parameter: call successful (2xx status)."
                );
                println!("parsed response: {:#?}", parsed_response);
                assert_eq!(parsed_response.object, "response");
                assert!(!parsed_response.output.is_empty());
                match parsed_response.output.first().unwrap() {
                    OutputItem::Message(msg) => {
                        assert_eq!(msg.role, "assistant");
                        assert!(!msg.content.is_empty());
                        let first_text_content = msg.content.first().unwrap();
                        assert_eq!(first_text_content.r#type, "output_text");
                        assert!(!first_text_content.text.is_empty());
                        println!("assistant reply: {}", first_text_content.text);
                    }
                    OutputItem::FunctionCall(fc) => {
                        panic!(
                            "expected a message output, but got a function call: {:?}",
                            fc
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "test_call_responses_api_with_input_parameter: call failed or returned non-2xx status."
                );
                eprintln!("error details: {:?}", e);
                panic!(
                    "/v1/responses api call attempt with 'input' parameter resulted in an error: {:?}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_call_responses_api_with_function_tool() {
        let _ = color_eyre::install();
        let api_key = match env::var("OPENAI_API_KEY") {
            Ok(key) => key,
            Err(_) => {
                eprintln!(
                    "test_call_responses_api_with_function_tool skipped: OPENAI_API_KEY not set."
                );
                return;
            }
        };
        let http_client = ReqwestClient::new();
        let model_id_val = "gpt-4.1";

        let mut params_props = HashMap::new();
        params_props.insert(
            "sql_query".to_string(),
            ToolFunctionParameterProperty {
                r#type: "string".to_string(),
                description: Some(
                    "the sql select query to execute. must start with 'select'.".to_string(),
                ),
                r#enum: Vec::new(),
            },
        );
        let tool_params = ToolFunctionParameters {
            r#type: "object".to_string(),
            properties: params_props,
            required: Some(vec!["sql_query".to_string()]),
            additional_properties: false,
        };
        let sql_tool = ToolDefinition {
            r#type: "function".to_string(),
            name: "execute_sql_query".to_string(),
            description: Some("executes a sql select query against the postgresql database and returns the results. only select queries are permitted.".to_string()),
            parameters: Some(tool_params),
            strict: Some(true),
        };
        let tools_val = vec![sql_tool];

        let initial_instruction_text = "you are a helpful assistant. use tools when appropriate. the user table is tool_test_users.";
        let input_items_initial_val = vec![InputItem::Message(InputMessageObject {
            role: "user".to_string(),
            content: "show me the email for user simple_alpha from the tool_test_users table"
                .to_string(),
        })];

        let initial_api_args = CallResponsesApiOptionalArgs {
            model_id: model_id_val,
            previous_response_id: None,
            tools: Some(tools_val.clone()),
            tool_choice: None,
            instructions: Some(initial_instruction_text),
            temperature: None,
            store: None,
        };

        println!("function call test: step 1 - requesting tool use...");
        let initial_response_result = call_responses_api(
            &http_client,
            &api_key,
            input_items_initial_val.clone(),
            initial_api_args,
        )
        .await;

        assert!(
            initial_response_result.is_ok(),
            "initial api call for function use failed: {:?}",
            initial_response_result.err()
        );
        let initial_response = initial_response_result.unwrap();
        println!(
            "function call test: step 1 - response: {:#?}",
            initial_response
        );

        assert!(
            !initial_response.output.is_empty(),
            "initial response output is empty"
        );
        let function_call_item = match initial_response.output.first().unwrap() {
            OutputItem::FunctionCall(fc) => fc,
            OutputItem::Message(msg) => {
                panic!("expected a function call, got a message: {:?}", msg)
            }
        };

        assert_eq!(function_call_item.name, "execute_sql_query");
        println!(
            "function call test: step 1 - model wants to call function: {}, with args: {}",
            function_call_item.name, function_call_item.arguments
        );

        let arguments_json: JsonValue = serde_json::from_str(&function_call_item.arguments)
            .expect("failed to parse function call arguments");
        let sql_query_from_model = arguments_json
            .get("sql_query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!(
            "function call test: sql query from model: {}",
            sql_query_from_model
        );

        let mock_sql_result = json!({
            "status": "success",
            "message": "query executed successfully.",
            "results": [{"email": "alpha@simpletest.com", "username": "simple_alpha"}]
        })
        .to_string();

        let mut second_call_input_items_val = input_items_initial_val;
        second_call_input_items_val.push(InputItem::Message(InputMessageObject {
            role: "assistant".to_string(),
            content: format!(
                "tool_call: id={}, call_id={}, name={}, args={}",
                function_call_item.id,
                function_call_item.call_id,
                function_call_item.name,
                function_call_item.arguments
            ),
        }));
        second_call_input_items_val.push(InputItem::FunctionCallOutput(FunctionCallOutputItem {
            r#type: "function_call_output".to_string(),
            call_id: function_call_item.call_id.clone(),
            output: mock_sql_result,
        }));

        let final_api_args = CallResponsesApiOptionalArgs {
            model_id: model_id_val,
            previous_response_id: Some(&initial_response.id),
            tools: Some(tools_val.clone()),
            tool_choice: None,
            instructions: Some(initial_instruction_text),
            temperature: None,
            store: None,
        };

        println!("function call test: step 2 - sending function result...");
        let final_response_result = call_responses_api(
            &http_client,
            &api_key,
            second_call_input_items_val,
            final_api_args,
        )
        .await;

        assert!(
            final_response_result.is_ok(),
            "final api call with function result failed: {:?}",
            final_response_result.err()
        );
        let final_response = final_response_result.unwrap();
        println!(
            "function call test: step 2 - final response: {:#?}",
            final_response
        );

        assert!(
            !final_response.output.is_empty(),
            "final response output is empty"
        );
        match final_response.output.first().unwrap() {
            OutputItem::Message(msg) => {
                assert_eq!(msg.role, "assistant");
                assert!(!msg.content.is_empty());
                let text_content = msg.content.first().unwrap();
                println!(
                    "function call test: step 2 - final assistant reply: {}",
                    text_content.text
                );
                assert!(text_content
                    .text
                    .to_lowercase()
                    .contains("alpha@simpletest.com"));
            }
            OutputItem::FunctionCall(fc) => {
                panic!(
                    "expected a final message, but got another function call: {:?}",
                    fc
                );
            }
        }
    }
}

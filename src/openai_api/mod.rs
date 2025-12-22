use anyhow::{anyhow, Context, Result};
use reqwest::Client as ReqwestClient;
use tracing::{error, info};

pub mod types;
pub use types::*;

const OPENAI_RESPONSES_API_URL: &str = "https://api.openai.com/v1/responses";

pub async fn call_responses_api<'a>(
    http_client: ReqwestClient,
    api_key: &str,
    input_items: Vec<InputItem>,
    args: CallResponsesApiOptionalArgs<'a>,
) -> Result<OpenAiApiResponse> {
    // build request payload without including null fields, and respecting
    // current api placement for verbosity (under text.verbosity) and reasoning
    // (under top-level reasoning.effort)
    let mut root = serde_json::Map::new();
    root.insert("input".to_string(), serde_json::to_value(&input_items)?);
    root.insert("model".to_string(), serde_json::to_value(args.model_id)?);

    if let Some(prev_id) = args.previous_response_id {
        root.insert(
            "previous_response_id".to_string(),
            serde_json::to_value(prev_id)?,
        );
    }
    if let Some(ref tools) = args.tools {
        root.insert("tools".to_string(), serde_json::to_value(tools)?);
    }
    if let Some(tool_choice) = args.tool_choice {
        root.insert(
            "tool_choice".to_string(),
            serde_json::to_value(tool_choice)?,
        );
    }
    if let Some(instructions) = args.instructions {
        root.insert(
            "instructions".to_string(),
            serde_json::to_value(instructions)?,
        );
    }
    if let Some(temperature) = args.temperature {
        root.insert(
            "temperature".to_string(),
            serde_json::to_value(temperature)?,
        );
    }
    if let Some(store) = args.store {
        root.insert("store".to_string(), serde_json::to_value(store)?);
    }
    if let Some(verbosity) = args.verbosity {
        let mut text_obj = serde_json::Map::new();
        text_obj.insert("verbosity".to_string(), serde_json::to_value(verbosity)?);
        root.insert("text".to_string(), serde_json::Value::Object(text_obj));
    }
    if let Some(effort) = args.reasoning_effort {
        let mut reasoning_obj = serde_json::Map::new();
        reasoning_obj.insert("effort".to_string(), serde_json::to_value(effort)?);
        root.insert(
            "reasoning".to_string(),
            serde_json::Value::Object(reasoning_obj),
        );
    }

    let request_payload = serde_json::Value::Object(root);

    let endpoint_url = OPENAI_RESPONSES_API_URL;
    let tools_count_val = args.tools.as_ref().map_or(0, std::vec::Vec::len);
    info!(
        url = %endpoint_url,
        model = %args.model_id,
        previous_id = ?args.previous_response_id,
        input_items_count = input_items.len(),
        tools_count = tools_count_val,
        "attempting to call openai {} endpoint", "/v1/responses"
    );

    let response = http_client
        .post(endpoint_url)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&request_payload)
        .send()
        .await
        .map_err(|e| {
            // log richer details about the underlying transport error so callers
            // (and logs) can distinguish between timeouts, dns/connect issues, etc.
            let is_timeout = e.is_timeout();
            let is_connect = e.is_connect();
            let is_request = e.is_request();
            let is_body = e.is_body();
            error!(
                url = %endpoint_url,
                error = %e,
                error_debug = ?e,
                is_timeout,
                is_connect,
                is_request,
                is_body,
                "failed to send request to openai api"
            );
            anyhow!(e).context("failed to send request to openai api")
        })?;

    let response_status = response.status();
    let response_text = response
        .text()
        .await
        .context("failed to read response text from openai api")?;

    if response_status.is_success() {
        match serde_json::from_str::<OpenAiApiResponse>(&response_text) {
            Ok(api_response) => {
                info!(
                    status = %response_status,
                    id = %api_response.id,
                    object = %api_response.object,
                    "successfully received response from {} status={} id={} type={}",
                    "/v1/responses",
                    response_status,
                    api_response.id,
                    api_response.object
                );
                Ok(api_response)
            }
            Err(e) => {
                error!(
                    status = %response_status,
                    response_body = %response_text,
                    error = %e,
                    "failed to deserialize successful openai api response for {}", "/v1/responses"
                );
                Err(anyhow!(
                    "failed to deserialize openai api response (status {}): {}. response body: {}",
                    response_status,
                    e,
                    response_text
                ))
            }
        }
    } else {
        error!(
            status = %response_status,
            response_body = %response_text,
            "error response from openai {} endpoint", "/v1/responses"
        );
        Err(anyhow!(
            "openai {} api call failed with status {}: {}. response: {}",
            "/v1/responses",
            response_status,
            response_status
                .canonical_reason()
                .unwrap_or("unknown status"),
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
        let api_key = if let Ok(key) = env::var("OPENAI_API_KEY") {
            key
        } else {
            eprintln!(
                "test_call_responses_api_with_input_parameter skipped: OPENAI_API_KEY not set."
            );
            return;
        };

        let http_client = ReqwestClient::new();
        let current_user_input = "tell me a three sentence bedtime story about a unicorn.";
        let model_id_val = "gpt-5.2";

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
            verbosity: None,
            reasoning_effort: None,
        };

        println!("attempting test call to call_responses_api with 'input' parameter...");
        let result = call_responses_api(http_client, &api_key, input_items_val, api_args).await;

        match result {
            Ok(parsed_response) => {
                println!(
                    "test_call_responses_api_with_input_parameter: call successful (2xx status)."
                );
                println!("parsed response: {parsed_response:#?}");
                assert_eq!(parsed_response.object, "response");
                assert!(!parsed_response.output.is_empty());
                // tolerate an initial reasoning item; find the first message item
                if let Some(OutputItem::Message(msg)) = parsed_response
                    .output
                    .iter()
                    .find(|item| matches!(item, OutputItem::Message(_)))
                {
                    assert_eq!(msg.role, "assistant");
                    assert!(!msg.content.is_empty());
                    let first_text_content = msg.content.first().unwrap();
                    assert_eq!(first_text_content.r#type, "output_text");
                    assert!(!first_text_content.text.is_empty());
                    println!("assistant reply: {}", first_text_content.text);
                } else {
                    panic!(
                        "expected a message output, but none was found in response output items"
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "test_call_responses_api_with_input_parameter: call failed or returned non-2xx status."
                );
                eprintln!("error details: {e:?}");
                panic!(
                    "/v1/responses api call attempt with 'input' parameter resulted in an error: {e:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_call_responses_api_with_function_tool() {
        let api_key = if let Ok(key) = env::var("OPENAI_API_KEY") {
            key
        } else {
            eprintln!(
                "test_call_responses_api_with_function_tool skipped: OPENAI_API_KEY not set."
            );
            return;
        };
        let http_client = ReqwestClient::new();
        let model_id_val = "gpt-5.2";

        let mut params_props = HashMap::new();
        params_props.insert(
            "sql_query".to_string(),
            ToolFunctionParameterPropertyBuilder::new_string()
                .description("the sql select query to execute. must start with 'select'.")
                .build(),
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
        let tools_val = vec![ApiToolType::Function(sql_tool)];

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
            verbosity: None,
            reasoning_effort: None,
        };

        println!("function call test: step 1 - requesting tool use...");
        let initial_response_result = call_responses_api(
            http_client.clone(),
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
        println!("function call test: step 1 - response: {initial_response:#?}");

        assert!(
            !initial_response.output.is_empty(),
            "initial response output is empty"
        );
        // tolerate an initial reasoning item; find the first function call
        let function_call_item = initial_response
            .output
            .iter()
            .find_map(|item| match item {
                OutputItem::FunctionCall(fc) => Some(fc),
                _ => None,
            })
            .expect("expected a function call in the output items, but none was found");

        assert_eq!(function_call_item.name, "execute_sql_query");
        println!(
            "function call test: step 1 - model wants to call function: {}, with args: {}",
            function_call_item.name, function_call_item.arguments
        );

        let arguments_json: serde_json::Value = serde_json::from_str(&function_call_item.arguments)
            .expect("failed to parse function call arguments");
        let sql_query_from_model = arguments_json
            .get("sql_query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("function call test: sql query from model: {sql_query_from_model}");

        let mock_sql_result = serde_json::json!({
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
            verbosity: None,
            reasoning_effort: None,
        };

        println!("function call test: step 2 - sending function result...");
        let final_response_result = call_responses_api(
            http_client.clone(),
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
        println!("function call test: step 2 - final response: {final_response:#?}");

        assert!(
            !final_response.output.is_empty(),
            "final response output is empty"
        );
        // tolerate a leading reasoning item; find the first assistant message
        if let Some(OutputItem::Message(msg)) = final_response
            .output
            .iter()
            .find(|item| matches!(item, OutputItem::Message(_)))
        {
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
        } else {
            panic!(
                "expected a final assistant message, but none was found in response output items"
            );
        }
    }
}

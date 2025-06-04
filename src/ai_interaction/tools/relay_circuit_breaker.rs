use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::{anyhow, Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, error, info, instrument, warn};

pub const RELAY_CIRCUIT_BREAKER_TOOL_NAME: &str = "relay_circuit_breaker";
const SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND: &str = "set_adjustment_circuit_breaker";
const SET_AUCTION_CIRCUIT_BREAKER_COMMAND: &str = "set_auction_circuit_breaker";

const COMMAND_PARAM: &str = "command";
const ENABLED_PARAM: &str = "enabled";

const ADJUSTMENT_CIRCUIT_BREAKER_PATH: &str = "adjustment_circuit_breaker";
const AUCTION_CIRCUIT_BREAKER_PATH: &str = "auction_circuit_breaker";

pub static RELAY_CIRCUIT_BREAKER_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        COMMAND_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the specific circuit breaker to control: 'set_adjustment_circuit_breaker' or 'set_auction_circuit_breaker'.",
            )
            .enum_string(&[
                SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND,
                SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            ])
            .build(),
    );
    params_props.insert(
        ENABLED_PARAM.to_string(),
        ToolFunctionParameterPropertyBuilder::new_boolean()
            .description(
                "the desired state for the circuit breaker. set to true to enable (activate/close) the circuit breaker (stopping operations), or false to disable (deactivate/open) it (resuming operations).",
            )
            .build(),
    );

    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec![COMMAND_PARAM.to_string(), ENABLED_PARAM.to_string()]),
        additional_properties: false,
    };

    ToolDefinition::new(
        RELAY_CIRCUIT_BREAKER_TOOL_NAME.to_string(),
        Some(
            "sets the state (enabled/disabled) of a specific relay circuit breaker (adjustment or auction). requires admin privileges.".to_string(),
        ),
        Some(tool_params),
    )
});

#[derive(Debug)]
pub struct RelayCircuitBreaker {
    http_client: ReqwestClient,
    admin_token: String,
    base_url: String,
}

impl RelayCircuitBreaker {
    pub fn new(admin_token: String, base_url: String) -> Self {
        Self {
            http_client: ReqwestClient::new(),
            admin_token,
            base_url,
        }
    }

    #[instrument(skip(self))]
    async fn set_state(&self, relative_path: &str, enabled: bool) -> Result<String> {
        let request_url = format!(
            "{}/ultrasound/v1/admin/{relative_path}?token={}&enable={}",
            self.base_url, self.admin_token, enabled
        );
        // let request_body = json!({ "enabled": enabled });

        info!(url = %request_url, "sending circuit breaker state request");

        match self.http_client.get(&request_url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let response_text = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "no response body".to_string());
                    info!(status = %status, response_body = %response_text, "circuit breaker state set successfully");
                    Ok(json!({
                        "status": "success",
                        "message": format!("circuit breaker '{relative_path}' state set to {enabled}."),
                        "details": response_text
                    })
                    .to_string())
                } else {
                    let err_text = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "failed to get error body".to_string());
                    warn!(status = %status, body = %err_text, "error response from relay admin endpoint");
                    Ok(json!({
                        "status": "error",
                        "message": format!("failed to set circuit breaker '{relative_path}' state. relay responded with status {status}."),
                        "details": err_text
                    })
                    .to_string())
                }
            }
            Err(e) => {
                error!(error = %e, "failed to send request to relay admin endpoint for path {relative_path}");
                Err(anyhow!(e)).context(format!("http request failed for {relative_path}"))
            }
        }
    }
}

struct ParsedArgs {
    command: String,
    enabled: bool,
}

fn parse_circuit_breaker_args(arguments_json_str: &str) -> Result<ParsedArgs, String> {
    match serde_json::from_str::<HashMap<String, JsonValue>>(arguments_json_str) {
        Ok(args_map) => {
            let command_val = args_map.get(COMMAND_PARAM);
            let enabled_val = args_map.get(ENABLED_PARAM);

            match (command_val, enabled_val) {
                (Some(JsonValue::String(cmd_str)), Some(JsonValue::Bool(enabled_bool))) => {
                    Ok(ParsedArgs {
                        command: cmd_str.clone(),
                        enabled: *enabled_bool,
                    })
                }
                _ => {
                    let mut missing_or_invalid_params = Vec::new();
                    if command_val.is_none_or(|v| !v.is_string()) {
                        missing_or_invalid_params
                            .push(format!("'{COMMAND_PARAM}' (must be a string)"));
                    }
                    if enabled_val.is_none_or(|v| !v.is_boolean()) {
                        missing_or_invalid_params
                            .push(format!("'{ENABLED_PARAM}' (must be a boolean)"));
                    }
                    Err(format!("missing or invalid argument(s): {}. expected a '{COMMAND_PARAM}' (string) and an '{ENABLED_PARAM}' (boolean).", missing_or_invalid_params.join(", ")))
                }
            }
        }
        Err(e) => Err(format!(
            "failed to parse arguments json for relay_circuit_breaker_tool: {e}"
        )),
    }
}

#[instrument(skip(circuit_breaker_client, arguments_json_str))]
pub async fn execute_relay_circuit_breaker_tool(
    circuit_breaker_client: &RelayCircuitBreaker,
    arguments_json_str: &str,
) -> Result<String> {
    info!(args = %arguments_json_str, "executing relay_circuit_breaker_tool");

    match parse_circuit_breaker_args(arguments_json_str) {
        Ok(parsed_args) => {
            let relative_path = match parsed_args.command.as_str() {
                SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND => ADJUSTMENT_CIRCUIT_BREAKER_PATH,
                SET_AUCTION_CIRCUIT_BREAKER_COMMAND => AUCTION_CIRCUIT_BREAKER_PATH,
                _ => {
                    let err_msg = format!("invalid command '{}' specified.", parsed_args.command);
                    warn!(args = %arguments_json_str, error = %err_msg);
                    return Ok(json!({ "status": "error", "message": "invalid_argument", "details": err_msg }).to_string());
                }
            };

            debug!(command = %parsed_args.command, enabled = %parsed_args.enabled, "parsed arguments for circuit breaker tool");

            match circuit_breaker_client
                .set_state(relative_path, parsed_args.enabled)
                .await
            {
                Ok(result_str) => Ok(result_str),
                Err(e) => {
                    warn!(command = %parsed_args.command, enabled = %parsed_args.enabled, error = %e, "error calling set_state");
                    Ok(json!({
                        "status": "error",
                        "message": "http_request_failed",
                        "details": format!("error setting circuit breaker for {}: {e}", parsed_args.command)
                    }).to_string())
                }
            }
        }
        Err(err_detail) => {
            let err_message_key = if err_detail.starts_with("failed to parse arguments json") {
                "argument_parsing_error"
            } else {
                "invalid_argument"
            };
            warn!(args = %arguments_json_str, error = %err_detail, "argument parsing failed");
            Ok(
                json!({ "status": "error", "message": err_message_key, "details": err_detail })
                    .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    #[tokio::test]
    async fn test_execute_set_adjustment_breaker_success() {
        let mut server = mockito::Server::new_async().await;
        let mock_server_url = server.url();
        let token = "test_token_success";

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), mock_server_url.clone());

        // Mockito path should be relative to server.url()
        let mock_path = format!(
            "/ultrasound/v1/admin/{ADJUSTMENT_CIRCUIT_BREAKER_PATH}?token={token}&enable=true"
        );
        let mock_api = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message": "adjustment circuit breaker enabled"}"#)
            // .match_body(Matcher::Json(json!({"enabled": true})))
            .create_async()
            .await;

        // Test the http_call helper directly
        let result_str = circuit_breaker_client
            .set_state(ADJUSTMENT_CIRCUIT_BREAKER_PATH, true)
            .await
            .unwrap();

        mock_api.assert_async().await;
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "success");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains(ADJUSTMENT_CIRCUIT_BREAKER_PATH));
        assert!(result_json["message"].as_str().unwrap().contains("true"));
    }

    #[tokio::test]
    async fn test_execute_set_auction_breaker_disabled_success() {
        let mut server = mockito::Server::new_async().await;
        let mock_server_url = server.url();
        let token = "test_token_auction_disabled";

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), mock_server_url.clone());

        let mock_path = format!(
            "/ultrasound/v1/admin/{AUCTION_CIRCUIT_BREAKER_PATH}?token={token}&enable=false"
        );
        let mock_api = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message": "auction circuit breaker disabled"}"#)
            // .match_body(Matcher::Json(json!({"enabled": false})))
            .create_async()
            .await;

        let result_str = circuit_breaker_client
            .set_state(AUCTION_CIRCUIT_BREAKER_PATH, false)
            .await
            .unwrap();

        mock_api.assert_async().await;
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "success");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains(AUCTION_CIRCUIT_BREAKER_PATH));
        assert!(result_json["message"].as_str().unwrap().contains("false"));
    }

    #[tokio::test]
    async fn test_execute_tool_with_valid_args_adjustment_enabled() {
        let mut server = mockito::Server::new_async().await;
        let mock_server_url = server.url();
        let token = "test_token_execute_adj";

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), mock_server_url.clone());

        let mock_path = format!(
            "/ultrasound/v1/admin/{ADJUSTMENT_CIRCUIT_BREAKER_PATH}?token={token}&enable=true"
        );
        let mock_api = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message": "adjustment cb enabled"}"#)
            // .match_body(Matcher::Json(json!({"enabled": true})))
            .create_async()
            .await;

        let args_json = json!({
            COMMAND_PARAM: SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND,
            ENABLED_PARAM: true
        })
        .to_string();

        let result_str = execute_relay_circuit_breaker_tool(&circuit_breaker_client, &args_json)
            .await
            .unwrap();

        mock_api.assert_async().await;
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "success");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains(ADJUSTMENT_CIRCUIT_BREAKER_PATH));
    }

    #[tokio::test]
    async fn test_execute_tool_with_valid_args_auction_disabled() {
        let mut server = mockito::Server::new_async().await;
        let mock_server_url = server.url();
        let token = "test_token_execute_auc";

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), mock_server_url.clone());

        let mock_path = format!(
            "/ultrasound/v1/admin/{AUCTION_CIRCUIT_BREAKER_PATH}?token={token}&enable=false"
        );
        let mock_api = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message": "auction cb disabled"}"#)
            // .match_body(Matcher::Json(json!({"enabled": false})))
            .create_async()
            .await;

        let args_json = json!({
            COMMAND_PARAM: SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            ENABLED_PARAM: false
        })
        .to_string();

        let result_str = execute_relay_circuit_breaker_tool(&circuit_breaker_client, &args_json)
            .await
            .unwrap();

        mock_api.assert_async().await;
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "success");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains(AUCTION_CIRCUIT_BREAKER_PATH));
    }

    #[tokio::test]
    async fn test_http_request_failure() {
        let token = "test_token_failure";
        // Use a base_url that won't resolve or connect.
        let non_existent_base_url = "http://localhost:12345"; // Assume this port is not listening

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), non_existent_base_url.to_string());

        let result = circuit_breaker_client
            .set_state(ADJUSTMENT_CIRCUIT_BREAKER_PATH, true)
            .await;

        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("http request failed"));
        assert!(err_msg.contains(ADJUSTMENT_CIRCUIT_BREAKER_PATH));
    }

    #[tokio::test]
    async fn test_relay_returns_error_status() {
        let mut server = mockito::Server::new_async().await;
        let mock_server_url = server.url();
        let token = "test_token_relay_error";

        let circuit_breaker_client =
            RelayCircuitBreaker::new(token.to_string(), mock_server_url.clone());

        let mock_path = format!(
            "/ultrasound/v1/admin/{AUCTION_CIRCUIT_BREAKER_PATH}?token={token}&enable=true"
        );
        let mock_api = server
            .mock("get", Matcher::Exact(mock_path.clone()))
            .with_status(500)
            .with_body(r#"{"error": "internal server blew up"}"#)
            .create_async()
            .await;

        let result_str = circuit_breaker_client
            .set_state(AUCTION_CIRCUIT_BREAKER_PATH, true)
            .await
            .unwrap();

        mock_api.assert_async().await;
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        assert_eq!(result_json["status"], "error");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains("responded with status 500"));
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("internal server blew up"));
    }

    #[tokio::test]
    async fn test_invalid_command_argument_in_execute_tool() {
        let circuit_breaker_client =
            RelayCircuitBreaker::new("token".to_string(), "http://base.url".to_string());

        let args_json = json!({
            COMMAND_PARAM: "invalid_command_name",
            ENABLED_PARAM: true
        })
        .to_string();

        let result_str = execute_relay_circuit_breaker_tool(&circuit_breaker_client, &args_json)
            .await
            .unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_argument");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("invalid command 'invalid_command_name'"));
    }

    #[tokio::test]
    async fn test_missing_enabled_argument_in_execute_tool() {
        let circuit_breaker_client =
            RelayCircuitBreaker::new("token".to_string(), "http://base.url".to_string());

        let args_json = json!({
            COMMAND_PARAM: SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND
        })
        .to_string();

        let result_str = execute_relay_circuit_breaker_tool(&circuit_breaker_client, &args_json)
            .await
            .unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_argument");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("missing or invalid argument(s)"));
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(ENABLED_PARAM));
    }

    #[tokio::test]
    async fn test_malformed_arguments_json_in_execute_tool() {
        let circuit_breaker_client =
            RelayCircuitBreaker::new("token".to_string(), "http://base.url".to_string());

        let args_json_str = "this is not json";
        let result_str = execute_relay_circuit_breaker_tool(&circuit_breaker_client, args_json_str)
            .await
            .unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "argument_parsing_error");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("failed to parse arguments json"));
    }

    // Tests for parse_circuit_breaker_args
    #[test]
    fn test_parse_args_valid() {
        let args_json = json!({
            COMMAND_PARAM: SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            ENABLED_PARAM: true
        })
        .to_string();
        let parsed = parse_circuit_breaker_args(&args_json).unwrap();
        assert_eq!(parsed.command, SET_AUCTION_CIRCUIT_BREAKER_COMMAND);
        assert!(parsed.enabled);
    }

    #[test]
    fn test_parse_args_missing_command() {
        let args_json = json!({
            ENABLED_PARAM: false
        })
        .to_string();
        let err = parse_circuit_breaker_args(&args_json).err().unwrap();
        assert!(err.contains(COMMAND_PARAM));
        assert!(err.contains("missing or invalid argument(s)"));
    }

    #[test]
    fn test_parse_args_missing_enabled() {
        let args_json = json!({
            COMMAND_PARAM: SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND
        })
        .to_string();
        let err = parse_circuit_breaker_args(&args_json).err().unwrap();
        assert!(err.contains(ENABLED_PARAM));
        assert!(err.contains("missing or invalid argument(s)"));
    }

    #[test]
    fn test_parse_args_invalid_command_type() {
        let args_json = json!({
            COMMAND_PARAM: 123,
            ENABLED_PARAM: true
        })
        .to_string();
        let err = parse_circuit_breaker_args(&args_json).err().unwrap();
        assert!(err.contains(COMMAND_PARAM));
        assert!(err.contains("must be a string"));
    }

    #[test]
    fn test_parse_args_invalid_enabled_type() {
        let args_json = json!({
            COMMAND_PARAM: SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            ENABLED_PARAM: "true_string"
        })
        .to_string();
        let err = parse_circuit_breaker_args(&args_json).err().unwrap();
        assert!(err.contains(ENABLED_PARAM));
        assert!(err.contains("must be a boolean"));
    }

    #[test]
    fn test_parse_args_malformed_json() {
        let args_json = "not a json {";
        let err = parse_circuit_breaker_args(args_json).err().unwrap();
        assert!(err.contains("failed to parse arguments json"));
    }
}

use crate::ai_interaction::HandlerContext;
use crate::db::Db;
use crate::env::ENV_CONFIG;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterProperty, ToolFunctionParameterPropertyBuilder,
    ToolFunctionParameters,
};
use eyre::{eyre, Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

pub const RELAY_CIRCUIT_BREAKER_TOOL_NAME: &str = "relay_circuit_breaker";
const SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND: &str = "set_adjustment_circuit_breaker";
const SET_AUCTION_CIRCUIT_BREAKER_COMMAND: &str = "set_auction_circuit_breaker";

#[derive(Debug, serde::Deserialize)]
struct RelayCircuitBreakerArgs {
    command_name: String,
    enabled: bool,
}

pub static RELAY_CIRCUIT_BREAKER_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "command_name".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description("the specific circuit breaker command to execute.")
            .enum_string(&[
                SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND,
                SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            ])
            .build(),
    );
    params_props.insert(
        "enabled".to_string(),
        ToolFunctionParameterProperty {
            r#type: "boolean".to_string(),
            description: Some(
                "whether to enable (true) or disable (false) the circuit breaker.".to_string(),
            ),
            r#enum: None,
        },
    );

    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["command_name".to_string(), "enabled".to_string()]),
        additional_properties: false,
    };

    ToolDefinition::new(
        RELAY_CIRCUIT_BREAKER_TOOL_NAME.to_string(),
        Some(
            "sets the state of a specific relay circuit breaker (adjustment or auction) to enabled or disabled. requires admin privileges implicitly via a configured secret.".to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(skip(http_client, circuit_breaker_url, admin_secret, command_type, enabled_state), fields(command_type = %command_type, enabled = %enabled_state))]
async fn call_circuit_breaker_api(
    http_client: &ReqwestClient,
    circuit_breaker_url: &str,
    admin_secret: &str,
    command_type: &str, // "adjustment" or "auction"
    enabled_state: bool,
) -> Result<JsonValue> {
    let request_url = format!("{}/set-state", circuit_breaker_url.trim_end_matches('/'));

    let payload = json!({
        "circuit_breaker_name": command_type,
        "enabled": enabled_state
    });

    info!(url = %request_url, payload = %payload, "calling circuit breaker api");

    match http_client
        .post(&request_url)
        .header("x-admin-secret", admin_secret) // header names are case-insensitive but conventionally lowercase
        .json(&payload)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let response_body_text = response.text().await.unwrap_or_else(|_| "failed to read response body".to_string());
            if status.is_success() {
                info!(status = %status, body = %response_body_text, "circuit breaker api call successful");
                match serde_json::from_str::<JsonValue>(&response_body_text) {
                    Ok(json_val) => Ok(json_val),
                    Err(_) => Ok(json!({
                        "status": "success",
                        "message": format!("circuit breaker '{}' state set to {}.", command_type, enabled_state),
                        "details": response_body_text
                    })),
                }
            } else {
                warn!(status = %status, body = %response_body_text, "error response from circuit breaker api");
                Err(eyre!(
                    "circuit breaker api returned error status {}: {}",
                    status,
                    response_body_text
                ))
            }
        }
        Err(e) => {
            error!(error = %e, "failed to send request to circuit breaker api");
            Err(eyre!(e)).context("failed to connect to circuit breaker api")
        }
    }
}

#[instrument(skip(ctx, arguments_json_str))]
pub async fn execute_relay_circuit_breaker_tool<D: Db>(
    ctx: &HandlerContext<'_, D>,
    arguments_json_str: &str,
) -> Result<String> {
    info!(args = %arguments_json_str, "executing {} tool", RELAY_CIRCUIT_BREAKER_TOOL_NAME);

    let (circuit_breaker_url, admin_secret) =
        match (&ENV_CONFIG.circuit_breaker_url, &ENV_CONFIG.circuit_breaker_admin_secret) {
            (Some(url), Some(secret)) => (url.as_str(), secret.as_str()),
            _ => {
                let err_msg = "circuit_breaker_url and/or circuit_breaker_admin_secret environment variable(s) not set.";
                error!(err_msg);
                return Ok(json!({
                    "status": "error",
                    "message": "tool_configuration_error",
                    "details": err_msg
                })
                .to_string());
            }
        };

    match serde_json::from_str::<RelayCircuitBreakerArgs>(arguments_json_str) {
        Ok(args) => {
            let circuit_breaker_type = match args.command_name.as_str() {
                SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND => "adjustment",
                SET_AUCTION_CIRCUIT_BREAKER_COMMAND => "auction",
                _ => {
                    let err_msg = format!("invalid command_name: '{}'", args.command_name);
                    warn!(err_msg);
                    return Ok(json!({
                        "status": "error",
                        "message": "invalid_argument",
                        "details": err_msg
                    })
                    .to_string());
                }
            };

            info!(command_name = %args.command_name, circuit_breaker_type = %circuit_breaker_type, enabled = %args.enabled, "parsed arguments for circuit breaker tool");

            match call_circuit_breaker_api(
                ctx.http_client,
                circuit_breaker_url,
                admin_secret,
                circuit_breaker_type,
                args.enabled,
            )
            .await
            {
                Ok(response_json) => Ok(response_json.to_string()),
                Err(e) => {
                    warn!(error = %e, "error calling circuit breaker api for '{}'", circuit_breaker_type);
                    Ok(json!({
                        "status": "error",
                        "message": "api_call_failed",
                        "details": e.to_string()
                    })
                    .to_string())
                }
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {}", e);
            warn!(args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(json!({
                "status": "error",
                "message": "argument_parsing_error",
                "details": err_msg
            })
            .to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_interaction::test_utils::create_test_handler_context; // You'll need to create/adjust this
    use crate::env::EnvConfig;
    use mockito::Matcher;
    use serde_json::json;
    use std::env;

    fn setup_env_vars(url: &str, secret: &str) {
        env::set_var("CIRCUIT_BREAKER_URL", url);
        env::set_var("CIRCUIT_BREAKER_ADMIN_SECRET", secret);
        // Force EnvConfig to reload, if it's statically initialized and cached.
        // This is a simplified approach; a real scenario might need a more robust way to update ENV_CONFIG for tests.
        unsafe {
           // this is a hack, in a real setup, you'd re-initialize or mock ENV_CONFIG
           // for testing purposes, we assume ENV_CONFIG picks up env vars at runtime or it's mocked.
           // for this example, we just set the vars and hope ENV_CONFIG in the test context picks them up.
        }
    }


    #[tokio::test]
    async fn test_set_adjustment_breaker_true_success() {
        let mut server = mockito::Server::new_async().await;
        let mock_url = server.url();
        let mock_secret = "test_secret_123";
        setup_env_vars(&mock_url, mock_secret);

        let handler_context = create_test_handler_context(None).await;

        let mock = server.mock("post", "/set-state")
            .match_header("x-admin-secret", mock_secret)
            .match_body(Matcher::Json(json!({
                "circuit_breaker_name": "adjustment",
                "enabled": true
            })))
            .with_status(200)
            .with_body(r#"{"status":"ok", "message":"adjustment breaker enabled"}"#)
            .create_async().await;

        let args = json!({
            "command_name": SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND,
            "enabled": true
        }).to_string();

        let result_str = execute_relay_circuit_breaker_tool(&handler_context, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();
        
        mock.assert_async().await;
        assert_eq!(result_json["status"], "ok");
        assert_eq!(result_json["message"], "adjustment breaker enabled");
    }

    #[tokio::test]
    async fn test_set_auction_breaker_false_success() {
        let mut server = mockito::Server::new_async().await;
        let mock_url = server.url();
        let mock_secret = "test_secret_auction";
        setup_env_vars(&mock_url, mock_secret);

        let handler_context = create_test_handler_context(None).await;

         let mock = server.mock("post", "/set-state")
            .match_header("x-admin-secret", mock_secret)
            .match_body(Matcher::Json(json!({
                "circuit_breaker_name": "auction",
                "enabled": false
            })))
            .with_status(200)
            .with_body(r#"{"custom_success_key":"all_good", "new_state": false}"#)
            .create_async().await;
        
        let args = json!({
            "command_name": SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            "enabled": false
        }).to_string();

        let result_str = execute_relay_circuit_breaker_tool(&handler_context, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        mock.assert_async().await;
        assert_eq!(result_json["custom_success_key"], "all_good");
        assert_eq!(result_json["new_state"], false);
    }
    
    #[tokio::test]
    async fn test_api_returns_error_status() {
        let mut server = mockito::Server::new_async().await;
        let mock_url = server.url();
        let mock_secret = "secret_for_error_test";
        setup_env_vars(&mock_url, mock_secret);
        let handler_context = create_test_handler_context(None).await;

        let mock = server.mock("post", "/set-state")
            .match_header("x-admin-secret", mock_secret)
            .match_body(Matcher::Json(json!({
                "circuit_breaker_name": "auction",
                "enabled": true
            })))
            .with_status(500)
            .with_body(r#"{"error_code": "internal_server_error", "details":"something broke"}"#)
            .create_async().await;

        let args = json!({
            "command_name": SET_AUCTION_CIRCUIT_BREAKER_COMMAND,
            "enabled": true
        }).to_string();
        
        let result_str = execute_relay_circuit_breaker_tool(&handler_context, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        mock.assert_async().await;
        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "api_call_failed");
        assert!(result_json["details"].as_str().unwrap().contains("circuit breaker api returned error status 500 internal server error"));
        assert!(result_json["details"].as_str().unwrap().contains(r#"{"error_code": "internal_server_error", "details":"something broke"}"#));
    }

    #[tokio::test]
    async fn test_missing_env_vars() {
        // Unset env vars for this test
        env::remove_var("CIRCUIT_BREAKER_URL");
        env::remove_var("CIRCUIT_BREAKER_ADMIN_SECRET");
        // Again, this assumes ENV_CONFIG would be reloaded or is instance-based in the test context
        
        let handler_context = create_test_handler_context(None).await; // this will use the fresh (unset) env vars
                                                                    // if ENV_CONFIG is a global static, this test might be flaky
                                                                    // without proper mocking of ENV_CONFIG itself.

        let args = json!({
            "command_name": SET_ADJUSTMENT_CIRCUIT_BREAKER_COMMAND,
            "enabled": true
        }).to_string();

        let result_str = execute_relay_circuit_breaker_tool(&handler_context, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "tool_configuration_error");
        assert_eq!(result_json["details"], "circuit_breaker_url and/or circuit_breaker_admin_secret environment variable(s) not set.");
    }
    
    #[tokio::test]
    async fn test_invalid_command_name_in_args() {
        // This scenario should ideally be caught by openaai based on the tool's enum definition,
        // but we test the tool's internal handling as a safeguard.
        setup_env_vars("http://localhost:1234", "some_secret"); // Env vars needed to pass initial check
        let handler_context = create_test_handler_context(None).await;
        
        let args = json!({
            "command_name": "set_non_existent_breaker",
            "enabled": true
        }).to_string();

        let result_str = execute_relay_circuit_breaker_tool(&handler_context, &args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_argument");
        assert_eq!(result_json["details"], "invalid command_name: 'set_non_existent_breaker'");
    }

    #[tokio::test]
    async fn test_malformed_arguments_json() {
        setup_env_vars("http://localhost:1234", "some_secret");
        let handler_context = create_test_handler_context(None).await;
        
        let malformed_args_json = r#"{"command_name": "set_adjustment_circuit_breaker", "enabled": tru"#; // malformed boolean

        let result_str = execute_relay_circuit_breaker_tool(&handler_context, malformed_args_json).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "argument_parsing_error");
        assert!(result_json["details"].as_str().unwrap().contains("failed to parse arguments json"));
    }

    // Note: For tests involving ENV_CONFIG, if it's a LazyLock or once_cell, 
    // its value gets cached on first access. Tests modifying env vars at runtime
    // might not see these changes reflected in ENV_CONFIG unless it's explicitly
    // designed to be re-evaluated or mocked. The `create_test_handler_context` 
    // or a direct `EnvConfig::from_env()` (if that's how it works) inside tests 
    // would be crucial. The `setup_env_vars` helper is a simplification.
}
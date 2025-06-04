use crate::env::ENV_CONFIG;
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{info, instrument, warn};

pub const CONVERSATION_ADMIN_TOOL_NAME: &str = "conversation_admin_tool";
const RESET_CONVERSATION_COMMAND_NAME: &str = "reset_conversation";
const ADMIN_CODE_PARAM_NAME: &str = "admin_code";
static EXPECTED_ADMIN_CODE: LazyLock<String> = LazyLock::new(|| {
    ENV_CONFIG
        .bot_admin_code
        .clone()
        .unwrap_or("vatu".to_string())
});

pub static CONVERSATION_ADMIN_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "command".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description("the command to execute. must be 'reset_conversation'.")
            .enum_string(&[RESET_CONVERSATION_COMMAND_NAME])
            .build(),
    );
    params_props.insert(
        ADMIN_CODE_PARAM_NAME.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the secret 4-character alphanumeric code required to authorize the reset.",
            )
            .build(),
    );

    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec![
            "command".to_string(),
            ADMIN_CODE_PARAM_NAME.to_string(),
        ]),
        additional_properties: false,
    };

    ToolDefinition::new(
        CONVERSATION_ADMIN_TOOL_NAME.to_string(),
        Some(
            "resets the current conversation with the ai, effectively clearing the chat history from the ai's perspective and starting a new conversation. this action requires a valid admin_code and the 'reset_conversation' command."
                .to_string(),
        ),
        Some(tool_params),
    )
});

fn reset_conversation_impl() -> String {
    // this special json signals to the calling code to reset the conversation.
    json!({
        "action": "reset_conversation",
        "status": "success",
        "message": "conversation reset has been initiated. the next message will start a new conversation."
    })
    .to_string()
}

#[instrument(skip(arguments_json_str))]
pub async fn execute_conversation_admin_command(arguments_json_str: &str) -> Result<String> {
    info!(args = %arguments_json_str, "executing conversation_admin_command tool");

    match serde_json::from_str::<HashMap<String, String>>(arguments_json_str) {
        Ok(args_map) => {
            let command_name = args_map.get("command");
            let admin_code_val = args_map.get(ADMIN_CODE_PARAM_NAME);

            if let (Some(cmd), Some(code)) = (command_name, admin_code_val) {
                if cmd == RESET_CONVERSATION_COMMAND_NAME && code == &*EXPECTED_ADMIN_CODE {
                    info!(command = %cmd, "admin command validated, proceeding with reset.");
                    Ok(reset_conversation_impl())
                } else if cmd != RESET_CONVERSATION_COMMAND_NAME {
                    let err_msg = format!(
                        "invalid command '{cmd}' specified. only '{RESET_CONVERSATION_COMMAND_NAME}' is supported."
                    );
                    warn!(args = %arguments_json_str, error = %err_msg);
                    Ok(json!({
                        "status": "error",
                        "message": err_msg
                    })
                    .to_string())
                } else {
                    // cmd is correct, so code must be wrong
                    let err_msg = "invalid admin_code provided.";
                    warn!(args = %arguments_json_str, error = err_msg);
                    Ok(json!({
                        "status": "error",
                        "message": err_msg
                    })
                    .to_string())
                }
            } else {
                let mut missing_params = Vec::new();
                if command_name.is_none() {
                    missing_params.push("'command'".to_string());
                }
                if admin_code_val.is_none() {
                    missing_params.push(format!("'{ADMIN_CODE_PARAM_NAME}'"));
                }
                let missing_params_str = missing_params.join(", ");
                let err_msg = format!("missing required argument(s): {missing_params_str}");
                warn!(args = %arguments_json_str, error = %err_msg);
                Ok(json!({
                    "status": "error",
                    "message": err_msg.as_str(), // use .as_str() for the json macro
                    "details": format!("expected json with 'command' and '{}' keys, got: {}", ADMIN_CODE_PARAM_NAME, arguments_json_str)
                }).to_string())
            }
        }
        Err(e) => {
            let parse_error_message =
                format!("failed to parse arguments json for admin command: {e}");
            warn!(args = %arguments_json_str, error = %parse_error_message, "json parsing error");
            Ok(json!({
                "status": "error",
                "message": "failed to parse tool arguments as json.".to_string(),
                "details": e.to_string()
            })
            .to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_conversation_admin_command_success() {
        let admin_code = ENV_CONFIG
            .bot_admin_code
            .clone()
            .unwrap_or_else(|| "vatu".to_string());
        let args = json!({
            "command": RESET_CONVERSATION_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: admin_code
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["action"], "reset_conversation");
        assert_eq!(result_json["status"], "success");
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_invalid_command() {
        let args = json!({
            "command": "wrong_command",
            ADMIN_CODE_PARAM_NAME: &*EXPECTED_ADMIN_CODE
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains("invalid command"));
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_invalid_code() {
        let args = json!({
            "command": RESET_CONVERSATION_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: "wrong_code"
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid admin_code provided.");
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_missing_command() {
        let args = json!({
            ADMIN_CODE_PARAM_NAME: &*EXPECTED_ADMIN_CODE
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert!(result_json["message"]
            .as_str()
            .unwrap()
            .contains("missing required argument(s): 'command'"));
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_missing_code() {
        let args = json!({
            "command": RESET_CONVERSATION_COMMAND_NAME
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert!(result_json["message"].as_str().unwrap().contains(&format!(
            "missing required argument(s): '{}'",
            ADMIN_CODE_PARAM_NAME
        )));
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_malformed_json() {
        let args = "not a json string";

        let result_str = execute_conversation_admin_command(args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(
            result_json["message"],
            "failed to parse tool arguments as json."
        );
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_empty_json_object() {
        let args = "{}";

        let result_str = execute_conversation_admin_command(args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        let msg = result_json["message"].as_str().unwrap();
        assert!(msg.contains("'command'"));
        assert!(msg.contains(&format!("'{}'", ADMIN_CODE_PARAM_NAME)));
    }
}

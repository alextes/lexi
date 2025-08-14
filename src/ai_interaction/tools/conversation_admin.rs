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
const SET_OPENAI_MODEL_COMMAND_NAME: &str = "set_openai_model";
const ADMIN_CODE_PARAM_NAME: &str = "admin_code";
const MODEL_ID_PARAM_NAME: &str = "model_id";

const GPT_5_MODEL_ID: &str = "gpt-5";
const GPT_5_MINI_MODEL_ID: &str = "gpt-5-mini";

// For enum_string in tool definition
static ALLOWED_MODEL_IDS_FOR_TOOL_DEF: &[&str] = &[GPT_5_MODEL_ID, GPT_5_MINI_MODEL_ID];

// For runtime checking
static ALLOWED_MODEL_IDS_VEC: LazyLock<Vec<String>> =
    LazyLock::new(|| vec![GPT_5_MODEL_ID.to_string(), GPT_5_MINI_MODEL_ID.to_string()]);

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
            .description(
                "the command to execute. can be 'reset_conversation' or 'set_openai_model'.",
            )
            .enum_string(&[
                RESET_CONVERSATION_COMMAND_NAME,
                SET_OPENAI_MODEL_COMMAND_NAME,
            ])
            .build(),
    );
    params_props.insert(
        ADMIN_CODE_PARAM_NAME.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the secret 4-character alphanumeric code required to authorize the command.",
            )
            .build(),
    );
    params_props.insert(
        MODEL_ID_PARAM_NAME.to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(
                "the identifier of the openai model to use. required if command is 'set_openai_model'.",
            )
            .enum_string(ALLOWED_MODEL_IDS_FOR_TOOL_DEF)
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
            "performs administrative actions on the conversation. can reset the conversation or set the openai model. requires a valid admin_code."
                .to_string(),
        ),
        Some(tool_params),
    )
});

fn reset_conversation_impl() -> String {
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
            let command_name_opt = args_map.get("command");
            let admin_code_val_opt = args_map.get(ADMIN_CODE_PARAM_NAME);

            if let Some(provided_admin_code) = admin_code_val_opt {
                if provided_admin_code != &*EXPECTED_ADMIN_CODE {
                    let err_msg = "invalid admin_code provided.";
                    warn!(args = %arguments_json_str, error = err_msg);
                    return Ok(json!({ "status": "error", "message": err_msg }).to_string());
                }
            } else {
                let err_msg = format!("missing required argument: '{ADMIN_CODE_PARAM_NAME}'");
                warn!(args = %arguments_json_str, error = %err_msg);
                return Ok(json!({
                    "status": "error",
                    "message": err_msg,
                    "details": format!("expected json with at least 'command' and '{}' keys.", ADMIN_CODE_PARAM_NAME)
                }).to_string());
            }

            if let Some(cmd) = command_name_opt {
                if cmd == RESET_CONVERSATION_COMMAND_NAME {
                    info!(command = %cmd, "admin command validated, proceeding with reset.");
                    Ok(reset_conversation_impl())
                } else if cmd == SET_OPENAI_MODEL_COMMAND_NAME {
                    info!(command = %cmd, "admin command validated, proceeding with set_openai_model.");
                    if let Some(model_id_val) = args_map.get(MODEL_ID_PARAM_NAME) {
                        if ALLOWED_MODEL_IDS_VEC.contains(model_id_val) {
                            info!(model_id = %model_id_val, "openai model will be set for future responses.");
                            Ok(json!({
                                "status": "success",
                                "action": "model_updated",
                                "new_model_id": model_id_val,
                                "message": format!("openai model for future responses set to '{}'.", model_id_val)
                            }).to_string())
                        } else {
                            let err_msg = format!(
                                "invalid '{}': '{}' for command '{}'. allowed values are: {:?}",
                                MODEL_ID_PARAM_NAME,
                                model_id_val,
                                SET_OPENAI_MODEL_COMMAND_NAME,
                                *ALLOWED_MODEL_IDS_VEC
                            );
                            warn!(args = %arguments_json_str, error = %err_msg);
                            Ok(json!({
                                "status": "error",
                                "message": "invalid_parameter_value",
                                "details": err_msg
                            })
                            .to_string())
                        }
                    } else {
                        let err_msg = format!(
                            "missing required argument: '{MODEL_ID_PARAM_NAME}' for command '{SET_OPENAI_MODEL_COMMAND_NAME}'"
                        );
                        warn!(args = %arguments_json_str, error = %err_msg);
                        Ok(json!({
                            "status": "error",
                            "message": "missing_required_argument_for_command",
                            "details": err_msg,
                        })
                        .to_string())
                    }
                } else {
                    let err_msg = format!(
                        "invalid command '{cmd}' specified. supported commands are: '{RESET_CONVERSATION_COMMAND_NAME}', '{SET_OPENAI_MODEL_COMMAND_NAME}'."
                    );
                    warn!(args = %arguments_json_str, error = %err_msg);
                    Ok(json!({ "status": "error", "message": err_msg }).to_string())
                }
            } else {
                let err_msg = "missing required argument: 'command'";
                warn!(args = %arguments_json_str, error = %err_msg);
                Ok(json!({
                    "status": "error",
                    "message": err_msg,
                    "details": format!("expected json with at least a 'command' key and a '{}' key.", ADMIN_CODE_PARAM_NAME)
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

    fn get_expected_admin_code() -> String {
        ENV_CONFIG
            .bot_admin_code
            .clone()
            .unwrap_or_else(|| "vatu".to_string())
    }

    #[tokio::test]
    async fn test_execute_conversation_admin_command_success() {
        let admin_code = get_expected_admin_code();
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
        assert_eq!(
            result_json["message"].as_str().unwrap(),
            "missing required argument: 'command'"
        );
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
        let msg = result_json["message"].as_str().unwrap();
        assert!(!msg.contains("'command'"));
        assert_eq!(
            msg,
            &format!("missing required argument: '{ADMIN_CODE_PARAM_NAME}'")
        );
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
        assert!(!msg.contains("'command'"));
        assert_eq!(
            msg,
            &format!("missing required argument: '{ADMIN_CODE_PARAM_NAME}'")
        );
    }

    #[tokio::test]
    async fn test_set_model_success() {
        let admin_code = get_expected_admin_code();
        let model_id = GPT_5_MINI_MODEL_ID;
        let args = json!({
            "command": SET_OPENAI_MODEL_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: admin_code,
            MODEL_ID_PARAM_NAME: model_id
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "success");
        assert_eq!(result_json["action"], "model_updated");
        assert_eq!(result_json["new_model_id"], model_id);
        assert!(result_json["message"].as_str().unwrap().contains(model_id));
    }

    #[tokio::test]
    async fn test_set_model_invalid_model_id() {
        let admin_code = get_expected_admin_code();
        let model_id = "gpt-invalid";
        let args = json!({
            "command": SET_OPENAI_MODEL_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: admin_code,
            MODEL_ID_PARAM_NAME: model_id
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_parameter_value");
        assert!(result_json["details"].as_str().unwrap().contains(model_id));
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(GPT_5_MODEL_ID));
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(GPT_5_MINI_MODEL_ID));
    }

    #[tokio::test]
    async fn test_set_model_missing_model_id_param() {
        let admin_code = get_expected_admin_code();
        let args = json!({
            "command": SET_OPENAI_MODEL_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: admin_code
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(
            result_json["message"],
            "missing_required_argument_for_command"
        );
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(MODEL_ID_PARAM_NAME));
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(SET_OPENAI_MODEL_COMMAND_NAME));
    }

    #[tokio::test]
    async fn test_set_model_invalid_admin_code() {
        let model_id = GPT_5_MODEL_ID;
        let args = json!({
            "command": SET_OPENAI_MODEL_COMMAND_NAME,
            ADMIN_CODE_PARAM_NAME: "wrong_admin_code",
            MODEL_ID_PARAM_NAME: model_id
        })
        .to_string();

        let result_str = execute_conversation_admin_command(&args).await.unwrap();
        let result_json: serde_json::Value = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid admin_code provided.");
    }
}

//! tool for retrieving embedded manual content
//!
//! manuals are stored in the `src/ai_interaction/tools/retrieve_manual/manuals` directory
//! and are named like `generate_proposer_reimbursement.md`
//!
//! the tool is used to retrieve the content of a manual for the assistant to use
//! when responding to user messages.
use crate::openai_api::{
    ToolDefinition, ToolFunctionParameterPropertyBuilder, ToolFunctionParameters,
};
use anyhow::Result;
use indoc::indoc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, instrument, warn};

pub const RETRIEVE_MANUAL_TOOL_NAME: &str = "retrieve_manual";
const GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME: &str = "generate_proposer_reimbursement";
const RELAY_DYNAMICS_MANUAL_NAME: &str = "relay_dynamics";

#[derive(Debug, serde::Deserialize)]
struct RetrieveManualArgs {
    manual_name: String,
}

pub static RETRIEVE_MANUAL_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "manual_name".to_string(),
        ToolFunctionParameterPropertyBuilder::new_string()
            .description(indoc!{"
                the name of the manual to retrieve. available manuals:
                'generate_proposer_reimbursement' (explains how to generate a proposer reimbursement).
                'relay_dynamics' (explains the ultra sound relay, and gives context on bids, headers, block builders, adjustments, optimistic simulation, node operators, and proposers).
            "}
            )
            .enum_string(&[GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME, RELAY_DYNAMICS_MANUAL_NAME])
            .build(),
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["manual_name".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        RETRIEVE_MANUAL_TOOL_NAME.to_string(),
        Some(
            "retrieves the content of a specified manual. this tool provides access to instructional documents for various tasks."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[instrument(fields(manual_name = %manual_name))]
fn get_manual_content(manual_name: &str) -> Result<String> {
    info!("retrieving manual '{}' using include_str!", manual_name);
    match manual_name {
        GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME => {
            Ok(include_str!("manuals/generate_proposer_reimbursement.md").to_string())
        }
        _ => {
            error!(%manual_name, "attempted to retrieve unknown manual");
            anyhow::bail!("unknown manual name: {}", manual_name)
        }
    }
}

#[instrument(skip(arguments_json_str), fields(tool_name = RETRIEVE_MANUAL_TOOL_NAME))]
pub async fn execute_retrieve_manual(arguments_json_str: &str) -> Result<String> {
    info!(
        args = %arguments_json_str,
        "executing retrieve_manual tool"
    );

    match serde_json::from_str::<RetrieveManualArgs>(arguments_json_str) {
        Ok(args) => match get_manual_content(&args.manual_name) {
            Ok(content) => Ok(json!({
                "manual_name": args.manual_name,
                "manual_content": content
            })
            .to_string()),
            Err(e) => {
                warn!(manual_name = %args.manual_name, error = %e, "error from get_manual_content in tool call");
                Ok(json!({
                    "status": "error",
                    "message": "invalid_manual_name",
                    "details": e.to_string()
                })
                .to_string())
            }
        },
        Err(e) => {
            let err_msg = format!("failed to parse arguments for retrieve_manual: {e}");
            warn!(args = %arguments_json_str, error = %err_msg);
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
    use serde_json::Value as JsonValue;

    #[tokio::test]
    async fn test_execute_retrieve_manual_success() {
        let manual_name = GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME;
        let args = json!({ "manual_name": manual_name }).to_string();

        let expected_content =
            include_str!("manuals/generate_proposer_reimbursement.md").to_string();

        let result_str = execute_retrieve_manual(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["manual_name"], manual_name);
        assert_eq!(result_json["manual_content"], expected_content);
    }

    #[tokio::test]
    async fn test_execute_retrieve_manual_invalid_name() {
        let manual_name = "non_existent_manual";
        let args = json!({ "manual_name": manual_name }).to_string();

        let result_str = execute_retrieve_manual(&args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "invalid_manual_name");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains(manual_name));
    }

    #[tokio::test]
    async fn test_execute_retrieve_manual_malformed_json_args() {
        let args = "{\"manual_name\": \"name\" சுகாதார"; // malformed json

        let result_str = execute_retrieve_manual(args).await.unwrap();
        let result_json: JsonValue = serde_json::from_str(&result_str).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "argument_parsing_error");
    }

    #[test]
    fn test_get_manual_content_success() {
        let manual_name = GENERATE_PROPOSER_REIMBURSEMENT_MANUAL_NAME;
        let expected_content =
            include_str!("manuals/generate_proposer_reimbursement.md").to_string();
        let result = get_manual_content(manual_name);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_content);
    }

    #[test]
    fn test_get_manual_content_not_found() {
        let manual_name = "surely_this_manual_does_not_exist_for_test";
        let result = get_manual_content(manual_name);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("unknown manual name"));
        assert!(err_msg.contains(manual_name));
    }
}

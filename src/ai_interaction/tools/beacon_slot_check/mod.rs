mod beacon_node;

use crate::openai_api::{ToolDefinition, ToolFunctionParameterProperty, ToolFunctionParameters};
use eyre::Result;
use reqwest::StatusCode;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{info, instrument, warn};

pub use beacon_node::BeaconNode;
pub use beacon_node::BeaconNodeHttp;
pub use beacon_node::MockBeaconNode;

pub const BEACON_SLOT_CHECK_TOOL_NAME: &str = "check_beacon_slot_missed";

#[derive(Debug)]
enum SlotStatus {
    NotMissed,
    Missed,
    Error(String),
}

impl SlotStatus {
    fn to_json_string(&self) -> String {
        match self {
            SlotStatus::NotMissed => json!({
                "status": "not_missed",
                "message": "a block header was found for the specified slot."
            })
            .to_string(),
            SlotStatus::Missed => json!({
                "status": "missed",
                "message": "the specified slot was missed (no block header found)."
            })
            .to_string(),
            SlotStatus::Error(e) => json!({
                "status": "error",
                "message": "an error occurred while checking the slot.",
                "details": e
            })
            .to_string(),
        }
    }
}

fn parse_slot_number_arg(arguments_json_str: &str) -> Result<u64, String> {
    match serde_json::from_str::<HashMap<String, JsonValue>>(arguments_json_str) {
        Ok(args_map) => {
            if let Some(slot_value) = args_map.get("slot_number") {
                if let Some(slot_number) = slot_value.as_u64() {
                    Ok(slot_number)
                } else {
                    Err("argument 'slot_number' was not a valid u64".to_string())
                }
            } else {
                Err("argument 'slot_number' missing".to_string())
            }
        }
        Err(e) => Err(format!("failed to parse arguments json: {e}")),
    }
}

#[instrument(skip(arguments_json_str, beacon_node))]
pub async fn execute_beacon_slot_check<BN: BeaconNode + ?Sized>(
    arguments_json_str: &str,
    beacon_node: &BN,
) -> Result<String> {
    info!(args = %arguments_json_str, "executing check_beacon_slot_missed tool");

    match parse_slot_number_arg(arguments_json_str) {
        Ok(slot_number) => {
            info!(slot = slot_number, "querying beacon node for slot status");
            match beacon_node.slot_status(slot_number).await {
                Ok(status_code) => {
                    let internal_status = if status_code == StatusCode::OK {
                        SlotStatus::NotMissed
                    } else if status_code == StatusCode::NOT_FOUND {
                        SlotStatus::Missed
                    } else {
                        SlotStatus::Error(format!(
                            "beacon node responded with http status: {}",
                            status_code
                        ))
                    };
                    Ok(internal_status.to_json_string())
                }
                Err(e) => {
                    warn!(slot = slot_number, error = %e, "beacon_node.slot_status call failed");
                    let internal_status = SlotStatus::Error(format!(
                        "failed to query beacon node for slot {}: {}",
                        slot_number, e
                    ));
                    Ok(internal_status.to_json_string())
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
            Ok(json!({
                "status": "error",
                "message": err_message_key,
                "details": err_detail
            })
            .to_string())
        }
    }
}

pub static BEACON_SLOT_CHECK_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "slot_number".to_string(),
        ToolFunctionParameterProperty {
            r#type: "integer".to_string(),
            description: Some("the beacon chain slot number to check.".to_string()),
            r#enum: None,
        },
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["slot_number".to_string()]),
        additional_properties: false,
    };
    ToolDefinition::new(
        BEACON_SLOT_CHECK_TOOL_NAME.to_string(),
        Some(
            "checks if a specific beacon chain slot was missed by querying a beacon node. returns if the slot was missed, not missed, or if an error occurred."
                .to_string(),
        ),
        Some(tool_params),
    )
});

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::eyre;
    use reqwest::StatusCode;
    use serde_json::{json, Value as JsonValue};

    #[test]
    fn test_parse_valid_slot_number() {
        let args_json = json!({ "slot_number": 12345 }).to_string();
        assert_eq!(parse_slot_number_arg(&args_json), Ok(12345u64));
    }

    #[test]
    fn test_parse_slot_number_not_u64() {
        let args_json = json!({ "slot_number": "not_a_number" }).to_string();
        match parse_slot_number_arg(&args_json) {
            Ok(_) => panic!("expected error for non-u64 slot_number"),
            Err(e) => assert!(e.contains("argument 'slot_number' was not a valid u64")),
        }
    }

    #[test]
    fn test_parse_slot_number_negative_integer() {
        let args_json = json!({ "slot_number": -10 }).to_string();
        match parse_slot_number_arg(&args_json) {
            Ok(_) => panic!("expected error for negative slot_number"),
            Err(e) => assert!(e.contains("argument 'slot_number' was not a valid u64")),
        }
    }

    #[test]
    fn test_parse_slot_number_missing() {
        let args_json = json!({ "other_arg": 123 }).to_string();
        match parse_slot_number_arg(&args_json) {
            Ok(_) => panic!("expected error for missing slot_number"),
            Err(e) => assert!(e.contains("argument 'slot_number' missing")),
        }
    }

    #[test]
    fn test_parse_malformed_json() {
        let args_json = "this is not json";
        match parse_slot_number_arg(args_json) {
            Ok(_) => panic!("expected error for malformed json"),
            Err(e) => assert!(e.contains("failed to parse arguments json")),
        }
    }

    #[test]
    fn test_parse_empty_json_object() {
        let args_json = json!({}).to_string();
        match parse_slot_number_arg(&args_json) {
            Ok(_) => panic!("expected error for empty json object (missing slot_number)"),
            Err(e) => assert!(e.contains("argument 'slot_number' missing")),
        }
    }

    #[tokio::test]
    async fn test_execute_slot_check_not_missed() {
        let slot_number = 12345u64;
        let mut mock_bn = MockBeaconNode::new();
        mock_bn
            .expect_slot_status()
            .withf(move |&s| s == slot_number)
            .times(1)
            .returning(|_| Ok(StatusCode::OK));

        let args = json!({ "slot_number": slot_number }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "not_missed",
            "message": "a block header was found for the specified slot."
        })
        .to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_slot_check_missed() {
        let slot_number = 54321u64;
        let mut mock_bn = MockBeaconNode::new();
        mock_bn
            .expect_slot_status()
            .withf(move |&s| s == slot_number)
            .times(1)
            .returning(|_| Ok(StatusCode::NOT_FOUND));

        let args = json!({ "slot_number": slot_number }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "missed",
            "message": "the specified slot was missed (no block header found)."
        })
        .to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_slot_check_other_status_code() {
        let slot_number = 67890u64;
        let mut mock_bn = MockBeaconNode::new();
        mock_bn
            .expect_slot_status()
            .withf(move |&s| s == slot_number)
            .times(1)
            .returning(|_| Ok(StatusCode::INTERNAL_SERVER_ERROR));

        let args = json!({ "slot_number": slot_number }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "error",
            "message": "an error occurred while checking the slot.",
            "details": "beacon node responded with http status: 500 internal server error"
        })
        .to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_slot_check_beacon_node_service_error() {
        let slot_number = 91011u64;
        let error_msg = "network connection failed".to_string();
        let mut mock_bn = MockBeaconNode::new();
        mock_bn
            .expect_slot_status()
            .withf(move |&s| s == slot_number)
            .times(1)
            .returning({
                let em = error_msg.clone();
                move |_| Err(eyre!(em.clone()))
            });

        let args = json!({ "slot_number": slot_number }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "error",
            "message": "an error occurred while checking the slot.",
            "details": format!("failed to query beacon node for slot {}: {}", slot_number, error_msg)
        }).to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_invalid_slot_arg_handled() {
        let mut mock_bn = MockBeaconNode::new();
        mock_bn.expect_slot_status().times(0);

        let args = json!({ "slot_number": "not_a_number" }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "error",
            "message": "invalid_argument",
            "details": "argument 'slot_number' was not a valid u64"
        })
        .to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_missing_slot_arg_handled() {
        let mut mock_bn = MockBeaconNode::new();
        mock_bn.expect_slot_status().times(0);

        let args = json!({ "other_arg": 123 }).to_string();
        let result = execute_beacon_slot_check(&args, &mock_bn).await.unwrap();
        let expected_json = json!({
            "status": "error",
            "message": "invalid_argument",
            "details": "argument 'slot_number' missing"
        })
        .to_string();
        assert_eq!(result, expected_json);
    }

    #[tokio::test]
    async fn test_execute_malformed_json_arg_handled() {
        let mut mock_bn = MockBeaconNode::new();
        mock_bn.expect_slot_status().times(0);

        let malformed_args = "not json";
        let result = execute_beacon_slot_check(malformed_args, &mock_bn)
            .await
            .unwrap();
        let result_json: JsonValue = serde_json::from_str(&result).unwrap();

        assert_eq!(result_json["status"], "error");
        assert_eq!(result_json["message"], "argument_parsing_error");
        assert!(result_json["details"]
            .as_str()
            .unwrap()
            .contains("failed to parse arguments json"));
    }
}

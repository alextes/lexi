use crate::env::ENV_CONFIG;
use crate::message_processor::HandlerContext;
use crate::openai_api::{ToolDefinition, ToolFunctionParameterProperty, ToolFunctionParameters};
use eyre::{eyre, Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const BEACON_SLOT_CHECK_TOOL_NAME: &str = "check_beacon_slot_missed";

#[derive(Debug)]
pub enum SlotStatus {
    NotMissed,     // Slot had a block header
    Missed,        // Slot was missed (404)
    Error(String), // Error during check
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

async fn fetch_beacon_header(
    http_client: &ReqwestClient,
    beacon_node_url: &str,
    slot: u64,
) -> Result<SlotStatus> {
    let request_url = format!("{beacon_node_url}/eth/v1/beacon/headers/{slot}");
    info!(url = %request_url, "fetching beacon header for slot");

    match http_client.get(&request_url).send().await {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                // we don't need to parse the body, just knowing it's 200 is enough
                info!(slot = slot, "beacon slot not missed (200 ok)");
                Ok(SlotStatus::NotMissed)
            } else if status == reqwest::StatusCode::NOT_FOUND {
                info!(slot = slot, "beacon slot missed (404 not found)");
                Ok(SlotStatus::Missed)
            } else {
                let err_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "failed to get error body".to_string());
                warn!(slot = slot, status = %status, body = %err_text, "error response from beacon node");
                Ok(SlotStatus::Error(format!(
                    "beacon node returned status {status} - {err_text}"
                )))
            }
        }
        Err(e) => {
            error!(slot = slot, error = %e, "failed to send request to beacon node");
            Err(eyre!(e)).context("failed to connect to beacon node")
        }
    }
}

pub async fn execute_beacon_slot_check(
    ctx: &HandlerContext<'_>,
    arguments_json_str: &str, // The arguments string from OutputFunctionCall
) -> Result<String> {
    // Returns a JSON string (SlotStatus or error)
    info!(args = %arguments_json_str, "executing check_beacon_slot_missed tool");

    let beacon_node_url = if let Some(url) = ENV_CONFIG.beacon_url.as_ref() {
        url.as_str()
    } else {
        let err_msg = "BEACON_URL environment variable not set. cannot check beacon slot.";
        error!(err_msg);
        return Ok(SlotStatus::Error(err_msg.to_string()).to_json_string());
    };

    match serde_json::from_str::<HashMap<String, JsonValue>>(arguments_json_str) {
        Ok(args_map) => {
            if let Some(slot_value) = args_map.get("slot_number") {
                if let Some(slot_number) = slot_value.as_u64() {
                    info!(slot = slot_number, "checking beacon slot from ai request");
                    match fetch_beacon_header(ctx.http_client, beacon_node_url, slot_number).await {
                        Ok(status) => Ok(status.to_json_string()),
                        Err(e) => {
                            warn!(slot = slot_number, error = %e, "error fetching beacon header");
                            Ok(
                                SlotStatus::Error(format!(
                                    "error checking slot {slot_number}: {e}"
                                ))
                                .to_json_string(),
                            )
                        }
                    }
                } else {
                    let err_msg = "argument 'slot_number' was not a valid u64";
                    warn!(args = %arguments_json_str, err_msg);
                    Ok(SlotStatus::Error(err_msg.to_string()).to_json_string())
                }
            } else {
                let err_msg = "argument 'slot_number' missing";
                warn!(args = %arguments_json_str, err_msg);
                Ok(SlotStatus::Error(err_msg.to_string()).to_json_string())
            }
        }
        Err(e) => {
            let err_msg = format!("failed to parse arguments json: {e}");
            warn!(args = %arguments_json_str, error = %e, "json parsing error for tool arguments");
            Ok(SlotStatus::Error(err_msg).to_json_string())
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
    use reqwest::Client as ReqwestClient;

    #[tokio::test]
    async fn test_fetch_beacon_header_slot_not_missed() {
        // slot 11805092 is an example of a slot that is not missed
        let slot_not_missed = 11805092;
        let mut server = mockito::Server::new_async().await;
        let http_client = ReqwestClient::new();

        let mock_path = format!("/eth/v1/beacon/headers/{slot_not_missed}");
        let mock = server
            .mock("get", mock_path.as_str()) // mockito method matching is case-insensitive
            .with_status(200)
            .create_async()
            .await;

        let result = fetch_beacon_header(&http_client, &server.url(), slot_not_missed).await;

        mock.assert_async().await; // verify the mock was called
        match result {
            Ok(SlotStatus::NotMissed) => { /* test passed */ }
            _ => panic!(
                "expected slot {slot_not_missed} to be slotstatus::notmissed, got {result:?}"
            ),
        }
    }

    #[tokio::test]
    async fn test_fetch_beacon_header_slot_missed() {
        // slot 11805091 is an example of a slot that is missed
        let slot_missed = 11805091;
        let mut server = mockito::Server::new_async().await;
        let http_client = ReqwestClient::new();

        let mock_path = format!("/eth/v1/beacon/headers/{slot_missed}");
        let mock = server
            .mock("get", mock_path.as_str())
            .with_status(404) // 404 indicates a missed slot
            .create_async()
            .await;

        let result = fetch_beacon_header(&http_client, &server.url(), slot_missed).await;

        mock.assert_async().await; // verify the mock was called
        match result {
            Ok(SlotStatus::Missed) => { /* test passed */ }
            _ => panic!("expected slot {slot_missed} to be slotstatus::missed, got {result:?}"),
        }
    }
}

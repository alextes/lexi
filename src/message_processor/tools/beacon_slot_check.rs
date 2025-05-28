use super::common::handle_tool_call_step2_openai_response;
use super::common::ToolStep2Context;
use crate::env::ENV_CONFIG;
use crate::message_processor::HandlerContext;
use crate::openai_api::{
    InputItem, OutputFunctionCall, ToolDefinition, ToolFunctionParameterProperty,
    ToolFunctionParameters,
};
use eyre::{eyre, Context, Result};
use reqwest::Client as ReqwestClient;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{error, info, warn};

// placeholder - this should be configurable later
// const BEACON_NODE_URL: &str = "http://localhost:5052"; // Removed const
pub const BEACON_SLOT_CHECK_TOOL_NAME: &str = "check_beacon_slot_missed";

#[derive(Debug)]
pub enum SlotStatus {
    NotMissed,     // Slot had a block
    Missed,        // Slot was missed (404)
    Error(String), // Error during check
}

impl SlotStatus {
    fn to_json_string(&self) -> String {
        match self {
            SlotStatus::NotMissed => json!({
                "status": "not_missed",
                "message": "a block was found for the specified slot."
            })
            .to_string(),
            SlotStatus::Missed => json!({
                "status": "missed",
                "message": "the specified slot was missed (no block found)."
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
    let request_url = format!("{}/eth/v1/beacon/headers/{}", beacon_node_url, slot);
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
                    "beacon node returned status {} - {}",
                    status, err_text
                )))
            }
        }
        Err(e) => {
            error!(slot = slot, error = %e, "failed to send request to beacon node");
            Err(eyre!(e)).context("failed to connect to beacon node")
        }
    }
}

pub static BEACON_SLOT_CHECK_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    let mut params_props = HashMap::new();
    params_props.insert(
        "slot_number".to_string(),
        ToolFunctionParameterProperty {
            r#type: "integer".to_string(), // Changed to integer for slot number
            description: Some("the beacon chain slot number to check.".to_string()),
            r#enum: Vec::new(),
        },
    );
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: params_props,
        required: Some(vec!["slot_number".to_string()]),
        additional_properties: false,
    };
    ToolDefinition {
        r#type: "function".to_string(),
        name: BEACON_SLOT_CHECK_TOOL_NAME.to_string(),
        description: Some(
            "checks if a specific beacon chain slot was missed by querying a beacon node. returns if the slot was missed, not missed, or if an error occurred."
                .to_string(),
        ),
        parameters: Some(tool_params),
        strict: Some(true),
    }
});

pub async fn handle_beacon_slot_check_tool_call(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64,
    function_call: &OutputFunctionCall,
    original_input_items: Vec<InputItem>,
    initial_api_response_id: &str,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    info!(chat_id = telegram_chat_id, args = %function_call.arguments, "received call for {}", function_call.name);

    let beacon_node_url = ENV_CONFIG
        .beacon_url
        .as_ref()
        .expect("BEACON_URL must be set to use the beacon slot check tool");

    match serde_json::from_str::<HashMap<String, JsonValue>>(&function_call.arguments) {
        Ok(args_map) => {
            if let Some(slot_value) = args_map.get("slot_number") {
                if let Some(slot_number) = slot_value.as_u64() {
                    info!(slot = slot_number, "checking beacon slot from ai request");

                    let slot_status_result = fetch_beacon_header(
                        ctx.http_client,
                        beacon_node_url, // Now a &str from .as_ref().expect()
                        slot_number,
                    )
                    .await;

                    let result_json_str = match slot_status_result {
                        Ok(status) => status.to_json_string(),
                        Err(e) => SlotStatus::Error(e.to_string()).to_json_string(),
                    };

                    let step2_ctx = ToolStep2Context {
                        telegram_chat_id,
                        function_name: &function_call.name,
                        function_id: &function_call.id,
                        function_call_id: &function_call.call_id,
                        function_arguments: &function_call.arguments,
                        original_input_items,
                        initial_api_response_id,
                        available_tools,
                        instructions,
                        tool_output_json_string: result_json_str,
                    };

                    handle_tool_call_step2_openai_response(ctx, step2_ctx).await
                } else {
                    warn!(
                        chat_id = telegram_chat_id,
                        "'slot_number' was not a valid u64 in {} args", function_call.name
                    );
                    Err(eyre!(
                        "argument 'slot_number' was not a valid u64 for tool {}",
                        function_call.name
                    ))
                }
            } else {
                warn!(
                    chat_id = telegram_chat_id,
                    "'slot_number' missing in {} args", function_call.name
                );
                Err(eyre!(
                    "argument 'slot_number' missing for tool {}",
                    function_call.name
                ))
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to parse args for {}", function_call.name);
            Err(e).context(format!(
                "failed to parse args for tool {}",
                function_call.name
            ))
        }
    }
}

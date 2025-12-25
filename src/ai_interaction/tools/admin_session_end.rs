use crate::openai_api::ToolDefinition;
use anyhow::Result;
use serde_json::json;
use std::sync::LazyLock;
use tracing::instrument;

pub const ADMIN_SESSION_END_TOOL_NAME: &str = "end_admin_session";

pub static ADMIN_SESSION_END_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    ToolDefinition::new(
        ADMIN_SESSION_END_TOOL_NAME.to_string(),
        Some(
            "ends the current admin session and restores normal conversation context.".to_string(),
        ),
        None, // ToolDefinition::new handles empty parameters for strict mode
    )
});

#[instrument(skip(arguments_json_str))]
pub async fn execute_admin_session_end_tool(arguments_json_str: &str) -> Result<String> {
    let _ = arguments_json_str;
    Ok(json!({
        "status": "success",
        "action": "end_admin_session",
        "message": "admin session ended; returning to normal mode."
    })
    .to_string())
}

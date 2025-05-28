use crate::message_processor::HandlerContext;
use crate::openai_api::{ToolDefinition, ToolFunctionParameters};
use eyre::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const MEVDB_SCHEMA_TOOL_NAME: &str = "get_mevdb_schema";
const MEVDB_SCHEMA_FILE_PATH: &str = "src/message_processor/tools/mevdb_schema.txt";

pub static MEVDB_SCHEMA_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    // Define empty parameters object as required by OpenAI for no-arg functions
    let tool_params = ToolFunctionParameters {
        r#type: "object".to_string(),
        properties: HashMap::new(),   // No properties
        required: Some(Vec::new()),   // Changed from None to Some(Vec::new())
        additional_properties: false, // Must be false
    };
    ToolDefinition {
        r#type: "function".to_string(),
        name: MEVDB_SCHEMA_TOOL_NAME.to_string(),
        description: Some(
            "retrieves the schema definition for the mev (maximal extractable value) database. this can be used to understand table structures before forming a query for the 'execute_mevdb_query' tool."
                .to_string(),
        ),
        parameters: Some(tool_params),
        strict: Some(true),
    }
});

fn get_mevdb_schema_from_file() -> Result<String> {
    info!(
        "attempting to read mevdb schema from file: {}",
        MEVDB_SCHEMA_FILE_PATH
    );
    fs::read_to_string(Path::new(MEVDB_SCHEMA_FILE_PATH)).map_err(|e| {
        error!(error = %e, path = MEVDB_SCHEMA_FILE_PATH, "failed to read mevdb schema file");
        eyre::eyre!("failed to read mevdb schema file: {}", e)
    })
}

// --- new simplified tool execution function ---
pub async fn execute_get_mevdb_schema(
    _ctx: &HandlerContext<'_>, // Context might be needed for future enhancements (e.g. dynamic schema)
    telegram_chat_id: i64,     // For logging
) -> Result<String> {
    // Returns the schema string or an error string
    info!(
        chat_id = telegram_chat_id,
        "executing get_mevdb_schema tool"
    );

    match get_mevdb_schema_from_file() {
        Ok(s) => Ok(s),
        Err(e) => {
            warn!(chat_id = telegram_chat_id, error = %e, "error getting mevdb schema for tool call");
            // Return a JSON string indicating the error, as OpenAI expects a JSON string from tools
            Ok(format!(
                "{{\"error\": \"failed_to_read_schema\", \"details\": \"{}\"}}",
                e.to_string().replace('"', "\\\"") // Basic JSON escaping for the error message
            ))
        }
    }
}

use super::common::{handle_tool_call_step2_openai_response, ToolStep2Context};
use crate::message_processor::HandlerContext;
use crate::openai_api::{InputItem, OutputFunctionCall, ToolDefinition};
use eyre::Result;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use tracing::{error, info, warn};

pub const MEVDB_SCHEMA_TOOL_NAME: &str = "get_mevdb_schema";
const MEVDB_SCHEMA_FILE_PATH: &str = "src/message_processor/tools/mevdb_schema.txt";

// --- tool definition ---
pub static MEVDB_SCHEMA_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| {
    ToolDefinition {
        r#type: "function".to_string(),
        name: MEVDB_SCHEMA_TOOL_NAME.to_string(),
        description: Some(
            "retrieves the schema definition for the mev (maximal extractable value) database. this can be used to understand table structures before forming a query for the 'execute_mevdb_query' tool."
                .to_string(),
        ),
        parameters: None, // No parameters for this tool
        strict: Some(true),
    }
});

// --- tool execution logic ---
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

// --- tool call handling ---
pub async fn handle_mevdb_schema_tool_call(
    ctx: &HandlerContext<'_>,
    telegram_chat_id: i64,
    function_call: &OutputFunctionCall,
    original_input_items: Vec<InputItem>,
    initial_api_response_id: &str,
    available_tools: Vec<ToolDefinition>,
    instructions: &str,
) -> Result<(String, String)> {
    info!(
        chat_id = telegram_chat_id,
        "received call for {}", function_call.name
    );

    let schema_result = get_mevdb_schema_from_file();
    let schema_string = match schema_result {
        Ok(s) => s,
        Err(e) => {
            warn!(chat_id = telegram_chat_id, error = %e, "error getting mevdb schema for tool call");
            format!("error retrieving mevdb schema: {}", e)
        }
    };

    let step2_ctx = ToolStep2Context {
        telegram_chat_id,
        function_name: &function_call.name,
        function_id: &function_call.id,
        function_call_id: &function_call.call_id,
        function_arguments: &function_call.arguments, // Will be empty or null for this tool
        original_input_items,
        initial_api_response_id,
        available_tools,
        instructions,
        tool_output_json_string: schema_string, // Send the schema (or error) as a string
    };

    handle_tool_call_step2_openai_response(ctx, step2_ctx).await
}

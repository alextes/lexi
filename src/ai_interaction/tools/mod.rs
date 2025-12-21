pub mod beacon_slot_check;
pub mod conversation_admin;
pub mod db_query;
pub mod db_schema;
pub mod relay_circuit_breaker;
pub mod retrieve_manual;

use crate::openai_api::ApiToolType;

pub const ADMIN_ONLY_TOOL_NAMES: &[&str] = &[
    conversation_admin::CONVERSATION_ADMIN_TOOL_NAME,
    relay_circuit_breaker::RELAY_CIRCUIT_BREAKER_TOOL_NAME,
];

pub fn is_admin_only_tool(tool: &ApiToolType) -> bool {
    match tool {
        ApiToolType::Function(f) => ADMIN_ONLY_TOOL_NAMES.contains(&f.name.as_str()),
        _ => false,
    }
}

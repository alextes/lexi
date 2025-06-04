use crate::openai_api::ToolDefinition;
use std::sync::LazyLock;

// as per openai documentation, the web_search tool is enabled by providing a tool with type "web_search".
// other fields like name, description, and parameters are not specified for this type,
// but `name` is a required field in our `ToolDefinition` struct.
pub const WEB_SEARCH_TOOL_NAME: &str = "web_search_enabled"; // name is mandatory in struct

pub static WEB_SEARCH_TOOL: LazyLock<ToolDefinition> = LazyLock::new(|| ToolDefinition {
    r#type: "web_search".to_string(),
    name: WEB_SEARCH_TOOL_NAME.to_string(),
    description: None,
    parameters: None,
    strict: None, // not applicable for non-function tools
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_search_tool_definition() {
        assert_eq!(WEB_SEARCH_TOOL.r#type, "web_search");
        assert_eq!(WEB_SEARCH_TOOL.name, WEB_SEARCH_TOOL_NAME);
        assert!(WEB_SEARCH_TOOL.description.is_none());
        assert!(WEB_SEARCH_TOOL.parameters.is_none());
        assert!(WEB_SEARCH_TOOL.strict.is_none());
    }
}

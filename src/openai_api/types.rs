use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "required")]
    Required,
    #[serde(rename = "none")]
    None,
}

#[derive(Serialize, Debug, Clone)]
pub struct CallResponsesApiOptionalArgs<'a> {
    pub model_id: &'a str,
    pub previous_response_id: Option<&'a str>,
    pub tools: Option<Vec<ApiToolType>>,
    pub tool_choice: Option<ToolChoice>,
    pub instructions: Option<&'a str>,
    pub temperature: Option<f64>,
    pub store: Option<bool>,
    // new optional knobs exposed by the responses api
    pub verbosity: Option<&'a str>, // e.g. "low" | "medium" | "high"
    pub reasoning_effort: Option<&'a str>, // e.g. "low" | "medium" | "high"
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolFunctionParameterProperty {
    pub r#type: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#enum: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ToolFunctionParameterPropertyBuilder {
    r#type: String,
    description: Option<String>,
    r#enum: Option<Vec<String>>,
}

impl ToolFunctionParameterPropertyBuilder {
    fn new(r#type: &str) -> Self {
        ToolFunctionParameterPropertyBuilder {
            r#type: r#type.to_string(),
            description: None,
            r#enum: None,
        }
    }

    #[must_use]
    pub fn new_string() -> Self {
        Self::new("string")
    }

    #[must_use]
    pub fn new_boolean() -> Self {
        Self::new("boolean")
    }

    #[must_use]
    pub fn integer() -> Self {
        Self::new("integer")
    }

    #[must_use]
    pub fn enum_string(mut self, values: &[&str]) -> Self {
        self.r#enum = Some(values.iter().map(|s| (*s).to_string()).collect());
        self
    }

    #[must_use]
    pub fn description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    #[must_use]
    pub fn build(self) -> ToolFunctionParameterProperty {
        ToolFunctionParameterProperty {
            r#type: self.r#type,
            description: self.description,
            r#enum: self.r#enum,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolFunctionParameters {
    pub r#type: String,
    pub properties: HashMap<String, ToolFunctionParameterProperty>,
    pub required: Option<Vec<String>>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: bool,
}

// This struct is now specifically for "function" tools.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolDefinition {
    pub r#type: String, // Will always be "function"
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<ToolFunctionParameters>,
    pub strict: Option<bool>,
}

impl ToolDefinition {
    #[must_use]
    pub fn new(
        name: String,
        description: Option<String>,
        parameters: Option<ToolFunctionParameters>,
    ) -> Self {
        // OpenAI strict mode requires all tools to have a parameters schema with
        // additionalProperties: false, even for tools that take no arguments.
        // Default to an empty schema if none provided.
        let mut updated_parameters = parameters.unwrap_or_else(|| ToolFunctionParameters {
            r#type: "object".to_string(),
            properties: HashMap::new(),
            required: Some(vec![]),
            additional_properties: false,
        });

        // for strict tools, required must include every key in properties per api docs
        let required_param_names: Vec<String> =
            updated_parameters.properties.keys().cloned().collect();
        updated_parameters.required = Some(required_param_names);
        updated_parameters.additional_properties = false;

        ToolDefinition {
            r#type: "function".to_string(),
            name,
            description,
            parameters: Some(updated_parameters),
            strict: Some(true),
        }
    }
}

// New struct for Web Search tool configuration
#[derive(Serialize, Debug, Clone)]
pub struct WebSearchToolConfig {
    pub r#type: String, // Will always be "web_search_preview"
                        // Potentially: pub user_location: Option<UserLocationConfig>,
                        // Potentially: pub search_context_size: Option<SearchContextSizeConfig>,
}

impl Default for WebSearchToolConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchToolConfig {
    pub fn new() -> Self {
        WebSearchToolConfig {
            r#type: "web_search_preview".to_string(),
        }
    }
}

// Enum to wrap different tool types for the API
#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum ApiToolType {
    Function(ToolDefinition),
    WebSearch(WebSearchToolConfig),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum InputItem {
    Text(String),
    Message(InputMessageObject),
    FunctionCallOutput(FunctionCallOutputItem),
    FunctionCallEcho(FunctionCallEchoItem),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InputMessageObject {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCallOutputItem {
    pub r#type: String,
    pub call_id: String,
    pub output: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCallEchoItem {
    pub r#type: String, // will be "function_call"
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OutputTextContent {
    pub r#type: String,
    pub text: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OutputFunctionCall {
    pub r#type: String,
    pub id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OutputReasoning {
    pub r#type: String, // should be "reasoning"
    pub id: String,
    // using jsonvalue for summary as its structure might be flexible or is currently unknown
    pub summary: Vec<OutputTextContent>,
}

// New struct for the web_search_call output item
#[derive(Deserialize, Debug, Clone)]
pub struct OutputWebSearchCall {
    pub r#type: String, // will be "web_search_call"
    pub id: String,
    pub status: String, // e.g., "completed"
                        // According to OpenAI docs, it only has id, type, status.
                        // If other fields like 'name' or 'arguments' appear for web_search_call,
                        // they would need to be added here.
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum OutputItem {
    Message(OutputMessage),
    FunctionCall(OutputFunctionCall),
    Reasoning(OutputReasoning),
    WebSearchCall(OutputWebSearchCall), // Added new variant
}

#[derive(Deserialize, Debug, Clone)]
pub struct OutputMessage {
    pub id: String,
    pub r#type: String,
    pub status: String,
    pub role: String,
    pub content: Vec<OutputTextContent>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OpenAiApiResponse {
    pub id: String,
    pub object: String,
    pub created_at: i64,
    pub model: String,
    pub status: String,
    pub output: Vec<OutputItem>,
    pub background: Option<bool>,
    pub error: Option<JsonValue>,
    pub incomplete_details: Option<JsonValue>,
    pub instructions: Option<String>,
    pub max_output_tokens: Option<i64>,
    pub parallel_tool_calls: Option<bool>,
    pub previous_response_id: Option<String>,
    pub reasoning: Option<JsonValue>,
    pub service_tier: Option<String>,
    pub store: Option<bool>,
    pub temperature: Option<f64>,
    pub text: Option<JsonValue>,
    pub tool_choice: Option<JsonValue>,
    pub usage: Option<JsonValue>,
    pub user: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_new_sets_required_to_all_keys_for_strict() {
        let mut props = std::collections::HashMap::new();
        props.insert(
            "a".to_string(),
            ToolFunctionParameterPropertyBuilder::new_string().build(),
        );
        props.insert(
            "b".to_string(),
            ToolFunctionParameterPropertyBuilder::integer().build(),
        );
        let params = ToolFunctionParameters {
            r#type: "object".to_string(),
            properties: props.clone(),
            required: Some(vec!["a".to_string()]), // intentionally incomplete
            additional_properties: true,           // will be overridden
        };

        let def = ToolDefinition::new("t".to_string(), Some("d".to_string()), Some(params));

        assert_eq!(def.r#type, "function");
        assert_eq!(def.strict, Some(true));
        let built_params = def.parameters.expect("parameters expected");
        assert_eq!(built_params.r#type, "object");
        // required must be all keys, not just ["a"]
        let mut expected: Vec<String> = props.keys().cloned().collect();
        expected.sort();
        let mut actual = built_params.required.unwrap();
        actual.sort();
        assert_eq!(actual, expected);
        assert!(!built_params.additional_properties);
    }
}

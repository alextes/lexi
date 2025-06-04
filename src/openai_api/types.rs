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
    pub tools: Option<Vec<ToolDefinition>>,
    pub tool_choice: Option<ToolChoice>,
    pub instructions: Option<&'a str>,
    pub temperature: Option<f64>,
    pub store: Option<bool>,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolDefinition {
    pub r#type: String,
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
        let mut updated_parameters = parameters;
        if let Some(ref mut params) = updated_parameters {
            let required_param_names: Vec<String> = params.properties.keys().cloned().collect();
            params.required = Some(required_param_names);
            params.additional_properties = false;
        }

        ToolDefinition {
            r#type: "function".to_string(),
            name,
            description,
            parameters: updated_parameters,
            strict: Some(true),
        }
    }
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
#[serde(untagged)]
pub enum OutputItem {
    Message(OutputMessage),
    FunctionCall(OutputFunctionCall),
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

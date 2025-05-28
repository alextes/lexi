use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

// --- structs for api request payloads ---

#[derive(Serialize, Debug, Clone)]
pub struct CallResponsesApiOptionalArgs<'a> {
    pub model_id: &'a str,
    pub previous_response_id: Option<&'a str>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub tool_choice: Option<JsonValue>,
    pub instructions: Option<&'a str>,
    pub temperature: Option<f64>,
    pub store: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolFunctionParameterProperty {
    pub r#type: String,
    pub description: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub r#enum: Vec<String>,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum InputItem {
    Text(String),
    Message(InputMessageObject),
    FunctionCallOutput(FunctionCallOutputItem),
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

// --- structs for deserializing api responses ---

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

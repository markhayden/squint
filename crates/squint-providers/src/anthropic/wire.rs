//! The Messages API subset the adapter reads (SPEC "Protocol requirements"): the response
//! envelope, usage and the error body. Requests are built as JSON values in `super`, since every
//! field is a plain mapping of the harness's view; content blocks the adapter does not interpret
//! stay raw values so they can be echoed byte for byte.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
    /// `None` when the answer carried no usage block.
    #[serde(default)]
    pub usage: Option<Usage>,
    /// Present when the API dropped or let through replayed thinking blocks.
    #[serde(default)]
    pub input_transformations: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StopDetails {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

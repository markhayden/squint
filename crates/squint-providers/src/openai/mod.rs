//! The OpenAI Responses adapter (SPEC "Protocol requirements"). Function tools with the strict wire
//! schema; `function_call` items answered by `function_call_output` items matched by `call_id`;
//! every output item of a model turn (reasoning items included) echoed exactly as received;
//! `store = false` on every request; an `incomplete` status is non-replayable; the final turn by
//! the configured mechanism: a developer message (refuse) or a forced `tool_choice` (forced).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use squint_core::config::Config;
use squint_core::image::{ImagePlan, Prepared, base64_encode};
use squint_core::ledger::Pricing;
use squint_core::provider::{
    ContentBlock, MessageRole, Provider, SendFuture, StopReason, ToolCall, ToolDefinition,
    TurnRequest, TurnResponse, Usage,
};
use squint_core::report::Recipient;
use squint_core::run::SUBMIT as SUBMIT_VERDICT;
use squint_core::{Error, Result};

use crate::anthropic::{FINAL_TURN_CONTROL, FinalTurn};
use crate::catalog::{Model, Profile};
use crate::gemini::host_of;
use crate::http::{Headers, Transport};
use crate::prepare::{self, Preparer};
use crate::privacy::Credential;
use crate::replay::Volatile;
use crate::wire::{Adapter, check_tool_set};

pub const ADAPTER_ID: &str = "openai-responses";

pub struct OpenAiProvider {
    profile_id: String,
    profile: Profile,
    model_id: String,
    model: Model,
    credential: Credential,
    transport: Arc<dyn Transport>,
    url: String,
    tools: Mutex<Option<Vec<Value>>>,
    /// Canonical schemas by tool name, for normalising call arguments.
    canonical: Mutex<BTreeMap<String, Value>>,
    reasoning_effort: Option<String>,
    image_detail: Option<String>,
    final_turn: FinalTurn,
    max_long_edge: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ResponseObject {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
    #[serde(default)]
    error: Option<ResponseError>,
    #[serde(default)]
    output: Vec<Value>,
    /// `None` when the answer carried no usage block.
    #[serde(default)]
    usage: Option<ResponseUsage>,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Default, Deserialize)]
struct ResponseUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    input_tokens_details: TokenDetails,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct TokenDetails {
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
}

impl OpenAiProvider {
    pub fn new(
        config: &Config,
        profile_id: &str,
        profile: &Profile,
        model: &Model,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Result<OpenAiProvider> {
        if profile.adapter != ADAPTER_ID || model.adapter != ADAPTER_ID {
            return Err(Error::Config(format!(
                "profile {profile_id:?} and model {:?} must both use {ADAPTER_ID}",
                config.provider.model
            )));
        }
        let control = |name: &str| {
            config
                .provider
                .controls
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let final_turn = match control(FINAL_TURN_CONTROL) {
            Some(m) => FinalTurn::parse(&m)
                .ok_or_else(|| Error::Config(format!("unknown final-turn mechanism {m:?}")))?,
            None => model
                .capabilities
                .final_turn
                .first()
                .and_then(|m| FinalTurn::parse(m))
                .ok_or_else(|| {
                    Error::Config(format!(
                        "model {:?} supports no final-turn mechanism",
                        config.provider.model
                    ))
                })?,
        };
        if final_turn == FinalTurn::ToolRemoval {
            return Err(Error::Config(
                "tool_removal is an Anthropic mechanism; OpenAI supports refuse and forced".into(),
            ));
        }
        let url = format!(
            "{}{}",
            profile.base_url.trim_end_matches('/'),
            profile.path.replace("{model}", &config.provider.model)
        );
        Ok(OpenAiProvider {
            profile_id: profile_id.to_string(),
            profile: profile.clone(),
            model_id: config.provider.model.clone(),
            model: model.clone(),
            credential,
            transport,
            url,
            tools: Mutex::new(None),
            canonical: Mutex::new(BTreeMap::new()),
            reasoning_effort: control("reasoning_effort"),
            image_detail: control("image_detail"),
            final_turn,
            max_long_edge: config.images.max_long_edge,
        })
    }

    pub fn final_turn(&self) -> FinalTurn {
        self.final_turn
    }

    pub fn volatile() -> Volatile {
        Volatile::common()
    }

    fn headers(&self) -> Headers {
        let mut headers: Headers = vec![
            (
                self.profile.auth.header.clone(),
                format!("Bearer {}", self.credential.expose()),
            ),
            ("content-type".into(), "application/json".into()),
        ];
        for (k, v) in &self.profile.headers {
            headers.push((k.clone(), v.clone()));
        }
        headers
    }

    fn wire_tools(&self, defs: &[ToolDefinition]) -> Result<Vec<Value>> {
        if let Ok(cache) = self.tools.lock()
            && let Some(t) = cache.as_ref()
        {
            return Ok(t.clone());
        }
        let tools: Vec<Value> =
            check_tool_set(Adapter::OpenaiResponses, &self.model.capabilities, defs)?
                .into_iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.definition.name,
                        "description": t.definition.description,
                        "parameters": t.definition.input_schema,
                        "strict": t.wire.strict,
                    })
                })
                .collect();
        if let Ok(mut cache) = self.tools.lock() {
            *cache = Some(tools.clone());
        }
        if let Ok(mut canonical) = self.canonical.lock() {
            for d in defs {
                canonical.insert(d.name.clone(), d.input_schema.clone());
            }
        }
        Ok(tools)
    }

    /// The request body for one turn. `store` is always false and reasoning is asked back in its
    /// encrypted form so the conversation stays stateless.
    pub fn build_body(&self, req: &TurnRequest<'_>) -> Result<Value> {
        let final_turn = req.history.iter().any(|m| m.role == MessageRole::System);
        let mut body = json!({
            "model": self.model_id,
            "instructions": req.system,
            "input": self.input(req)?,
            "tools": self.wire_tools(req.tools)?,
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "max_output_tokens": req.max_output_tokens,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "truncation": "disabled",
        });
        if let Some(e) = &self.reasoning_effort {
            body["reasoning"] = json!({ "effort": e });
        }
        if final_turn && self.final_turn == FinalTurn::Forced {
            body["tool_choice"] = json!({ "type": "function", "name": SUBMIT_VERDICT });
        }
        Ok(body)
    }

    fn image_part(&self, id: &str, images: &BTreeMap<String, Prepared>) -> Result<Value> {
        let p = images.get(id).ok_or_else(|| {
            Error::Image(format!("{id}: named by the request but never prepared"))
        })?;
        let mut part = json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", p.media_type, base64_encode(&p.bytes)),
        });
        if let Some(d) = &self.image_detail {
            part["detail"] = Value::String(d.clone());
        }
        Ok(part)
    }

    fn input(&self, req: &TurnRequest<'_>) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        for m in req.history {
            match m.role {
                MessageRole::User => {
                    // Function outputs are items of their own; text and images form one message.
                    let mut message: Vec<Value> = Vec::new();
                    for b in &m.content {
                        match b {
                            ContentBlock::Text { text } => {
                                message.push(json!({ "type": "input_text", "text": text }))
                            }
                            ContentBlock::Image { image_id } => {
                                message.push(self.image_part(image_id, req.images)?)
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                                images,
                            } => {
                                // The Responses API has no error flag on a function result, so
                                // a failed call says so in its text.
                                let content = if *is_error {
                                    format!("error: {content}")
                                } else {
                                    content.clone()
                                };
                                let output = if images.is_empty() {
                                    Value::String(content)
                                } else {
                                    let mut parts =
                                        vec![json!({ "type": "input_text", "text": content })];
                                    for id in images {
                                        parts.push(self.image_part(id, req.images)?);
                                    }
                                    Value::Array(parts)
                                };
                                out.push(json!({ "type": "function_call_output", "call_id": tool_use_id, "output": output }));
                            }
                            ContentBlock::ToolUse { .. } | ContentBlock::Opaque { .. } => {
                                return Err(Error::Provider(
                                    "a user message cannot carry model items".into(),
                                ));
                            }
                        }
                    }
                    if !message.is_empty() {
                        out.push(json!({ "type": "message", "role": "user", "content": message }));
                    }
                }
                MessageRole::Assistant => {
                    for b in &m.content {
                        match b {
                            ContentBlock::Opaque { adapter, block } => {
                                if adapter != ADAPTER_ID {
                                    return Err(Error::Provider(format!(
                                        "history carries an opaque block from adapter {adapter:?}; this conversation belongs to {ADAPTER_ID}"
                                    )));
                                }
                                out.push(block.clone());
                            }
                            ContentBlock::Text { text } => out.push(json!({
                                "type": "message", "role": "assistant",
                                "content": [{ "type": "output_text", "text": text, "annotations": [] }]
                            })),
                            ContentBlock::ToolUse { id, name, arguments } => out.push(json!({
                                "type": "function_call", "call_id": id, "name": name,
                                "arguments": arguments.to_string()
                            })),
                            ContentBlock::Image { .. } | ContentBlock::ToolResult { .. } => {
                                return Err(Error::Provider(
                                    "an assistant message cannot carry user items".into(),
                                ));
                            }
                        }
                    }
                }
                MessageRole::System => {
                    let text: String = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    out.push(json!({ "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": text }] }));
                }
            }
        }
        Ok(out)
    }

    /// Call arguments in canonical form (SPEC "Wire schemas"): what the provider left out, dropped
    /// or respelled is put back before validation.
    fn normalize_arguments(&self, name: &str, arguments: Value) -> Value {
        match self
            .canonical
            .lock()
            .ok()
            .and_then(|c| c.get(name).cloned())
        {
            Some(schema) => crate::wire::normalize(&schema, arguments),
            None => arguments,
        }
    }

    /// A response body into the harness's view; every output item is kept for the echo.
    pub fn parse_response(
        &self,
        body: &str,
        header_request_id: Option<String>,
    ) -> Result<TurnResponse> {
        let parsed: ResponseObject = serde_json::from_str(body)
            .map_err(|e| Error::Provider(format!("unreadable Responses body: {e}")))?;
        if parsed.status.as_deref() == Some("failed") {
            let e = parsed.error.unwrap_or(ResponseError {
                code: None,
                message: "unknown failure".into(),
            });
            return Err(Error::Provider(format!(
                "response failed: {} ({})",
                e.message,
                e.code.unwrap_or_default()
            )));
        }
        let mut text = Vec::new();
        let mut tool_calls = Vec::new();
        let mut content = Vec::new();
        let mut refused: Option<String> = None;
        for item in &parsed.output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    for part in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => text.push(
                                part.get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            Some("refusal") => {
                                refused = Some(
                                    part.get("refusal")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                )
                            }
                            _ => {}
                        }
                    }
                }
                Some("function_call") => {
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if call_id.is_empty() || name.is_empty() {
                        return Err(Error::Provider(
                            "function_call item without call_id or name".into(),
                        ));
                    }
                    let raw = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    // Arguments that are not JSON stay a string: the tool argument check rejects them.
                    let arguments = self.normalize_arguments(
                        &name,
                        serde_json::from_str(raw).unwrap_or(Value::String(raw.to_string())),
                    );
                    tool_calls.push(ToolCall {
                        id: call_id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
            content.push(ContentBlock::Opaque {
                adapter: ADAPTER_ID.into(),
                block: item.clone(),
            });
        }
        let stop_reason = match (
            parsed.status.as_deref(),
            parsed.incomplete_details.and_then(|d| d.reason),
        ) {
            (Some("incomplete"), Some(reason)) if reason == "max_output_tokens" => {
                StopReason::MaxTokens
            }
            (Some("incomplete"), Some(reason)) if reason == "content_filter" => {
                text.push("refusal: content_filter".into());
                StopReason::Refusal
            }
            (Some("incomplete"), reason) => {
                StopReason::Other(format!("incomplete: {}", reason.unwrap_or_default()))
            }
            _ if refused.is_some() => {
                text.push(format!("refusal: {}", refused.unwrap_or_default()));
                StopReason::Refusal
            }
            _ if !tool_calls.is_empty() => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        Ok(TurnResponse {
            text,
            tool_calls,
            content,
            stop_reason,
            usage: parsed.usage.map(|u| Usage {
                input_tokens: u
                    .input_tokens
                    .saturating_sub(u.input_tokens_details.cached_tokens),
                output_tokens: u.output_tokens,
                cache_read_tokens: u.input_tokens_details.cached_tokens,
                cache_write_tokens: u.input_tokens_details.cache_write_tokens,
            }),
            provider_request_id: header_request_id.or(parsed.id),
        })
    }

    async fn turn(&self, req: TurnRequest<'_>) -> Result<TurnResponse> {
        prepare::preflight(&self.model.images, &req)?;
        let body = self.build_body(&req)?;
        let headers = self.headers();
        let response = self.transport.post_json(&self.url, &headers, &body).await?;
        let request_id = response.header("x-request-id").map(str::to_string);
        let response = response.ok(names_no_cause)?;
        self.parse_response(&response.body, request_id)
    }
}

impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        ADAPTER_ID
    }

    fn send<'a>(&'a self, req: TurnRequest<'a>) -> SendFuture<'a> {
        Box::pin(self.turn(req))
    }

    fn pricing(&self) -> Option<&dyn Pricing> {
        Some(&self.model)
    }

    fn prep_version(&self) -> &str {
        prepare::PREP_VERSION
    }

    fn recipient(&self) -> Option<Recipient> {
        Some(Recipient {
            profile: self.profile_id.clone(),
            host: host_of(&self.profile.base_url),
            upstream: None,
            model: self.model_id.clone(),
            api_version: "v1".into(),
        })
    }

    fn prepare_image(
        &self,
        id: &str,
        pixels: &image::DynamicImage,
        plan: ImagePlan,
    ) -> Result<Prepared> {
        Preparer::new(&self.model.images, self.max_long_edge, plan).prepare(id, pixels)
    }
}

/// Whether an OpenAI error body names no cause (F3): the envelope
/// `{"error": {"message": …, "type": …, "param": …, "code": …}}` with no `param`, no `code` and an
/// empty or missing message. A parameter (`input[0].content[1].image_url`), a code
/// (`invalid_value`) or any message is a cause; a body that is not the envelope is not read as
/// causeless either.
pub fn names_no_cause(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let Some(Value::Object(error)) = v.get("error") else {
        return false;
    };
    ["message", "param", "code"]
        .iter()
        .all(|k| match error.get(*k) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.trim().is_empty(),
            Some(_) => false,
        })
}

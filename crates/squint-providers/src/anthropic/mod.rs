//! The Anthropic Messages adapter (SPEC "Protocol requirements", "Final turn mechanics"). One
//! request per turn over the injected transport; the harness's history mapped to the wire with
//! thinking blocks echoed exactly as received; images attached from the prepared bytes; tools
//! declared with the wire schema and `strict: true` where the catalog allows; controls only as
//! the catalog permits (no sampling parameters, no forced tool choice where the model rejects it);
//! the final-turn instruction as a mid-conversation system message, optionally with
//! `tool_removal` blocks or a forced `tool_choice`, by the configured mechanism.

pub mod wire;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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

use crate::catalog::{Model, Profile};
use crate::http::{Headers, Transport};
use crate::prepare::{self, Preparer};
use crate::privacy::Credential;
use crate::replay::Volatile;
use crate::wire::{Adapter, check_tool_set};

pub const ADAPTER_ID: &str = "anthropic-messages";
pub const API_VERSION: &str = "2023-06-01";
/// The control naming the final-turn mechanism; its values are the catalog's `final_turn` list.
pub const FINAL_TURN_CONTROL: &str = "final_turn";
pub const PREFIX_CHECK_CONTROL: &str = "prefix_mismatch_behavior";

/// How the final turn is restricted (SPEC "Final turn mechanics").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalTurn {
    Refuse,
    Forced,
    ToolRemoval,
}

impl FinalTurn {
    pub fn parse(s: &str) -> Option<FinalTurn> {
        match s {
            "refuse" => Some(FinalTurn::Refuse),
            "forced" => Some(FinalTurn::Forced),
            "tool_removal" => Some(FinalTurn::ToolRemoval),
            _ => None,
        }
    }
}

pub struct AnthropicProvider {
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
    effort: Option<String>,
    prefix_mismatch: Option<String>,
    final_turn: FinalTurn,
    max_long_edge: Option<u32>,
}

impl AnthropicProvider {
    /// Builds the adapter for a resolved config. The catalog has already validated the controls;
    /// this reads them and derives the request settings.
    pub fn new(
        config: &Config,
        profile_id: &str,
        profile: &Profile,
        model: &Model,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Result<AnthropicProvider> {
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
        if final_turn == FinalTurn::Forced && !model.capabilities.forced_tool_choice {
            return Err(Error::Config(format!(
                "model {:?} rejects forced tool choice",
                config.provider.model
            )));
        }
        if final_turn == FinalTurn::ToolRemoval && !model.betas.contains_key("tool_changes") {
            return Err(Error::Config(format!(
                "model {:?} has no tool-changes beta pinned",
                config.provider.model
            )));
        }
        let url = format!(
            "{}{}",
            profile.base_url.trim_end_matches('/'),
            profile.path.replace("{model}", &config.provider.model)
        );
        Ok(AnthropicProvider {
            profile_id: profile_id.to_string(),
            profile: profile.clone(),
            model_id: config.provider.model.clone(),
            model: model.clone(),
            credential,
            transport,
            url,
            tools: Mutex::new(None),
            canonical: Mutex::new(BTreeMap::new()),
            effort: control("effort"),
            prefix_mismatch: control(PREFIX_CHECK_CONTROL),
            final_turn,
            max_long_edge: config.images.max_long_edge,
        })
    }

    pub fn final_turn(&self) -> FinalTurn {
        self.final_turn
    }

    /// The fields a recording leaves out of comparison for this adapter.
    pub fn volatile() -> Volatile {
        Volatile::common()
    }

    /// Request headers: the credential in the one header the scheme names, the API version, and
    /// the betas the configuration needs.
    fn headers(&self) -> Headers {
        let mut headers: Headers = vec![
            (
                self.profile.auth.header.clone(),
                self.credential.expose().to_string(),
            ),
            ("anthropic-version".into(), API_VERSION.into()),
            ("content-type".into(), "application/json".into()),
        ];
        for (k, v) in &self.profile.headers {
            if !headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(k)) {
                headers.push((k.clone(), v.clone()));
            }
        }
        let mut betas: Vec<&str> = Vec::new();
        if self.prefix_mismatch.is_some()
            && let Some(b) = self.model.betas.get("thinking_binding")
        {
            betas.push(b);
        }
        if self.final_turn == FinalTurn::ToolRemoval
            && let Some(b) = self.model.betas.get("tool_changes")
        {
            betas.push(b);
        }
        if !betas.is_empty() {
            headers.push(("anthropic-beta".into(), betas.join(",")));
        }
        headers
    }

    /// Tool definitions in wire form, transformed once and reused: the `tools` array never changes
    /// for the life of a conversation.
    fn wire_tools(&self, defs: &[ToolDefinition]) -> Result<Vec<Value>> {
        if let Ok(cache) = self.tools.lock()
            && let Some(t) = cache.as_ref()
        {
            return Ok(t.clone());
        }
        let strict = self.model.capabilities.strict_tools;
        let tools: Vec<Value> =
            check_tool_set(Adapter::AnthropicMessages, &self.model.capabilities, defs)?
                .into_iter()
                .map(|t| {
                    let mut v = json!({
                        "name": t.definition.name,
                        "description": t.definition.description,
                        "input_schema": t.definition.input_schema,
                    });
                    if strict && t.wire.strict {
                        v["strict"] = Value::Bool(true);
                    }
                    v
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

    /// The request body for one turn.
    pub fn build_body(&self, req: &TurnRequest<'_>) -> Result<Value> {
        let final_turn = req.history.iter().any(|m| m.role == MessageRole::System);
        let other_tools: Vec<&str> = req
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .filter(|n| *n != SUBMIT_VERDICT)
            .collect();
        let messages = self.messages(req, &other_tools)?;
        let mut thinking = json!({ "type": "adaptive" });
        if let Some(b) = &self.prefix_mismatch {
            thinking["block_binding"] = json!({ "prefix_mismatch_behavior": b });
        }
        let mut body = json!({
            "model": self.model_id,
            "max_tokens": req.max_output_tokens,
            "system": [{ "type": "text", "text": req.system }],
            "messages": messages,
            "tools": self.wire_tools(req.tools)?,
            "thinking": thinking,
        });
        if let Some(e) = &self.effort {
            body["output_config"] = json!({ "effort": e });
        }
        if final_turn && self.final_turn == FinalTurn::Forced {
            body["tool_choice"] = json!({ "type": "tool", "name": SUBMIT_VERDICT });
        }
        Ok(body)
    }

    fn messages(&self, req: &TurnRequest<'_>, other_tools: &[&str]) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        for m in req.history {
            match m.role {
                MessageRole::User => out.push(json!({ "role": "user", "content": self.user_content(&m.content, req.images)? })),
                MessageRole::Assistant => out.push(json!({ "role": "assistant", "content": self.assistant_content(&m.content)? })),
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
                    if self.model.capabilities.system_messages_in_history {
                        let mut content: Vec<Value> = Vec::new();
                        if self.final_turn == FinalTurn::ToolRemoval {
                            for name in other_tools {
                                content.push(json!({ "type": "tool_removal", "tool": { "type": "tool_reference", "name": name } }));
                            }
                        }
                        content.push(json!({ "type": "text", "text": text }));
                        out.push(json!({ "role": "system", "content": content }));
                    } else {
                        // No system role mid-conversation: the instruction closes the preceding user
                        // message as its last text block.
                        let last_is_user = out
                            .last()
                            .and_then(|v| v.get("role"))
                            .and_then(Value::as_str)
                            == Some("user");
                        let appended = last_is_user
                            && out
                                .last_mut()
                                .and_then(|v| v.get_mut("content"))
                                .and_then(Value::as_array_mut)
                                .map(|content| {
                                    content.push(json!({ "type": "text", "text": text }));
                                })
                                .is_some();
                        if !appended {
                            out.push(json!({ "role": "user", "content": [{ "type": "text", "text": text }] }));
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn image_block(&self, id: &str, images: &BTreeMap<String, Prepared>) -> Result<Value> {
        let p = images.get(id).ok_or_else(|| {
            Error::Image(format!("{id}: named by the request but never prepared"))
        })?;
        Ok(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": p.media_type, "data": base64_encode(&p.bytes) }
        }))
    }

    fn user_content(
        &self,
        blocks: &[ContentBlock],
        images: &BTreeMap<String, Prepared>,
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for b in blocks {
            match b {
                ContentBlock::Text { text } => out.push(json!({ "type": "text", "text": text })),
                ContentBlock::Image { image_id } => out.push(self.image_block(image_id, images)?),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    images: ids,
                } => {
                    let content_value = if ids.is_empty() {
                        Value::String(content.clone())
                    } else {
                        let mut parts = vec![json!({ "type": "text", "text": content })];
                        for id in ids {
                            parts.push(self.image_block(id, images)?);
                        }
                        Value::Array(parts)
                    };
                    let mut v = json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content_value });
                    if *is_error {
                        v["is_error"] = Value::Bool(true);
                    }
                    out.push(v);
                }
                ContentBlock::ToolUse { .. } | ContentBlock::Opaque { .. } => {
                    return Err(Error::Provider(
                        "a user message cannot carry assistant blocks".into(),
                    ));
                }
            }
        }
        Ok(out)
    }

    fn assistant_content(&self, blocks: &[ContentBlock]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for b in blocks {
            match b {
                ContentBlock::Text { text } => out.push(json!({ "type": "text", "text": text })),
                ContentBlock::ToolUse {
                    id,
                    name,
                    arguments,
                } => out.push(
                    json!({ "type": "tool_use", "id": id, "name": name, "input": arguments }),
                ),
                ContentBlock::Opaque { adapter, block } => {
                    if adapter != ADAPTER_ID {
                        return Err(Error::Provider(format!(
                            "history carries an opaque block from adapter {adapter:?}; this conversation belongs to {ADAPTER_ID}"
                        )));
                    }
                    out.push(block.clone());
                }
                ContentBlock::Image { .. } | ContentBlock::ToolResult { .. } => {
                    return Err(Error::Provider(
                        "an assistant message cannot carry user blocks".into(),
                    ));
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

    /// A response body into the harness's view, with every block kept for the echo.
    pub fn parse_response(&self, body: &str, request_id: Option<String>) -> Result<TurnResponse> {
        let parsed: wire::MessageResponse = serde_json::from_str(body)
            .map_err(|e| Error::Provider(format!("unreadable Messages response: {e}")))?;
        let mut text = Vec::new();
        let mut tool_calls = Vec::new();
        let mut content = Vec::new();
        for block in parsed.content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let t = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    text.push(t.clone());
                    content.push(ContentBlock::Text { text: t });
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let arguments = self.normalize_arguments(
                        &name,
                        block.get("input").cloned().unwrap_or(Value::Null),
                    );
                    if id.is_empty() || name.is_empty() {
                        return Err(Error::Provider("tool_use block without id or name".into()));
                    }
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                    // Echoed exactly as received, like every other block: the harness works from
                    // the normalised arguments above, but the prefix check binds the thinking to
                    // the block the model produced.
                    content.push(ContentBlock::Opaque {
                        adapter: ADAPTER_ID.into(),
                        block,
                    });
                }
                // Thinking, redacted thinking and anything newer: echoed as received.
                _ => content.push(ContentBlock::Opaque {
                    adapter: ADAPTER_ID.into(),
                    block,
                }),
            }
        }
        let stop_reason = match parsed.stop_reason.as_deref() {
            Some("end_turn") | Some("stop_sequence") | None => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            // Both leave the output cut short; neither response is dispatched.
            Some("max_tokens") | Some("model_context_window_exceeded") => StopReason::MaxTokens,
            Some("refusal") => StopReason::Refusal,
            Some(other) => StopReason::Other(other.to_string()),
        };
        if stop_reason == StopReason::Refusal
            && let Some(d) = &parsed.stop_details
        {
            text.push(format!(
                "refusal: {}{}",
                d.category.as_deref().unwrap_or("uncategorised"),
                d.explanation
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            ));
        }
        Ok(TurnResponse {
            text,
            tool_calls,
            content,
            stop_reason,
            usage: parsed.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens: u.cache_read_input_tokens,
                cache_write_tokens: u.cache_creation_input_tokens,
            }),
            provider_request_id: request_id.or(Some(parsed.id)),
        })
    }

    async fn turn(&self, req: TurnRequest<'_>) -> Result<TurnResponse> {
        prepare::preflight(&self.model.images, &req)?;
        let body = self.build_body(&req)?;
        let headers = self.headers();
        let response = self.transport.post_json(&self.url, &headers, &body).await?;
        let request_id = response.header("request-id").map(str::to_string);
        let response = response.ok(names_no_cause)?;
        self.parse_response(&response.body, request_id)
    }
}

impl Provider for AnthropicProvider {
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
        let host = self
            .profile
            .base_url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        Some(Recipient {
            profile: self.profile_id.clone(),
            host,
            upstream: None,
            model: self.model_id.clone(),
            api_version: API_VERSION.into(),
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

/// Whether an Anthropic error body names no cause (F3): the envelope
/// `{"type": "error", "error": {"type": …, "message": …}}` with an empty or missing message.
/// Anthropic's messages name what was wrong (`messages.0.content: Field required`), so any
/// message is a cause; a body that is not the envelope is not read as causeless either.
pub fn names_no_cause(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    match v.get("error") {
        Some(Value::Object(error)) => blank(error.get("message")),
        _ => false,
    }
}

/// A missing, null or whitespace-only string field.
fn blank(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(_) => false,
    }
}

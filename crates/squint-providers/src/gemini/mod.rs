//! The Gemini `generateContent` adapter (SPEC "Protocol requirements"). `functionCall` parts are
//! answered by `functionResponse` parts matched by the call's `id`; a response without ids is
//! accepted only for models the catalog flags `function_call_ids = false`, and then only with one
//! call. Every part of a model turn is echoed exactly as received (thought signatures ride on the
//! parts), tools are declared with `parametersJsonSchema` in the wire form, and the final turn uses
//! the function-calling mode `ANY` restricted to `submit_verdict` where the mechanism is `forced`;
//! the instruction itself closes the preceding user content, since the API has no system role
//! mid-conversation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use squint_core::config::Config;
use squint_core::image::{Prepared, base64_encode};
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
use crate::http::{Headers, Transport};
use crate::prepare::{self, Preparer};
use crate::privacy::Credential;
use crate::replay::Volatile;
use crate::wire::{Adapter, check_tool_set};

pub const ADAPTER_ID: &str = "gemini-generate-content";
/// Call ids the adapter assigns when the API sends a `functionCall` without one; their
/// `functionResponse` carries no id either.
pub const SYNTHETIC_ID_PREFIX: &str = "gemini-noid:";

pub struct GeminiProvider {
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
    thinking_level: Option<String>,
    media_resolution: Option<String>,
    final_turn: FinalTurn,
    max_long_edge: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default, rename = "promptFeedback")]
    prompt_feedback: Option<PromptFeedback>,
    /// `None` when the answer carried no `usageMetadata`.
    #[serde(default, rename = "usageMetadata")]
    usage: Option<UsageMetadata>,
    #[serde(default, rename = "responseId")]
    response_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
    #[serde(default, rename = "finishMessage")]
    finish_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct PromptFeedback {
    #[serde(default, rename = "blockReason")]
    block_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_tokens: u64,
    #[serde(default, rename = "cachedContentTokenCount")]
    cached_tokens: u64,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_tokens: u64,
    #[serde(default, rename = "thoughtsTokenCount")]
    thoughts_tokens: u64,
}

impl GeminiProvider {
    pub fn new(
        config: &Config,
        profile_id: &str,
        profile: &Profile,
        model: &Model,
        credential: Credential,
        transport: Arc<dyn Transport>,
    ) -> Result<GeminiProvider> {
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
                "tool_removal is an Anthropic mechanism; Gemini supports refuse and forced".into(),
            ));
        }
        let url = format!(
            "{}{}",
            profile.base_url.trim_end_matches('/'),
            profile.path.replace("{model}", &config.provider.model)
        );
        Ok(GeminiProvider {
            profile_id: profile_id.to_string(),
            profile: profile.clone(),
            model_id: config.provider.model.clone(),
            model: model.clone(),
            credential,
            transport,
            url,
            tools: Mutex::new(None),
            canonical: Mutex::new(BTreeMap::new()),
            thinking_level: control("thinking_level"),
            media_resolution: control("media_resolution"),
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
                self.credential.expose().to_string(),
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
        let declarations: Vec<Value> = check_tool_set(
            Adapter::GeminiGenerateContent,
            &self.model.capabilities,
            defs,
        )?
        .into_iter()
        .map(|t| {
            json!({
                "name": t.definition.name,
                "description": t.definition.description,
                "parametersJsonSchema": t.definition.input_schema,
            })
        })
        .collect();
        let tools = vec![json!({ "functionDeclarations": declarations })];
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
        let mut generation = json!({ "maxOutputTokens": req.max_output_tokens });
        if let Some(level) = &self.thinking_level {
            generation["thinkingConfig"] = json!({ "thinkingLevel": level });
        }
        let mut body = json!({
            "systemInstruction": { "parts": [{ "text": req.system }] },
            "contents": self.contents(req)?,
            "tools": self.wire_tools(req.tools)?,
            "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
            "generationConfig": generation,
        });
        if final_turn && self.final_turn == FinalTurn::Forced {
            body["toolConfig"]["functionCallingConfig"] =
                json!({ "mode": "ANY", "allowedFunctionNames": [SUBMIT_VERDICT] });
        }
        Ok(body)
    }

    fn image_part(&self, id: &str, images: &BTreeMap<String, Prepared>) -> Result<Value> {
        let p = images.get(id).ok_or_else(|| {
            Error::Image(format!("{id}: named by the request but never prepared"))
        })?;
        let mut part =
            json!({ "inlineData": { "mimeType": p.media_type, "data": base64_encode(&p.bytes) } });
        if let Some(level) = &self.media_resolution {
            part["mediaResolution"] =
                json!({ "level": format!("MEDIA_RESOLUTION_{}", level.to_ascii_uppercase()) });
        }
        Ok(part)
    }

    fn contents(&self, req: &TurnRequest<'_>) -> Result<Vec<Value>> {
        let mut out: Vec<Value> = Vec::new();
        // Function responses need the function's name: taken from the call that preceded them.
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        for m in req.history {
            match m.role {
                MessageRole::User => {
                    let mut parts = Vec::new();
                    for b in &m.content {
                        match b {
                            ContentBlock::Text { text } => parts.push(json!({ "text": text })),
                            ContentBlock::Image { image_id } => {
                                parts.push(self.image_part(image_id, req.images)?)
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                                images,
                            } => {
                                let name = names.get(tool_use_id).cloned().ok_or_else(|| {
                                    Error::Provider(format!(
                                        "tool result {tool_use_id:?} answers no call in the history"
                                    ))
                                })?;
                                let key = if *is_error { "error" } else { "output" };
                                let mut fr = json!({ "name": name, "response": { key: content } });
                                if !tool_use_id.starts_with(SYNTHETIC_ID_PREFIX) {
                                    fr["id"] = Value::String(tool_use_id.clone());
                                }
                                if !images.is_empty() {
                                    let mut fparts = Vec::new();
                                    for id in images {
                                        let p = req.images.get(id).ok_or_else(|| {
                                            Error::Image(format!(
                                                "{id}: named by the request but never prepared"
                                            ))
                                        })?;
                                        fparts.push(json!({ "inlineData": { "mimeType": p.media_type, "data": base64_encode(&p.bytes) } }));
                                    }
                                    fr["parts"] = Value::Array(fparts);
                                }
                                parts.push(json!({ "functionResponse": fr }));
                            }
                            ContentBlock::ToolUse { .. } | ContentBlock::Opaque { .. } => {
                                return Err(Error::Provider(
                                    "a user message cannot carry model parts".into(),
                                ));
                            }
                        }
                    }
                    out.push(json!({ "role": "user", "parts": parts }));
                }
                MessageRole::Assistant => {
                    let mut parts = Vec::new();
                    for b in &m.content {
                        match b {
                            ContentBlock::Opaque { adapter, block } => {
                                if adapter != ADAPTER_ID {
                                    return Err(Error::Provider(format!(
                                        "history carries an opaque block from adapter {adapter:?}; this conversation belongs to {ADAPTER_ID}"
                                    )));
                                }
                                if let Some(fc) = block.get("functionCall") {
                                    let id = fc
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .map(str::to_string)
                                        .unwrap_or_else(|| synthetic_id(block));
                                    if let Some(name) = fc.get("name").and_then(Value::as_str) {
                                        names.insert(id, name.to_string());
                                    }
                                }
                                parts.push(block.clone());
                            }
                            ContentBlock::Text { text } => parts.push(json!({ "text": text })),
                            ContentBlock::ToolUse {
                                id,
                                name,
                                arguments,
                            } => {
                                names.insert(id.clone(), name.clone());
                                let mut fc = json!({ "name": name, "args": arguments });
                                if !id.starts_with(SYNTHETIC_ID_PREFIX) {
                                    fc["id"] = Value::String(id.clone());
                                }
                                parts.push(json!({ "functionCall": fc }));
                            }
                            ContentBlock::Image { .. } | ContentBlock::ToolResult { .. } => {
                                return Err(Error::Provider(
                                    "a model message cannot carry user parts".into(),
                                ));
                            }
                        }
                    }
                    out.push(json!({ "role": "model", "parts": parts }));
                }
                MessageRole::System => {
                    // No system role mid-conversation: the instruction closes the preceding user
                    // content, or opens a new one.
                    let text: String = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let last_is_user = out
                        .last()
                        .and_then(|v| v.get("role"))
                        .and_then(Value::as_str)
                        == Some("user");
                    let appended = last_is_user
                        && out
                            .last_mut()
                            .and_then(|v| v.get_mut("parts"))
                            .and_then(Value::as_array_mut)
                            .map(|parts| parts.push(json!({ "text": text })))
                            .is_some();
                    if !appended {
                        out.push(json!({ "role": "user", "parts": [{ "text": text }] }));
                    }
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

    /// A response body into the harness's view; every model part is kept for the echo.
    pub fn parse_response(
        &self,
        body: &str,
        header_request_id: Option<String>,
    ) -> Result<TurnResponse> {
        let parsed: GenerateContentResponse = serde_json::from_str(body)
            .map_err(|e| Error::Provider(format!("unreadable generateContent response: {e}")))?;
        let usage = parsed.usage.map(|u| Usage {
            input_tokens: u.prompt_tokens.saturating_sub(u.cached_tokens),
            output_tokens: u.candidates_tokens + u.thoughts_tokens,
            cache_read_tokens: u.cached_tokens,
            cache_write_tokens: 0,
        });
        let request_id = parsed.response_id.clone().or(header_request_id);
        let Some(candidate) = parsed.candidates.into_iter().next() else {
            let reason = parsed
                .prompt_feedback
                .and_then(|f| f.block_reason)
                .unwrap_or_else(|| "no candidates".into());
            return Ok(TurnResponse {
                text: vec![format!("refusal: prompt blocked ({reason})")],
                tool_calls: vec![],
                content: vec![],
                stop_reason: StopReason::Refusal,
                usage,
                provider_request_id: request_id,
            });
        };
        let parts = candidate.content.map(|c| c.parts).unwrap_or_default();
        let mut text = Vec::new();
        let mut tool_calls = Vec::new();
        let mut content = Vec::new();
        let mut without_id = 0usize;
        for part in &parts {
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Provider("functionCall without a name".into()))?
                    .to_string();
                let id = match fc.get("id").and_then(Value::as_str) {
                    Some(id) => id.to_string(),
                    None => {
                        without_id += 1;
                        synthetic_id(part)
                    }
                };
                let arguments =
                    self.normalize_arguments(&name, fc.get("args").cloned().unwrap_or(json!({})));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            } else if let Some(t) = part.get("text").and_then(Value::as_str)
                && part.get("thought") != Some(&Value::Bool(true))
            {
                text.push(t.to_string());
            }
            content.push(ContentBlock::Opaque {
                adapter: ADAPTER_ID.into(),
                block: part.clone(),
            });
        }
        if without_id > 0 && (self.model.capabilities.function_call_ids || tool_calls.len() > 1) {
            return Err(Error::Provider(format!(
                "{without_id} functionCall part(s) without an id in a response with {} calls; results cannot be paired",
                tool_calls.len()
            )));
        }
        let stop_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") | None if !tool_calls.is_empty() => StopReason::ToolUse,
            Some("STOP") | None => StopReason::EndTurn,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            Some(
                "SAFETY"
                | "RECITATION"
                | "BLOCKLIST"
                | "PROHIBITED_CONTENT"
                | "SPII"
                | "IMAGE_SAFETY"
                | "IMAGE_PROHIBITED_CONTENT"
                | "IMAGE_RECITATION",
            ) => {
                text.push(format!(
                    "refusal: {}{}",
                    candidate.finish_reason.as_deref().unwrap_or_default(),
                    candidate
                        .finish_message
                        .as_deref()
                        .map(|m| format!(" ({m})"))
                        .unwrap_or_default()
                ));
                StopReason::Refusal
            }
            Some(other) => StopReason::Other(other.to_string()),
        };
        Ok(TurnResponse {
            text,
            tool_calls,
            content,
            stop_reason,
            usage,
            provider_request_id: request_id,
        })
    }

    async fn turn(&self, req: TurnRequest<'_>) -> Result<TurnResponse> {
        prepare::preflight(&self.model.images, &req)?;
        let body = self.build_body(&req)?;
        let headers = self.headers();
        let response = self.transport.post_json(&self.url, &headers, &body).await?;
        let request_id = response.header("x-request-id").map(str::to_string);
        let response = response.ok()?;
        self.parse_response(&response.body, request_id)
    }
}

/// A stable id for a `functionCall` part the API sent without one, from the part's bytes.
fn synthetic_id(part: &Value) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(part.to_string().as_bytes());
    format!(
        "{SYNTHETIC_ID_PREFIX}{}",
        squint_core::image::hex(&digest[..8])
    )
}

impl Provider for GeminiProvider {
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
            api_version: "v1beta".into(),
        })
    }

    fn prepare_image(&self, id: &str, pixels: &image::DynamicImage) -> Result<Prepared> {
        Preparer::new(&self.model.images, self.max_long_edge).prepare(id, pixels)
    }
}

/// The host of an `https://` base URL.
pub fn host_of(base_url: &str) -> String {
    base_url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

//! Task 2.5a: the Anthropic Messages adapter maps the harness's history to the wire and back:
//! images from prepared bytes, tool results paired by id, strict tools, thinking echoed exactly,
//! controls as the catalog allows, the final turn by the configured mechanism, preflight before
//! anything is sent.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use squint_core::config::Config;
use squint_core::image::Prepared;
use squint_core::provider::{
    ContentBlock, Message, MessageRole, Provider, StopReason, ToolDefinition, TurnRequest,
};
use squint_core::submission;
use squint_providers::anthropic::{ADAPTER_ID, AnthropicProvider, FinalTurn};
use squint_providers::catalog::Catalog;
use squint_providers::http::{Response, Transport, TransportFuture};
use squint_providers::privacy::credential;

/// One request as the transport saw it: URL, headers, body.
type Seen = (String, Vec<(String, String)>, Value);

struct Capture {
    seen: Mutex<Vec<Seen>>,
    response: Mutex<Response>,
}

impl Transport for Capture {
    fn post_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        body: &'a Value,
    ) -> TransportFuture<'a> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), headers.to_vec(), body.clone()));
            Ok(self.response.lock().unwrap().clone())
        })
    }
}

fn ok_response(body: Value) -> Response {
    Response {
        status: 200,
        headers: [("request-id".to_string(), "req_abc".to_string())]
            .into_iter()
            .collect(),
        body: body.to_string(),
    }
}

fn config(model: &str, controls: &str) -> Config {
    let text = format!(
        "[provider]\nprofile = \"anthropic\"\nmodel = \"{model}\"\ncontrols = {{ {controls} }}\nmax_output_tokens = 4000\n\n[privacy]\ncloud_calls = true\naccount = \"anthropic:org-test\"\n\n[output]\ndir = \"out\"\n"
    );
    Config::parse(&text, &[], &|_| None).unwrap()
}

fn provider(model: &str, controls: &str, response: Value) -> (AnthropicProvider, Arc<Capture>) {
    let catalog = Catalog::shipped().unwrap();
    let config = config(model, controls);
    catalog.check_config(&config, false).unwrap();
    let profile = catalog.profile("anthropic").unwrap();
    let cred = credential(profile, &|_| Some("sk-ant-test-key".into())).unwrap();
    let transport = Arc::new(Capture {
        seen: Mutex::new(Vec::new()),
        response: Mutex::new(ok_response(response)),
    });
    let p = AnthropicProvider::new(
        &config,
        "anthropic",
        profile,
        catalog.model(model).unwrap(),
        cred,
        transport.clone(),
    )
    .unwrap();
    (p, transport)
}

fn prepared(id: &str, bytes: &[u8]) -> Prepared {
    Prepared {
        id: id.into(),
        media_type: "image/jpeg".into(),
        width: 64,
        height: 48,
        source_width: 64,
        source_height: 48,
        sha256: "0".repeat(64),
        prep_version: "1".into(),
        bytes: bytes.to_vec(),
    }
}

fn regions_tool() -> ToolDefinition {
    ToolDefinition {
        name: "regions".into(),
        description: "Finds regions.".into(),
        input_schema: json!({ "type": "object", "additionalProperties": false, "required": ["image"], "properties": { "image": { "type": "string" } } }),
    }
}

fn history() -> Vec<Message> {
    vec![
        Message {
            role: MessageRole::User,
            content: vec![
                ContentBlock::Text {
                    text: "Image `candidate`.".into(),
                },
                ContentBlock::Image {
                    image_id: "candidate".into(),
                },
                ContentBlock::Text {
                    text: "checklist".into(),
                },
            ],
        },
        Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Opaque {
                    adapter: ADAPTER_ID.into(),
                    block: json!({ "type": "thinking", "thinking": "", "signature": "SIG1" }),
                },
                ContentBlock::Text {
                    text: "Looking closer.".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "regions".into(),
                    arguments: json!({ "image": "candidate" }),
                },
            ],
        },
        Message {
            role: MessageRole::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: "{\"status\":\"ok\"}".into(),
                    is_error: false,
                    images: vec!["crop:call:1:1".into()],
                },
                ContentBlock::Text {
                    text: "checklist again".into(),
                },
            ],
        },
    ]
}

fn images() -> BTreeMap<String, Prepared> {
    let mut m = BTreeMap::new();
    m.insert("candidate".to_string(), prepared("candidate", b"JPEG-A"));
    m.insert(
        "crop:call:1:1".to_string(),
        prepared("crop:call:1:1", b"JPEG-B"),
    );
    m
}

fn tools() -> Vec<ToolDefinition> {
    vec![regions_tool(), submission::submit_verdict_tool()]
}

fn request<'a>(
    history: &'a [Message],
    tools: &'a [ToolDefinition],
    images: &'a BTreeMap<String, Prepared>,
) -> TurnRequest<'a> {
    TurnRequest {
        system: "You judge images.",
        history,
        tools,
        max_output_tokens: 4000,
        images,
    }
}

#[test]
fn anthropic_request_body_maps_history_images_tools_and_controls() {
    let (p, _) = provider("claude-opus-5-5", "effort = \"high\"", json!({}));
    let (h, t, im) = (history(), tools(), images());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 4000);
    assert_eq!(
        body["system"],
        json!([{ "type": "text", "text": "You judge images." }])
    );
    assert_eq!(body["thinking"], json!({ "type": "adaptive" }));
    assert_eq!(body["output_config"], json!({ "effort": "high" }));
    for forbidden in [
        "temperature",
        "top_p",
        "top_k",
        "tool_choice",
        "stream",
        "metadata",
    ] {
        assert!(body.get(forbidden).is_none(), "{forbidden}");
    }
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["content"][1],
        json!({ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "SlBFRy1B" } })
    );
    // The assistant turn is echoed in order with the thinking block byte for byte.
    assert_eq!(
        messages[1]["content"][0],
        json!({ "type": "thinking", "thinking": "", "signature": "SIG1" })
    );
    assert_eq!(
        messages[1]["content"][2],
        json!({ "type": "tool_use", "id": "toolu_1", "name": "regions", "input": { "image": "candidate" } })
    );
    // Tool results first, with the crop attached as an image block, then the text.
    let result = &messages[2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_1");
    assert_eq!(
        result["content"][0],
        json!({ "type": "text", "text": "{\"status\":\"ok\"}" })
    );
    assert_eq!(result["content"][1]["source"]["data"], "SlBFRy1C");
    assert!(result.get("is_error").is_none());
    assert_eq!(messages[2]["content"][1]["type"], "text");
    // Tools carry the wire schema and strict mode; the submission schema lost its constraints.
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["name"], "regions");
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(tools[1]["name"], "submit_verdict");
    assert!(tools[1]["input_schema"].to_string().contains("$defs"));
    assert!(
        !tools[1]["input_schema"]
            .to_string()
            .contains("\"minItems\":4")
    );
    // Headers: the key in its header, the version, no betas without the controls that need them.
    let (_, _) = (p.final_turn(), FinalTurn::Refuse);
}

#[tokio::test]
async fn anthropic_headers_carry_the_credential_and_betas_only_when_needed() {
    let response = json!({ "id": "msg_1", "content": [{ "type": "text", "text": "hi" }], "stop_reason": "end_turn", "usage": { "input_tokens": 1, "output_tokens": 1 } });
    let (p, t) = provider("claude-opus-5-5", "", response.clone());
    let (h, tl, im) = (history(), tools(), images());
    p.send(request(&h, &tl, &im)).await.unwrap();
    let (url, headers, body) = t.seen.lock().unwrap()[0].clone();
    assert_eq!(url, "https://api.anthropic.com/v1/messages");
    let get = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    assert_eq!(get("x-api-key").as_deref(), Some("sk-ant-test-key"));
    assert_eq!(get("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(get("anthropic-beta"), None);
    assert!(
        body.get("output_config").is_none(),
        "no effort control, none sent"
    );
    // The prefix check adds its beta and the body field (owner decision 2 during recording).
    let (p, t) = provider(
        "claude-opus-5-5",
        "prefix_mismatch_behavior = \"error\"",
        response,
    );
    p.send(request(&h, &tl, &im)).await.unwrap();
    let (_, headers, body) = t.seen.lock().unwrap()[0].clone();
    let beta = headers
        .iter()
        .find(|(k, _)| k == "anthropic-beta")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(beta, "thinking-binding-controls-2026-08-01");
    assert_eq!(
        body["thinking"],
        json!({ "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "error" } })
    );
}

#[test]
fn anthropic_final_turn_follows_the_configured_mechanism() {
    let mut h = history();
    h.push(Message {
        role: MessageRole::System,
        content: vec![ContentBlock::Text {
            text: "Final turn: call submit_verdict.".into(),
        }],
    });
    let (t, im) = (tools(), images());
    // refuse (the default): a mid-conversation system message, nothing else changes.
    let (p, _) = provider("claude-opus-5-5", "", json!({}));
    assert_eq!(p.final_turn(), FinalTurn::Refuse);
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["messages"][3],
        json!({ "role": "system", "content": [{ "type": "text", "text": "Final turn: call submit_verdict." }] })
    );
    assert!(body.get("tool_choice").is_none());
    // tool_removal: one block per tool other than submit_verdict, and the inline-tools beta.
    let (p, _) = provider(
        "claude-opus-5-5",
        "final_turn = \"tool_removal\"",
        json!({}),
    );
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["messages"][3]["content"],
        json!([
            { "type": "tool_removal", "tool": { "type": "tool_reference", "name": "regions" } },
            { "type": "text", "text": "Final turn: call submit_verdict." }
        ])
    );
    assert_eq!(
        body["tools"].as_array().unwrap().len(),
        2,
        "the tools array never changes"
    );
    // forced: Haiku accepts a forced tool choice on the final turn only.
    let (p, _) = provider("claude-haiku-5-5", "final_turn = \"forced\"", json!({}));
    let before = p.build_body(&request(&history(), &t, &im)).unwrap();
    assert!(before.get("tool_choice").is_none());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({ "type": "tool", "name": "submit_verdict" })
    );
    // Opus rejects forced tool choice: the catalog refuses the control before the adapter exists.
    let catalog = Catalog::shipped().unwrap();
    let err = catalog
        .check_config(&config("claude-opus-5-5", "final_turn = \"forced\""), false)
        .unwrap_err();
    assert!(err.to_string().contains("final_turn"), "{err}");
}

#[test]
fn anthropic_parses_responses_and_keeps_every_block_for_the_echo() {
    let (p, _) = provider("claude-opus-5-5", "", json!({}));
    let thinking = json!({ "type": "thinking", "thinking": "", "signature": "EqQB" });
    let body = json!({
        "id": "msg_1", "model": "claude-opus-5-5", "role": "assistant",
        "content": [thinking, { "type": "text", "text": "Checking." }, { "type": "tool_use", "id": "toolu_9", "name": "regions", "input": { "image": "candidate" } }],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 5 }
    });
    let r = p
        .parse_response(&body.to_string(), Some("req_1".into()))
        .unwrap();
    assert_eq!(r.text, vec!["Checking."]);
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "toolu_9");
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    let usage = r.usage.unwrap();
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.cache_read_tokens, 40);
    assert_eq!(usage.cache_write_tokens, 5);
    assert_eq!(r.provider_request_id.as_deref(), Some("req_1"));
    assert_eq!(r.content.len(), 3);
    assert!(
        matches!(&r.content[0], ContentBlock::Opaque { adapter, block } if adapter == ADAPTER_ID && block["signature"] == "EqQB")
    );
    // The tool_use block is kept as received, not rebuilt from the normalised arguments, so the
    // echo is byte-exact under the prefix check.
    assert!(
        matches!(&r.content[2], ContentBlock::Opaque { adapter, block } if adapter == ADAPTER_ID && block["type"] == "tool_use" && block["id"] == "toolu_9" && block["input"]["image"] == "candidate")
    );
    // Stop reasons.
    let stop = |s: &str, details: Value| {
        let mut b = json!({ "id": "m", "content": [], "stop_reason": s, "usage": { "input_tokens": 1, "output_tokens": 0 } });
        if !details.is_null() {
            b["stop_details"] = details;
        }
        p.parse_response(&b.to_string(), None).unwrap()
    };
    assert_eq!(
        stop("end_turn", Value::Null).stop_reason,
        StopReason::EndTurn
    );
    assert_eq!(
        stop("max_tokens", Value::Null).stop_reason,
        StopReason::MaxTokens
    );
    assert_eq!(
        stop("model_context_window_exceeded", Value::Null).stop_reason,
        StopReason::MaxTokens
    );
    let refused = stop(
        "refusal",
        json!({ "type": "refusal", "category": "cyber", "explanation": "no" }),
    );
    assert_eq!(refused.stop_reason, StopReason::Refusal);
    assert_eq!(refused.text, vec!["refusal: cyber (no)"]);
    assert_eq!(
        refused.provider_request_id.as_deref(),
        Some("m"),
        "the message id stands in"
    );
    assert_eq!(
        stop("pause_turn", Value::Null).stop_reason,
        StopReason::Other("pause_turn".into())
    );
}

#[tokio::test]
async fn anthropic_errors_become_transport_errors_and_preflight_runs_before_sending() {
    let (p, t) = provider("claude-opus-5-5", "", json!({}));
    *t.response.lock().unwrap() = Response {
        status: 429,
        headers: [("retry-after".to_string(), "4".to_string())].into_iter().collect(),
        body: json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "slow" }, "request_id": "req_x" }).to_string(),
    };
    let (h, tl, im) = (history(), tools(), images());
    match p.send(request(&h, &tl, &im)).await.unwrap_err() {
        squint_core::Error::Transport(e) => {
            assert_eq!(e.status, Some(429));
            assert_eq!(e.retry_after, Some(std::time::Duration::from_secs(4)));
            assert!(e.retryable());
            assert!(e.message.contains("rate_limit_error"));
        }
        other => panic!("{other}"),
    }
    *t.response.lock().unwrap() = Response { status: 400, headers: BTreeMap::new(), body: "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad\"}}".into() };
    match p.send(request(&h, &tl, &im)).await.unwrap_err() {
        squint_core::Error::Transport(e) => assert!(!e.retryable()),
        other => panic!("{other}"),
    }
    assert_eq!(t.seen.lock().unwrap().len(), 2);
    // An image the model cannot take is refused before any request.
    let mut bad = images();
    bad.get_mut("candidate").unwrap().media_type = "image/bmp".into();
    let err = p.send(request(&h, &tl, &bad)).await.unwrap_err();
    assert_eq!(err.class(), "image_limit", "{err}");
    assert_eq!(t.seen.lock().unwrap().len(), 2, "nothing was sent");
    // An opaque block from another adapter never reaches the wire.
    let mut foreign = history();
    foreign[1].content[0] = ContentBlock::Opaque {
        adapter: "openai-responses".into(),
        block: json!({ "type": "reasoning" }),
    };
    let err = p.send(request(&foreign, &tl, &im)).await.unwrap_err();
    assert_eq!(err.class(), "provider");
    assert_eq!(t.seen.lock().unwrap().len(), 2);
}

#[test]
fn anthropic_reports_recipient_pricing_and_preparation() {
    let (p, _) = provider("claude-sonnet-5-5", "", json!({}));
    assert_eq!(p.id(), ADAPTER_ID);
    let r = p.recipient().unwrap();
    assert_eq!(
        (
            r.profile.as_str(),
            r.host.as_str(),
            r.model.as_str(),
            r.upstream,
            r.api_version.as_str()
        ),
        (
            "anthropic",
            "api.anthropic.com",
            "claude-sonnet-5-5",
            None,
            "2023-06-01"
        )
    );
    assert_eq!(p.prep_version(), "1");
    let pricing = p.pricing().unwrap();
    let cost = pricing.cost(&squint_core::provider::Usage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    });
    assert!((cost - 2.0).abs() < 1e-9);
    let img =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(10, 8, image::Rgb([1, 2, 3])));
    let prepared = p.prepare_image("candidate", &img).unwrap();
    assert_eq!(
        (
            prepared.media_type.as_str(),
            prepared.width,
            prepared.height,
            prepared.prep_version.as_str()
        ),
        ("image/jpeg", 10, 8, "1")
    );
}

/// An answer without a usage block reports none: not zero tokens (table-manners 4.5).
#[test]
fn anthropic_without_usage_reports_none() {
    let (p, _) = provider("claude-opus-5-5", "", json!({}));
    let body = json!({ "id": "m", "content": [{ "type": "text", "text": "hi" }], "stop_reason": "end_turn" });
    let r = p.parse_response(&body.to_string(), None).unwrap();
    assert_eq!(r.usage, None);
}

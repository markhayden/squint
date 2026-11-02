//! Task 2.7a: the OpenAI Responses adapter sends function tools with the strict wire schema, pairs
//! function_call_output items by call_id, echoes every output item including reasoning, sends
//! `store = false` on every request, treats `incomplete` as non-replayable, and forces
//! submit_verdict on the final turn where configured.
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
use squint_providers::anthropic::FinalTurn;
use squint_providers::catalog::Catalog;
use squint_providers::http::{Response, Transport, TransportFuture};
use squint_providers::openai::{ADAPTER_ID, OpenAiProvider};
use squint_providers::privacy::credential;

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
        headers: [("x-request-id".to_string(), "req_o1".to_string())]
            .into_iter()
            .collect(),
        body: body.to_string(),
    }
}

fn config(model: &str, controls: &str) -> Config {
    let text = format!(
        "[provider]\nprofile = \"openai\"\nmodel = \"{model}\"\ncontrols = {{ {controls} }}\nmax_output_tokens = 4000\n\n[privacy]\ncloud_calls = true\naccount = \"openai:org-test\"\n\n[output]\ndir = \"out\"\n"
    );
    Config::parse(&text, &[], &|_| None).unwrap()
}

fn provider(model: &str, controls: &str, response: Value) -> (OpenAiProvider, Arc<Capture>) {
    let catalog = Catalog::shipped().unwrap();
    let config = config(model, controls);
    catalog.check_config(&config, false).unwrap();
    let profile = catalog.profile("openai").unwrap();
    let cred = credential(profile, &|_| Some("sk-test-key".into())).unwrap();
    let transport = Arc::new(Capture {
        seen: Mutex::new(Vec::new()),
        response: Mutex::new(ok_response(response)),
    });
    let p = OpenAiProvider::new(
        &config,
        "openai",
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
        input_schema: json!({ "type": "object", "additionalProperties": false, "required": ["image"], "properties": { "image": { "type": "string" }, "against": { "type": "string" } } }),
    }
}

fn reasoning_item() -> Value {
    json!({ "type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ENC1" })
}

fn call_item() -> Value {
    json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "regions", "arguments": "{\"image\":\"candidate\"}" })
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
            ],
        },
        Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Opaque {
                    adapter: ADAPTER_ID.into(),
                    block: reasoning_item(),
                },
                ContentBlock::Opaque {
                    adapter: ADAPTER_ID.into(),
                    block: call_item(),
                },
            ],
        },
        Message {
            role: MessageRole::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "{\"status\":\"ok\"}".into(),
                    is_error: false,
                    images: vec!["crop:call:1:1".into()],
                },
                ContentBlock::Text {
                    text: "checklist".into(),
                },
            ],
        },
    ]
}

/// The Responses API has no error flag on a function result, so a failed call is prefixed.
#[test]
fn openai_marks_a_failed_tool_result_in_its_text() {
    let (p, _) = provider("gpt-6.1-sol", "", json!({}));
    let mut h = history();
    if let ContentBlock::ToolResult {
        is_error, content, ..
    } = &mut h[2].content[0]
    {
        *is_error = true;
        *content = "regions: no such image".into();
    }
    let body = p.build_body(&request(&h, &tools(), &images())).unwrap();
    let output = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "function_call_output")
        .unwrap();
    let text = output["output"][0]["text"].as_str().unwrap();
    assert_eq!(text, "error: regions: no such image");
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
fn openai_request_body_maps_history_tools_and_controls_with_store_false() {
    let (p, _) = provider(
        "gpt-6.1-sol",
        "reasoning_effort = \"high\", image_detail = \"high\"",
        json!({}),
    );
    let (h, t, im) = (history(), tools(), images());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(body["model"], "gpt-6.1-sol");
    assert_eq!(body["instructions"], "You judge images.");
    assert_eq!(body["store"], false, "every request is stateless");
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["reasoning"], json!({ "effort": "high" }));
    assert_eq!(body["max_output_tokens"], 4000);
    assert_eq!(body["tool_choice"], "auto");
    for forbidden in [
        "temperature",
        "top_p",
        "metadata",
        "previous_response_id",
        "stream",
    ] {
        assert!(body.get(forbidden).is_none(), "{forbidden}");
    }
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 5, "{input:#?}");
    assert_eq!(input[0]["role"], "user");
    assert_eq!(
        input[0]["content"][1],
        json!({ "type": "input_image", "image_url": "data:image/jpeg;base64,SlBFRy1B", "detail": "high" })
    );
    // The model turn is echoed item for item, reasoning first.
    assert_eq!(input[1], reasoning_item());
    assert_eq!(input[2], call_item());
    // The function output is its own item, images included, then the user text message.
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(
        input[3]["output"][0],
        json!({ "type": "input_text", "text": "{\"status\":\"ok\"}" })
    );
    assert_eq!(
        input[3]["output"][1]["image_url"],
        "data:image/jpeg;base64,SlBFRy1C"
    );
    assert_eq!(
        input[4],
        json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "checklist" }] })
    );
    // Tools: strict with every property required and the optional one nullable.
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(
        tools[0]["parameters"]["required"],
        json!(["against", "image"])
    );
    assert_eq!(
        tools[0]["parameters"]["properties"]["against"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(tools[1]["name"], "submit_verdict");
}

#[test]
fn openai_parses_output_items_statuses_and_usage() {
    let (p, _) = provider("gpt-6.1-sol", "", json!({}));
    let body = json!({
        "id": "resp_1", "status": "completed",
        "output": [reasoning_item(), { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Checking.", "annotations": [] }] }, call_item()],
        "usage": { "input_tokens": 100, "input_tokens_details": { "cached_tokens": 30 }, "output_tokens": 50, "output_tokens_details": { "reasoning_tokens": 40 } }
    });
    let r = p
        .parse_response(&body.to_string(), Some("req_h".into()))
        .unwrap();
    assert_eq!(r.text, vec!["Checking."]);
    assert_eq!(r.tool_calls.len(), 1);
    assert_eq!(r.tool_calls[0].id, "call_1");
    assert_eq!(r.tool_calls[0].arguments, json!({ "image": "candidate" }));
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(r.content.len(), 3);
    assert!(
        matches!(&r.content[0], ContentBlock::Opaque { block, .. } if block["encrypted_content"] == "ENC1")
    );
    let usage = r.usage.unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.cache_read_tokens,
            usage.output_tokens
        ),
        (70, 30, 50)
    );
    assert_eq!(r.provider_request_id.as_deref(), Some("req_h"));
    let status = |status: &str, details: Value, output: Value| {
        let mut b = json!({ "id": "resp_x", "status": status, "output": output });
        if !details.is_null() {
            b["incomplete_details"] = details;
        }
        p.parse_response(&b.to_string(), None)
    };
    assert_eq!(
        status(
            "incomplete",
            json!({ "reason": "max_output_tokens" }),
            json!([])
        )
        .unwrap()
        .stop_reason,
        StopReason::MaxTokens
    );
    assert_eq!(
        status(
            "incomplete",
            json!({ "reason": "content_filter" }),
            json!([])
        )
        .unwrap()
        .stop_reason,
        StopReason::Refusal
    );
    let refused = status("completed", Value::Null, json!([{ "type": "message", "role": "assistant", "content": [{ "type": "refusal", "refusal": "no" }] }])).unwrap();
    assert_eq!(refused.stop_reason, StopReason::Refusal);
    assert_eq!(refused.text, vec!["refusal: no"]);
    assert_eq!(refused.provider_request_id.as_deref(), Some("resp_x"));
    assert_eq!(
        status("completed", Value::Null, json!([]))
            .unwrap()
            .stop_reason,
        StopReason::EndTurn
    );
    let failed = json!({ "id": "r", "status": "failed", "error": { "code": "server_error", "message": "boom" }, "output": [] });
    let err = p.parse_response(&failed.to_string(), None).unwrap_err();
    assert_eq!(err.class(), "provider");
    assert!(err.to_string().contains("boom"));
    // Arguments that are not JSON stay a string for the argument check to reject.
    let odd = json!({ "id": "r", "status": "completed", "output": [{ "type": "function_call", "call_id": "c", "name": "regions", "arguments": "{not json" }] });
    assert_eq!(
        p.parse_response(&odd.to_string(), None).unwrap().tool_calls[0].arguments,
        json!("{not json")
    );
}

#[test]
fn openai_final_turn_is_a_developer_message_or_a_forced_function() {
    let mut h = history();
    h.push(Message {
        role: MessageRole::System,
        content: vec![ContentBlock::Text {
            text: "Final turn.".into(),
        }],
    });
    let (t, im) = (tools(), images());
    let (p, _) = provider("gpt-6.1-sol", "", json!({}));
    assert_eq!(p.final_turn(), FinalTurn::Refuse);
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["input"].as_array().unwrap().last().unwrap(),
        &json!({ "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Final turn." }] })
    );
    assert_eq!(body["tool_choice"], "auto");
    let (p, _) = provider("gpt-6.1-terra", "final_turn = \"forced\"", json!({}));
    let before = p.build_body(&request(&history(), &t, &im)).unwrap();
    assert_eq!(before["tool_choice"], "auto");
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({ "type": "function", "name": "submit_verdict" })
    );
}

#[tokio::test]
async fn openai_sends_a_bearer_token_runs_preflight_first_and_maps_transport_errors() {
    let response = json!({ "id": "resp_1", "status": "completed", "output": [], "usage": { "input_tokens": 1, "output_tokens": 1 } });
    let (p, t) = provider("gpt-6.1-sol", "", response);
    let (h, tl, im) = (history(), tools(), images());
    let r = p.send(request(&h, &tl, &im)).await.unwrap();
    assert_eq!(r.provider_request_id.as_deref(), Some("req_o1"));
    let (url, headers, body) = t.seen.lock().unwrap()[0].clone();
    assert_eq!(url, "https://api.openai.com/v1/responses");
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test-key")
    );
    assert_eq!(body["store"], false);
    *t.response.lock().unwrap() = Response {
        status: 429,
        headers: [("retry-after".to_string(), "2".to_string())]
            .into_iter()
            .collect(),
        body: "{\"error\":{\"message\":\"rate\",\"type\":\"rate_limit_error\"}}".into(),
    };
    match p.send(request(&h, &tl, &im)).await.unwrap_err() {
        squint_core::Error::Transport(e) => {
            assert_eq!(e.status, Some(429));
            assert_eq!(e.retry_after, Some(std::time::Duration::from_secs(2)));
        }
        other => panic!("{other}"),
    }
    let mut bad = images();
    bad.get_mut("candidate").unwrap().media_type = "image/bmp".into();
    assert_eq!(
        p.send(request(&h, &tl, &bad)).await.unwrap_err().class(),
        "image_limit"
    );
    assert_eq!(t.seen.lock().unwrap().len(), 2);
    let r = p.recipient().unwrap();
    assert_eq!(
        (r.host.as_str(), r.model.as_str()),
        ("api.openai.com", "gpt-6.1-sol")
    );
    assert_eq!(p.prep_version(), "1");
    let foreign_tool = ToolDefinition {
        name: "odd".into(),
        description: "d".into(),
        input_schema: json!({ "type": "object", "properties": { "x": { "type": "string" } }, "patternProperties": {} }),
    };
    // The tool set is transformed once per conversation, so a fresh adapter sees the bad tool.
    let (fresh, _) = provider("gpt-6.1-sol", "", json!({}));
    let err = fresh
        .build_body(&request(&h, &[foreign_tool], &im))
        .unwrap_err();
    assert_eq!(err.class(), "resolve", "{err}");
}

/// An answer without a usage block reports none: not zero tokens (table-manners 4.5).
#[test]
fn openai_without_usage_reports_none() {
    let (p, _) = provider("gpt-6.1-sol", "", json!({}));
    let body = json!({ "id": "resp_1", "status": "completed", "output": [{ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "hi", "annotations": [] }] }] });
    let r = p.parse_response(&body.to_string(), None).unwrap();
    assert_eq!(r.usage, None);
}

/// F3: an OpenAI 400 names no cause only when its body is empty or the error envelope has no
/// message, no `param` and no `code`; any of them is a cause.
#[tokio::test]
async fn openai_a_400_names_no_cause_only_without_message_param_or_code() {
    let (p, t) = provider("gpt-6.1-sol", "", json!({}));
    let (h, tl, im) = (history(), tools(), images());
    let envelope = |message: &str, param: Value, code: Value| {
        json!({ "error": { "message": message, "type": "invalid_request_error", "param": param, "code": code } }).to_string()
    };
    let cases = [
        (400, String::new(), true),
        (400, envelope("", Value::Null, Value::Null), true),
        (
            400,
            envelope(
                "Invalid image.",
                json!("input[0].content[1].image_url"),
                json!("invalid_value"),
            ),
            false,
        ),
        (400, envelope("", json!("input"), Value::Null), false),
        (
            400,
            envelope("", Value::Null, json!("invalid_value")),
            false,
        ),
        (
            400,
            envelope(
                "We could not parse the JSON body of your request.",
                Value::Null,
                Value::Null,
            ),
            false,
        ),
        (400, "Bad Request".to_string(), false),
        (422, String::new(), false),
    ];
    for (status, body, causeless) in cases {
        *t.response.lock().unwrap() = Response {
            status,
            headers: BTreeMap::new(),
            body: body.clone(),
        };
        match p.send(request(&h, &tl, &im)).await.unwrap_err() {
            squint_core::Error::Transport(e) => {
                assert_eq!(e.causeless_rejection(), causeless, "{status} {body:?}");
                assert!(!e.retryable(), "{status} {body:?}");
            }
            other => panic!("{other}"),
        }
    }
}

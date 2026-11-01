//! Task 2.6a: the Gemini generateContent adapter maps history to contents, pairs functionResponse
//! parts by id (with the catalog exception for responses without ids), echoes every model part
//! so thought signatures ride along, declares tools with the wire schema, uses function-calling
//! mode for the forced final turn, and a moved or altered signature fails replay.
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
use squint_providers::gemini::{ADAPTER_ID, GeminiProvider, SYNTHETIC_ID_PREFIX};
use squint_providers::http::{Response, Transport, TransportFuture};
use squint_providers::privacy::credential;
use squint_providers::replay::{Exchange, Player, RecordedRequest, RecordedResponse, Volatile};

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
        headers: BTreeMap::new(),
        body: body.to_string(),
    }
}

fn config(model: &str, controls: &str) -> Config {
    let text = format!(
        "[provider]\nprofile = \"google-ai-studio\"\nmodel = \"{model}\"\ncontrols = {{ {controls} }}\nmax_output_tokens = 4000\n\n[privacy]\ncloud_calls = true\naccount = \"google:proj-test\"\n\n[output]\ndir = \"out\"\n"
    );
    Config::parse(&text, &[], &|_| None).unwrap()
}

fn provider(model: &str, controls: &str, response: Value) -> (GeminiProvider, Arc<Capture>) {
    let catalog = Catalog::shipped().unwrap();
    let config = config(model, controls);
    catalog.check_config(&config, false).unwrap();
    let profile = catalog.profile("google-ai-studio").unwrap();
    let cred = credential(profile, &|_| Some("AIza-test-key".into())).unwrap();
    let transport = Arc::new(Capture {
        seen: Mutex::new(Vec::new()),
        response: Mutex::new(ok_response(response)),
    });
    let p = GeminiProvider::new(
        &config,
        "google-ai-studio",
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

fn call_part(id: Option<&str>) -> Value {
    let mut fc = json!({ "name": "regions", "args": { "image": "candidate" } });
    if let Some(id) = id {
        fc["id"] = json!(id);
    }
    json!({ "functionCall": fc, "thoughtSignature": "U0lHMQ==" })
}

fn history(call_id: &str, model_parts: Vec<Value>) -> Vec<Message> {
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
            content: model_parts
                .into_iter()
                .map(|p| ContentBlock::Opaque {
                    adapter: ADAPTER_ID.into(),
                    block: p,
                })
                .collect(),
        },
        Message {
            role: MessageRole::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: call_id.into(),
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
fn gemini_request_body_maps_history_with_ids_images_tools_and_controls() {
    let (p, _) = provider(
        "gemini-4-argon",
        "thinking_level = \"high\", media_resolution = \"high\"",
        json!({}),
    );
    let h = history(
        "fc_1",
        vec![json!({ "text": "Looking." }), call_part(Some("fc_1"))],
    );
    let (t, im) = (tools(), images());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["systemInstruction"],
        json!({ "parts": [{ "text": "You judge images." }] })
    );
    assert_eq!(
        body["generationConfig"],
        json!({ "maxOutputTokens": 4000, "thinkingConfig": { "thinkingLevel": "high" } })
    );
    assert_eq!(
        body["toolConfig"],
        json!({ "functionCallingConfig": { "mode": "AUTO" } })
    );
    for forbidden in ["temperature", "topP", "topK", "cachedContent"] {
        assert!(
            body["generationConfig"].get(forbidden).is_none() && body.get(forbidden).is_none(),
            "{forbidden}"
        );
    }
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(
        contents[0]["parts"][1],
        json!({ "inlineData": { "mimeType": "image/jpeg", "data": "SlBFRy1B" }, "mediaResolution": { "level": "MEDIA_RESOLUTION_HIGH" } })
    );
    // The model turn is echoed part for part, signature included.
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][1], call_part(Some("fc_1")));
    // The function response carries the call's id and name, the crop as a part, then the text.
    let fr = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(fr["id"], "fc_1");
    assert_eq!(fr["name"], "regions");
    assert_eq!(fr["response"], json!({ "output": "{\"status\":\"ok\"}" }));
    assert_eq!(fr["parts"][0]["inlineData"]["data"], "SlBFRy1C");
    assert_eq!(contents[2]["parts"][1], json!({ "text": "checklist" }));
    // Tools: one functionDeclarations group with the wire schema (references inlined).
    let decls = body["tools"][0]["functionDeclarations"].as_array().unwrap();
    assert_eq!(decls.len(), 2);
    assert_eq!(decls[1]["name"], "submit_verdict");
    assert!(
        !decls[1]["parametersJsonSchema"]
            .to_string()
            .contains("$ref")
    );
    assert!(decls[1].get("strict").is_none());
}

#[test]
fn gemini_calls_without_ids_are_accepted_singly_and_answered_without_an_id() {
    let (p, _) = provider("gemini-3.8-flash", "", json!({}));
    let one = json!({ "candidates": [{ "content": { "role": "model", "parts": [call_part(None)] }, "finishReason": "STOP" }], "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 2 }, "responseId": "r1" });
    let r = p.parse_response(&one.to_string(), None).unwrap();
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(r.tool_calls.len(), 1);
    assert!(
        r.tool_calls[0].id.starts_with(SYNTHETIC_ID_PREFIX),
        "{}",
        r.tool_calls[0].id
    );
    assert_eq!(r.provider_request_id.as_deref(), Some("r1"));
    // The echoed turn and its result pair up by the synthetic id; the wire carries no id.
    let h = history(&r.tool_calls[0].id, vec![call_part(None)]);
    let (t, im) = (tools(), images());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    let fr = &body["contents"][2]["parts"][0]["functionResponse"];
    assert!(fr.get("id").is_none());
    assert_eq!(fr["name"], "regions");
    // Two calls without ids cannot be paired: error: provider.
    let two = json!({ "candidates": [{ "content": { "parts": [call_part(None), call_part(None)] }, "finishReason": "STOP" }] });
    let err = p.parse_response(&two.to_string(), None).unwrap_err();
    assert_eq!(err.class(), "provider");
    assert!(err.to_string().contains("without an id"), "{err}");
    // A result for a call the history never made is refused.
    let orphan = history("fc_missing", vec![json!({ "text": "hm" })]);
    let err = p.build_body(&request(&orphan, &t, &im)).unwrap_err();
    assert!(err.to_string().contains("answers no call"), "{err}");
}

#[test]
fn gemini_parses_finish_reasons_usage_and_thoughts() {
    let (p, _) = provider("gemini-4-argon", "", json!({}));
    let body = json!({
        "candidates": [{ "content": { "role": "model", "parts": [
            { "text": "thinking…", "thought": true, "thoughtSignature": "U0lH" },
            { "text": "Done." }
        ] }, "finishReason": "STOP" }],
        "usageMetadata": { "promptTokenCount": 120, "cachedContentTokenCount": 20, "candidatesTokenCount": 5, "thoughtsTokenCount": 30 }
    });
    let r = p
        .parse_response(&body.to_string(), Some("x-1".into()))
        .unwrap();
    assert_eq!(r.text, vec!["Done."], "thought text is not model text");
    assert_eq!(r.content.len(), 2, "every part is echoed");
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(
        (
            r.usage.input_tokens,
            r.usage.cache_read_tokens,
            r.usage.output_tokens
        ),
        (100, 20, 35)
    );
    assert_eq!(r.provider_request_id.as_deref(), Some("x-1"));
    let finish = |reason: &str| {
        let b = json!({ "candidates": [{ "content": { "parts": [] }, "finishReason": reason, "finishMessage": "m" }] });
        p.parse_response(&b.to_string(), None).unwrap()
    };
    assert_eq!(finish("MAX_TOKENS").stop_reason, StopReason::MaxTokens);
    let safety = finish("SAFETY");
    assert_eq!(safety.stop_reason, StopReason::Refusal);
    assert_eq!(safety.text, vec!["refusal: SAFETY (m)"]);
    assert_eq!(
        finish("MALFORMED_FUNCTION_CALL").stop_reason,
        StopReason::Other("MALFORMED_FUNCTION_CALL".into())
    );
    let blocked = json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" }, "usageMetadata": { "promptTokenCount": 3 } });
    let r = p.parse_response(&blocked.to_string(), None).unwrap();
    assert_eq!(r.stop_reason, StopReason::Refusal);
    assert!(r.text[0].contains("PROHIBITED_CONTENT"));
}

#[test]
fn gemini_final_turn_closes_the_user_content_or_forces_submit_verdict() {
    let mut h = history("fc_1", vec![call_part(Some("fc_1"))]);
    h.push(Message {
        role: MessageRole::System,
        content: vec![ContentBlock::Text {
            text: "Final turn.".into(),
        }],
    });
    let (t, im) = (tools(), images());
    let (p, _) = provider("gemini-4-argon", "", json!({}));
    assert_eq!(p.final_turn(), FinalTurn::Refuse);
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(
        contents.len(),
        3,
        "no system role: the instruction joins the last user content"
    );
    assert_eq!(
        contents[2]["parts"].as_array().unwrap().last().unwrap(),
        &json!({ "text": "Final turn." })
    );
    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    let (p, _) = provider("gemini-4-argon", "final_turn = \"forced\"", json!({}));
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"],
        json!({ "mode": "ANY", "allowedFunctionNames": ["submit_verdict"] })
    );
    let err = Catalog::shipped()
        .unwrap()
        .check_config(
            &config("gemini-4-argon", "final_turn = \"tool_removal\""),
            false,
        )
        .unwrap_err();
    assert!(err.to_string().contains("final_turn"), "{err}");
}

#[tokio::test]
async fn gemini_sends_the_key_in_its_header_and_runs_preflight_first() {
    let response = json!({ "candidates": [{ "content": { "parts": [{ "text": "hi" }] }, "finishReason": "STOP" }], "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1 } });
    let (p, t) = provider("gemini-4-argon", "", response);
    let h = history("fc_1", vec![call_part(Some("fc_1"))]);
    let (tl, im) = (tools(), images());
    p.send(request(&h, &tl, &im)).await.unwrap();
    let (url, headers, _) = t.seen.lock().unwrap()[0].clone();
    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-argon:generateContent"
    );
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "x-goog-api-key" && v == "AIza-test-key")
    );
    assert!(!url.contains("key="), "the key never travels in the URL");
    let mut bad = images();
    bad.get_mut("candidate").unwrap().media_type = "image/bmp".into();
    let err = p.send(request(&h, &tl, &bad)).await.unwrap_err();
    assert_eq!(err.class(), "image_limit");
    assert_eq!(t.seen.lock().unwrap().len(), 1);
    assert_eq!(
        p.recipient().unwrap().host,
        "generativelanguage.googleapis.com"
    );
    assert_eq!(p.prep_version(), "1");
}

#[tokio::test]
async fn gemini_replay_fails_when_a_thought_signature_moves_or_changes() {
    let (p, _) = provider("gemini-4-argon", "", json!({}));
    let h = history(
        "fc_1",
        vec![json!({ "text": "Looking." }), call_part(Some("fc_1"))],
    );
    let (t, im) = (tools(), images());
    let recorded = p.build_body(&request(&h, &t, &im)).unwrap();
    let url =
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-argon:generateContent";
    let exchange = Exchange {
        request: RecordedRequest {
            url: url.into(),
            headers: BTreeMap::new(),
            body: recorded,
        },
        response: RecordedResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({ "candidates": [] }),
        },
    };
    // The same history replays.
    let player = Player::new(vec![exchange.clone()], Volatile::common());
    let body = p.build_body(&request(&h, &t, &im)).unwrap();
    player.post_json(url, &[], &body).await.unwrap();
    // The signature moved to the other part: the echo differs and the replay fails at the part.
    let mut moved_parts = vec![
        json!({ "text": "Looking.", "thoughtSignature": "U0lHMQ==" }),
        call_part(Some("fc_1")),
    ];
    moved_parts[1]
        .as_object_mut()
        .unwrap()
        .remove("thoughtSignature");
    let moved = history("fc_1", moved_parts);
    let player = Player::new(vec![exchange.clone()], Volatile::common());
    let err = player
        .post_json(url, &[], &p.build_body(&request(&moved, &t, &im)).unwrap())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("/contents/1/parts/0"), "{err}");
    // An altered signature fails at the field.
    let mut altered = call_part(Some("fc_1"));
    altered["thoughtSignature"] = json!("dGFtcGVyZWQ=");
    let altered = history("fc_1", vec![json!({ "text": "Looking." }), altered]);
    let player = Player::new(vec![exchange], Volatile::common());
    let err = player
        .post_json(
            url,
            &[],
            &p.build_body(&request(&altered, &t, &im)).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("/contents/1/parts/1/thoughtSignature"),
        "{err}"
    );
}

#[test]
fn gemini_fills_null_fields_the_model_left_out_of_a_submission() {
    // Once the tools are known, a submission with null-valued fields omitted is completed.
    let (p, _) = provider("gemini-4-argon", "", json!({}));
    let (t, im) = (tools(), images());
    p.build_body(&request(
        &history("fc_1", vec![call_part(Some("fc_1"))]),
        &t,
        &im,
    ))
    .unwrap();
    let submit = json!({ "candidates": [{ "content": { "parts": [{ "functionCall": { "id": "fc_9", "name": "submit_verdict", "args": {
        "schema_version": 1, "findings": [], "opinion": { "verdict": "pass", "reason": "r" },
        "items": [{ "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "r", "evidence": [] }]
    } } }] }, "finishReason": "STOP" }] });
    let r = p.parse_response(&submit.to_string(), None).unwrap();
    let item = &r.tool_calls[0].arguments["items"][0];
    assert_eq!(item["allowed_by"], Value::Null);
    assert_eq!(item["location"], Value::Null);
    squint_core::submission::validate_schema(&r.tool_calls[0].arguments).unwrap();
}

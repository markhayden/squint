//! F1: a conversation whose tool images can carry it past the model's many-image threshold is
//! planned at resolve, through the Anthropic adapter and its real preflight: prepared at the
//! many-image size from the first request when it can pass the threshold, refused with no request
//! sent when even that cannot fit the per-request count.
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use image::{DynamicImage, Rgb};
use serde_json::{Value, json};
use squint_core::config::Config;
use squint_core::image::base64_decode;
use squint_core::job::Job;
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_providers::anthropic::AnthropicProvider;
use squint_providers::catalog::Catalog;
use squint_providers::http::{Response, Transport, TransportFuture};
use squint_providers::privacy::credential;
use squint_testkit::ScriptedTool;

/// Answers each request with the next scripted body and keeps every request body.
struct Script {
    seen: Mutex<Vec<Value>>,
    bodies: Mutex<VecDeque<Value>>,
}

impl Transport for Script {
    fn post_json<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(String, String)],
        body: &'a Value,
    ) -> TransportFuture<'a> {
        Box::pin(async move {
            self.seen.lock().unwrap().push(body.clone());
            let next = self.bodies.lock().unwrap().pop_front().unwrap();
            Ok(Response {
                status: 200,
                headers: Default::default(),
                body: next.to_string(),
            })
        })
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-many-images-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pair whose candidate is 2100 pixels long: under claude-opus-5-5's native 2576, over its
/// many-image limit of 2000 (thin, so the debug-build resize stays fast).
fn pair_job(dir: &Path) -> Job {
    let gradient = |w: u32, h: u32| {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            Rgb([(x % 256) as u8, (y * 9 % 256) as u8, ((x + y) % 256) as u8])
        }))
    };
    gradient(2100, 20).save(dir.join("candidate.png")).unwrap();
    gradient(64, 48).save(dir.join("source.png")).unwrap();
    let path = dir.join("job.toml");
    std::fs::write(
        &path,
        "[job]\nuse_case = \"source-fidelity\"\nbrief = \"Keep the structure.\"\n\n\
         [[job.images]]\nid = \"candidate\"\nrole = \"candidate\"\nsource = { path = \"candidate.png\" }\n\n\
         [[job.images]]\nid = \"source\"\nrole = \"source\"\nsource = { path = \"source.png\" }\n",
    )
    .unwrap();
    Job::load(&path).unwrap()
}

fn config(tool_calls: u32) -> Config {
    let text = "[provider]\nprofile = \"anthropic\"\nmodel = \"claude-opus-5-5\"\nmax_output_tokens = 4000\n\n\
                [tools]\nenabled = [\"fake_regions\"]\n\n\
                [privacy]\ncloud_calls = true\naccount = \"anthropic:org-test\"\n\n\
                [output]\ndir = \"out\"\n";
    let overrides = vec![
        format!("budget.tool_calls={tool_calls}"),
        "budget.spend_cap_usd=100".to_string(),
    ];
    Config::parse(text, &overrides, &|_| None).unwrap()
}

fn provider(config: &Config, bodies: Vec<Value>) -> (AnthropicProvider, Arc<Script>) {
    let catalog = Catalog::shipped().unwrap();
    catalog.check_config(config, false).unwrap();
    let profile = catalog.profile("anthropic").unwrap();
    let cred = credential(profile, &|_| Some("sk-ant-test-key".into())).unwrap();
    let transport = Arc::new(Script {
        seen: Mutex::new(Vec::new()),
        bodies: Mutex::new(bodies.into()),
    });
    let p = AnthropicProvider::new(
        config,
        "anthropic",
        profile,
        catalog.model("claude-opus-5-5").unwrap(),
        cred,
        transport.clone(),
    )
    .unwrap();
    (p, transport)
}

/// The fake regions tool returning a 32x24 crop of the candidate on each of `calls` calls.
fn crop_tool(calls: u32) -> ToolRegistry {
    let text =
        std::fs::read_to_string(root().join("fixtures/tool-scripts/crop-image.json")).unwrap();
    let crop: Value = serde_json::from_str::<Vec<Value>>(&text).unwrap().remove(0);
    let envelopes: Vec<Envelope> = (1..=calls)
        .map(|n| {
            let mut e = crop.clone();
            e["images"][0]["frame"] = json!(format!("crop:call:{n}:0"));
            serde_json::from_value(e).unwrap()
        })
        .collect();
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(envelopes))];
    ToolRegistry::new(tools, &["fake_regions".into()], &[]).unwrap()
}

fn usage() -> Value {
    json!({ "input_tokens": 1000, "output_tokens": 50 })
}

/// One turn of `calls` region calls on the candidate.
fn tool_turn(calls: u32) -> Value {
    let content: Vec<Value> = (1..=calls)
        .map(|i| json!({ "type": "tool_use", "id": format!("toolu_{i}"), "name": "fake_regions", "input": { "image": "candidate" } }))
        .collect();
    json!({ "id": "msg_1", "content": content, "stop_reason": "tool_use", "usage": usage() })
}

fn submission() -> Value {
    let item = |id: &str| json!({ "id": id, "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": [] });
    json!({
        "id": "msg_2",
        "content": [{ "type": "tool_use", "id": "toolu_submit", "name": "submit_verdict", "input": {
            "schema_version": 1,
            "items": [item("chk:ob:SF1@candidate"), item("chk:ob:SF2@candidate")],
            "findings": [],
            "opinion": { "verdict": "pass", "reason": "scripted" }
        } }],
        "stop_reason": "tool_use",
        "usage": usage()
    })
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn report(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
}

/// The images of a request body, in order, as (width, height) of the decoded bytes.
fn image_sizes(body: &Value) -> Vec<(u32, u32)> {
    fn walk(v: &Value, out: &mut Vec<(u32, u32)>) {
        match v {
            Value::Object(m) => {
                if m.get("type") == Some(&json!("image"))
                    && let Some(data) = m["source"]["data"].as_str()
                {
                    let bytes = base64_decode(data).unwrap();
                    let img = image::load_from_memory(&bytes).unwrap();
                    out.push((img.width(), img.height()));
                }
                m.values().for_each(|v| walk(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

#[test]
fn a_tool_budget_that_can_reach_21_images_prepares_at_2000_pixels_from_the_first_request() {
    // Two job images and 19 calls of one image each: 21 images, over the threshold of 20. At the
    // usual size the 2100-pixel candidate fails the second request's preflight, mid-run.
    let dir = tempdir("prepared-smaller");
    let job = pair_job(&dir);
    let config = config(19);
    let (p, t) = provider(&config, vec![tool_turn(19), submission()]);
    let tools = crop_tool(19);
    let o = block_on(run::check(
        &config,
        &job,
        &dir,
        &run::Services::new(&p, &tools),
        &dir.join("out"),
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let seen = t.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        image_sizes(&seen[0]),
        [(2000, 19), (64, 48)],
        "the candidate is prepared at the many-image size from the first request"
    );
    let second = image_sizes(&seen[1]);
    assert_eq!(second.len(), 21, "the history crossed the threshold");
    assert!(second.iter().all(|(w, h)| *w.max(h) <= 2000));
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["prep_version"], "1+many2000");
    assert_eq!(r["cost"]["overrun_usd"], 0.0);
    assert!(
        r["cost"]["spent_usd"].as_f64().unwrap() <= r["cost"]["envelope"]["usd"].as_f64().unwrap()
    );
}

#[test]
fn a_tool_budget_past_the_per_request_count_is_refused_with_no_request_sent() {
    // 2 + 599 = 601 images, over claude-opus-5-5's 600 per request.
    let dir = tempdir("refused");
    let job = pair_job(&dir);
    let config = config(599);
    let (p, t) = provider(&config, vec![tool_turn(1), submission()]);
    let tools = crop_tool(1);
    let o = block_on(run::check(
        &config,
        &job,
        &dir,
        &run::Services::new(&p, &tools),
        &dir.join("out"),
        "test",
    ))
    .unwrap();
    assert_ne!(o.exit_code, 0);
    assert!(t.seen.lock().unwrap().is_empty(), "no request was sent");
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["error"]["class"], "image_limit");
    assert_eq!(r["cost"]["spent_usd"], 0.0);
    assert_eq!(r["cost"]["attempts"], 0);
}

#[test]
fn a_tool_budget_that_cannot_reach_the_threshold_prepares_as_before() {
    // 2 + 18 = 20 images: not above the threshold, so the candidate keeps its 2100 pixels.
    let dir = tempdir("as-before");
    let job = pair_job(&dir);
    let config = config(18);
    let (p, t) = provider(&config, vec![tool_turn(1), submission()]);
    let tools = crop_tool(1);
    let o = block_on(run::check(
        &config,
        &job,
        &dir,
        &run::Services::new(&p, &tools),
        &dir.join("out"),
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert_eq!(
        image_sizes(&t.seen.lock().unwrap()[0]),
        [(2100, 20), (64, 48)]
    );
    assert_eq!(report(&o.dir)["runs"][0]["prep_version"], "1");
}

//! A scripted fake provider: answers each turn with the next canned response from a JSON file,
//! so the run loop can be driven through every state offline. An entry may instead be a
//! transport error (`{"error": {"status": 429, "retry_after_s": 2}}`, with `"sent": false` for a
//! connection that never carried the request and `"causeless": true` for a 400 whose body names
//! no cause) to drive the retries, or a stall
//! (`{"stall_s": 30}`: the next entry comes only after that long, a call in flight). A response
//! without `usage` reported zero tokens; `"usage": null` is an answer that reported none. Given a
//! model's per-request image limits ([`ScriptedProvider::with_image_limits`]), it prices with them
//! and refuses a request over them the way an adapter's preflight does.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use squint_core::image::RequestImageLimits;
use squint_core::ledger::{Prices, Pricing, RequestShape};
use squint_core::provider::{
    ContentBlock, Message, Provider, SendFuture, ToolDefinition, TurnRequest, TurnResponse, Usage,
};
use squint_core::retry::TransportError;

use crate::{Error, Result};

/// One scripted turn: a response, or a transport failure.
#[derive(Debug, Clone)]
pub enum ScriptTurn {
    Response(TurnResponse),
    Error(TransportError),
    /// The call hangs this long before the next entry answers it.
    Stall(Duration),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Error { error: ScriptError },
    Stall { stall_s: u64 },
    Response(TurnResponse),
}

#[derive(Deserialize)]
struct ScriptError {
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    retry_after_s: Option<u64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    sent: Option<bool>,
    #[serde(default)]
    causeless: bool,
}

/// The stub catalog entry the scripted provider prices with: cheap enough that the default
/// envelope fits the default cap, dear enough that a one-cent cap refuses it.
pub const FAKE_PRICES: Prices = Prices {
    input_per_million: 0.10,
    output_per_million: 0.40,
    per_image: 0.0002,
};

/// What a scripted provider prices with: [`FAKE_PRICES`], and the image limits of the model it
/// stands in for, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedPricing {
    pub prices: Prices,
    pub image_limits: Option<RequestImageLimits>,
}

impl Pricing for ScriptedPricing {
    fn bound(&self, shape: &RequestShape) -> squint_core::Result<f64> {
        self.prices.bound(shape)
    }

    fn cost(&self, usage: &Usage) -> f64 {
        self.prices.cost(usage)
    }

    fn image_limits(&self) -> Option<RequestImageLimits> {
        self.image_limits
    }
}

/// Replays a list of scripted turns in order. Exhausting the script is a provider error, which
/// is what a test wants when the harness asks for more turns than expected.
pub struct ScriptedProvider {
    turns: Mutex<VecDeque<ScriptTurn>>,
    /// Every request's history length, for tests that assert the loop's shape.
    pub seen_history_lengths: Mutex<Vec<usize>>,
    /// Every request as sent, for tests that assert the provider history is append-only.
    pub recorded: Mutex<Vec<RecordedRequest>>,
    /// A file every request appends a line to, so a test in another process can count calls.
    calls_log: Option<std::path::PathBuf>,
    pricing: ScriptedPricing,
}

/// The variable a testkit build reads for [`ScriptedProvider::with_calls_log`]'s file.
pub const CALLS_LOG_ENV: &str = "SQUINT_TESTKIT_PROVIDER_CALLS";

/// A request as the scripted provider saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRequest {
    pub system: String,
    pub history: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_output_tokens: u32,
    /// The prepared size (width, height) of every image the request carried, by content-block id.
    pub image_sizes: std::collections::BTreeMap<String, (u32, u32)>,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<TurnResponse>) -> ScriptedProvider {
        ScriptedProvider::from_turns(turns.into_iter().map(ScriptTurn::Response).collect())
    }

    pub fn from_turns(turns: Vec<ScriptTurn>) -> ScriptedProvider {
        ScriptedProvider {
            turns: Mutex::new(turns.into()),
            seen_history_lengths: Mutex::new(Vec::new()),
            recorded: Mutex::new(Vec::new()),
            calls_log: None,
            pricing: ScriptedPricing {
                prices: FAKE_PRICES,
                image_limits: None,
            },
        }
    }

    /// Stands in for a model with these per-request image limits: resolve plans the images
    /// against them, and a request over them is refused with `image_limit` before it is answered.
    pub fn with_image_limits(mut self, limits: RequestImageLimits) -> ScriptedProvider {
        self.pricing.image_limits = Some(limits);
        self
    }

    /// Appends a line to `path` on every request: the call counter a test outside this process
    /// reads (the proof that describing and estimating call no provider, table-manners 7).
    pub fn with_calls_log(mut self, path: Option<std::path::PathBuf>) -> ScriptedProvider {
        self.calls_log = path;
        self
    }

    /// Parses a JSON array of turn responses and transport errors.
    pub fn from_json(text: &str) -> Result<ScriptedProvider> {
        let entries: Vec<Entry> =
            serde_json::from_str(text).map_err(|e| Error::Script(e.to_string()))?;
        let turns = entries
            .into_iter()
            .map(|e| match e {
                Entry::Response(r) => ScriptTurn::Response(r),
                Entry::Error { error } => ScriptTurn::Error(TransportError {
                    status: error.status,
                    retry_after: error.retry_after_s.map(Duration::from_secs),
                    message: error.message.unwrap_or_else(|| "scripted failure".into()),
                    sent: error.sent.unwrap_or(true),
                    causeless: error.causeless,
                }),
                Entry::Stall { stall_s } => ScriptTurn::Stall(Duration::from_secs(stall_s)),
            })
            .collect();
        Ok(ScriptedProvider::from_turns(turns))
    }

    /// Loads a script file.
    pub fn from_path(path: &Path) -> Result<ScriptedProvider> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Script(format!("cannot read {}: {e}", path.display())))?;
        ScriptedProvider::from_json(&text)
            .map_err(|e| Error::Script(format!("{}: {e}", path.display())))
    }
}

impl ScriptedProvider {
    /// Turns not yet consumed.
    pub fn remaining(&self) -> usize {
        self.turns.lock().map(|t| t.len()).unwrap_or(0)
    }
}

impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    fn pricing(&self) -> Option<&dyn Pricing> {
        Some(&self.pricing)
    }

    fn send<'a>(&'a self, req: TurnRequest<'a>) -> SendFuture<'a> {
        Box::pin(async move {
            if let Some(log) = &self.calls_log {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                {
                    let _ = writeln!(file, "call");
                }
            }
            if let Ok(mut seen) = self.seen_history_lengths.lock() {
                seen.push(req.history.len());
            }
            if let Ok(mut rec) = self.recorded.lock() {
                rec.push(RecordedRequest {
                    system: req.system.to_string(),
                    history: req.history.to_vec(),
                    tools: req.tools.to_vec(),
                    max_output_tokens: req.max_output_tokens,
                    image_sizes: req
                        .images
                        .iter()
                        .map(|(id, p)| (id.clone(), (p.width, p.height)))
                        .collect(),
                });
            }
            if let Some(limits) = self.pricing.image_limits {
                over_limits(&limits, &req)?;
            }
            let mut next = self.turns.lock().ok().and_then(|mut t| t.pop_front());
            if let Some(ScriptTurn::Stall(d)) = next {
                tokio::time::sleep(d).await;
                next = self.turns.lock().ok().and_then(|mut t| t.pop_front());
            }
            match next {
                Some(ScriptTurn::Response(r)) => Ok(r),
                Some(ScriptTurn::Error(t)) => Err(squint_core::Error::Transport(t)),
                Some(ScriptTurn::Stall(_)) => Err(squint_core::Error::Provider(
                    "scripted provider: two stalls in a row".into(),
                )),
                None => Err(squint_core::Error::Provider(
                    "scripted provider: script exhausted".into(),
                )),
            }
        })
    }
}

/// The count and many-image checks of an adapter's preflight, over every image the history names.
fn over_limits(limits: &RequestImageLimits, req: &TurnRequest<'_>) -> squint_core::Result<()> {
    let mut ids: Vec<&str> = Vec::new();
    for block in req.history.iter().flat_map(|m| &m.content) {
        match block {
            ContentBlock::Image { image_id } => ids.push(image_id),
            ContentBlock::ToolResult { images, .. } => {
                ids.extend(images.iter().map(String::as_str))
            }
            _ => {}
        }
    }
    let count = ids.len() as u64;
    if count > u64::from(limits.max_per_request) {
        return Err(squint_core::Error::ImageLimit(format!(
            "{count} images in the request exceed the model's limit of {} per request",
            limits.max_per_request
        )));
    }
    if let Some(many) = limits.many
        && count > u64::from(many.threshold)
    {
        for id in ids {
            let long = req.images.get(id).map_or(0, |p| p.width.max(p.height));
            if long > many.max_long_edge {
                return Err(squint_core::Error::ImageLimit(format!(
                    "{id}: {long} pixels exceed {}, the limit when a request carries more than {} images ({count} here)",
                    many.max_long_edge, many.threshold
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use squint_core::provider::StopReason;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        // The scripted provider never yields, so polling once with a no-op waker suffices.
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn replays_turns_in_order_then_errors() {
        let p = ScriptedProvider::new(vec![
            TurnResponse {
                stop_reason: StopReason::ToolUse,
                ..TurnResponse::default_for_test()
            },
            TurnResponse {
                stop_reason: StopReason::EndTurn,
                ..TurnResponse::default_for_test()
            },
        ]);
        let images = std::collections::BTreeMap::new();
        let req = TurnRequest {
            system: "",
            history: &[],
            tools: &[],
            max_output_tokens: 1,
            images: &images,
        };
        assert_eq!(
            block_on(p.send(req.clone())).expect("turn 1").stop_reason,
            StopReason::ToolUse
        );
        assert_eq!(
            block_on(p.send(req.clone())).expect("turn 2").stop_reason,
            StopReason::EndTurn
        );
        assert!(
            block_on(p.send(req)).is_err(),
            "a third turn exhausts the script"
        );
    }

    trait DefaultForTest {
        fn default_for_test() -> Self;
    }
    impl DefaultForTest for TurnResponse {
        fn default_for_test() -> Self {
            TurnResponse {
                text: vec![],
                tool_calls: vec![],
                content: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Some(Default::default()),
                provider_request_id: None,
            }
        }
    }
}

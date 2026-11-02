//! The model catalog (SPEC "Model catalog", ADR 0008): `catalog.toml` beside this crate's `src/`, embedded and
//! versioned, part of the cache key. Per model: adapter, limits, image fields, prices with a
//! `verified` date and sources, capabilities, the controls a config may set and their values.
//! Configuration a model does not support is rejected at resolve time; a request's cost bound
//! comes from the catalog's prices through `squint_core::ledger::Pricing`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use squint_core::config::Config;
use squint_core::image::{ManyImages, RequestImageLimits};
use squint_core::ledger::{Pricing, RequestShape};
use squint_core::provider::Usage;
use squint_core::{Error, Result};

pub const CATALOG_VERSION: u32 = 1;

/// The shipped catalog file.
pub const SHIPPED: &str = include_str!("../catalog.toml");

/// Adapters this release implements.
pub const ADAPTERS: &[&str] = &[
    "anthropic-messages",
    "gemini-generate-content",
    "openai-responses",
];
/// Final-turn mechanisms (SPEC "Final turn mechanics").
pub const FINAL_TURN_MECHANISMS: &[&str] = &["refuse", "forced", "tool_removal"];
/// The test-only profile and model the scripted provider answers for.
pub const FAKE_PROFILE: &str = "fake";
pub const FAKE_MODEL: &str = "fake-model";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub catalog_version: u32,
    pub profiles: BTreeMap<String, Profile>,
    pub models: BTreeMap<String, Model>,
}

/// A shipped endpoint profile, in the form the first release needs: adapter, endpoint and the
/// API-key scheme. Gateway profiles with upstream contracts arrive with their fixtures (2.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// The prefix a `[privacy] account` must carry to consent to this profile (`anthropic` for
    /// `anthropic:org-…`).
    pub account: String,
    pub adapter: String,
    pub base_url: String,
    /// May contain `{model}`.
    pub path: String,
    pub auth: Auth,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    /// `api-key-header` or `bearer`.
    pub scheme: String,
    pub header: String,
    /// The environment variable holding the credential; never read here.
    pub env: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub adapter: String,
    pub vendor: String,
    pub display: String,
    /// `stable`; preview models are excluded from the catalog.
    pub status: String,
    pub context_tokens: u64,
    pub max_output_tokens: u32,
    /// The date the values were read from `sources` (YYYY-MM-DD).
    pub verified: String,
    pub sources: Vec<String>,
    /// Fields the documentation does not state, carried as best estimates until recordings.
    #[serde(default)]
    pub unverified: Vec<String>,
    pub prices: Prices,
    pub images: Images,
    pub capabilities: Capabilities,
    /// Controls a config may set under `[provider] controls`, with their allowed values.
    #[serde(default)]
    pub controls: BTreeMap<String, Control>,
    /// Beta headers the adapter sends for named features.
    #[serde(default)]
    pub betas: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prices {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cached_input_per_million: f64,
    #[serde(default)]
    pub cache_write_per_million: Option<f64>,
    /// A per-image charge beyond tokens; zero for every current model.
    pub per_image: f64,
    /// Higher prices for prompts over a threshold (Claude Haiku 5.5).
    #[serde(default)]
    pub long_prompt: Option<LongPrompt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LongPrompt {
    pub threshold_tokens: u64,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cached_input_per_million: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Images {
    pub formats: Vec<String>,
    /// The largest accepted dimension; `None` when the documentation states none.
    #[serde(default)]
    pub max_dimension_px: Option<u32>,
    pub max_bytes: u64,
    /// The long edge above which the provider downsizes; images are prepared to this size.
    pub native_long_edge_px: u32,
    pub max_per_request: u32,
    /// Above this many images in a request, a stricter per-image dimension applies.
    #[serde(default)]
    pub many_image_threshold: Option<u32>,
    #[serde(default)]
    pub many_image_max_dimension_px: Option<u32>,
    pub max_request_bytes: u64,
    /// The provider's detail control, when one exists (`media_resolution`, `image_detail`).
    #[serde(default)]
    pub detail_control: Option<String>,
    pub tokens: TokenEstimate,
}

/// How the provider counts image tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TokenEstimate {
    /// `ceil(w / patch) × ceil(h / patch) × multiplier`, capped at `max_tokens` or `max_patches`
    /// by downsizing; more than `reject_patches` is rejected outright.
    Patch {
        patch_px: u32,
        #[serde(default)]
        max_tokens: Option<u32>,
        #[serde(default)]
        max_patches: Option<u32>,
        #[serde(default)]
        reject_patches: Option<u32>,
        multiplier: f64,
    },
    /// `tokens_per_tile` per `tile_px` tile, one tile when both sides are at most `small_px`,
    /// capped at `max_tokens` by the detail control.
    Tile {
        tile_px: u32,
        tokens_per_tile: u32,
        small_px: u32,
        max_tokens: u32,
    },
}

impl TokenEstimate {
    /// The most tokens one prepared image can cost.
    pub fn max_tokens_per_image(&self) -> u64 {
        match self {
            TokenEstimate::Patch {
                max_tokens,
                max_patches,
                multiplier,
                ..
            } => match (max_tokens, max_patches) {
                (Some(t), _) => u64::from(*t),
                (None, Some(p)) => (f64::from(*p) * multiplier).ceil() as u64,
                (None, None) => 0,
            },
            TokenEstimate::Tile { max_tokens, .. } => u64::from(*max_tokens),
        }
    }

    /// The tokens an image of these dimensions costs before any downsizing.
    pub fn tokens_for(&self, width: u32, height: u32) -> u64 {
        match self {
            TokenEstimate::Patch {
                patch_px,
                multiplier,
                ..
            } => {
                let patches =
                    u64::from(width.div_ceil(*patch_px)) * u64::from(height.div_ceil(*patch_px));
                (patches as f64 * multiplier).ceil() as u64
            }
            TokenEstimate::Tile {
                tile_px,
                tokens_per_tile,
                small_px,
                ..
            } => {
                if width <= *small_px && height <= *small_px {
                    return u64::from(*tokens_per_tile);
                }
                let tiles =
                    u64::from(width.div_ceil(*tile_px)) * u64::from(height.div_ceil(*tile_px));
                tiles * u64::from(*tokens_per_tile)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Whether `tool_choice` may force a tool (Claude Opus 5.5 and Sonnet 5.5 reject it).
    pub forced_tool_choice: bool,
    /// Whether the provider enforces tool schemas (strict mode).
    pub strict_tools: bool,
    /// Constraint keywords the wire transformation removes for this model (canonical validation
    /// keeps enforcing them). Constructs with no wire form at all are refused by `wire`.
    pub strict_unsupported_keywords: Vec<String>,
    /// Whether every tool call carries an id the result must echo.
    pub function_call_ids: bool,
    pub parallel_tools: bool,
    /// Whether `role: system` messages may appear inside the history.
    pub system_messages_in_history: bool,
    /// Final-turn mechanisms the model supports, first is the default.
    pub final_turn: Vec<String>,
    /// Whether sampling parameters (temperature, top_p, top_k) are accepted.
    pub sampling: bool,
    pub prefill: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub values: Vec<String>,
    #[serde(default)]
    pub default: Option<String>,
}

fn resolve(message: impl Into<String>) -> Error {
    Error::Config(format!("catalog: {}", message.into()))
}

impl Catalog {
    /// The catalog this binary ships, validated.
    pub fn shipped() -> Result<Catalog> {
        Catalog::parse(SHIPPED)
    }

    pub fn parse(text: &str) -> Result<Catalog> {
        let catalog: Catalog = toml::from_str(text).map_err(|e| resolve(e.to_string()))?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// The self-test: every entry complete, dated, sourced, and internally consistent.
    pub fn validate(&self) -> Result<()> {
        if self.catalog_version != CATALOG_VERSION {
            return Err(resolve(format!(
                "catalog_version must be {CATALOG_VERSION}, got {}",
                self.catalog_version
            )));
        }
        for (id, p) in &self.profiles {
            if !ADAPTERS.contains(&p.adapter.as_str()) {
                return Err(resolve(format!(
                    "profile {id:?}: unknown adapter {:?}",
                    p.adapter
                )));
            }
            if !matches!(p.auth.scheme.as_str(), "api-key-header" | "bearer") {
                return Err(resolve(format!(
                    "profile {id:?}: unknown auth scheme {:?}",
                    p.auth.scheme
                )));
            }
            if p.auth.env.is_empty()
                || p.auth.header.is_empty()
                || p.base_url.is_empty()
                || p.path.is_empty()
            {
                return Err(resolve(format!(
                    "profile {id:?}: base_url, path, auth.header and auth.env must be set"
                )));
            }
            if p.account.is_empty() || p.account.contains(':') {
                return Err(resolve(format!(
                    "profile {id:?}: account must be a plain prefix such as \"anthropic\""
                )));
            }
            if !p.base_url.starts_with("https://") {
                return Err(resolve(format!("profile {id:?}: base_url must be https")));
            }
        }
        for (id, m) in &self.models {
            let fail = |what: &str| resolve(format!("model {id:?}: {what}"));
            if !ADAPTERS.contains(&m.adapter.as_str()) {
                return Err(fail(&format!("unknown adapter {:?}", m.adapter)));
            }
            if m.capabilities
                .final_turn
                .iter()
                .any(|f| f == "tool_removal")
                && !m.capabilities.system_messages_in_history
            {
                return Err(fail(
                    "final_turn lists tool_removal but system_messages_in_history is false; the removal blocks travel in a system message",
                ));
            }
            if m.status != "stable" {
                return Err(fail("only stable models are catalogued"));
            }
            if m.display.is_empty() || m.vendor.is_empty() {
                return Err(fail("display and vendor must be set"));
            }
            if m.context_tokens == 0 || m.max_output_tokens == 0 {
                return Err(fail(
                    "context_tokens and max_output_tokens must be positive",
                ));
            }
            if !is_date(&m.verified) {
                return Err(fail("verified must be a YYYY-MM-DD date"));
            }
            if m.sources.len() < 2 || m.sources.iter().any(|s| !s.starts_with("https://")) {
                return Err(fail("at least two https sources are required"));
            }
            let p = &m.prices;
            if p.input_per_million <= 0.0
                || p.output_per_million <= 0.0
                || p.cached_input_per_million < 0.0
                || p.per_image < 0.0
            {
                return Err(fail(
                    "prices must be positive (per_image and cached may be zero)",
                ));
            }
            if let Some(lp) = &p.long_prompt
                && (lp.threshold_tokens == 0
                    || lp.input_per_million <= 0.0
                    || lp.output_per_million <= 0.0)
            {
                return Err(fail(
                    "long_prompt prices must be positive with a positive threshold",
                ));
            }
            let im = &m.images;
            if im.formats.is_empty()
                || im.max_bytes == 0
                || im.native_long_edge_px == 0
                || im.max_per_request == 0
                || im.max_request_bytes == 0
            {
                return Err(fail(
                    "images: formats, max_bytes, native_long_edge_px, max_per_request and max_request_bytes must be set",
                ));
            }
            if im.many_image_threshold.is_some() != im.many_image_max_dimension_px.is_some() {
                return Err(fail(
                    "images: many_image_threshold and many_image_max_dimension_px go together",
                ));
            }
            if im.tokens.max_tokens_per_image() == 0 {
                return Err(fail("images.tokens must bound the tokens per image"));
            }
            if let Some(c) = &im.detail_control
                && !m.controls.contains_key(c)
            {
                return Err(fail(&format!(
                    "images.detail_control {c:?} is not a declared control"
                )));
            }
            let caps = &m.capabilities;
            if caps.final_turn.is_empty() {
                return Err(fail(
                    "capabilities.final_turn must name at least one mechanism",
                ));
            }
            for f in &caps.final_turn {
                if !FINAL_TURN_MECHANISMS.contains(&f.as_str()) {
                    return Err(fail(&format!("unknown final-turn mechanism {f:?}")));
                }
            }
            if caps.final_turn.iter().any(|f| f == "forced") && !caps.forced_tool_choice {
                return Err(fail(
                    "final_turn lists `forced` but forced_tool_choice is false",
                ));
            }
            if caps.final_turn.iter().any(|f| f == "tool_removal")
                && !m.betas.contains_key("tool_changes")
            {
                return Err(fail(
                    "final_turn lists `tool_removal` but no `tool_changes` beta is pinned",
                ));
            }
            for (name, c) in &m.controls {
                if c.values.is_empty() {
                    return Err(fail(&format!("control {name:?} has no values")));
                }
                if let Some(d) = &c.default
                    && !c.values.contains(d)
                {
                    return Err(fail(&format!(
                        "control {name:?}: default {d:?} is not among its values"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.get(id)
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.get(id)
    }

    /// Resolve-time check of a config against the catalog: the profile and model exist and agree
    /// on the adapter, every control is one the model declares with an allowed value, the image
    /// setting only lowers the native size, and the output allowance fits. `allow_fake` admits
    /// the test-only scripted pair.
    pub fn check_config(&self, config: &Config, allow_fake: bool) -> Result<()> {
        let p = &config.provider;
        if p.profile == FAKE_PROFILE || p.model == FAKE_MODEL {
            if allow_fake && p.profile == FAKE_PROFILE && p.model == FAKE_MODEL {
                return Ok(());
            }
            return Err(resolve(format!(
                "profile {:?} and model {:?} are the test double, available only in builds with the testkit feature",
                p.profile, p.model
            )));
        }
        let profile = self.profile(&p.profile).ok_or_else(|| {
            resolve(format!(
                "profile {:?} is not in the catalog (shipped: {})",
                p.profile,
                self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        let model = self.model(&p.model).ok_or_else(|| {
            resolve(format!(
                "model {:?} is not in the catalog (shipped: {})",
                p.model,
                self.models.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        if profile.adapter != model.adapter {
            return Err(resolve(format!(
                "profile {:?} speaks {} but model {:?} needs {}",
                p.profile, profile.adapter, p.model, model.adapter
            )));
        }
        for (name, value) in &p.controls {
            // Request controls derived from capabilities rather than declared per model.
            if name == "final_turn" {
                match value.as_str() {
                    Some(m) if model.capabilities.final_turn.iter().any(|f| f == m) => continue,
                    _ => {
                        return Err(resolve(format!(
                            "control final_turn = {value} is not supported by {:?}; allowed: {:?}",
                            p.model, model.capabilities.final_turn
                        )));
                    }
                }
            }
            if name == "prefix_mismatch_behavior" {
                match value.as_str() {
                    Some("error" | "drop_block")
                        if model.betas.contains_key("thinking_binding") =>
                    {
                        continue;
                    }
                    _ => {
                        return Err(resolve(format!(
                            "control prefix_mismatch_behavior = {value} needs a model with the thinking-binding beta and one of [\"error\", \"drop_block\"]"
                        )));
                    }
                }
            }
            let Some(control) = model.controls.get(name) else {
                return Err(resolve(format!(
                    "model {:?} has no control {name:?}; it accepts {}",
                    p.model,
                    if model.controls.is_empty() {
                        "none".to_string()
                    } else {
                        model
                            .controls
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )));
            };
            let Some(text) = value.as_str() else {
                return Err(resolve(format!(
                    "control {name:?} must be a string, one of {:?}",
                    control.values
                )));
            };
            if !control.values.iter().any(|v| v == text) {
                return Err(resolve(format!(
                    "control {name:?} = {text:?} is not supported by {:?}; allowed: {:?}",
                    p.model, control.values
                )));
            }
        }
        if let Some(edge) = config.images.max_long_edge
            && edge > model.images.native_long_edge_px
        {
            return Err(resolve(format!(
                "images.max_long_edge = {edge} may only lower {:?}'s native size of {}",
                p.model, model.images.native_long_edge_px
            )));
        }
        if p.max_output_tokens > model.max_output_tokens {
            return Err(resolve(format!(
                "provider.max_output_tokens = {} exceeds {:?}'s maximum of {}",
                p.max_output_tokens, p.model, model.max_output_tokens
            )));
        }
        if config.budget.repair_output_tokens > model.max_output_tokens {
            return Err(resolve(format!(
                "budget.repair_output_tokens = {} exceeds {:?}'s maximum of {}",
                config.budget.repair_output_tokens, p.model, model.max_output_tokens
            )));
        }
        Ok(())
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit())
}

impl Model {
    /// Prices that apply to a prompt of this many tokens (the long-prompt tier where one exists).
    /// Per-million prices for a prompt of this many tokens (every input token, cached or not):
    /// input, output and cached input.
    fn tier(&self, prompt_tokens: u64) -> (f64, f64, f64) {
        match &self.prices.long_prompt {
            Some(lp) if prompt_tokens > lp.threshold_tokens => (
                lp.input_per_million,
                lp.output_per_million,
                lp.cached_input_per_million,
            ),
            _ => (
                self.prices.input_per_million,
                self.prices.output_per_million,
                self.prices.cached_input_per_million,
            ),
        }
    }
}

/// Catalog prices as the ledger sees them: text tokens at the model's rates, every image at its
/// maximum token cost plus any per-image charge.
impl Pricing for Model {
    fn bound(&self, shape: &RequestShape) -> Result<f64> {
        let image_tokens = shape.images * self.images.tokens.max_tokens_per_image();
        let input = shape.input_tokens + image_tokens;
        let (in_price, out_price, _) = self.tier(input);
        Ok(input as f64 * in_price / 1e6
            + shape.output_tokens as f64 * out_price / 1e6
            + shape.images as f64 * self.prices.per_image)
    }

    /// The count per request and the many-image threshold with its stricter long edge.
    fn image_limits(&self) -> Option<RequestImageLimits> {
        Some(RequestImageLimits {
            max_per_request: self.images.max_per_request,
            many: self
                .images
                .many_image_threshold
                .zip(self.images.many_image_max_dimension_px)
                .map(|(threshold, max_long_edge)| ManyImages {
                    threshold,
                    max_long_edge,
                }),
        })
    }

    /// Settled usage: uncached input at the input rate, cache reads at the cached rate, cache
    /// writes at the write rate (the input rate when the catalog states none), output at the
    /// output rate; the long-prompt tier is chosen from the whole prompt.
    fn cost(&self, usage: &Usage) -> f64 {
        let prompt = usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
        let (in_price, out_price, cached_price) = self.tier(prompt);
        let write_price = self.prices.cache_write_per_million.unwrap_or(in_price);
        (usage.input_tokens as f64 * in_price
            + usage.cache_read_tokens as f64 * cached_price
            + usage.cache_write_tokens as f64 * write_price
            + usage.output_tokens as f64 * out_price)
            / 1e6
    }
}

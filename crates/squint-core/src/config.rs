//! The config file: one TOML file per job type (SPEC "Config"). Loading parses the file, applies
//! `--set path=value` overrides, expands `${VAR}` from the environment only (every expanded value is
//! treated as a secret and redacted from every output), validates against the typed schema with
//! unknown keys refused, and applies the rules that need no catalog: the cloud-calls opt-in and
//! credential names in pack `env`. Profile and model existence are checked against the catalog from
//! task 2.1.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::image::DecodeBounds;
use crate::{Error, Result};

pub const CONFIG_VERSION: u32 = 1;

fn d_config_version() -> u32 {
    CONFIG_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The config schema version (`CONFIG_VERSION`); absent means the current one.
    #[serde(default = "d_config_version")]
    pub config_version: u32,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub images: ImagesConfig,
    #[serde(default)]
    pub rules: Option<RulesConfig>,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    pub privacy: PrivacyConfig,
    pub output: OutputConfig,
    /// `squint serve` only. Left out of the serialised config when it is the default, so a
    /// decision's recorded config (and its cache key) does not change with server settings.
    #[serde(default, skip_serializing_if = "ServeConfig::is_default")]
    pub serve: ServeConfig,
    /// Values that came from `${VAR}` expansion; never serialised, redacted everywhere.
    #[serde(skip)]
    #[schemars(skip)]
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// An endpoint profile id (`anthropic`, `google-ai-studio`, `openai`, a user profile, or
    /// `fake` for tests).
    pub profile: String,
    /// A model id present in the catalog.
    pub model: String,
    /// Provider-specific controls (effort, thinking level), validated against the catalog in 2.1.
    #[serde(default)]
    pub controls: BTreeMap<String, serde_json::Value>,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

fn default_max_output_tokens() -> u32 {
    16_000
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImagesConfig {
    #[serde(default = "default_max_decode_pixels")]
    pub max_decode_pixels: u64,
    #[serde(default = "default_max_decode_bytes")]
    pub max_decode_bytes: u64,
    /// May only lower the catalog's native size (checked in 2.3).
    #[serde(default)]
    pub max_long_edge: Option<u32>,
}

fn default_max_decode_pixels() -> u64 {
    50_000_000
}
fn default_max_decode_bytes() -> u64 {
    64 * 1024 * 1024
}

impl Default for ImagesConfig {
    fn default() -> Self {
        ImagesConfig {
            max_decode_pixels: default_max_decode_pixels(),
            max_decode_bytes: default_max_decode_bytes(),
            max_long_edge: None,
        }
    }
}

impl ImagesConfig {
    pub fn decode_bounds(&self) -> DecodeBounds {
        DecodeBounds {
            max_pixels: self.max_decode_pixels,
            max_bytes: self.max_decode_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesConfig {
    /// A shipped base rule set, for example `source-fidelity`.
    pub base: String,
    /// An optional shipped profile of that base, for example `architecture`.
    #[serde(default)]
    pub profile: Option<String>,
    /// Consumer rules files, applied in order after the base and profile.
    #[serde(default)]
    pub extra: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub packs: BTreeMap<String, PackConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackConfig {
    /// The provisioned bundle, by package and version (`squint tools setup` installs it).
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// An executable to run as the pack's MCP server instead of a provisioned bundle, with its
    /// arguments; for tests and local development.
    #[serde(default)]
    pub command: Option<PathBuf>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables passed through to the pack. Credential names are refused.
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default = "default_startup_timeout_s")]
    pub startup_timeout_s: u64,
    #[serde(default)]
    pub shared: bool,
}

fn default_startup_timeout_s() -> u64 {
    120
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    #[serde(default = "d_tool_turns")]
    pub tool_turns: u32,
    #[serde(default = "d_tool_calls")]
    pub tool_calls: u32,
    #[serde(default = "d_evidence_items")]
    pub evidence_items: u32,
    #[serde(default = "d_repairs")]
    pub repairs: u32,
    #[serde(default = "d_repair_output_tokens")]
    pub repair_output_tokens: u32,
    #[serde(default = "d_retries")]
    pub retries: u32,
    #[serde(default = "d_decision_runs")]
    pub decision_runs: u32,
    #[serde(default = "d_spend_cap_usd")]
    pub spend_cap_usd: f64,
    #[serde(default = "d_request_timeout_s")]
    pub request_timeout_s: u64,
    #[serde(default = "d_tool_timeout_s")]
    pub tool_timeout_s: u64,
}

fn d_tool_turns() -> u32 {
    4
}
fn d_tool_calls() -> u32 {
    8
}
fn d_evidence_items() -> u32 {
    24
}
fn d_repairs() -> u32 {
    1
}
fn d_repair_output_tokens() -> u32 {
    32_000
}
fn d_retries() -> u32 {
    3
}
fn d_decision_runs() -> u32 {
    1
}
fn d_spend_cap_usd() -> f64 {
    2.0
}
fn d_request_timeout_s() -> u64 {
    300
}
fn d_tool_timeout_s() -> u64 {
    120
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            tool_turns: d_tool_turns(),
            tool_calls: d_tool_calls(),
            evidence_items: d_evidence_items(),
            repairs: d_repairs(),
            repair_output_tokens: d_repair_output_tokens(),
            retries: d_retries(),
            decision_runs: d_decision_runs(),
            spend_cap_usd: d_spend_cap_usd(),
            request_timeout_s: d_request_timeout_s(),
            tool_timeout_s: d_tool_timeout_s(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum PolicyOutcome {
    Fail,
    Pass,
    #[default]
    Review,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    #[serde(default)]
    pub on_unsure: PolicyOutcome,
    #[serde(default)]
    pub on_error: PolicyOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default = "d_true")]
    pub read: bool,
    #[serde(default = "d_true")]
    pub write: bool,
}

fn d_true() -> bool {
    true
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            dir: None,
            read: true,
            write: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrivacyConfig {
    /// Must be true: squint sends images and text to the configured provider.
    pub cloud_calls: bool,
    /// The provider or gateway account the operator attests to, for example `anthropic:org-xxxx`.
    pub account: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub dir: PathBuf,
}

/// `[serve]`: the MCP server's own limits (table-manners 4.5, 10.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServeConfig {
    /// Calls served at once; serial by default.
    #[serde(default = "d_concurrency")]
    pub concurrency: u32,
    /// Spend across every call one server process serves, in USD.
    #[serde(default = "d_session_budget_usd")]
    pub session_budget_usd: f64,
    /// Directories an image path must sit inside, narrowing the client's MCP roots; empty means
    /// the client's roots alone.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

fn d_concurrency() -> u32 {
    1
}
/// Ten checks at the default per-call cap.
fn d_session_budget_usd() -> f64 {
    10.0 * d_spend_cap_usd()
}

impl Default for ServeConfig {
    fn default() -> Self {
        ServeConfig {
            concurrency: d_concurrency(),
            session_budget_usd: d_session_budget_usd(),
            roots: Vec::new(),
        }
    }
}

impl ServeConfig {
    fn is_default(&self) -> bool {
        *self == ServeConfig::default()
    }
}

/// Environment variable names that may never be passed through to a tool pack, in addition to any
/// name ending in `_KEY`, `_TOKEN` or `_SECRET`.
pub const CREDENTIAL_ENV_NAMES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENAI_API_KEY",
    "AI_GATEWAY_API_KEY",
    "CF_AIG_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// True for names the environment filter removes.
pub fn is_credential_name(name: &str) -> bool {
    CREDENTIAL_ENV_NAMES.contains(&name)
        || name.ends_with("_KEY")
        || name.ends_with("_TOKEN")
        || name.ends_with("_SECRET")
}

/// The file name squint looks for: in the working directory, then in the user's config directory.
pub const CONFIG_FILE: &str = "squint.toml";

/// Where the configuration is, or would be (see [`Config::locate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub exists: bool,
}

/// The configuration a process starts with (table-manners 4.6: read once, at start), and where
/// it came from or would come from.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub state: ConfigState,
}

/// Whether a configuration loaded (table-manners 3.1).
#[derive(Debug, Clone)]
pub enum ConfigState {
    Loaded(Box<Config>),
    /// No file at the path.
    Missing,
    /// There and does not load: the reason (secrets redacted) and, when that is the cause, the
    /// `${VAR}` it expands that is unset.
    Invalid {
        reason: String,
        unset: Option<String>,
    },
}

impl ConfigFile {
    /// The loaded configuration, if it loaded.
    pub fn config(&self) -> Option<&Config> {
        match &self.state {
            ConfigState::Loaded(config) => Some(config),
            _ => None,
        }
    }

    /// The configuration, or the usage error a command that cannot run without one prints: what
    /// is wrong, and where squint looked.
    pub fn require(self) -> Result<Config> {
        match self.state {
            ConfigState::Loaded(config) => Ok(*config),
            ConfigState::Missing => Err(Error::Config(no_config_message(&self.path))),
            ConfigState::Invalid { reason, .. } => Err(Error::Config(reason)),
        }
    }
}

/// Where squint looks for a configuration, in words: the error when none is found, and the
/// setup sentence of a `missing` config requirement.
pub fn no_config_message(path: &Path) -> String {
    format!(
        "no configuration at {}: pass --config <path>, or create ./{CONFIG_FILE} or \
         $XDG_CONFIG_HOME/squint/{CONFIG_FILE} (~/.config/squint/{CONFIG_FILE})",
        path.display()
    )
}

/// `$XDG_CONFIG_HOME/squint/squint.toml`, `~/.config/squint/squint.toml` when that is unset or
/// not absolute (the XDG base directory rule), none without either.
fn user_config_file(env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let base = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            env("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".config"))
        })?;
    Some(base.join("squint").join(CONFIG_FILE))
}

/// Why a configuration does not load.
struct Unloadable {
    reason: String,
    unset: Option<String>,
}

/// A config error's message without the class prefix `Error` displays.
fn config_message(e: Error) -> String {
    match e {
        Error::Config(m) => m,
        other => other.to_string(),
    }
}

/// `text` with every non-empty secret replaced by `[redacted]`.
fn redact_text(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_string(), |out, s| {
            out.replace(s.as_str(), "[redacted]")
        })
}

/// A call's effective per-call cap (table-manners 4.5): the configured cap, lowered by the call's
/// `max_spend_usd` when it names one. A value at or below zero is no cap, and one above the
/// configured cap is refused rather than silently lowered (a usage error).
pub fn call_cap(configured_usd: f64, max_usd: Option<f64>) -> Result<f64> {
    match max_usd {
        None => Ok(configured_usd),
        Some(max) if max.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) => Err(
            Error::Config(format!("max_spend_usd {max} must be a positive amount")),
        ),
        Some(max) if max > configured_usd => Err(Error::Config(format!(
            "max_spend_usd {max} is above the configured per-call cap of {configured_usd}; it \
             may lower the cap, never raise it"
        ))),
        Some(max) => Ok(max),
    }
}

impl Config {
    /// Sets the per-call cap to the call's `max_spend_usd` ([`call_cap`]): lower, or refused.
    pub fn call_cap(&mut self, max_usd: f64) -> Result<()> {
        self.budget.spend_cap_usd = call_cap(self.budget.spend_cap_usd, Some(max_usd))?;
        Ok(())
    }

    /// Loads a config file with no overrides.
    pub fn load(path: &Path) -> Result<Config> {
        Config::load_with_overrides(path, &[])
    }

    /// Loads a config file, applies `path=value` overrides, expands `${VAR}`, validates.
    pub fn load_with_overrides(path: &Path, overrides: &[String]) -> Result<Config> {
        Config::load_detailed(path, overrides, &|name| std::env::var(name).ok())
            .map_err(|e| Error::Config(e.reason))
    }

    /// Like [`Config::load_with_overrides`], saying which `${VAR}` was unset when that is why it
    /// does not load.
    fn load_detailed(
        path: &Path,
        overrides: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> std::result::Result<Config, Unloadable> {
        let text = std::fs::read_to_string(path).map_err(|e| Unloadable {
            reason: format!("cannot read {}: {e}", path.display()),
            unset: None,
        })?;
        Config::parse_detailed(&text, overrides, env).map_err(|e| Unloadable {
            reason: format!("{}: {}", path.display(), e.reason),
            unset: e.unset,
        })
    }

    /// Where squint's configuration is (ADR 0018 amendment): the path given, else
    /// `./squint.toml`, else `$XDG_CONFIG_HOME/squint/squint.toml` (`~/.config/squint/squint.toml`).
    /// The first that exists wins; when none does, the one a person would create.
    pub fn locate(given: Option<&Path>) -> Located {
        Config::locate_with(given, &|name| std::env::var_os(name))
    }

    /// [`Config::locate`] with the environment injected (for tests).
    pub fn locate_with(
        given: Option<&Path>,
        env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Located {
        if let Some(path) = given {
            return Located {
                path: path.to_path_buf(),
                exists: path.exists(),
            };
        }
        let here = std::path::absolute(CONFIG_FILE).unwrap_or_else(|_| PathBuf::from(CONFIG_FILE));
        if here.exists() {
            return Located {
                path: here,
                exists: true,
            };
        }
        match user_config_file(env) {
            Some(user) => Located {
                exists: user.exists(),
                path: user,
            },
            None => Located {
                path: here,
                exists: false,
            },
        }
    }

    /// The configuration [`Config::locate`] finds, loaded with `overrides`: present, missing, or
    /// invalid with the reason. Never an error: describing and serving start without one (3.1).
    pub fn open(given: Option<&Path>, overrides: &[String]) -> ConfigFile {
        Config::open_with(given, overrides, &|name| std::env::var(name).ok())
    }

    /// [`Config::open`] with the `${VAR}` lookup injected (for tests).
    pub fn open_with(
        given: Option<&Path>,
        overrides: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> ConfigFile {
        let located = Config::locate(given);
        let state = if !located.exists {
            ConfigState::Missing
        } else {
            match Config::load_detailed(&located.path, overrides, env) {
                Ok(config) => ConfigState::Loaded(Box::new(config)),
                Err(e) => ConfigState::Invalid {
                    reason: e.reason,
                    unset: e.unset,
                },
            }
        };
        ConfigFile {
            path: located.path,
            state,
        }
    }

    /// Parses config text with overrides and an environment lookup (injected for tests).
    pub fn parse(
        text: &str,
        overrides: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config> {
        Config::parse_detailed(text, overrides, env).map_err(|e| Error::Config(e.reason))
    }

    fn parse_detailed(
        text: &str,
        overrides: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> std::result::Result<Config, Unloadable> {
        let plain = |e: Error| Unloadable {
            reason: config_message(e),
            unset: None,
        };
        let mut table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| plain(Error::Config(e.to_string())))?;
        for ov in overrides {
            apply_override(&mut table, ov).map_err(plain)?;
        }
        let mut secrets = Vec::new();
        let mut value = toml::Value::Table(table);
        let mut unset = None;
        expand_env(&mut value, env, &mut secrets, &mut unset, "").map_err(|e| Unloadable {
            reason: config_message(e),
            unset: unset.clone(),
        })?;
        // From here an error may quote an expanded value: it is redacted like every output.
        let hide = |e: Error| Unloadable {
            reason: redact_text(&config_message(e), &secrets),
            unset: None,
        };
        let mut config: Config = value
            .try_into()
            .map_err(|e: toml::de::Error| hide(Error::Config(e.to_string())))?;
        config.validate().map_err(hide)?;
        config.secrets = secrets;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.config_version != CONFIG_VERSION {
            return Err(Error::Config(format!(
                "config_version must be {CONFIG_VERSION}, got {}",
                self.config_version
            )));
        }
        if !self.privacy.cloud_calls {
            return Err(Error::Config(
                "[privacy] cloud_calls must be true: squint sends images and text to the configured \
                 provider, and refuses to run without that explicit opt-in"
                    .into(),
            ));
        }
        if self.privacy.account.trim().is_empty() {
            return Err(Error::Config(
                "[privacy] account must name the provider or gateway account".into(),
            ));
        }
        for (name, pack) in &self.tools.packs {
            match (&pack.command, &pack.package, &pack.version) {
                (Some(_), None, None) | (None, Some(_), Some(_)) => {}
                _ => {
                    return Err(Error::Config(format!(
                        "[tools.packs.{name}]: give either `command` or both `package` and `version`"
                    )));
                }
            }
            for var in &pack.env {
                if is_credential_name(var) {
                    return Err(Error::Config(format!(
                        "[tools.packs.{name}] env: {var:?} is a credential name and may not be passed to a pack"
                    )));
                }
            }
        }
        if self.budget.decision_runs == 0 || self.budget.decision_runs > 2 {
            return Err(Error::Config(
                "[budget] decision_runs must be 1 or 2".into(),
            ));
        }
        if self.budget.spend_cap_usd.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(Error::Config(
                "[budget] spend_cap_usd must be positive".into(),
            ));
        }
        if self.serve.concurrency == 0 {
            return Err(Error::Config(
                "[serve] concurrency must be at least 1".into(),
            ));
        }
        if self.serve.session_budget_usd.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(Error::Config(
                "[serve] session_budget_usd must be positive".into(),
            ));
        }
        if let Some(root) = self.serve.roots.iter().find(|r| !r.is_absolute()) {
            return Err(Error::Config(format!(
                "[serve] roots: {} is not absolute",
                root.display()
            )));
        }
        Ok(())
    }

    /// The resolved config as JSON with every expanded secret replaced by `[redacted]`.
    pub fn redacted_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        redact(&mut v, &self.secrets);
        v
    }

    /// The JSON Schema for the config file.
    pub fn json_schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(Config)).unwrap_or(serde_json::Value::Null)
    }
}

/// Applies one `a.b.c=value` override to the parsed table. The value is parsed as TOML; if that
/// fails it is taken as a string.
fn apply_override(table: &mut toml::Table, ov: &str) -> Result<()> {
    let (path, raw) = ov
        .split_once('=')
        .ok_or_else(|| Error::Config(format!("override {ov:?} must be path=value")))?;
    let value: toml::Value = format!("v = {raw}")
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_string()));
    let keys: Vec<&str> = path.split('.').collect();
    let mut current = table;
    for key in &keys[..keys.len() - 1] {
        let entry = current
            .entry((*key).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        current = entry
            .as_table_mut()
            .ok_or_else(|| Error::Config(format!("override {ov:?}: {key} is not a table")))?;
    }
    let last = keys
        .last()
        .ok_or_else(|| Error::Config(format!("override {ov:?}: empty path")))?;
    current.insert((*last).to_string(), value);
    Ok(())
}

/// Expands `${NAME}` in every string from the environment lookup. A missing variable is an error;
/// every expanded value is recorded as a secret.
fn expand_env(
    value: &mut toml::Value,
    env: &dyn Fn(&str) -> Option<String>,
    secrets: &mut Vec<String>,
    unset: &mut Option<String>,
    path: &str,
) -> Result<()> {
    match value {
        toml::Value::String(s) if s.contains("${") => {
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 2..];
                let end = after
                    .find('}')
                    .ok_or_else(|| Error::Config(format!("{path}: unterminated ${{ in {s:?}")))?;
                let name = &after[..end];
                let Some(v) = env(name) else {
                    *unset = Some(name.to_string());
                    return Err(Error::Config(format!(
                        "{path}: environment variable {name} is not set"
                    )));
                };
                secrets.push(v.clone());
                out.push_str(&v);
                rest = &after[end + 1..];
            }
            out.push_str(rest);
            *s = out;
        }
        toml::Value::Table(t) => {
            for (k, v) in t.iter_mut() {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                expand_env(v, env, secrets, unset, &p)?;
            }
        }
        toml::Value::Array(a) => {
            for (i, v) in a.iter_mut().enumerate() {
                expand_env(v, env, secrets, unset, &format!("{path}[{i}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replaces any string containing a secret with `[redacted]`, recursively.
pub fn redact(value: &mut serde_json::Value, secrets: &[String]) {
    match value {
        serde_json::Value::String(s) => {
            if secrets
                .iter()
                .any(|sec| !sec.is_empty() && s.contains(sec.as_str()))
            {
                *s = "[redacted]".into();
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|v| redact(v, secrets)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|v| redact(v, secrets)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// A call may lower the configured cap and never raise it; a cap must stay positive
    /// (table-manners 4.5).
    /// `max_spend_usd` lowers the cap; above the configured cap it is refused, never silently
    /// lowered (table-manners 4.5), and so is one at or below zero.
    #[test]
    fn a_spend_cap_is_lowered_and_never_raised() {
        let text = crate::fixture("config/minimal.toml");
        let mut config = Config::parse(&text, &[], &no_env).expect("parses");
        let configured = config.budget.spend_cap_usd;
        assert!(matches!(
            config.call_cap(configured * 10.0),
            Err(Error::Config(m)) if m.contains("above the configured")
        ));
        assert_eq!(config.budget.spend_cap_usd, configured);
        config.call_cap(configured).unwrap();
        assert_eq!(config.budget.spend_cap_usd, configured);
        config.call_cap(0.01).unwrap();
        assert_eq!(config.budget.spend_cap_usd, 0.01);
        for bad in [0.0, -1.0, f64::NAN] {
            assert!(matches!(config.call_cap(bad), Err(Error::Config(_))));
        }
        assert_eq!(config.budget.spend_cap_usd, 0.01);
        assert_eq!(call_cap(2.0, None).unwrap(), 2.0);
    }

    #[test]
    fn parses_the_minimal_fixture_with_defaults() {
        let text = crate::fixture("config/minimal.toml");
        let config = Config::parse(&text, &[], &no_env).expect("parses");
        assert_eq!(config.provider.profile, "fake");
        assert_eq!(config.budget.tool_turns, 4);
        assert_eq!(config.policy.on_unsure, PolicyOutcome::Review);
        assert!(config.rules.is_none());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = crate::fixture("config/minimal.toml")
            .replace("[provider]\n", "[provider]\nbogus = 1\n");
        assert!(
            matches!(Config::parse(&text, &[], &no_env), Err(Error::Config(m)) if m.contains("bogus"))
        );
    }

    #[test]
    fn cloud_calls_false_is_refused() {
        let text = crate::fixture("config/minimal.toml")
            .replace("cloud_calls = true", "cloud_calls = false");
        assert!(
            matches!(Config::parse(&text, &[], &no_env), Err(Error::Config(m)) if m.contains("cloud_calls"))
        );
    }

    #[test]
    fn credential_names_in_pack_env_are_refused() {
        let text = format!(
            "{}\n[tools.packs.python]\npackage = \"squint-tools\"\nversion = \"0.1.0\"\nenv = [\"ANTHROPIC_API_KEY\"]\n",
            crate::fixture("config/minimal.toml")
        );
        assert!(
            matches!(Config::parse(&text, &[], &no_env), Err(Error::Config(m)) if m.contains("credential"))
        );
        let text2 = text.replace("ANTHROPIC_API_KEY", "MY_PACK_TOKEN");
        assert!(
            Config::parse(&text2, &[], &no_env).is_err(),
            "_TOKEN suffix is refused too"
        );
        let text3 = text.replace("ANTHROPIC_API_KEY", "SQUINT_MODEL_DIR");
        assert!(Config::parse(&text3, &[], &no_env).is_ok());
    }

    #[test]
    fn overrides_apply_to_single_values() {
        let text = crate::fixture("config/minimal.toml");
        let config = Config::parse(
            &text,
            &[
                "budget.tool_turns=2".into(),
                "policy.on_unsure=\"pass\"".into(),
            ],
            &no_env,
        )
        .expect("parses");
        assert_eq!(config.budget.tool_turns, 2);
        assert_eq!(config.policy.on_unsure, PolicyOutcome::Pass);
        assert!(Config::parse(&text, &["budget.tool_turns=notanumber".into()], &no_env).is_err());
    }

    #[test]
    fn env_expansion_records_secrets_and_redacts_them() {
        let text = crate::fixture("config/minimal.toml").replace(
            "account = \"fake:test\"",
            "account = \"fake:${DJV_TEST_ACCOUNT}\"",
        );
        let env = |name: &str| (name == "DJV_TEST_ACCOUNT").then(|| "org-secret-123".to_string());
        let config = Config::parse(&text, &[], &env).expect("parses");
        assert_eq!(config.privacy.account, "fake:org-secret-123");
        assert_eq!(config.secrets, vec!["org-secret-123"]);
        let json = serde_json::to_string(&config.redacted_json()).expect("json");
        assert!(!json.contains("org-secret-123"));
        assert!(json.contains("[redacted]"));
        assert!(
            matches!(Config::parse(&text, &[], &no_env), Err(Error::Config(m)) if m.contains("DJV_TEST_ACCOUNT"))
        );
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squint-config-locate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The lookup (ADR 0018 amendment): a given path wins even when it is missing; else the
    /// user's config directory (`$XDG_CONFIG_HOME`, then `~/.config`); none found is not an
    /// error and names the file a person would create.
    #[test]
    fn the_lookup_takes_the_given_path_then_the_user_file() {
        let dir = scratch("order");
        let xdg = dir.join("xdg");
        let home = dir.join("home");
        let env = |xdg: Option<&Path>, home: Option<&Path>| {
            let (xdg, home) = (xdg.map(PathBuf::from), home.map(PathBuf::from));
            move |name: &str| match name {
                "XDG_CONFIG_HOME" => xdg.clone().map(Into::into),
                "HOME" => home.clone().map(Into::into),
                _ => None,
            }
        };
        let given = dir.join("given.toml");
        let found = Config::locate_with(Some(&given), &env(Some(&xdg), Some(&home)));
        assert_eq!((found.path, found.exists), (given.clone(), false));

        let user = xdg.join("squint/squint.toml");
        let none = Config::locate_with(None, &env(Some(&xdg), Some(&home)));
        assert_eq!((none.path.clone(), none.exists), (user.clone(), false));
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, "").unwrap();
        let some = Config::locate_with(None, &env(Some(&xdg), Some(&home)));
        assert_eq!((some.path, some.exists), (user, true));

        // A relative XDG_CONFIG_HOME is ignored, as the XDG rule says; HOME then decides.
        let fallback = Config::locate_with(None, &env(Some(Path::new("rel")), Some(&home)));
        assert_eq!(fallback.path, home.join(".config/squint/squint.toml"));
        // With neither, the working directory's file is the one to create.
        let bare = Config::locate_with(None, &env(None, None));
        assert!(bare.path.ends_with("squint.toml") && bare.path.is_absolute());
        assert!(!bare.exists);
        std::fs::write(&given, "").unwrap();
        assert!(Config::locate_with(Some(&given), &env(None, None)).exists);
    }

    /// Opening never fails: a file that does not parse, or names an unset `${VAR}`, is invalid
    /// with the reason (and the variable), and an expanded value never reaches the reason.
    #[test]
    fn opening_reports_missing_invalid_and_loaded() {
        let dir = scratch("open");
        let path = dir.join("squint.toml");
        assert!(matches!(
            Config::open_with(Some(&path), &[], &no_env).state,
            ConfigState::Missing
        ));
        std::fs::write(&path, "[provider\n").unwrap();
        assert!(matches!(
            Config::open_with(Some(&path), &[], &no_env).state,
            ConfigState::Invalid { unset: None, .. }
        ));
        let text = crate::fixture("config/minimal.toml").replace(
            "account = \"fake:test\"",
            "account = \"fake:${DJV_OPEN_ACCOUNT}\"",
        );
        std::fs::write(&path, &text).unwrap();
        match Config::open_with(Some(&path), &[], &no_env).state {
            ConfigState::Invalid { unset, reason } => {
                assert_eq!(unset.as_deref(), Some("DJV_OPEN_ACCOUNT"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        let env = |name: &str| (name == "DJV_OPEN_ACCOUNT").then(|| "org-open-secret".to_string());
        let opened = Config::open_with(Some(&path), &[], &env);
        assert_eq!(
            opened.config().unwrap().privacy.account,
            "fake:org-open-secret"
        );
        // A value expanded into a field of the wrong type is quoted by the parser: redacted.
        let wrong = text.replace(
            "[provider]\n",
            "[provider]\nmax_output_tokens = \"${DJV_OPEN_ACCOUNT}\"\n",
        );
        std::fs::write(&path, &wrong).unwrap();
        match Config::open_with(Some(&path), &[], &env).state {
            ConfigState::Invalid { reason, .. } => assert!(!reason.contains("org-open-secret")),
            other => panic!("{other:?}"),
        }
        // A directory where the file should be cannot be read: invalid, not missing.
        let unreadable = dir.join("a-directory.toml");
        std::fs::create_dir_all(&unreadable).unwrap();
        assert!(matches!(
            Config::open_with(Some(&unreadable), &[], &no_env).state,
            ConfigState::Invalid { .. }
        ));
        assert!(
            Config::open_with(Some(&dir.join("none.toml")), &[], &no_env)
                .require()
                .unwrap_err()
                .to_string()
                .contains("--config")
        );
    }

    #[test]
    fn is_credential_name_covers_known_names_and_suffixes() {
        assert!(is_credential_name("OPENAI_API_KEY"));
        assert!(is_credential_name("SOMETHING_SECRET"));
        assert!(!is_credential_name("SQUINT_MODEL_DIR"));
        assert!(!is_credential_name("PATH"));
    }
}

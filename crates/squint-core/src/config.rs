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

impl Config {
    /// Lowers the per-call spend cap to `max_usd` when that is lower; a call never raises it.
    pub fn lower_spend_cap(&mut self, max_usd: f64) -> Result<()> {
        if max_usd.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(Error::Config(format!(
                "max_spend_usd {max_usd} must be a positive amount"
            )));
        }
        self.budget.spend_cap_usd = self.budget.spend_cap_usd.min(max_usd);
        Ok(())
    }

    /// Loads a config file with no overrides.
    pub fn load(path: &Path) -> Result<Config> {
        Config::load_with_overrides(path, &[])
    }

    /// Loads a config file, applies `path=value` overrides, expands `${VAR}`, validates.
    pub fn load_with_overrides(path: &Path, overrides: &[String]) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        Config::parse(&text, overrides, &|name| std::env::var(name).ok())
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }

    /// Parses config text with overrides and an environment lookup (injected for tests).
    pub fn parse(
        text: &str,
        overrides: &[String],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Config> {
        let mut table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| Error::Config(e.to_string()))?;
        for ov in overrides {
            apply_override(&mut table, ov)?;
        }
        let mut secrets = Vec::new();
        let mut value = toml::Value::Table(table);
        expand_env(&mut value, env, &mut secrets, "")?;
        let mut config: Config = value
            .try_into()
            .map_err(|e: toml::de::Error| Error::Config(e.to_string()))?;
        config.secrets = secrets;
        config.validate()?;
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
                let v = env(name).ok_or_else(|| {
                    Error::Config(format!("{path}: environment variable {name} is not set"))
                })?;
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
                expand_env(v, env, secrets, &p)?;
            }
        }
        toml::Value::Array(a) => {
            for (i, v) in a.iter_mut().enumerate() {
                expand_env(v, env, secrets, &format!("{path}[{i}]"))?;
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
    #[test]
    fn a_spend_cap_is_lowered_never_raised() {
        let text = crate::fixture("config/minimal.toml");
        let mut config = Config::parse(&text, &[], &no_env).expect("parses");
        let configured = config.budget.spend_cap_usd;
        config.lower_spend_cap(configured * 10.0).unwrap();
        assert_eq!(config.budget.spend_cap_usd, configured);
        config.lower_spend_cap(0.01).unwrap();
        assert_eq!(config.budget.spend_cap_usd, 0.01);
        for bad in [0.0, -1.0, f64::NAN] {
            assert!(matches!(config.lower_spend_cap(bad), Err(Error::Config(_))));
        }
        assert_eq!(config.budget.spend_cap_usd, 0.01);
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

    #[test]
    fn is_credential_name_covers_known_names_and_suffixes() {
        assert!(is_credential_name("OPENAI_API_KEY"));
        assert!(is_credential_name("SOMETHING_SECRET"));
        assert!(!is_credential_name("SQUINT_MODEL_DIR"));
        assert!(!is_credential_name("PATH"));
    }
}

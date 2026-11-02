//! Starter configuration files: `squint init`'s personal default config (task 11.4) and what
//! `squint rules init` writes beside its rules file (task 11.3). A starter never holds a secret: the credential is read from the profile's variable at
//! run time, and the account id behind `[privacy] account` from `SQUINT_<VENDOR>_ACCOUNT`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use squint_providers::catalog::Catalog;

use crate::EXIT_USAGE;

#[derive(clap::Args)]
pub(crate) struct InitArgs {
    /// The provider profile: `anthropic` (default), `google-ai-studio` or `openai`.
    #[arg(long)]
    provider: Option<String>,
    /// The model, one the profile serves (`squint catalog list`); default a mid-tier one.
    #[arg(long)]
    model: Option<String>,
}

/// `squint init`: the personal default config at the path the lookup reads after
/// `./squint.toml`, never overwritten, holding no secret.
pub(crate) fn run(args: &InitArgs) -> i32 {
    let usage = |e: String| {
        eprintln!("squint: {e}");
        EXIT_USAGE
    };
    let choice = match choose(args.provider.as_deref(), args.model.as_deref()) {
        Ok(c) => c,
        Err(e) => return usage(e),
    };
    let Some(path) = squint_core::config::Config::user_file() else {
        return usage(
            "neither XDG_CONFIG_HOME nor HOME names an absolute directory to put the config in"
                .into(),
        );
    };
    let text = config_text(&choice, None, "squint init");
    if let Err(e) = write_new(&[(path.clone(), text)]) {
        return usage(e);
    }
    println!("wrote {}", path.display());
    eprintln!(
        "squint finds it without --config (a ./squint.toml where you run squint wins).\n\
         Set in your environment, never in the file:\n  \
         {}  the {} API key\n  \
         {}  your account id, which [privacy] account attests to (`{}:<id>`)\n\
         Then: squint check --use-case <use-case> --candidate <image> …",
        choice.key_env,
        choice.profile,
        choice.account_env(),
        choice.account_prefix
    );
    0
}

/// The model a starter config names for each shipped profile when none is asked for: a mid-tier
/// or cheap model, since most checks need no more.
const DEFAULT_MODELS: &[(&str, &str)] = &[
    ("anthropic", "claude-haiku-5-5"),
    ("google-ai-studio", "gemini-3.8-flash"),
    ("openai", "gpt-6.1-terra"),
];

/// The profile a starter config uses when none is asked for.
pub(crate) const DEFAULT_PROFILE: &str = "anthropic";

/// A profile and model the catalog has, with the variables a run under them reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub profile: String,
    pub model: String,
    /// The variable holding the API key (the profile's `auth.env`).
    pub key_env: String,
    /// The `[privacy] account` prefix the profile consents under.
    pub account_prefix: String,
}

impl Choice {
    /// The variable a starter's `[privacy] account` expands for the account id.
    pub fn account_env(&self) -> String {
        format!(
            "SQUINT_{}_ACCOUNT",
            self.account_prefix.to_ascii_uppercase().replace('-', "_")
        )
    }
}

/// The profile (default `anthropic`) and model (default the profile's in [`DEFAULT_MODELS`]),
/// checked against the shipped catalog: both must exist and the model must speak the profile's
/// adapter.
pub(crate) fn choose(profile: Option<&str>, model: Option<&str>) -> Result<Choice, String> {
    let catalog = Catalog::shipped().map_err(|e| e.to_string())?;
    let profile = profile.unwrap_or(DEFAULT_PROFILE);
    let known = || {
        catalog
            .profiles
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let p = catalog.profile(profile).ok_or_else(|| {
        format!(
            "no provider profile {profile:?}; the profiles are {}",
            known()
        )
    })?;
    let model = match model {
        Some(m) => m.to_string(),
        None => DEFAULT_MODELS
            .iter()
            .find(|(id, _)| *id == profile)
            .map(|(_, m)| m.to_string())
            .ok_or_else(|| format!("no default model for {profile:?}; pass --model"))?,
    };
    let m = catalog.model(&model).ok_or_else(|| {
        format!("no model {model:?} in the catalog; `squint catalog list` names them")
    })?;
    if m.adapter != p.adapter {
        let fits: Vec<&str> = catalog
            .models
            .iter()
            .filter(|(_, m)| m.adapter == p.adapter)
            .map(|(id, _)| id.as_str())
            .collect();
        return Err(format!(
            "model {model:?} is not served by the {profile:?} profile; its models are {}",
            fits.join(", ")
        ));
    }
    Ok(Choice {
        profile: profile.to_string(),
        model,
        key_env: p.auth.env.clone(),
        account_prefix: p.account.clone(),
    })
}

/// A starter config. `rules` names the base and the rules file `[rules] extra` lists; without
/// it the config has no `[rules]` section, so the base is the job's use case.
pub(crate) fn config_text(
    choice: &Choice,
    rules: Option<(&str, &Path)>,
    written_by: &str,
) -> String {
    let mut text = format!(
        "# squint configuration, written by `{written_by}`. Every key: SPEC.md \"Config\".\n\
         # It holds no secret: the API key is read from {key} at run time, and the account id\n\
         # [privacy] account attests to from {account}.\n\
         # Another provider without editing: --set provider.profile=… --set provider.model=…\n\
         # --set privacy.account=<vendor>:${{SQUINT_<VENDOR>_ACCOUNT}} (`squint catalog list`).\n\
         \n\
         [provider]\n\
         profile = \"{profile}\"\n\
         model = \"{model}\"\n",
        key = choice.key_env,
        account = choice.account_env(),
        profile = choice.profile,
        model = choice.model,
    );
    if let Some((base, file)) = rules {
        text.push_str(&format!(
            "\n[rules]\n\
             base = \"{base}\"\n\
             # Rules files apply in order after the base; a relative path resolves against the\n\
             # directory squint runs from, so this one is absolute.\n\
             extra = [{file}]\n",
            file = toml_string(&file.display().to_string()),
        ));
    }
    text.push_str(&format!(
        "\n[budget]\n\
         # The cap is checked before the first request against the worst case (every tool turn,\n\
         # the final turn and the repair at the full output allowance, plus a reserve for retries),\n\
         # not against what a check usually costs.\n\
         spend_cap_usd = 2.00\n\
         \n\
         [privacy]\n\
         cloud_calls = true\n\
         account = \"{prefix}:${{{account}}}\"\n\
         \n\
         [output]\n\
         dir = \"./squint-out\"\n",
        prefix = choice.account_prefix,
        account = choice.account_env(),
    ));
    text
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// Writes every file new, or none: an existing one is refused before anything is written, and
/// each is created exclusively, so a file that appears meanwhile is never overwritten.
pub(crate) fn write_new(files: &[(PathBuf, String)]) -> Result<(), String> {
    if let Some((path, _)) = files.iter().find(|(p, _)| p.exists()) {
        return Err(format!(
            "{} exists; squint never overwrites it (move it away first)",
            path.display()
        ));
    }
    for (path, text) in files {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_model_is_in_the_catalog_under_its_profile() {
        for (profile, model) in DEFAULT_MODELS {
            let choice = choose(Some(profile), None).expect("a shipped default");
            assert_eq!(&choice.model, model);
        }
        assert_eq!(
            choose(None, None).expect("default").account_env(),
            "SQUINT_ANTHROPIC_ACCOUNT"
        );
    }

    #[test]
    fn an_unknown_or_mismatched_choice_is_refused() {
        assert!(choose(Some("nope"), None).is_err());
        assert!(choose(None, Some("nope")).is_err());
        let err = choose(Some("openai"), Some("claude-haiku-5-5")).expect_err("mismatch");
        assert!(err.contains("gpt-"), "{err}");
    }

    #[test]
    fn a_starter_config_loads_and_holds_no_value_of_the_account() {
        let choice = choose(Some("google-ai-studio"), None).expect("valid");
        let text = config_text(
            &choice,
            Some(("reality-checks", Path::new("/x/r.toml"))),
            "t",
        );
        let table: toml::Table = toml::from_str(&text).expect("TOML");
        assert_eq!(
            table["provider"]["model"].as_str(),
            Some("gemini-3.8-flash")
        );
        assert_eq!(table["rules"]["extra"][0].as_str(), Some("/x/r.toml"));
        assert_eq!(
            table["privacy"]["account"].as_str(),
            Some("google:${SQUINT_GOOGLE_ACCOUNT}")
        );
    }
}

//! Readiness (table-manners 3.1, task 9.3): what stands between each action and a run, by local
//! inspection only (never a network call, a paid step or a run), plus the provider answers this
//! session already had. squint's actions share one provider and one tool set, so every action has
//! the same requirements: the configuration, then (when it loaded) the credential its profile
//! reads, the model it calls, and the segmenter file when an enabled pack tool needs it.
//!
//! What needs the model catalog or the installed packs (which this crate does not read) arrives
//! in the [`Inspection`] the binary builds; this module turns it and the session's [`Answers`]
//! into the contract's [`Readiness`], and, for one call, into the requirements it needs that hold
//! it back ([`Inspection::unmet`], task 9.6): a call refused for them gets no run.

use std::collections::HashMap;
use std::sync::Mutex;

use table_manners::{Readiness, ReadinessState, Requirement, RequirementKind, RequirementState};

use crate::config::{CONFIG_FILE, ConfigFile, ConfigState};

/// The pack tools that run the segmenter, so need its file (ADR 0018 amendment).
pub const SEGMENTER_TOOLS: &[&str] = &["regions", "classes"];

/// The segmenter's record name in the pack (`models/<name>.json`).
pub const SEGMENTER: &str = "segmenter-v1";

/// What a local look found, once, when the process started.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    /// The configuration: `present`, `missing` or `invalid` (always listed).
    pub config: Requirement,
    /// The variable the configured profile reads its key from, and whether it is set; none
    /// without a loaded configuration or for the scripted fake.
    pub credential: Option<Credential>,
    /// The configured model and whether the catalog has it; none without a loaded
    /// configuration or for the scripted fake.
    pub model: Option<String>,
    /// Local files an enabled tool needs.
    pub files: Vec<LocalFile>,
}

/// A credential variable by name, and whether it is set: never its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub name: String,
    pub present: bool,
}

/// A local file by path, and whether it is there at its recorded size (never hashed while
/// describing, so never `valid`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub path: String,
    pub present: bool,
    pub setup: String,
}

/// What a provider said to a request: accepted (a known answer that was not a rejection) or
/// rejected (401 or 403).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Accepted,
    Rejected,
}

/// The provider answers this session had, by credential name and by model: the latest settles
/// each.
#[derive(Debug, Default)]
pub struct Answers {
    credentials: Mutex<HashMap<String, Answer>>,
    models: Mutex<HashMap<String, Answer>>,
}

impl Answers {
    /// Records an answer to a request made with `credential` (when the profile reads one) for
    /// `model`.
    pub fn record(&self, credential: Option<&str>, model: &str, answer: Answer) {
        if let (Some(name), Ok(mut held)) = (credential, self.credentials.lock()) {
            held.insert(name.to_string(), answer);
        }
        // A rejection says nothing about the model: the request never reached it.
        if answer == Answer::Accepted
            && let Ok(mut held) = self.models.lock()
        {
            held.insert(model.to_string(), answer);
        }
    }

    fn credential(&self, name: &str) -> Option<Answer> {
        self.credentials.lock().ok()?.get(name).copied()
    }

    fn model(&self, name: &str) -> Option<Answer> {
        self.models.lock().ok()?.get(name).copied()
    }
}

/// The `config` requirement (table-manners 3.1): `present` at the path it loaded from, `missing`
/// at the path the lookup would use, or `invalid` with what to fix (an unset variable by name,
/// never a value).
pub fn config_requirement(file: &ConfigFile) -> Requirement {
    let (state, setup) = match &file.state {
        ConfigState::Loaded(_) => (RequirementState::Present, None),
        ConfigState::Missing => (
            RequirementState::Missing,
            Some(format!(
                "Create {} or pass --config <path> (squint also looks for ./{CONFIG_FILE} and \
                 $XDG_CONFIG_HOME/squint/{CONFIG_FILE}).",
                file.path.display()
            )),
        ),
        ConfigState::Invalid {
            unset: Some(name), ..
        } => (
            RequirementState::Invalid,
            Some(format!(
                "Set {name} in the environment squint is started with: the configuration expands it."
            )),
        ),
        ConfigState::Invalid { reason, .. } => (
            RequirementState::Invalid,
            Some(format!("Fix the configuration: {reason}")),
        ),
    };
    Requirement {
        kind: RequirementKind::Config,
        name: file.path.display().to_string(),
        state,
        setup,
    }
}

/// The configuration requirement made `invalid` for a reason found after it loaded (a model or
/// profile the catalog lacks, rules that do not resolve).
pub fn invalid_config(config: &Requirement, reason: &str) -> Requirement {
    Requirement {
        state: RequirementState::Invalid,
        setup: Some(format!("Fix the configuration: {reason}")),
        ..config.clone()
    }
}

impl Inspection {
    /// An inspection that found only the configuration (none loaded, or the scripted fake).
    pub fn config_only(config: Requirement) -> Inspection {
        Inspection {
            config,
            credential: None,
            model: None,
            files: Vec::new(),
        }
    }

    /// Every action's readiness now: the inspection's requirements in order (config, credential,
    /// model, files), a credential `valid` or `invalid` and a model `valid` only from `answers`.
    pub fn readiness(&self, answers: &Answers) -> Readiness {
        let mut requirements = vec![self.config.clone()];
        if let Some(credential) = &self.credential {
            let (state, setup) = match (credential.present, answers.credential(&credential.name)) {
                (false, _) => (RequirementState::Missing, set_it(&credential.name)),
                (true, None) => (RequirementState::Present, set_it(&credential.name)),
                (true, Some(Answer::Accepted)) => {
                    (RequirementState::Valid, set_it(&credential.name))
                }
                (true, Some(Answer::Rejected)) => (
                    RequirementState::Invalid,
                    format!(
                        "The provider rejected {} in this session: set a working key in the \
                         environment squint is started with, then restart it.",
                        credential.name
                    ),
                ),
            };
            requirements.push(Requirement {
                kind: RequirementKind::Credential,
                name: credential.name.clone(),
                state,
                setup: Some(setup),
            });
        }
        if let Some(model) = &self.model {
            requirements.push(Requirement {
                kind: RequirementKind::Model,
                name: model.clone(),
                state: match answers.model(model) {
                    Some(Answer::Accepted) => RequirementState::Valid,
                    _ => RequirementState::Present,
                },
                setup: None,
            });
        }
        for file in &self.files {
            requirements.push(Requirement {
                kind: RequirementKind::File,
                name: file.path.clone(),
                state: if file.present {
                    RequirementState::Present
                } else {
                    RequirementState::Missing
                },
                setup: Some(file.setup.clone()),
            });
        }
        let blocked = requirements.iter().any(|r| {
            matches!(
                r.state,
                RequirementState::Missing | RequirementState::Invalid
            )
        });
        Readiness {
            state: if blocked {
                ReadinessState::NeedsSetup
            } else {
                ReadinessState::Ready
            },
            requirements,
        }
    }
}

/// What one call needs before it runs (table-manners 3.1): the configuration always; the
/// credential and the model when it sends to the provider; a requirement's file when one of the
/// tools it offers runs it (the segmenter's, for [`SEGMENTER_TOOLS`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    /// Whether the call sends requests to the configured provider.
    pub provider: bool,
    /// The tools the call offers, by name.
    pub tools: Vec<String>,
}

impl Needs {
    fn needs(&self, requirement: &Requirement) -> bool {
        match requirement.kind {
            RequirementKind::Config => true,
            RequirementKind::Credential | RequirementKind::Model => self.provider,
            RequirementKind::File => self
                .tools
                .iter()
                .any(|t| SEGMENTER_TOOLS.contains(&t.as_str())),
            _ => false,
        }
    }
}

impl Inspection {
    /// The requirements `needs` names that are `missing` or `invalid` now, in readiness order:
    /// empty when the call may run, else what its `not_ready` refusal names (table-manners 3.1).
    /// A call that does not need a missing requirement is not held back by it.
    pub fn unmet(&self, answers: &Answers, needs: &Needs) -> Vec<Requirement> {
        self.readiness(answers)
            .requirements
            .into_iter()
            .filter(|r| {
                matches!(
                    r.state,
                    RequirementState::Missing | RequirementState::Invalid
                ) && needs.needs(r)
            })
            .collect()
    }
}

/// The sentence a `not_ready` refusal carries, on the command line and over MCP alike: each
/// requirement by name and state, then what to do about it. Names only, never a value.
pub fn not_ready_message(requirements: &[Requirement]) -> String {
    let each: Vec<String> = requirements
        .iter()
        .map(|r| {
            let state = match r.state {
                RequirementState::Missing => "missing",
                RequirementState::Invalid => "invalid",
                RequirementState::Present => "present",
                RequirementState::Valid => "valid",
                RequirementState::Unknown => "unknown",
            };
            match &r.setup {
                Some(setup) => format!("{} {state} ({setup})", r.name),
                None => format!("{} {state}", r.name),
            }
        })
        .collect();
    format!(
        "not ready: {}; nothing ran, nothing was spent",
        each.join("; ")
    )
}

fn set_it(name: &str) -> String {
    format!("Set {name} in the environment squint is started with.")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::Config;

    fn present_config() -> Requirement {
        Requirement {
            kind: RequirementKind::Config,
            name: "/etc/squint.toml".into(),
            state: RequirementState::Present,
            setup: None,
        }
    }

    fn paid(present: bool) -> Inspection {
        Inspection {
            config: present_config(),
            credential: Some(Credential {
                name: "ANTHROPIC_API_KEY".into(),
                present,
            }),
            model: Some("claude-haiku-5-5".into()),
            files: Vec::new(),
        }
    }

    fn states(readiness: &Readiness) -> Vec<(RequirementKind, RequirementState)> {
        readiness
            .requirements
            .iter()
            .map(|r| (r.kind, r.state))
            .collect()
    }

    /// `config`: present when it loads (no setup), missing at the path the lookup would use,
    /// invalid when it does not parse or a `${VAR}` it expands is unset (named, never a value).
    #[test]
    fn the_config_row() {
        let dir = std::env::temp_dir().join(format!("squint-readiness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("squint.toml");
        let _ = std::fs::remove_file(&path);
        let missing = config_requirement(&Config::open_with(Some(&path), &[], &|_| None));
        assert_eq!(missing.state, RequirementState::Missing);
        assert_eq!(missing.name, path.display().to_string());
        std::fs::write(&path, "not toml [").unwrap();
        let invalid = config_requirement(&Config::open_with(Some(&path), &[], &|_| None));
        assert_eq!(invalid.state, RequirementState::Invalid);
        let text = crate::fixture("config/minimal.toml").replace(
            "account = \"fake:test\"",
            "account = \"fake:${DJV_ROW_ACCOUNT}\"",
        );
        std::fs::write(&path, &text).unwrap();
        let unset = config_requirement(&Config::open_with(Some(&path), &[], &|_| None));
        assert_eq!(unset.state, RequirementState::Invalid);
        assert!(unset.setup.unwrap().contains("DJV_ROW_ACCOUNT"));
        let env = |_: &str| Some("org-row-secret".to_string());
        let loaded = config_requirement(&Config::open_with(Some(&path), &[], &env));
        assert_eq!(
            (loaded.state, loaded.setup),
            (RequirementState::Present, None)
        );
        // With no configuration, it is the only requirement.
        let none = Inspection::config_only(missing).readiness(&Answers::default());
        assert_eq!(none.state, ReadinessState::NeedsSetup);
        assert_eq!(
            states(&none),
            [(RequirementKind::Config, RequirementState::Missing)]
        );
        // One found invalid after loading (a model the catalog lacks) says what to fix.
        let unknown = invalid_config(&present_config(), "model \"x\" is not in the catalog");
        assert_eq!(unknown.state, RequirementState::Invalid);
        assert!(unknown.setup.unwrap().contains("not in the catalog"));
    }

    /// `credential`: missing or present by name; valid or invalid only from this session's
    /// answers, the latest settling it; a missing key stays missing whatever was answered.
    #[test]
    fn the_credential_row() {
        let answers = Answers::default();
        let credential = |i: &Inspection| {
            let r = i.readiness(&answers);
            (r.state, r.requirements[1].state)
        };
        assert_eq!(
            credential(&paid(false)),
            (ReadinessState::NeedsSetup, RequirementState::Missing)
        );
        assert_eq!(
            credential(&paid(true)),
            (ReadinessState::Ready, RequirementState::Present)
        );
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Accepted,
        );
        assert_eq!(
            credential(&paid(true)),
            (ReadinessState::Ready, RequirementState::Valid)
        );
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Rejected,
        );
        assert_eq!(
            credential(&paid(true)),
            (ReadinessState::NeedsSetup, RequirementState::Invalid)
        );
        assert_eq!(
            credential(&paid(false)),
            (ReadinessState::NeedsSetup, RequirementState::Missing)
        );
        let r = paid(true).readiness(&answers);
        assert!(
            r.requirements[1]
                .setup
                .as_deref()
                .unwrap()
                .contains("rejected")
        );
        // Another key's answers are not this one's.
        let other = Answers::default();
        other.record(Some("OPENAI_API_KEY"), "claude-haiku-5-5", Answer::Rejected);
        assert_eq!(
            paid(true).readiness(&other).requirements[1].state,
            RequirementState::Present
        );
    }

    /// `model`: present as configured; valid after an accepted answer, and a rejection (the
    /// request never reached it) leaves it present.
    #[test]
    fn the_model_row() {
        let answers = Answers::default();
        let model = |a: &Answers| paid(true).readiness(a).requirements[2].clone();
        let r = model(&answers);
        assert_eq!(
            (r.kind, r.name.as_str(), r.state),
            (
                RequirementKind::Model,
                "claude-haiku-5-5",
                RequirementState::Present
            )
        );
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Rejected,
        );
        assert_eq!(model(&answers).state, RequirementState::Present);
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Accepted,
        );
        assert_eq!(model(&answers).state, RequirementState::Valid);
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Rejected,
        );
        assert_eq!(model(&answers).state, RequirementState::Valid);
    }

    /// What a call needs (task 9.6): the configuration always, the credential and the model only
    /// when it sends to the provider, the segmenter file only when it offers a tool that runs
    /// it; and of those, only the `missing` and `invalid` ones hold it back.
    #[test]
    fn a_call_is_held_back_only_by_what_it_needs() {
        let answers = Answers::default();
        let mut inspection = paid(false);
        inspection.files.push(LocalFile {
            path: "/cache/squint/models/segmenter-v1/seg.onnx".into(),
            present: false,
            setup: "Fetch the segmenter.".into(),
        });
        let kinds = |needs: &Needs| -> Vec<(RequirementKind, RequirementState)> {
            inspection
                .unmet(&answers, needs)
                .iter()
                .map(|r| (r.kind, r.state))
                .collect()
        };
        let checking = Needs {
            provider: true,
            tools: vec!["crop".into(), "regions".into()],
        };
        assert_eq!(
            kinds(&checking),
            [
                (RequirementKind::Credential, RequirementState::Missing),
                (RequirementKind::File, RequirementState::Missing)
            ]
        );
        // Without the segmenter's tools, the file is not this call's.
        let no_segmenter = Needs {
            provider: true,
            tools: vec!["crop".into()],
        };
        assert_eq!(
            kinds(&no_segmenter),
            [(RequirementKind::Credential, RequirementState::Missing)]
        );
        // A call that sends nothing (an estimate) needs neither the key nor the model.
        assert!(kinds(&Needs::default()).is_empty());
        // A rejected key holds a call back as `invalid`; an accepted one does not.
        let set = paid(true);
        assert!(set.unmet(&answers, &no_segmenter).is_empty());
        answers.record(
            Some("ANTHROPIC_API_KEY"),
            "claude-haiku-5-5",
            Answer::Rejected,
        );
        let unmet = set.unmet(&answers, &no_segmenter);
        assert_eq!(
            unmet.iter().map(|r| r.state).collect::<Vec<_>>(),
            [RequirementState::Invalid]
        );
        // The configuration holds back every call, a model the catalog lacks included.
        let unknown = Inspection::config_only(invalid_config(
            &present_config(),
            "model \"x\" is not in the catalog",
        ));
        assert_eq!(
            unknown.unmet(&Answers::default(), &Needs::default())[0].kind,
            RequirementKind::Config
        );
        let message = not_ready_message(&unmet);
        assert!(
            message.starts_with("not ready: ANTHROPIC_API_KEY invalid ("),
            "{message}"
        );
    }

    /// `file`: missing or present, never valid (describing never hashes it).
    #[test]
    fn the_file_row() {
        for present in [false, true] {
            let mut inspection = Inspection::config_only(present_config());
            let path = PathBuf::from("/cache/squint/models/segmenter-v1/seg.onnx");
            inspection.files.push(LocalFile {
                path: path.display().to_string(),
                present,
                setup: "Fetch the segmenter.".into(),
            });
            let r = inspection.readiness(&Answers::default());
            let (state, file) = if present {
                (ReadinessState::Ready, RequirementState::Present)
            } else {
                (ReadinessState::NeedsSetup, RequirementState::Missing)
            };
            assert_eq!(r.state, state);
            assert_eq!(
                states(&r),
                [
                    (RequirementKind::Config, RequirementState::Present),
                    (RequirementKind::File, file)
                ]
            );
        }
    }
}

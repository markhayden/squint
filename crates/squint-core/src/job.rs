//! The job: what is judged, against what, and under which use case.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A job file: `[job]` plus `[[job.images]]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobFile {
    pub job: Job,
}

/// One job. Requirements arrive in task 1.4a.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub use_case: String,
    #[serde(default)]
    pub brief: Option<String>,
    /// Structured obligations with the same semantics as rules; ids `RQ<n>`.
    #[serde(default)]
    pub requirements: Vec<crate::obligations::Obligation>,
    #[serde(default)]
    pub images: Vec<ImageSpec>,
}

/// An image in the job, before loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSpec {
    pub id: String,
    pub role: Role,
    #[serde(default)]
    pub label: Option<String>,
    pub source: ImageSource,
    #[serde(default)]
    pub projection: Option<Projection>,
    /// The contract's image reference the caller expects this file to have (table-manners 8.1):
    /// its digest and dimensions are verified before anything else runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<table_manners::images::ImageReference>,
}

/// What an image is for. Exactly one candidate; at most one source; any number of references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Candidate,
    Source,
    Reference,
}

/// Where the bytes come from. Base64 sources are decoded in task 1.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ImageSource {
    /// A file, optionally pinned to its SHA-256 (evaluation-set jobs always pin); the loader
    /// refuses a file whose hash differs.
    Path {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
    Base64 {
        base64: String,
        media_type: String,
    },
}

/// The image's projection. Detection rules arrive in task 1.2; `flat` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    #[default]
    Flat,
    Equirectangular,
}

impl Job {
    /// Loads and validates a job file.
    pub fn load(path: &Path) -> Result<Job> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Job(format!("cannot read {}: {e}", path.display())))?;
        let file: JobFile =
            toml::from_str(&text).map_err(|e| Error::Job(format!("{}: {e}", path.display())))?;
        file.job.validate()?;
        Ok(file.job)
    }

    /// Exactly one candidate, at most one source, unique ids.
    pub fn validate(&self) -> Result<()> {
        let candidates = self
            .images
            .iter()
            .filter(|i| i.role == Role::Candidate)
            .count();
        if candidates != 1 {
            return Err(Error::Job(format!(
                "a job has exactly one image with role = \"candidate\"; found {candidates}"
            )));
        }
        let sources = self
            .images
            .iter()
            .filter(|i| i.role == Role::Source)
            .count();
        if sources > 1 {
            return Err(Error::Job(format!(
                "a job has at most one image with role = \"source\"; found {sources}"
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for image in &self.images {
            if image.id.is_empty() {
                return Err(Error::Job("an image id must not be empty".into()));
            }
            if !image.id.chars().all(is_id_char) {
                return Err(Error::Job(format!(
                    "image id {:?}: ids are ASCII letters, digits and `_ . : / -` only",
                    image.id
                )));
            }
            if !seen.insert(image.id.as_str()) {
                return Err(Error::Job(format!("duplicate image id {:?}", image.id)));
            }
        }
        Ok(())
    }

    /// The candidate image spec. Validation guarantees there is exactly one.
    pub fn candidate(&self) -> Option<&ImageSpec> {
        self.images.iter().find(|i| i.role == Role::Candidate)
    }

    /// Builds a job from the CLI shorthand: `--candidate`, optional `--source`, repeated
    /// `--ref label=path` (ids `ref:<label>`), `--brief`, `--use-case`.
    pub fn from_shorthand(
        use_case: &str,
        candidate: PathBuf,
        source: Option<PathBuf>,
        refs: &[(String, PathBuf)],
        brief: Option<String>,
    ) -> Result<Job> {
        let mut images = vec![ImageSpec {
            id: "candidate".into(),
            role: Role::Candidate,
            label: None,
            source: ImageSource::Path {
                path: candidate,
                sha256: None,
            },
            projection: None,
            expect: None,
        }];
        if let Some(path) = source {
            images.push(ImageSpec {
                id: "source".into(),
                role: Role::Source,
                label: None,
                source: ImageSource::Path { path, sha256: None },
                projection: None,
                expect: None,
            });
        }
        for (label, path) in refs {
            images.push(ImageSpec {
                id: format!("ref:{label}"),
                role: Role::Reference,
                label: Some(label.clone()),
                source: ImageSource::Path {
                    path: path.clone(),
                    sha256: None,
                },
                projection: None,
                expect: None,
            });
        }
        let job = Job {
            use_case: use_case.to_string(),
            brief,
            requirements: Vec::new(),
            images,
        };
        job.validate()?;
        Ok(job)
    }

    /// The job with the requirements stated on the command line (task 11.2). A job that already
    /// has `[[job.requirements]]` takes none: one place states a job's requirements, so nothing
    /// is merged or renumbered. `origin` names the job's file for that refusal.
    pub fn with_intent(mut self, intent: &Intent, origin: &str) -> Result<Job> {
        if intent.is_empty() {
            return Ok(self);
        }
        if !self.requirements.is_empty() {
            return Err(Error::Job(format!(
                "{origin} has its own [[job.requirements]]; state every requirement there, or \
                 every one with --must-hold, --must-not and --allow"
            )));
        }
        self.requirements = intent.requirements()?;
        Ok(self)
    }

    /// A report-safe view of the job: base64 payloads are replaced by their byte count.
    pub fn summary(&self) -> JobSummary {
        JobSummary {
            use_case: self.use_case.clone(),
            brief: self.brief.clone(),
            images: self
                .images
                .iter()
                .map(|i| ImageSummary {
                    id: i.id.clone(),
                    role: i.role,
                    label: i.label.clone(),
                    source: match &i.source {
                        ImageSource::Path { path, .. } => format!("path:{}", path.display()),
                        ImageSource::Base64 { base64, media_type } => {
                            format!("base64:{media_type}:{} chars", base64.len())
                        }
                    },
                    projection: i.projection,
                })
                .collect(),
        }
    }
}

/// Requirements in a person's words, from `--must-hold`, `--must-not` and `--allow` (task 11.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Intent {
    pub must_hold: Vec<String>,
    pub must_not: Vec<String>,
    pub allow: Vec<String>,
}

impl Intent {
    pub fn is_empty(&self) -> bool {
        self.must_hold.is_empty() && self.must_not.is_empty() && self.allow.is_empty()
    }

    /// The job requirements these state: `RQ1`, `RQ2`, … for the `must_hold` texts, then the
    /// `must_not` texts, then the `allow` texts, each kind in the order given, so the numbering
    /// does not depend on how the flags interleave. Every one takes the obligation model's
    /// defaults (scope the candidate; evidence every source and reference); an `allow` is an
    /// exemption that waives nothing (`waives = []`): it says what may change.
    pub fn requirements(&self) -> Result<Vec<crate::obligations::Obligation>> {
        use crate::obligations::{Kind, Obligation};
        let stated = self
            .must_hold
            .iter()
            .map(|t| (Kind::MustHold, "--must-hold", t))
            .chain(
                self.must_not
                    .iter()
                    .map(|t| (Kind::MustNot, "--must-not", t)),
            )
            .chain(self.allow.iter().map(|t| (Kind::Allowed, "--allow", t)));
        let mut out = Vec::new();
        for (n, (kind, flag, text)) in stated.enumerate() {
            let text = text.trim();
            if text.is_empty() {
                return Err(Error::Job(format!("{flag} needs a sentence; it was empty")));
            }
            out.push(Obligation {
                id: format!("RQ{}", n + 1),
                kind,
                text: text.to_string(),
                matters: None,
                hidden_ok: false,
                scope: None,
                evidence: None,
                evidence_kinds: None,
                waives: Vec::new(),
                condition: None,
                disabled: false,
            });
        }
        Ok(out)
    }
}

/// Image ids name frames in prompts, tool arguments and checklist ids, so they are plain tokens.
pub fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-')
}

/// The job as written into a report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobSummary {
    pub use_case: String,
    pub brief: Option<String>,
    pub images: Vec<ImageSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageSummary {
    pub id: String,
    pub role: Role,
    pub label: Option<String>,
    pub source: String,
    pub projection: Option<Projection>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(roles: &[(&str, Role)]) -> Job {
        Job {
            use_case: "source-fidelity".into(),
            brief: None,
            requirements: Vec::new(),
            images: roles
                .iter()
                .map(|(id, role)| ImageSpec {
                    id: (*id).into(),
                    role: *role,
                    label: None,
                    source: ImageSource::Path {
                        path: "x.png".into(),
                        sha256: None,
                    },
                    projection: None,
                    expect: None,
                })
                .collect(),
        }
    }

    #[test]
    fn one_candidate_and_one_source_is_valid() {
        assert!(
            job(&[("candidate", Role::Candidate), ("source", Role::Source)])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn two_candidates_are_rejected() {
        let err = job(&[("a", Role::Candidate), ("b", Role::Candidate)]).validate();
        assert!(matches!(err, Err(Error::Job(m)) if m.contains("exactly one")));
    }

    #[test]
    fn missing_candidate_is_rejected() {
        assert!(job(&[("source", Role::Source)]).validate().is_err());
    }

    #[test]
    fn two_sources_are_rejected() {
        let err = job(&[
            ("c", Role::Candidate),
            ("s1", Role::Source),
            ("s2", Role::Source),
        ])
        .validate();
        assert!(matches!(err, Err(Error::Job(m)) if m.contains("at most one")));
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let err = job(&[("x", Role::Candidate), ("x", Role::Reference)]).validate();
        assert!(matches!(err, Err(Error::Job(m)) if m.contains("duplicate")));
    }

    #[test]
    fn intent_numbers_must_hold_then_must_not_then_allow() {
        use crate::obligations::Kind;
        let intent = Intent {
            must_hold: vec!["the dog's markings are unchanged".into()],
            must_not: vec!["a white patch is added".into(), "a second dog".into()],
            allow: vec![" the pose may change ".into()],
        };
        let r = intent.requirements().expect("valid");
        let ids: Vec<(&str, Kind)> = r.iter().map(|o| (o.id.as_str(), o.kind)).collect();
        assert_eq!(
            ids,
            [
                ("RQ1", Kind::MustHold),
                ("RQ2", Kind::MustNot),
                ("RQ3", Kind::MustNot),
                ("RQ4", Kind::Allowed)
            ]
        );
        assert_eq!(r[3].text, "the pose may change");
        assert!(r[3].waives.is_empty() && r[3].condition.is_none());
        assert!(r.iter().all(|o| o.scope.is_none() && o.evidence.is_none()));
    }

    #[test]
    fn intent_refuses_an_empty_sentence_and_a_job_with_requirements() {
        let blank = Intent {
            must_not: vec!["  ".into()],
            ..Intent::default()
        };
        assert!(matches!(blank.requirements(), Err(Error::Job(m)) if m.contains("--must-not")));
        let intent = Intent {
            must_hold: vec!["x".into()],
            ..Intent::default()
        };
        let mut with = job(&[("candidate", Role::Candidate)]);
        with.requirements = intent.requirements().expect("valid");
        let err = with.clone().with_intent(&intent, "job.toml");
        assert!(matches!(err, Err(Error::Job(m)) if m.starts_with("job.toml has its own")));
        assert!(
            with.with_intent(&Intent::default(), "job.toml").is_ok(),
            "no flags: the file's requirements stand"
        );
        let plain = job(&[("candidate", Role::Candidate)])
            .with_intent(&intent, "job.toml")
            .expect("a job without requirements takes the flags");
        assert_eq!(plain.requirements.len(), 1);
    }

    #[test]
    fn parses_the_pair_fixture() {
        let text = crate::fixture("jobs/pair.toml");
        let file: JobFile = toml::from_str(&text).expect("parses");
        assert_eq!(file.job.images.len(), 2);
        assert_eq!(
            file.job.candidate().map(|c| c.id.as_str()),
            Some("candidate")
        );
    }
}

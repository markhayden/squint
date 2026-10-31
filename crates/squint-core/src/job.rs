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

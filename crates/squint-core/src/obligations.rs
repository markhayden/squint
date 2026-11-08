//! Obligations: rules and job requirements under one model (SPEC "Obligations"). Task 1.4a ships
//! the types, the shipped-set lookup, the merge (base, `extends` chain, consumer files, job
//! requirements) and every validation rule that needs no tool registry. Scope and evidence
//! resolution to instances, and rendering, arrive in task 1.4b; the walking slice's hard-coded
//! instance stays until then.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::job::{Job, Role};
use crate::{Error, Result};

/// What an obligation does in aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    MustHold,
    MustNot,
    Allowed,
    Observe,
}

impl Kind {
    pub fn is_mandatory(self) -> bool {
        matches!(self, Kind::MustHold | Kind::MustNot)
    }
}

/// One obligation as written in a rules file or a job's `[[job.requirements]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Obligation {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    /// Controls uncertainty only. Defaults to true for mandatory kinds; ignored for other kinds.
    #[serde(default)]
    pub matters: Option<bool>,
    /// Mandatory kinds only: `hidden` is recorded instead of unsure.
    #[serde(default)]
    pub hidden_ok: bool,
    /// Image roles or ids whose correctness this judges; omitted means `candidate`.
    #[serde(default)]
    pub scope: Option<Vec<String>>,
    /// Image roles or ids consulted to judge it; omitted means every source and reference.
    #[serde(default)]
    pub evidence: Option<Vec<String>>,
    /// Tool evidence kinds whose regions bear on this; omitted means the set's default.
    #[serde(default)]
    pub evidence_kinds: Option<Vec<String>>,
    /// Exemptions only: the mandatory obligation ids this may waive (may be empty).
    #[serde(default)]
    pub waives: Vec<String>,
    /// Exemptions only: the condition under which the exemption applies.
    #[serde(default)]
    pub condition: Option<String>,
    /// Drops an obligation of this id from the merge.
    #[serde(default)]
    pub disabled: bool,
}

impl Obligation {
    pub fn is_mandatory(&self) -> bool {
        self.kind.is_mandatory()
    }

    /// The effective `matters` flag: explicit value, else true for mandatory kinds.
    pub fn matters(&self) -> bool {
        self.matters.unwrap_or(self.kind.is_mandatory())
    }
}

/// The `[rules]` header of a rules file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesHeader {
    pub use_case: String,
    pub version: u32,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub requires_roles: Vec<Role>,
    #[serde(default)]
    pub default_evidence_kinds: Vec<String>,
}

/// A rules file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    pub rules: RulesHeader,
    #[serde(default, rename = "rule")]
    pub obligations: Vec<Obligation>,
    /// Evidence hints per tool, injected into tool calls by the harness.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub hints: BTreeMap<String, toml::Table>,
}

impl RulesFile {
    pub fn parse(text: &str, name: &str) -> Result<RulesFile> {
        toml::from_str(text).map_err(|e| Error::Config(format!("rules {name}: {e}")))
    }

    pub fn load(path: &Path) -> Result<RulesFile> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("rules: cannot read {}: {e}", path.display())))?;
        RulesFile::parse(&text, &path.display().to_string())
    }
}

/// Every shipped base set name, one per use case.
pub const SHIPPED_BASES: &[&str] = &[
    "source-fidelity",
    "reference-consistency",
    "reality-checks",
    "brief-compliance",
    "product-fidelity",
];

/// Every shipped profile as `(base, profile)`; [`shipped`] finds each as `<base>/<profile>`.
pub const SHIPPED_PROFILES: &[(&str, &str)] = &[("source-fidelity", "architecture")];

/// The shipped rule sets, embedded at build time. Keys are base names and `<base>/<profile>`.
pub fn shipped(name: &str) -> Option<&'static str> {
    match name {
        "source-fidelity" => Some(include_str!("../rules/source-fidelity.toml")),
        "source-fidelity/architecture" => Some(include_str!(
            "../rules/profiles/source-fidelity-architecture.toml"
        )),
        "reference-consistency" => Some(include_str!("../rules/reference-consistency.toml")),
        "reality-checks" => Some(include_str!("../rules/reality-checks.toml")),
        "brief-compliance" => Some(include_str!("../rules/brief-compliance.toml")),
        "product-fidelity" => Some(include_str!("../rules/product-fidelity.toml")),
        _ => None,
    }
}

/// An obligation after merge, with where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MergedObligation {
    #[serde(flatten)]
    pub obligation: Obligation,
    /// `base:<name>`, `profile:<name>`, `file:<path>` or `job`.
    pub provenance: String,
}

/// The result of merging rules and requirements for one job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Merged {
    pub use_case: String,
    pub base: String,
    pub profile: Option<String>,
    /// Versions per layer, for the cache key and the report.
    pub versions: BTreeMap<String, u32>,
    pub requires_roles: Vec<Role>,
    pub default_evidence_kinds: Vec<String>,
    pub obligations: Vec<MergedObligation>,
    pub hints: BTreeMap<String, toml::Table>,
    pub warnings: Vec<String>,
}

impl Merged {
    pub fn get(&self, id: &str) -> Option<&Obligation> {
        self.obligations
            .iter()
            .find(|o| o.obligation.id == id)
            .map(|o| &o.obligation)
    }

    pub fn mandatory(&self) -> impl Iterator<Item = &Obligation> {
        self.obligations
            .iter()
            .map(|o| &o.obligation)
            .filter(|o| o.is_mandatory())
    }
}

/// Inputs to a merge.
pub struct MergeInput<'a> {
    pub base: &'a str,
    pub profile: Option<&'a str>,
    /// Consumer rules files, in order.
    pub extra: &'a [RulesFile],
    pub extra_names: &'a [String],
    pub job: Option<&'a Job>,
    /// Enabled tool names; `[hints.<tool>]` for any other tool is dropped with a warning.
    pub enabled_tools: &'a [String],
}

/// Merges base, profile, consumer files and job requirements, then validates.
pub fn merge(input: &MergeInput<'_>) -> Result<Merged> {
    let base_text = shipped(input.base).ok_or_else(|| {
        Error::Config(format!("rules: no shipped base set named {:?}", input.base))
    })?;
    let base = RulesFile::parse(base_text, input.base)?;
    let mut layers: Vec<(String, RulesFile)> = vec![(format!("base:{}", input.base), base.clone())];

    if let Some(profile) = input.profile {
        let key = format!("{}/{profile}", input.base);
        let text = shipped(&key).ok_or_else(|| {
            Error::Config(format!(
                "rules: no shipped profile {profile:?} of {:?}",
                input.base
            ))
        })?;
        let file = RulesFile::parse(text, &key)?;
        if file.rules.extends.as_deref() != Some(input.base) {
            return Err(Error::Config(format!(
                "rules: profile {profile:?} extends {:?}, not {:?}",
                file.rules.extends, input.base
            )));
        }
        layers.push((format!("profile:{profile}"), file));
    }
    for (file, name) in input.extra.iter().zip(input.extra_names) {
        layers.push((format!("file:{name}"), file.clone()));
    }

    let mut merged = Merged {
        use_case: base.rules.use_case.clone(),
        base: input.base.to_string(),
        profile: input.profile.map(str::to_string),
        versions: BTreeMap::new(),
        requires_roles: base.rules.requires_roles.clone(),
        default_evidence_kinds: base.rules.default_evidence_kinds.clone(),
        obligations: Vec::new(),
        hints: BTreeMap::new(),
        warnings: Vec::new(),
    };

    let mut hinted_by_files = std::collections::BTreeSet::new();
    for (provenance, file) in &layers {
        if file.rules.use_case != merged.use_case {
            return Err(Error::Config(format!(
                "rules: {provenance} is for use case {:?}, the base is {:?}",
                file.rules.use_case, merged.use_case
            )));
        }
        merged
            .versions
            .insert(provenance.clone(), file.rules.version);
        if !file.rules.requires_roles.is_empty() && !provenance.starts_with("base:") {
            merged.requires_roles = file.rules.requires_roles.clone();
        }
        if !file.rules.default_evidence_kinds.is_empty() && !provenance.starts_with("base:") {
            merged.default_evidence_kinds = file.rules.default_evidence_kinds.clone();
        }
        for ob in &file.obligations {
            apply(&mut merged.obligations, ob, provenance)?;
        }
        for (tool, table) in &file.hints {
            merged.hints.insert(tool.clone(), table.clone());
            if provenance.starts_with("file:") {
                hinted_by_files.insert(tool.clone());
            }
        }
    }
    // Hints for a tool that is not enabled are dropped. A consumer's own file naming one is
    // probably a typo and is warned about; the shipped rules carry hints for every shipped tool,
    // enabled or not, and drop them silently.
    let hinted: Vec<String> = merged.hints.keys().cloned().collect();
    for tool in hinted {
        if !input.enabled_tools.iter().any(|t| t == &tool) {
            if hinted_by_files.contains(&tool) {
                merged.warnings.push(format!(
                    "rules: [hints.{tool}] dropped: tool {tool:?} is not enabled"
                ));
            }
            merged.hints.remove(&tool);
        }
    }
    if let Some(job) = input.job {
        for req in &job.requirements {
            apply(&mut merged.obligations, req, "job")?;
        }
    }

    validate(&mut merged, input.job)?;
    Ok(merged)
}

fn apply(list: &mut Vec<MergedObligation>, ob: &Obligation, provenance: &str) -> Result<()> {
    check_id_format(&ob.id, provenance)?;
    if ob.disabled {
        list.retain(|m| m.obligation.id != ob.id);
        return Ok(());
    }
    match list.iter_mut().find(|m| m.obligation.id == ob.id) {
        Some(existing) => {
            existing.obligation = ob.clone();
            existing.provenance = provenance.to_string();
        }
        None => list.push(MergedObligation {
            obligation: ob.clone(),
            provenance: provenance.to_string(),
        }),
    }
    Ok(())
}

/// Rule ids are two letters plus a number (`SF1`); requirement ids are `RQ` plus a number.
fn check_id_format(id: &str, provenance: &str) -> Result<()> {
    let (letters, digits) = id.split_at(id.find(|c: char| c.is_ascii_digit()).unwrap_or(id.len()));
    let ok = letters.len() == 2
        && letters.chars().all(|c| c.is_ascii_uppercase())
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit());
    if !ok {
        return Err(Error::Config(format!(
            "rules: {provenance}: id {id:?} must be two capital letters and a number (SF1, RQ2)"
        )));
    }
    if provenance == "job" && letters != "RQ" {
        return Err(Error::Config(format!(
            "rules: job requirement id {id:?} must start with RQ"
        )));
    }
    Ok(())
}

fn validate(merged: &mut Merged, job: Option<&Job>) -> Result<()> {
    let ids: Vec<&str> = merged
        .obligations
        .iter()
        .map(|o| o.obligation.id.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    for id in &ids {
        if !seen.insert(*id) {
            return Err(Error::Config(format!(
                "rules: duplicate obligation id {id:?} after merge"
            )));
        }
    }
    let by_id: HashMap<&str, &Obligation> = merged
        .obligations
        .iter()
        .map(|o| (o.obligation.id.as_str(), &o.obligation))
        .collect();

    for o in merged.obligations.iter().map(|m| &m.obligation) {
        if o.text.trim().is_empty() {
            return Err(Error::Config(format!("rules: {} has no text", o.id)));
        }
        match o.kind {
            Kind::Allowed => {
                for w in &o.waives {
                    match by_id.get(w.as_str()) {
                        None => {
                            return Err(Error::Config(format!(
                                "rules: {} waives unknown obligation {w:?}",
                                o.id
                            )));
                        }
                        Some(t) if !t.is_mandatory() => {
                            return Err(Error::Config(format!(
                                "rules: {} waives {w}, which is not mandatory",
                                o.id
                            )));
                        }
                        _ => {}
                    }
                }
                if !o.waives.is_empty()
                    && o.condition
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or("")
                        .is_empty()
                {
                    return Err(Error::Config(format!(
                        "rules: exemption {} waives {:?} but has no condition",
                        o.id, o.waives
                    )));
                }
            }
            _ => {
                if !o.waives.is_empty() {
                    return Err(Error::Config(format!(
                        "rules: {} is not an exemption and cannot waive",
                        o.id
                    )));
                }
                if o.condition.is_some() {
                    return Err(Error::Config(format!(
                        "rules: {} is not an exemption and takes no condition",
                        o.id
                    )));
                }
            }
        }
        if o.hidden_ok && !o.is_mandatory() {
            return Err(Error::Config(format!(
                "rules: hidden_ok on {} is only for mandatory kinds",
                o.id
            )));
        }
        if let Some(scope) = &o.scope {
            if scope.is_empty() {
                return Err(Error::Config(format!(
                    "rules: {} has an explicit empty scope",
                    o.id
                )));
            }
            if let Some(job) = job {
                for entry in scope {
                    check_image_ref(entry, job, &o.id, "scope")?;
                }
            }
        }
        if let (Some(evidence), Some(job)) = (&o.evidence, job) {
            for entry in evidence {
                check_image_ref(entry, job, &o.id, "evidence")?;
            }
        }
    }

    if let Some(job) = job {
        if job.use_case != merged.use_case {
            return Err(Error::Config(format!(
                "rules: the job's use case {:?} does not match the rules' {:?}",
                job.use_case, merged.use_case
            )));
        }
        for role in &merged.requires_roles {
            if !job.images.iter().any(|i| i.role == *role) {
                return Err(Error::Config(format!(
                    "rules: {:?} requires an image with role {:?}, the job has none",
                    merged.use_case,
                    role_name(*role)
                )));
            }
        }
    }

    Ok(())
}

fn check_image_ref(entry: &str, job: &Job, id: &str, field: &str) -> Result<()> {
    let is_role = matches!(entry, "candidate" | "source" | "reference");
    let is_id = job.images.iter().any(|i| i.id == entry);
    if is_role || is_id {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "rules: {id} {field} entry {entry:?} is neither a role nor an image id in the job"
        )))
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Candidate => "candidate",
        Role::Source => "source",
        Role::Reference => "reference",
    }
}

/// One obligation resolved to one target image: `SF2@candidate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Instance {
    pub obligation_id: String,
    pub target: String,
    pub kind: Kind,
    pub matters: bool,
    pub hidden_ok: bool,
    pub text: String,
    /// Image ids consulted to judge this instance.
    pub evidence: Vec<String>,
    /// Tool evidence kinds whose regions bear on this instance.
    pub evidence_kinds: Vec<String>,
    pub provenance: String,
}

impl Instance {
    /// The instance id, `<obligation>@<target>`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.obligation_id, self.target)
    }

    pub fn is_mandatory(&self) -> bool {
        self.kind.is_mandatory()
    }
}

/// An exemption resolved to the targets it may waive on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exemption {
    pub id: String,
    pub text: String,
    pub waives: Vec<String>,
    pub condition: Option<String>,
    pub targets: Vec<String>,
    pub provenance: String,
}

impl Exemption {
    /// True when this exemption may waive `obligation_id` on `target`.
    pub fn applies_to(&self, obligation_id: &str, target: &str) -> bool {
        self.waives.iter().any(|w| w == obligation_id) && self.targets.iter().any(|t| t == target)
    }
}

/// An observational obligation resolved to its targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub text: String,
    pub targets: Vec<String>,
    pub provenance: String,
}

/// Everything resolution produces for one job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub instances: Vec<Instance>,
    pub exemptions: Vec<Exemption>,
    pub observations: Vec<Observation>,
}

impl Resolved {
    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id() == id)
    }
}

/// Expands a scope or evidence list to image ids: roles expand to every image with that role, ids
/// pass through, duplicates are removed, job order is kept.
fn expand(entries: &[String], job: &Job) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for image in &job.images {
        let hit = entries.iter().any(|e| {
            e == &image.id
                || matches!(
                    (e.as_str(), image.role),
                    ("candidate", Role::Candidate)
                        | ("source", Role::Source)
                        | ("reference", Role::Reference)
                )
        });
        if hit && !out.contains(&image.id) {
            out.push(image.id.clone());
        }
    }
    out
}

/// Resolves merged obligations against a job into instances, exemptions and observations
/// (SPEC "Instances"). Errors: a mandatory obligation with no target, a job with no mandatory
/// instance, a reference image no mandatory obligation consults.
pub fn resolve(merged: &Merged, job: &Job) -> Result<Resolved> {
    let default_scope = vec!["candidate".to_string()];
    let default_evidence = vec!["source".to_string(), "reference".to_string()];
    let mut resolved = Resolved {
        instances: Vec::new(),
        exemptions: Vec::new(),
        observations: Vec::new(),
    };

    for m in &merged.obligations {
        let o = &m.obligation;
        let targets = expand(o.scope.as_ref().unwrap_or(&default_scope), job);
        match o.kind {
            Kind::MustHold | Kind::MustNot => {
                if targets.is_empty() {
                    return Err(Error::Config(format!(
                        "rules: {} resolves to no target image in this job (scope {:?}); disable it \
                         rather than leaving it empty",
                        o.id,
                        o.scope.as_ref().unwrap_or(&default_scope)
                    )));
                }
                let evidence = expand(o.evidence.as_ref().unwrap_or(&default_evidence), job);
                let evidence_kinds = o
                    .evidence_kinds
                    .clone()
                    .unwrap_or_else(|| merged.default_evidence_kinds.clone());
                for target in targets {
                    resolved.instances.push(Instance {
                        obligation_id: o.id.clone(),
                        target,
                        kind: o.kind,
                        matters: o.matters(),
                        hidden_ok: o.hidden_ok,
                        text: o.text.clone(),
                        evidence: evidence.clone(),
                        evidence_kinds: evidence_kinds.clone(),
                        provenance: m.provenance.clone(),
                    });
                }
            }
            Kind::Allowed => resolved.exemptions.push(Exemption {
                id: o.id.clone(),
                text: o.text.clone(),
                waives: o.waives.clone(),
                condition: o.condition.clone(),
                targets,
                provenance: m.provenance.clone(),
            }),
            Kind::Observe => resolved.observations.push(Observation {
                id: o.id.clone(),
                text: o.text.clone(),
                targets,
                provenance: m.provenance.clone(),
            }),
        }
    }

    if resolved.instances.is_empty() {
        return Err(Error::Config(
            "rules: this check has no mandatory instance; at least one must_hold or must_not obligation \
             must apply to an image in the job"
                .into(),
        ));
    }
    for image in job.images.iter().filter(|i| i.role == Role::Reference) {
        let consulted = resolved
            .instances
            .iter()
            .any(|i| i.evidence.contains(&image.id));
        if !consulted {
            return Err(Error::Config(format!(
                "rules: reference image {:?} is consulted by no mandatory obligation; remove it from the \
                 job or add a requirement that uses it",
                image.id
            )));
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_sets_parse() {
        let profiles = SHIPPED_PROFILES.iter().map(|(b, p)| format!("{b}/{p}"));
        for name in SHIPPED_BASES.iter().map(|b| b.to_string()).chain(profiles) {
            let file = RulesFile::parse(shipped(&name).expect("shipped"), &name).expect("parses");
            let base = name.split('/').next().unwrap_or_default();
            assert_eq!(file.rules.use_case, base, "{name}");
        }
        assert!(shipped("nope").is_none());
    }

    #[test]
    fn matters_defaults_by_kind() {
        let mut o = Obligation {
            id: "SF1".into(),
            kind: Kind::MustHold,
            text: "t".into(),
            matters: None,
            hidden_ok: false,
            scope: None,
            evidence: None,
            evidence_kinds: None,
            waives: vec![],
            condition: None,
            disabled: false,
        };
        assert!(o.matters());
        o.kind = Kind::Observe;
        assert!(!o.matters());
        o.matters = Some(true);
        assert!(
            o.matters(),
            "explicit value wins even for observations (aggregation ignores it)"
        );
    }

    #[test]
    fn id_format_is_enforced() {
        assert!(check_id_format("SF1", "base:x").is_ok());
        assert!(check_id_format("RQ12", "job").is_ok());
        assert!(check_id_format("sf1", "base:x").is_err());
        assert!(check_id_format("SFX", "base:x").is_err());
        assert!(check_id_format("S1", "base:x").is_err());
        assert!(
            check_id_format("SF1", "job").is_err(),
            "job requirements start with RQ"
        );
    }
}

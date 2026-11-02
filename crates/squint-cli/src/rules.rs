//! `squint rules list` and `squint rules init` (task 11.3): the shipped rule sets shown without
//! reading the source, and a starter config with a rules file to change them.

use std::fmt::Write as _;
use std::path::PathBuf;

use squint_core::obligations::{
    Kind, MergeInput, Obligation, RulesFile, SHIPPED_BASES, SHIPPED_PROFILES, merge, shipped,
};

use crate::EXIT_USAGE;

#[derive(clap::Args)]
pub(crate) struct RulesListArgs {
    /// A use case (`reference-consistency`, …) to print its rules; without one, the shipped sets.
    use_case: Option<String>,
    /// A shipped profile of that use case, merged over its base (`architecture`).
    #[arg(long, requires = "use_case")]
    profile: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct RulesInitArgs {
    /// The use case whose rules to start from (`reference-consistency`, …).
    use_case: String,
    /// The directory to write `squint.toml` and `rules.toml` into; created if missing.
    #[arg(long, default_value = ".")]
    out: PathBuf,
}

fn known_bases() -> String {
    SHIPPED_BASES.join(", ")
}

fn unknown(use_case: &str) -> i32 {
    eprintln!(
        "squint: no shipped rules for the use case {use_case:?}; the use cases are {}",
        known_bases()
    );
    EXIT_USAGE
}

pub(crate) fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::MustHold => "must_hold",
        Kind::MustNot => "must_not",
        Kind::Allowed => "allowed",
        Kind::Observe => "observe",
    }
}

pub(crate) fn list(args: &RulesListArgs) -> i32 {
    match &args.use_case {
        None => {
            print!("{}", sets());
            0
        }
        Some(use_case) if !SHIPPED_BASES.contains(&use_case.as_str()) => unknown(use_case),
        Some(use_case) => match rules_of(use_case, args.profile.as_deref()) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(e) => {
                eprintln!("squint: {e}");
                EXIT_USAGE
            }
        },
    }
}

/// The shipped bases: use case, the roles a job needs, and its profiles.
fn sets() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<23} {:<22} profiles", "use case", "a job needs");
    for base in SHIPPED_BASES {
        let roles = shipped(base)
            .and_then(|t| RulesFile::parse(t, base).ok())
            .map(|f| {
                f.rules
                    .requires_roles
                    .iter()
                    .map(|r| {
                        serde_json::to_value(r)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let profiles: Vec<&str> = SHIPPED_PROFILES
            .iter()
            .filter(|(b, _)| b == base)
            .map(|(_, p)| *p)
            .collect();
        let profiles = if profiles.is_empty() {
            "-".to_string()
        } else {
            profiles.join(", ")
        };
        let _ = writeln!(out, "{base:<23} {roles:<22} {profiles}");
    }
    let _ = writeln!(
        out,
        "\nsquint rules list <use-case> prints a set's rules; squint rules init <use-case> writes a \
         config and a rules file to change them."
    );
    out
}

fn list_or(values: &Option<Vec<String>>, default: &str) -> String {
    match values {
        Some(v) => v.join(", "),
        None => default.to_string(),
    }
}

/// One rule: id, kind, scope, evidence (mandatory kinds), what an exemption waives, then its text.
fn rule_lines(o: &Obligation, provenance: Option<&str>) -> String {
    let mut head = format!(
        "{:<5} {:<9}  scope: {}",
        o.id,
        kind_word(o.kind),
        list_or(&o.scope, "candidate (default)")
    );
    if o.is_mandatory() {
        let _ = write!(
            head,
            "  evidence: {}",
            list_or(&o.evidence, "every source and reference (default)")
        );
        if o.hidden_ok {
            head.push_str("  hidden_ok");
        }
        if o.matters == Some(false) {
            head.push_str("  matters: false");
        }
    }
    if o.kind == Kind::Allowed {
        if o.waives.is_empty() {
            head.push_str("  waives nothing");
        } else {
            let _ = write!(head, "  waives {}", o.waives.join(", "));
        }
    }
    if let Some(p) = provenance {
        let _ = write!(head, "  [{p}]");
    }
    let mut out = format!("{head}\n      {}\n", o.text);
    if let Some(c) = &o.condition {
        let _ = writeln!(out, "      when: {c}");
    }
    out
}

/// A use case's rules (merged over its profile when one is named), then how to change them.
fn rules_of(use_case: &str, profile: Option<&str>) -> Result<String, String> {
    let merged = merge(&MergeInput {
        base: use_case,
        profile,
        extra: &[],
        extra_names: &[],
        job: None,
        enabled_tools: &[],
    })
    .map_err(|e| e.to_string())?;
    let mut out = String::new();
    let roles: Vec<String> = merged
        .requires_roles
        .iter()
        .filter_map(|r| {
            serde_json::to_value(r)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
        })
        .collect();
    let _ = writeln!(
        out,
        "{use_case}{}: a job needs {}; tool evidence kinds by default: {}\n",
        profile
            .map(|p| format!(" (profile {p})"))
            .unwrap_or_default(),
        roles.join(", "),
        merged.default_evidence_kinds.join(", ")
    );
    for m in &merged.obligations {
        let provenance = profile.map(|_| m.provenance.as_str());
        out.push_str(&rule_lines(&m.obligation, provenance));
    }
    let _ = write!(
        out,
        "\nTo change these:\n\
         \x20 for one check, add requirements in your words: --must-hold \"…\", --must-not \"…\", \
         --allow \"…\"\n\
         \x20   (job requirements RQ1, RQ2, …; --allow says what may change and waives nothing)\n\
         \x20 to keep changes, list a rules file in the config's [rules] extra: the same id replaces \
         a rule,\n\
         \x20   `disabled = true` (with its id and kind) removes it, a new id adds one\n\
         \x20 squint rules init {use_case} writes a squint.toml and a rules.toml with every rule above \
         commented out\n\
         \x20 squint rules show --config squint.toml prints the merged result\n"
    );
    Ok(out)
}

pub(crate) fn init(args: &RulesInitArgs) -> i32 {
    let use_case = args.use_case.as_str();
    let Some(base_text) = shipped(use_case).filter(|_| SHIPPED_BASES.contains(&use_case)) else {
        return unknown(use_case);
    };
    let usage = |e: String| {
        eprintln!("squint: {e}");
        EXIT_USAGE
    };
    let dir = match std::path::absolute(&args.out) {
        Ok(d) => d,
        Err(e) => return usage(format!("{}: {e}", args.out.display())),
    };
    let config_path = dir.join(squint_core::config::CONFIG_FILE);
    let rules_path = dir.join("rules.toml");
    let choice = match crate::init::choose(None, None) {
        Ok(c) => c,
        Err(e) => return usage(e),
    };
    let config = crate::init::config_text(
        &choice,
        Some((use_case, &rules_path)),
        &format!("squint rules init {use_case}"),
    );
    let rules = match rules_text(use_case, base_text, &config_path) {
        Ok(r) => r,
        Err(e) => return usage(e),
    };
    if let Err(e) =
        crate::init::write_new(&[(config_path.clone(), config), (rules_path.clone(), rules)])
    {
        return usage(e);
    }
    println!("wrote {}", config_path.display());
    println!("wrote {}", rules_path.display());
    eprintln!(
        "next: edit {} (every {use_case} rule is there, commented out), then\n  \
         squint rules show --config {}\n  \
         squint check --config {} --candidate …\n\
         the key is read from {} and the account id from {}",
        rules_path.display(),
        config_path.display(),
        config_path.display(),
        choice.key_env,
        choice.account_env()
    );
    0
}

/// The rules file `rules init` writes: the header a rules file needs, then every shipped rule of
/// the base as written in the base (comments included), commented out.
fn rules_text(
    use_case: &str,
    base_text: &str,
    config_path: &std::path::Path,
) -> Result<String, String> {
    let base = RulesFile::parse(base_text, use_case).map_err(|e| e.to_string())?;
    let body = base_text
        .find("[[rule]]")
        .map(|i| &base_text[i..])
        .unwrap_or("");
    let mut out = format!(
        "# Your rules for {use_case}, applied after the shipped base (`squint rules list {use_case}`)\n\
         # and listed in the config's [rules] extra. Every base rule is below, commented out:\n\
         #   - to change one, uncomment its block and edit it: the same id replaces the base's rule;\n\
         #   - to remove one, uncomment its `[[rule]]`, `id` and `kind` lines and add `disabled = true`;\n\
         #   - to add one, write a new block with a new id (two capital letters and a number, say MY1).\n\
         # Every field: docs/rules.md. For one check only, the command line takes --must-hold,\n\
         # --must-not and --allow instead. Check the merged result:\n\
         #   squint rules show --config {}\n\
         \n\
         [rules]\n\
         use_case = \"{use_case}\"\n\
         # Bump when you change this file; it is part of the cache key.\n\
         version = 1\n\
         \n",
        config_path.display()
    );
    for line in body.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "# {line}");
        }
    }
    // What was written must load as a rules file and change nothing until edited.
    let file = RulesFile::parse(&out, "rules.toml").map_err(|e| e.to_string())?;
    if !file.obligations.is_empty() || base.obligations.is_empty() {
        return Err(format!(
            "the starter rules for {use_case} did not come out commented"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uncommenting every base rule gives back the base's rules exactly.
    #[test]
    fn the_starter_rules_hold_every_base_rule_commented_out() {
        for base in SHIPPED_BASES {
            let text = shipped(base).expect("shipped");
            let starter =
                rules_text(base, text, std::path::Path::new("/x/squint.toml")).expect("written");
            let header_end = starter.find("version = 1\n").expect("header") + "version = 1\n".len();
            let uncommented: String = starter[header_end..]
                .lines()
                .map(|l| l.strip_prefix("# ").unwrap_or(l))
                .collect::<Vec<_>>()
                .join("\n");
            let restored =
                RulesFile::parse(&format!("{}{uncommented}", &starter[..header_end]), base)
                    .expect("parses once uncommented");
            let original = RulesFile::parse(text, base).expect("shipped parses");
            assert_eq!(restored.obligations, original.obligations, "{base}");
        }
    }

    #[test]
    fn a_rule_prints_its_kind_scope_evidence_and_text() {
        let text = rules_of("reference-consistency", None).expect("lists");
        assert!(
            text.contains("RC1   must_hold  scope: candidate  evidence: reference"),
            "{text}"
        );
        assert!(
            text.contains("RC4   allowed    scope: candidate (default)  waives nothing"),
            "{text}"
        );
        assert!(text.contains("To change these:"), "{text}");
    }
}

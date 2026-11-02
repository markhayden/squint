//! Task 11.3: every TOML example on `docs/rules.md` is copy-paste correct. A block with a
//! `[rules]` table naming `use_case` is a rules file: it parses and merges over its base. A block
//! whose `[rules]` names `base` is a config's section: it parses as one and its base and profile
//! exist. Anything else is an example a reader could not use, and fails here.

use squint_core::config::RulesConfig;
use squint_core::obligations::{MergeInput, RulesFile, merge};

fn page() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/rules.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The ```toml blocks of a Markdown page, with the line each starts on.
fn toml_blocks(text: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (n, line) in text.lines().enumerate() {
        match &mut current {
            None if line.trim_start() == "```toml" => current = Some((n + 1, String::new())),
            Some(_) if line.trim_start() == "```" => blocks.extend(current.take()),
            Some((_, body)) => {
                body.push_str(line);
                body.push('\n');
            }
            None => {}
        }
    }
    blocks
}

fn merged(base: &str, profile: Option<&str>, extra: &[RulesFile]) -> Result<(), String> {
    let names: Vec<String> = (0..extra.len()).map(|i| format!("docs-{i}")).collect();
    merge(&MergeInput {
        base,
        profile,
        extra,
        extra_names: &names,
        job: None,
        enabled_tools: &[],
    })
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// What is wrong with one block, if anything.
fn check_block(body: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(body).map_err(|e| format!("not TOML: {e}"))?;
    let rules = table
        .get("rules")
        .and_then(|r| r.as_table())
        .ok_or("no [rules] table: a rules file needs its [rules] header (use_case, version)")?;
    if rules.contains_key("use_case") {
        let file = RulesFile::parse(body, "docs/rules.md").map_err(|e| e.to_string())?;
        let base = file.rules.use_case.clone();
        merged(&base, None, &[file])
    } else if rules.contains_key("base") {
        let config: RulesConfig = rules
            .clone()
            .try_into()
            .map_err(|e| format!("not a config's [rules]: {e}"))?;
        merged(&config.base, config.profile.as_deref(), &[])
    } else {
        Err("[rules] names neither use_case (a rules file) nor base (a config)".into())
    }
}

#[test]
fn every_toml_example_on_the_rules_page_loads() {
    let blocks = toml_blocks(&page());
    assert!(blocks.len() >= 3, "the page shows its examples");
    let failures: Vec<String> = blocks
        .iter()
        .filter_map(|(line, body)| check_block(body).err().map(|e| format!("line {line}: {e}")))
        .collect();
    assert!(failures.is_empty(), "docs/rules.md: {failures:#?}");
}

#[test]
fn a_rules_file_without_its_header_is_caught() {
    let headerless = "[[rule]]\nid = \"SF1\"\nkind = \"must_hold\"\ntext = \"t\"\n";
    assert!(check_block(headerless).is_err());
    let complete = format!("[rules]\nuse_case = \"source-fidelity\"\nversion = 1\n\n{headerless}");
    assert_eq!(check_block(&complete), Ok(()));
}

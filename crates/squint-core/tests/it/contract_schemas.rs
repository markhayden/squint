//! Task 7.1: the table-manners schemas copied under `schemas/contract/` are the pinned release's,
//! byte for byte, and `SOURCES.toml` records the tag and each file's sha256, so an edit to the copy
//! fails here. After a deliberate version bump of the `table-manners` dependency,
//! `UPDATE_CONTRACT=1` rewrites the copy and the record from the crate.
#![allow(clippy::unwrap_used)]

use sha2::{Digest, Sha256};

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/contract")
}

/// `urn:table-manners:core:result` is copied to `core/result.json`.
fn path_of(id: &str) -> String {
    let rest = id.strip_prefix("urn:table-manners:").unwrap();
    let (profile, name) = rest.split_once(':').unwrap();
    format!("{profile}/{name}.json")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write_copy() {
    let mut sources = format!(
        "# The table-manners schemas squint conforms to, copied from the crate pinned in Cargo.toml.\n\
         # Regenerate after a version bump: UPDATE_CONTRACT=1 cargo test -p squint-core --test contract_schemas\n\
         tag = \"v{}\"\n",
        table_manners::CONTRACT_VERSION
    );
    for (id, text) in table_manners::SCHEMAS {
        let path = path_of(id);
        let file = dir().join(&path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        sources += &format!(
            "\n[[file]]\npath = \"{path}\"\nid = \"{id}\"\nsha256 = \"{}\"\n",
            sha256(text.as_bytes())
        );
    }
    std::fs::write(dir().join("SOURCES.toml"), sources).unwrap();
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    tag: String,
    file: Vec<SourceFile>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    path: String,
    id: String,
    sha256: String,
}

#[test]
fn the_copied_contract_schemas_are_the_pinned_release() {
    if std::env::var_os("UPDATE_CONTRACT").is_some() {
        write_copy();
    }
    let text = std::fs::read_to_string(dir().join("SOURCES.toml"))
        .expect("schemas/contract/SOURCES.toml; run with UPDATE_CONTRACT=1 to create it");
    let sources: Sources = toml::from_str(&text).unwrap();
    assert_eq!(
        sources.tag,
        format!("v{}", table_manners::CONTRACT_VERSION),
        "SOURCES.toml names another release than the crate pinned in Cargo.toml"
    );

    let mut recorded: Vec<&str> = sources.file.iter().map(|f| f.id.as_str()).collect();
    let mut released: Vec<&str> = table_manners::SCHEMAS.iter().map(|(id, _)| *id).collect();
    recorded.sort_unstable();
    released.sort_unstable();
    assert_eq!(
        recorded, released,
        "SOURCES.toml lists other schemas than the release"
    );

    for file in &sources.file {
        assert_eq!(file.path, path_of(&file.id), "{}: path", file.id);
        let bytes = std::fs::read(dir().join(&file.path)).unwrap();
        assert_eq!(
            sha256(&bytes),
            file.sha256,
            "{}: edited after copying",
            file.path
        );
        let (_, text) = table_manners::SCHEMAS
            .iter()
            .find(|(id, _)| *id == file.id)
            .unwrap();
        assert_eq!(
            bytes,
            text.as_bytes(),
            "{}: differs from the release",
            file.path
        );
    }

    let mut on_disk = Vec::new();
    for profile in std::fs::read_dir(dir()).unwrap() {
        let profile = profile.unwrap().path();
        if profile.is_dir() {
            for file in std::fs::read_dir(&profile).unwrap() {
                let file = file.unwrap().path();
                on_disk.push(
                    file.strip_prefix(dir())
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    on_disk.sort();
    let mut listed: Vec<String> = sources.file.iter().map(|f| f.path.clone()).collect();
    listed.sort();
    assert_eq!(
        on_disk, listed,
        "a file under schemas/contract/ is not in SOURCES.toml"
    );
}

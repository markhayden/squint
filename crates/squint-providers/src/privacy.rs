//! Privacy enforcement before any live call (SPEC "Data leaving the machine", "Auth schemes").
//! Consent is bound to the profile, the account and the upstream allowlist; a credential comes from
//! the one environment variable the profile names, is never printed, and is added to the redaction
//! list; outgoing requests carry no filenames, paths or image metadata, which `scan` checks on
//! every recorded fixture.

use std::fmt;
use std::path::{Path, PathBuf};

use squint_core::config::Config;
use squint_core::{Error, Result};

use crate::catalog::Profile;

/// The consent a run proceeds under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consent {
    pub profile: String,
    pub account: String,
    /// Upstream providers the profile allows; empty for a direct provider endpoint.
    pub upstream_allow: Vec<String>,
}

/// Binds `[privacy]` to the selected profile: cloud calls allowed, and the account carries the
/// profile's prefix, so pointing a config at another provider changes the attestation with it.
pub fn consent(config: &Config, profile_id: &str, profile: &Profile) -> Result<Consent> {
    if !config.privacy.cloud_calls {
        return Err(Error::Config(
            "[privacy] cloud_calls must be true: squint sends images and text to the configured provider"
                .into(),
        ));
    }
    let account = config.privacy.account.trim();
    let (prefix, id) = account.split_once(':').unwrap_or((account, ""));
    if id.trim().is_empty() {
        return Err(Error::Config(format!(
            "[privacy] account must be `{}:<account id>` for profile {profile_id:?}",
            profile.account
        )));
    }
    if prefix != profile.account {
        return Err(Error::Config(format!(
            "[privacy] account {:?} does not consent to profile {profile_id:?}: it needs the prefix `{}:`",
            redact_account(account),
            profile.account
        )));
    }
    Ok(Consent {
        profile: profile_id.to_string(),
        account: account.to_string(),
        upstream_allow: Vec::new(),
    })
}

/// The account with its id shortened, for messages.
fn redact_account(account: &str) -> String {
    match account.split_once(':') {
        Some((p, id)) if id.chars().count() > 4 => {
            format!("{p}:{}…", id.chars().take(2).collect::<String>())
        }
        _ => account.to_string(),
    }
}

/// A credential read from the environment. Never printed: `Debug` and `Display` redact it.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    env: String,
    value: String,
}

impl Credential {
    /// The variable it came from.
    pub fn env(&self) -> &str {
        &self.env
    }

    /// The secret itself, for the one header that carries it and for the redaction list.
    pub fn expose(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential({}=<redacted>)", self.env)
    }
}

impl fmt::Display for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted>")
    }
}

/// The profile's credential from the environment. Missing or empty is a configuration error
/// (`resolve`, the contract's `usage`): a call that needs a missing key is refused `not_ready`
/// before it gets here (table-manners 3.1), so this guards only callers that skipped readiness.
pub fn credential(profile: &Profile, env: &dyn Fn(&str) -> Option<String>) -> Result<Credential> {
    match &profile.auth.scheme[..] {
        "api-key-header" | "bearer" => {}
        other => {
            return Err(Error::Config(format!(
                "auth scheme {other:?} is not available in this release"
            )));
        }
    }
    let name = &profile.auth.env;
    match env(name) {
        Some(v) if !v.trim().is_empty() => Ok(Credential {
            env: name.clone(),
            value: v.trim().to_string(),
        }),
        _ => Err(Error::Config(format!(
            "set {name} to the API key for this profile; squint reads credentials from the environment only"
        ))),
    }
}

/// Byte sequences no outgoing request may contain: local paths and image metadata markers.
pub const FORBIDDEN_MARKERS: &[&str] = &[
    "/Users/",
    "/home/",
    "/tmp/",
    "/var/folders/",
    "/private/",
    "C:\\",
    "file://",
    "fixtures/",
    "Exif",
    "eXIf",
    "<x:xmpmeta",
    "GPano",
];

/// Every forbidden marker found in a request body. JSON is walked value by value so that image
/// bytes, which are base64 and can spell any short marker by chance, are left out of the search;
/// anything else is scanned as text.
pub fn scan(text: &str) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => {
            let mut out = Vec::new();
            scan_value(&v, "", &mut out);
            out
        }
        Err(_) => scan_text(text, "")
            .into_iter()
            .map(|(marker, pos)| format!("{marker:?} at byte {pos}"))
            .collect(),
    }
}

fn scan_value(v: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(o) => {
            for (k, val) in o {
                let p = format!("{path}/{k}");
                for (marker, _) in scan_text(k, &p) {
                    out.push(format!("{marker:?} in key {p}"));
                }
                scan_value(val, &p, out);
            }
        }
        serde_json::Value::Array(a) => {
            for (i, val) in a.iter().enumerate() {
                scan_value(val, &format!("{path}/{i}"), out);
            }
        }
        serde_json::Value::String(s) if !is_image_data(s) => {
            for (marker, _) in scan_text(s, path) {
                out.push(format!(
                    "{marker:?} in {}",
                    if path.is_empty() { "/" } else { path }
                ));
            }
        }
        _ => {}
    }
}

/// Base64 image bytes (raw or as a data URL): long and drawn from the base64 alphabet only.
fn is_image_data(s: &str) -> bool {
    let body = s
        .strip_prefix("data:")
        .and_then(|r| r.split_once(";base64,"))
        .map(|(_, b)| b)
        .unwrap_or(s);
    body.len() >= 256
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

fn scan_text(text: &str, _path: &str) -> Vec<(&'static str, usize)> {
    let mut out = Vec::new();
    for marker in FORBIDDEN_MARKERS {
        let mut from = 0;
        while let Some(pos) = text[from..].find(marker) {
            out.push((*marker, from + pos));
            from += pos + marker.len();
        }
    }
    out
}

/// Scans every file under the recorded-fixture roots (`fixtures/adapters`, `fixtures/profiles`)
/// for forbidden markers. Returns `path: finding` lines; empty when clean or when no recordings
/// exist yet.
pub fn scan_fixtures(repo_root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    for sub in ["fixtures/adapters", "fixtures/profiles"] {
        let root = repo_root.join(sub);
        if !root.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect(&root, &mut files)?;
        files.sort();
        for path in files {
            let bytes = std::fs::read(&path)?;
            let text = String::from_utf8_lossy(&bytes);
            for f in scan(&text) {
                findings.push(format!(
                    "{}: {f}",
                    path.strip_prefix(repo_root).unwrap_or(&path).display()
                ));
            }
        }
    }
    Ok(findings)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

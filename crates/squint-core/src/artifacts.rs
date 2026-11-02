//! The content-addressed artifact store under `<run dir>/artifacts/` (SPEC "Report and journal").
//! Every artifact a journal event references is written, fsynced and renamed into place before the
//! event is appended; files are mode 0600. Text bodies are redacted of every expanded secret and
//! bounded at 2 MiB: a larger body is truncated with a marker and the SHA-256 of the full body is
//! recorded beside the hash of what was stored. Images and binary blobs are stored exactly.

use std::io::Write;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::image::hex;
use crate::{Error, Result};

/// Bodies larger than this are truncated.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// What replaces a secret in a stored body.
pub const REDACTED: &str = "[redacted]";

/// Replaces every expanded secret, in the raw form and as a JSON string would escape it. The one
/// redaction pass: stored bodies, journaled error messages (a provider's error excerpt among them)
/// and the command line's stderr all go through it.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(s.as_str(), REDACTED);
        let escaped = serde_json::to_string(s).unwrap_or_default();
        let escaped = escaped.trim_matches('"');
        if escaped != s && !escaped.is_empty() {
            out = out.replace(escaped, REDACTED);
        }
    }
    out
}

/// A reference to a stored artifact, as journal events and the report carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// SHA-256 of the stored bytes; the file name under `artifacts/`.
    pub sha256: String,
    /// Stored size.
    pub bytes: u64,
    pub truncated: bool,
    /// SHA-256 of the full body when it was truncated.
    pub full_sha256: Option<String>,
}

/// The store for one run.
#[derive(Debug)]
pub struct ArtifactStore {
    dir: PathBuf,
    secrets: Vec<String>,
}

impl ArtifactStore {
    /// Creates `artifacts/` under the run directory. `secrets` are redacted from every text body.
    pub fn create(run_dir: &Path, secrets: &[String]) -> Result<ArtifactStore> {
        let dir = run_dir.join("artifacts");
        std::fs::create_dir_all(&dir)?;
        Ok(ArtifactStore {
            dir,
            secrets: secrets.iter().filter(|s| !s.is_empty()).cloned().collect(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of an artifact by reference.
    pub fn path(&self, r: &ArtifactRef) -> PathBuf {
        self.dir.join(&r.sha256)
    }

    /// Stores bytes exactly (an image as sent, a blob a tool returned).
    pub fn put(&self, bytes: &[u8]) -> Result<ArtifactRef> {
        let sha256 = hex(&Sha256::digest(bytes));
        self.write(&sha256, bytes)?;
        Ok(ArtifactRef {
            sha256,
            bytes: bytes.len() as u64,
            truncated: false,
            full_sha256: None,
        })
    }

    /// Stores a text body: redacted, then truncated at `MAX_BODY_BYTES` with a marker.
    pub fn put_body(&self, body: &str) -> Result<ArtifactRef> {
        let redacted = self.redact(body);
        if redacted.len() <= MAX_BODY_BYTES {
            return self.put(redacted.as_bytes());
        }
        let full_sha256 = hex(&Sha256::digest(redacted.as_bytes()));
        let mut cut = MAX_BODY_BYTES;
        while !redacted.is_char_boundary(cut) {
            cut -= 1;
        }
        let stored = format!(
            "{}\n[truncated: {} bytes in full, sha256 {full_sha256}]\n",
            &redacted[..cut],
            redacted.len()
        );
        let mut r = self.put(stored.as_bytes())?;
        r.truncated = true;
        r.full_sha256 = Some(full_sha256);
        Ok(r)
    }

    /// Replaces every expanded secret of this run ([`redact`]).
    pub fn redact(&self, text: &str) -> String {
        redact(text, &self.secrets)
    }

    /// Write to a temporary name, fsync, rename into place, mode 0600. An artifact that already
    /// exists (same content) is left alone.
    fn write(&self, sha256: &str, bytes: &[u8]) -> Result<()> {
        let final_path = self.dir.join(sha256);
        if final_path.exists() {
            return Ok(());
        }
        let tmp = self
            .dir
            .join(format!(".{sha256}.{}.tmp", std::process::id()));
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut f = options.open(&tmp).map_err(|e| {
                Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("cannot create {}: {e}", tmp.display()),
                ))
            })?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }
}

/// Writes a file under the run directory with mode 0600, replacing any previous content.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    f.write_all(bytes)?;
    make_private(path)
}

/// Sets mode 0600 on an existing file (one another library wrote).
pub fn make_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str, secrets: &[&str]) -> ArtifactStore {
        let dir =
            std::env::temp_dir().join(format!("squint-artifacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        let secrets: Vec<String> = secrets.iter().map(|s| s.to_string()).collect();
        ArtifactStore::create(&dir, &secrets).expect("store")
    }

    #[test]
    fn artifacts_are_stored_by_content_hash_and_idempotently() {
        let s = store("hash", &[]);
        let a = s.put(b"hello").expect("put");
        let b = s.put(b"hello").expect("put again");
        assert_eq!(a, b);
        assert_eq!(a.sha256, hex(&Sha256::digest(b"hello")));
        assert_eq!(std::fs::read(s.path(&a)).expect("read"), b"hello");
        assert_eq!(
            std::fs::read_dir(s.dir()).expect("dir").count(),
            1,
            "no leftover temp file"
        );
    }

    #[test]
    fn artifacts_truncate_large_bodies_and_record_the_full_hash() {
        let s = store("trunc", &[]);
        let body = "x".repeat(MAX_BODY_BYTES + 1000);
        let r = s.put_body(&body).expect("put");
        assert!(r.truncated);
        assert_eq!(
            r.full_sha256.as_deref(),
            Some(hex(&Sha256::digest(body.as_bytes())).as_str())
        );
        let stored = std::fs::read_to_string(s.path(&r)).expect("read");
        assert!(stored.len() < body.len());
        assert!(
            stored.contains("[truncated: "),
            "{}",
            &stored[stored.len() - 120..]
        );
        assert_eq!(
            r.sha256,
            hex(&Sha256::digest(stored.as_bytes())),
            "the hash names the stored bytes"
        );
    }

    #[test]
    fn artifacts_redact_every_secret_in_text_bodies() {
        let s = store("redact", &["SYNTHETIC-token-1", "SYNTHETIC-token-2"]);
        let r = s
            .put_body("auth SYNTHETIC-token-1 and again SYNTHETIC-token-2 end")
            .expect("put");
        let stored = std::fs::read_to_string(s.path(&r)).expect("read");
        assert_eq!(stored, "auth [redacted] and again [redacted] end");
        assert!(!r.truncated);
        // A secret with characters JSON escapes is caught in its escaped form too.
        let s = store("redact-escaped", &["pa\"ss\\word"]);
        let body = serde_json::json!({ "text": "token pa\"ss\\word here" }).to_string();
        let r = s.put_body(&body).expect("put");
        let stored = std::fs::read_to_string(s.path(&r)).expect("read");
        assert!(!stored.contains("ss\\\\word"), "{stored}");
        assert!(stored.contains("[redacted]"));
    }

    #[cfg(unix)]
    #[test]
    fn artifacts_have_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let s = store("mode", &[]);
        let r = s.put(b"private").expect("put");
        let mode = std::fs::metadata(s.path(&r))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

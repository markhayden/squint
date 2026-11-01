//! Provisioning bundles (SPEC "Python tool pack"): a bundle directory holds the pack's wheel, a
//! provisioning project pinning it by path, that project's `uv.lock`, `MODEL.md`, and
//! `bundle.toml` with the SHA-256 of every file and the declared Python. `squint tools setup`
//! verifies every digest, unpacks the bundle under `$XDG_DATA_HOME/squint/tools/<package>-<version>/`
//! and runs `uv sync --frozen --no-dev` there. The harness later launches the installed executable
//! with `UV_OFFLINE=1` and `UV_PYTHON_DOWNLOADS=never`. A missing environment or a mismatched
//! digest is `error: resolve`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use squint_core::image::hex;
use squint_core::{Error, Result};

use crate::mcp::Spawn;

pub const BUNDLE_FILE: &str = "bundle.toml";
pub const LOCK_FILE: &str = "uv.lock";

/// `bundle.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFile {
    pub bundle: BundleHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleHeader {
    pub package: String,
    pub version: String,
    /// The Python the pack declares, as a uv version request (`3.12`, `>=3.12`).
    pub python: String,
    /// The executable the install produces, relative to the install directory.
    pub executable: String,
    /// Every file in the bundle but this one, with its SHA-256.
    pub files: BTreeMap<String, String>,
}

/// A bundle directory with its manifest read.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub dir: PathBuf,
    pub header: BundleHeader,
}

fn resolve(message: impl Into<String>) -> Error {
    Error::Config(format!("provisioning: {}", message.into()))
}

fn sha256_of(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).map_err(|e| resolve(format!("cannot read {}: {e}", path.display())))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

impl Bundle {
    pub fn load(dir: &Path) -> Result<Bundle> {
        let path = dir.join(BUNDLE_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| resolve(format!("cannot read {}: {e}", path.display())))?;
        let file: BundleFile =
            toml::from_str(&text).map_err(|e| resolve(format!("{}: {e}", path.display())))?;
        file.bundle
            .check()
            .map_err(|m| resolve(format!("{}: {m}", path.display())))?;
        Ok(Bundle {
            dir: dir.to_path_buf(),
            header: file.bundle,
        })
    }

    /// Every listed file is present with its digest, and nothing unlisted sits beside them.
    pub fn verify(&self) -> Result<()> {
        for (name, expected) in &self.header.files {
            let actual = sha256_of(&self.dir.join(name))?;
            if &actual != expected {
                return Err(resolve(format!(
                    "{}: digest mismatch for {name} (expected {expected}, found {actual})",
                    self.dir.display()
                )));
            }
        }
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name != BUNDLE_FILE && !self.header.files.contains_key(&name) {
                return Err(resolve(format!(
                    "{}: {name} is not listed in {BUNDLE_FILE}",
                    self.dir.display()
                )));
            }
        }
        Ok(())
    }

    /// The SHA-256 of the lock, recorded in tool provenance and the cache key.
    pub fn lock_digest(&self) -> &str {
        self.header
            .files
            .get(LOCK_FILE)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Verifies, copies the bundle into its install directory (replacing any previous install),
    /// verifies the copy, runs `uv sync --frozen --no-dev` with the declared Python, then the
    /// pack's `fetch` entry point as `fetch` says, so the model artifacts are in place before
    /// the first run (SPEC "Python tool pack").
    pub fn install(&self, data_home: &Path, fetch: &Fetch) -> Result<Installed> {
        self.verify()?;
        let dir = install_dir(data_home, &self.header.package, &self.header.version);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        for name in self.header.files.keys() {
            std::fs::copy(self.dir.join(name), dir.join(name))?;
        }
        std::fs::copy(self.dir.join(BUNDLE_FILE), dir.join(BUNDLE_FILE))?;
        Bundle::load(&dir)?.verify()?;
        let uv = std::env::var_os("UV").unwrap_or_else(|| "uv".into());
        uv_sync(&dir, &self.header.python, Path::new(&uv), std::env::vars())?;
        let installed = Installed::locate(data_home, &self.header.package, &self.header.version)?;
        installed.fetch(fetch, std::env::vars())?;
        Ok(installed)
    }
}

/// Where the pack's `fetch` entry point takes its model artifacts from after the sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// The release the pack's record names (network).
    Release,
    /// A directory holding the files, for example the export's output before publication; and
    /// where to put them, the pack's default (`$XDG_CACHE_HOME/squint/models`) when `None`.
    From {
        dir: PathBuf,
        models: Option<PathBuf>,
    },
    /// Do not run `fetch`: the pack's tools that need an artifact report it missing until it is.
    Skip,
}

impl Fetch {
    /// The arguments after `fetch`, or `None` when nothing is to run.
    pub fn args(&self) -> Option<Vec<String>> {
        match self {
            Fetch::Release => Some(vec!["fetch".into()]),
            Fetch::From { dir, models } => {
                let mut args = vec![
                    "fetch".to_string(),
                    "--from".into(),
                    dir.display().to_string(),
                ];
                if let Some(models) = models {
                    args.extend(["--models".to_string(), models.display().to_string()]);
                }
                Some(args)
            }
            Fetch::Skip => None,
        }
    }
}

/// A plain file or directory name: no separators, not `.` or `..`, nothing hidden.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

impl BundleHeader {
    /// Every path the header names stays inside the bundle and the install directory: file keys
    /// are plain names, package and version are plain tokens, the executable is a relative path
    /// without `..`, and the lock is listed.
    fn check(&self) -> std::result::Result<(), String> {
        if self.python.is_empty() {
            return Err("python must be set".into());
        }
        for (field, value) in [("package", &self.package), ("version", &self.version)] {
            if !is_plain_name(value) {
                return Err(format!("{field} {value:?} must be a plain token"));
            }
        }
        for name in self.files.keys() {
            if !is_plain_name(name) {
                return Err(format!("file {name:?} must be a plain file name"));
            }
        }
        if !self.files.contains_key(LOCK_FILE) {
            return Err(format!("files must include {LOCK_FILE}"));
        }
        let exe = Path::new(&self.executable);
        let relative_and_contained = !self.executable.is_empty()
            && exe.is_relative()
            && exe
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !relative_and_contained {
            return Err(format!(
                "executable {:?} must be a relative path inside the install directory",
                self.executable
            ));
        }
        Ok(())
    }
}

/// `<data_home>/squint/tools/<package>-<version>`.
pub fn install_dir(data_home: &Path, package: &str, version: &str) -> PathBuf {
    data_home
        .join("squint")
        .join("tools")
        .join(format!("{package}-{version}"))
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME")
        && !x.is_empty()
    {
        return PathBuf::from(x);
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".local").join("share")
}

/// Where the pack keeps fetched model artifacts, as the pack itself decides it (its
/// `models.models_dir`): `$XDG_CACHE_HOME/squint/models`, else `~/.cache/squint/models`.
pub fn models_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
        });
    cache.join("squint").join("models")
}

/// What a model record shipped with the pack (`squint_tools/models/<name>.json`) says about its
/// artifact: the file name and its size. The digest is the pack's to check when it fetches.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ModelRecord {
    pub artifact: String,
    pub size_bytes: u64,
}

impl ModelRecord {
    /// Where the pack expects the artifact: `<models dir>/<record name>/<artifact>`.
    pub fn artifact_path(&self, models: &Path, name: &str) -> PathBuf {
        models.join(name).join(&self.artifact)
    }
}

/// The environment a provisioning subprocess (`uv sync`, the pack's `fetch`) gets: the pack
/// allowlist (`PATH`, `HOME`, locale) plus uv's own settings (`UV_CACHE_DIR`, `UV_OFFLINE`,
/// `UV_PYTHON` and the rest of `UV_*`), which decide where the locked wheels come from; never a
/// credential, a credential-shaped name, or an index password.
fn provisioning_env<I, K, V>(environment: I) -> std::collections::BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let all: Vec<(String, String)> = environment
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    let uv: Vec<String> = all
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| k.starts_with("UV_") && !k.contains("PASSWORD") && !k.contains("USERNAME"))
        .collect();
    squint_core::env::allowlist(&uv, all)
}

/// Runs `uv sync` in the install directory with the environment the server launch would get:
/// the allowlist (`PATH`, `HOME`, locale; never a credential) and nothing else from the caller.
fn uv_sync<I, K, V>(dir: &Path, python: &str, uv: &Path, environment: I) -> Result<()>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let output = Command::new(uv)
        .args(["sync", "--frozen", "--no-dev", "--python", python])
        .current_dir(dir)
        .env_clear()
        .envs(provisioning_env(environment))
        .output()
        .map_err(|e| resolve(format!("cannot run uv ({}): {e}", uv.display())))?;
    if !output.status.success() {
        return Err(resolve(format!(
            "uv sync failed in {}:\n{}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// An installed pack: its directory, executable and lock digest.
#[derive(Debug, Clone)]
pub struct Installed {
    pub dir: PathBuf,
    pub executable: PathBuf,
    pub lock_digest: String,
    pub header: BundleHeader,
}

impl Installed {
    /// The install for a package and version, which must hold its bundle file and executable.
    pub fn locate(data_home: &Path, package: &str, version: &str) -> Result<Installed> {
        let dir = install_dir(data_home, package, version);
        let bundle = Bundle::load(&dir).map_err(|e| {
            resolve(format!(
                "{package} {version} is not installed under {} (run `squint tools setup`): {e}",
                dir.display()
            ))
        })?;
        let executable = dir.join(&bundle.header.executable);
        if !executable.is_file() {
            return Err(resolve(format!(
                "{package} {version}: {} is missing; run `squint tools setup` again",
                executable.display()
            )));
        }
        Ok(Installed {
            lock_digest: bundle.lock_digest().to_string(),
            dir,
            executable,
            header: bundle.header,
        })
    }

    /// Runs the installed pack's `fetch` entry point as `fetch` says, with the environment the
    /// server gets: the allowlist, never a credential, plus the harness's own variables. A
    /// refusal (a hash mismatch, an unreachable source) is `error: resolve` carrying the pack's
    /// own words.
    pub fn fetch<I, K, V>(&self, fetch: &Fetch, environment: I) -> Result<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let Some(args) = fetch.args() else {
            return Ok(());
        };
        let spawn = self.spawn(vec![]);
        let mut env = provisioning_env(environment);
        env.extend(spawn.extra_env.iter().cloned());
        // The pack reports each file on stderr as it goes; a release fetch is 1.2 GB, so those
        // lines are passed on as they come rather than after minutes of silence, and the last
        // of them carry a refusal into the error.
        let mut child = Command::new(&self.executable)
            .args(&args)
            .env_clear()
            .envs(&env)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                resolve(format!(
                    "cannot run {} fetch: {e}",
                    self.executable.display()
                ))
            })?;
        let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        if let Some(stderr) = child.stderr.take() {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(std::io::Result::ok)
            {
                eprintln!("  {line}");
                if tail.len() == 40 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
        let status = child
            .wait()
            .map_err(|e| resolve(format!("{} fetch did not finish: {e}", self.header.package)))?;
        if !status.success() {
            return Err(resolve(format!(
                "{} {} failed:\n{}",
                self.header.package,
                args.join(" "),
                Vec::from(tail).join("\n")
            )));
        }
        Ok(())
    }

    /// The model record `name` as this install ships it, read from the package inside its
    /// environment (`.venv/lib/<python>/site-packages/squint_tools/models/<name>.json`); none
    /// when the install has no such record. A local read, never a fetch.
    pub fn model_record(&self, name: &str) -> Option<ModelRecord> {
        let lib = self.dir.join(".venv").join("lib");
        std::fs::read_dir(lib)
            .ok()?
            .flatten()
            .map(|python| {
                python
                    .path()
                    .join("site-packages/squint_tools/models")
                    .join(format!("{name}.json"))
            })
            .find_map(|path| {
                let text = std::fs::read_to_string(path).ok()?;
                serde_json::from_str(&text).ok()
            })
    }

    /// How the harness launches the pack: offline, no Python downloads, the lock digest exposed.
    pub fn spawn(&self, declared_env: Vec<String>) -> Spawn {
        Spawn {
            command: self.executable.clone(),
            args: vec!["serve".into()],
            declared_env,
            cwd: None,
            extra_env: vec![
                ("UV_OFFLINE".into(), "1".into()),
                ("UV_PYTHON_DOWNLOADS".into(), "never".into()),
                ("SQUINT_LOCK_DIGEST".into(), self.lock_digest.clone()),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    /// An installed pack whose executable is the given script, for tests of the subprocesses.
    fn installed_with(script: &str, tag: &str) -> super::Installed {
        let dir =
            std::env::temp_dir().join(format!("squint-provision-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("pack.sh");
        std::fs::write(&exe, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        super::Installed {
            dir: dir.clone(),
            executable: exe,
            lock_digest: "digest".into(),
            header: super::BundleHeader {
                package: "fake-pack".into(),
                version: "0.0.1".into(),
                python: ">=3.12".into(),
                executable: "pack.sh".into(),
                files: std::collections::BTreeMap::from([(
                    super::LOCK_FILE.to_string(),
                    "digest".to_string(),
                )]),
            },
        }
    }

    #[test]
    fn a_failed_fetch_is_resolve_with_the_packs_words_and_skip_never_runs_it() {
        let installed = installed_with(
            "#!/bin/sh\necho \"model.onnx: sha256 abc is not the recorded def\" >&2\nexit 1\n",
            "fetch-fails",
        );
        let err = installed
            .fetch(&super::Fetch::Release, std::env::vars())
            .unwrap_err();
        assert_eq!(err.class(), "resolve");
        assert!(
            err.to_string().contains("is not the recorded") && err.to_string().contains("fetch"),
            "{err}"
        );
        let never = installed_with("#!/bin/sh\nexit 1\n", "fetch-skip");
        never.fetch(&super::Fetch::Skip, std::env::vars()).unwrap();
    }

    #[test]
    fn the_fetch_subprocess_sees_the_allowlist_and_the_harness_variables_and_no_credential() {
        // The script fails when a credential-shaped variable reached it, and prints what it saw.
        let installed = installed_with(
            "#!/bin/sh\nif [ -n \"$SQUINT_TEST_API_KEY\" ]; then echo leaked >&2; exit 1; fi\n\
             [ -n \"$PATH\" ] || { echo no-path >&2; exit 1; }\n\
             [ \"$UV_OFFLINE\" = 1 ] || { echo no-uv-offline >&2; exit 1; }\n\
             [ \"$SQUINT_LOCK_DIGEST\" = digest ] || { echo no-digest >&2; exit 1; }\n\
             echo \"$1 $2 $3\"\nexit 0\n",
            "fetch-env",
        );
        // Passed straight to the script, the credential is seen and the script fails.
        let output = std::process::Command::new(&installed.executable)
            .arg("fetch")
            .env("SQUINT_TEST_API_KEY", "SYNTHETIC")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "the script detects a passed credential"
        );
        // Through `Installed::fetch` the same environment is filtered first: the credential is
        // gone, the allowlisted PATH and the harness's own variables arrive.
        let environment = vec![
            ("SQUINT_TEST_API_KEY".to_string(), "SYNTHETIC".to_string()),
            (
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            ),
        ];
        installed
            .fetch(
                &super::Fetch::From {
                    dir: "/exports".into(),
                    models: None,
                },
                environment,
            )
            .unwrap();
    }

    #[test]
    fn provisioning_keeps_uvs_settings_and_drops_credentials() {
        // The network-free install needs the cache and offline settings; a credential, an index
        // password and an unrelated variable never reach the subprocess.
        let env = [
            ("PATH", "/usr/bin"),
            ("UV_CACHE_DIR", "/cache/uv"),
            ("UV_OFFLINE", "1"),
            ("UV_PYTHON", "3.12"),
            ("UV_INDEX_PRIVATE_PASSWORD", "SYNTHETIC"),
            ("UV_PUBLISH_TOKEN", "SYNTHETIC"),
            ("ANTHROPIC_API_KEY", "SYNTHETIC"),
            ("EDITOR", "vi"),
        ];
        let kept = super::provisioning_env(env);
        assert_eq!(
            kept.keys().map(String::as_str).collect::<Vec<_>>(),
            ["PATH", "UV_CACHE_DIR", "UV_OFFLINE", "UV_PYTHON"]
        );
    }

    #[test]
    fn fetch_arguments_name_the_source_or_nothing_to_run() {
        use super::Fetch;
        assert_eq!(Fetch::Release.args(), Some(vec!["fetch".to_string()]));
        assert_eq!(
            Fetch::From {
                dir: "/exports/segmenter".into(),
                models: None
            }
            .args(),
            Some(vec![
                "fetch".to_string(),
                "--from".to_string(),
                "/exports/segmenter".to_string()
            ])
        );
        assert_eq!(
            Fetch::From {
                dir: "/exports/segmenter".into(),
                models: Some("/cache/models".into())
            }
            .args()
            .unwrap()[3..],
            ["--models".to_string(), "/cache/models".to_string()]
        );
        assert_eq!(Fetch::Skip.args(), None);
    }

    use super::*;

    fn synthetic(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("squint-bundle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        std::fs::write(dir.join("uv.lock"), "lock").expect("write");
        std::fs::write(dir.join("MODEL.md"), "model").expect("write");
        let digest = |s: &str| hex(&Sha256::digest(s.as_bytes()));
        std::fs::write(
            dir.join(BUNDLE_FILE),
            format!(
                "[bundle]\npackage = \"p\"\nversion = \"1\"\npython = \">=3.12\"\nexecutable = \".venv/bin/p\"\n[bundle.files]\n\"uv.lock\" = \"{}\"\n\"MODEL.md\" = \"{}\"\n",
                digest("lock"),
                digest("model")
            ),
        )
        .expect("write");
        dir
    }

    /// An install's model record is read from the package in its environment, whatever the
    /// Python version directory is called; an install without one has none.
    #[test]
    fn an_installed_pack_ships_its_model_record() {
        let dir = synthetic("record");
        let installed = Installed {
            dir: dir.clone(),
            executable: dir.join(".venv/bin/p"),
            lock_digest: String::new(),
            header: Bundle::load(&dir).expect("load").header,
        };
        assert_eq!(installed.model_record("segmenter-v1"), None);
        let models = dir.join(".venv/lib/python3.13/site-packages/squint_tools/models");
        std::fs::create_dir_all(&models).expect("mkdir");
        std::fs::write(
            models.join("segmenter-v1.json"),
            r#"{"artifact": "seg.onnx", "size_bytes": 42, "sha256": "ab"}"#,
        )
        .expect("write");
        let record = installed.model_record("segmenter-v1").expect("record");
        assert_eq!(
            record,
            ModelRecord {
                artifact: "seg.onnx".into(),
                size_bytes: 42
            }
        );
        assert_eq!(
            record.artifact_path(Path::new("/cache/models"), "segmenter-v1"),
            Path::new("/cache/models/segmenter-v1/seg.onnx")
        );
    }

    #[test]
    fn provisioning_bundle_verifies_digests_and_refuses_tampering() {
        let dir = synthetic("ok");
        let bundle = Bundle::load(&dir).expect("load");
        bundle.verify().expect("intact");
        assert_eq!(bundle.lock_digest(), hex(&Sha256::digest(b"lock")));
        std::fs::write(dir.join("MODEL.md"), "tampered").expect("write");
        let err = Bundle::load(&dir)
            .expect("load")
            .verify()
            .expect_err("tampered");
        assert_eq!(err.class(), "resolve");
        assert!(
            err.to_string().contains("digest mismatch for MODEL.md"),
            "{err}"
        );
    }

    #[test]
    fn provisioning_bundle_refuses_unlisted_and_missing_files() {
        let dir = synthetic("extra");
        std::fs::write(dir.join("stray.bin"), "x").expect("write");
        let err = Bundle::load(&dir)
            .expect("load")
            .verify()
            .expect_err("unlisted");
        assert!(err.to_string().contains("stray.bin is not listed"), "{err}");
        std::fs::remove_file(dir.join("stray.bin")).expect("rm");
        std::fs::remove_file(dir.join("MODEL.md")).expect("rm");
        let err = Bundle::load(&dir)
            .expect("load")
            .verify()
            .expect_err("missing");
        assert!(err.to_string().contains("cannot read"), "{err}");
    }

    #[test]
    fn provisioning_bundle_refuses_paths_that_escape() {
        let dir = synthetic("escape");
        let digest = |s: &str| hex(&Sha256::digest(s.as_bytes()));
        let cases = [
            (
                "file key",
                format!(
                    "[bundle]\npackage = \"p\"\nversion = \"1\"\npython = \">=3.12\"\nexecutable = \".venv/bin/p\"\n[bundle.files]\n\"uv.lock\" = \"{}\"\n\"../escape\" = \"{}\"\n",
                    digest("lock"),
                    digest("x")
                ),
            ),
            (
                "package",
                format!(
                    "[bundle]\npackage = \"../p\"\nversion = \"1\"\npython = \">=3.12\"\nexecutable = \".venv/bin/p\"\n[bundle.files]\n\"uv.lock\" = \"{}\"\n",
                    digest("lock")
                ),
            ),
            (
                "executable",
                format!(
                    "[bundle]\npackage = \"p\"\nversion = \"1\"\npython = \">=3.12\"\nexecutable = \"../../bin/sh\"\n[bundle.files]\n\"uv.lock\" = \"{}\"\n",
                    digest("lock")
                ),
            ),
            (
                "absolute executable",
                format!(
                    "[bundle]\npackage = \"p\"\nversion = \"1\"\npython = \">=3.12\"\nexecutable = \"/bin/sh\"\n[bundle.files]\n\"uv.lock\" = \"{}\"\n",
                    digest("lock")
                ),
            ),
        ];
        for (what, header) in cases {
            std::fs::write(dir.join(BUNDLE_FILE), header).expect("write");
            let err = Bundle::load(&dir).expect_err(what);
            assert_eq!(err.class(), "resolve", "{what}");
        }
    }

    #[test]
    fn provisioning_install_dir_and_locate() {
        let home = synthetic("home");
        assert_eq!(install_dir(&home, "p", "1"), home.join("squint/tools/p-1"));
        let err = Installed::locate(&home, "p", "1").expect_err("not installed");
        assert_eq!(err.class(), "resolve");
        assert!(err.to_string().contains("squint tools setup"), "{err}");
    }
}

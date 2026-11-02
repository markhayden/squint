//! Reading an invocation bundle (table-manners 4.9): the archive file or a directory with the same
//! members, checked whole before anything runs. squint never builds one (no tool does, 4.9); the
//! test-only writer is `squint_testkit::bundle`.
//!
//! The archive is an uncompressed POSIX ustar with fixed headers (rascal ADR 0022's rules, which
//! the contract adopts): `invocation.json` first, then every other member sorted by name; each a
//! regular file with mode 0644, uid and gid 0, mtime 0, no user, group or link name, no name prefix;
//! zero padding; and the two zero blocks that close it with nothing after. The reader is written
//! here (no archive dependency) and never unpacks to disk: members are read into memory within
//! the bounds below, each checked before its bytes are read, so a hostile archive costs at most a
//! header past the fault it is refused for.
//!
//! The manifest is checked as soon as it is read, before any other member: against the
//! contract's `invocation-bundle.json`, then against what squint replays (every step contract
//! `0.4.0` and tool `squint`; no `artifacts`, `unportable` or `reference_only` entry, since squint
//! takes no native inputs and `squint call` has no way to supply a left-out one; each step's
//! `inputs` exactly the image arguments of its `invocation`, by slot). Every later member must be
//! listed by it, and every listed member present, its bytes hashing to the `file_sha256` of the
//! image it carries. Each refusal is an `input` error naming the member or the step, never a
//! member's bytes.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::path::Path;
use std::sync::OnceLock;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use table_manners::images::ImageReference;
use table_manners::{BundleStep, CONTRACT_VERSION, InvocationBundle};

use crate::image::hex;
use crate::{Error, Result};

/// The manifest's member name; always the archive's first member.
pub const MANIFEST: &str = "invocation.json";
/// The most bytes `invocation.json` may hold.
pub const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
/// The most bytes any other member may hold (squint's default image decode bound).
pub const MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
/// The most members a bundle may hold, the manifest included.
pub const MAX_MEMBERS: usize = 64;
/// The most bytes all members together may hold, the manifest included.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// The longest member name: the ustar name field (a canonical header leaves the prefix empty).
pub const MAX_NAME_BYTES: usize = 100;

const BLOCK: usize = 512;
/// The largest archive the bounds allow: every member's contents, a header and at most a block of
/// padding each, and the two closing zero blocks.
const MAX_ARCHIVE_BYTES: u64 = MAX_TOTAL_BYTES + (MAX_MEMBERS as u64) * 2 * BLOCK as u64 + 1024;
const MODE: u64 = 0o644;

/// A bundle read whole and checked: its manifest and the members it lists.
#[derive(Debug, Clone)]
pub struct Bundle {
    /// The manifest, validated against the contract's schema and squint's replay rules.
    pub manifest: InvocationBundle,
    /// SHA-256 of `invocation.json`'s bytes, lowercase hex: names the bundle's contents.
    pub manifest_sha256: String,
    members: BTreeMap<String, Vec<u8>>,
    /// Per step, each image argument of its invocation (a JSON pointer into `arguments`) and the
    /// member that carries it.
    located: Vec<Vec<Located>>,
}

/// An image argument of a step's invocation and its member.
#[derive(Debug, Clone)]
struct Located {
    /// The argument as a JSON pointer into `invocation.arguments` (`/references/1`).
    pointer: String,
    /// Its member, `inputs/<file_sha256>.<ext>`.
    member: String,
}

impl Bundle {
    /// Reads a bundle at `path`: a directory is read as the unpacked form, anything else as the
    /// archive. The path itself is the caller's to have checked (the path rule, 5.2).
    pub fn read(path: &Path) -> Result<Bundle> {
        let meta = std::fs::metadata(path).map_err(|e| refuse(format!("cannot read it: {e}")))?;
        if meta.is_dir() {
            return Bundle::from_dir(path);
        }
        if !meta.is_file() {
            return Err(refuse("neither an archive file nor a directory"));
        }
        if meta.len() > MAX_ARCHIVE_BYTES {
            return Err(refuse(format!(
                "the archive is {} bytes, over the {MAX_ARCHIVE_BYTES} its bounds allow",
                meta.len()
            )));
        }
        let file = File::open(path).map_err(|e| refuse(format!("cannot read it: {e}")))?;
        Bundle::from_archive(BufReader::new(file))
    }

    /// Reads the archive form from `reader`, which is read only as far as the checks allow: a
    /// refused header stops the read before its member's bytes.
    pub fn from_archive(mut reader: impl Read) -> Result<Bundle> {
        let mut collector = Collector::default();
        let mut previous: Option<String> = None;
        loop {
            let mut header = [0u8; BLOCK];
            read_block(&mut reader, &mut header)?;
            if header.iter().all(|&b| b == 0) {
                let mut second = [0u8; BLOCK];
                read_block(&mut reader, &mut second)?;
                if second.iter().any(|&b| b != 0) {
                    return Err(refuse(
                        "a zero block not followed by a second one (an archive closes with two)",
                    ));
                }
                let mut probe = [0u8; 1];
                match reader.read(&mut probe) {
                    Ok(0) => break,
                    Ok(_) => return Err(refuse("bytes after the two zero blocks that close it")),
                    Err(e) => return Err(refuse(format!("cannot read it: {e}"))),
                }
            }
            let entry = parse_header(&header, collector.count)?;
            match &previous {
                None if entry.name != MANIFEST => {
                    return Err(refuse(format!(
                        "the first member is {}, not {MANIFEST}",
                        shown(&entry.name)
                    )));
                }
                Some(_) if entry.name == MANIFEST => {
                    return Err(refuse(format!(
                        "{MANIFEST} appears again after the first member"
                    )));
                }
                Some(prev) if prev != MANIFEST && entry.name.as_bytes() <= prev.as_bytes() => {
                    return Err(refuse(if entry.name == *prev {
                        format!("member {} appears twice", shown(&entry.name))
                    } else {
                        format!(
                            "member {} comes after {}: members after {MANIFEST} are sorted by name",
                            shown(&entry.name),
                            shown(prev)
                        )
                    }));
                }
                _ => {}
            }
            collector.admit(&entry.name, entry.size)?;
            let mut bytes = Vec::new();
            let read = reader
                .by_ref()
                .take(entry.size)
                .read_to_end(&mut bytes)
                .map_err(|e| refuse(format!("cannot read member {}: {e}", shown(&entry.name))))?;
            if (read as u64) < entry.size {
                return Err(refuse(format!(
                    "truncated: member {} declares {} bytes and the archive ends after {read}",
                    shown(&entry.name),
                    entry.size
                )));
            }
            let pad = (BLOCK - (entry.size as usize % BLOCK)) % BLOCK;
            let mut padding = [0u8; BLOCK];
            match reader.read_exact(&mut padding[..pad]) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    return Err(refuse(format!(
                        "truncated: the archive ends inside member {}'s padding",
                        shown(&entry.name)
                    )));
                }
                Err(e) => return Err(refuse(format!("cannot read it: {e}"))),
            }
            if padding[..pad].iter().any(|&b| b != 0) {
                return Err(refuse(format!(
                    "non-zero padding after member {}",
                    shown(&entry.name)
                )));
            }
            collector.accept(&entry.name, bytes)?;
            previous = Some(entry.name);
        }
        collector.finish()
    }

    /// Reads the unpacked form: `invocation.json` and the directories `inputs/` and `artifacts/`,
    /// regular files only (a symlink is refused, never followed), within the same bounds.
    pub fn from_dir(dir: &Path) -> Result<Bundle> {
        let mut names: Vec<String> = Vec::new();
        let mut subdirs: Vec<&'static str> = Vec::new();
        let mut list = |relative: &str, subdirs: &mut Vec<&'static str>| -> Result<()> {
            let at = if relative.is_empty() {
                dir.to_path_buf()
            } else {
                dir.join(relative)
            };
            let entries =
                std::fs::read_dir(&at).map_err(|e| refuse(format!("cannot list it: {e}")))?;
            for entry in entries {
                let entry = entry.map_err(|e| refuse(format!("cannot list it: {e}")))?;
                let file_name = entry.file_name();
                let Some(file_name) = file_name.to_str() else {
                    return Err(refuse(format!(
                        "member {}: a name that is not UTF-8",
                        shown(&format!("{relative}{}", file_name.to_string_lossy()))
                    )));
                };
                let name = format!("{relative}{file_name}");
                let kind = entry
                    .file_type()
                    .map_err(|e| refuse(format!("cannot read member {}: {e}", shown(&name))))?;
                let what = if kind.is_symlink() {
                    "a symbolic link"
                } else if kind.is_dir() {
                    "a directory"
                } else if kind.is_file() {
                    "a regular file"
                } else {
                    "a special file (a device, FIFO or socket)"
                };
                if relative.is_empty() {
                    let sub = match file_name {
                        "inputs" => Some("inputs"),
                        "artifacts" => Some("artifacts"),
                        _ => None,
                    };
                    if let Some(sub) = sub {
                        if !kind.is_dir() {
                            return Err(refuse(format!(
                                "{} is {what}, not a directory",
                                shown(sub)
                            )));
                        }
                        subdirs.push(sub);
                        continue;
                    }
                }
                check_name(&name)?;
                if !kind.is_file() {
                    return Err(refuse(format!(
                        "member {} is {what}, not a regular file",
                        shown(&name)
                    )));
                }
                names.push(name);
                if names.len() > MAX_MEMBERS {
                    return Err(refuse(format!("more than {MAX_MEMBERS} members")));
                }
            }
            Ok(())
        };
        list("", &mut subdirs)?;
        subdirs.sort_unstable();
        for sub in subdirs {
            list(&format!("{sub}/"), &mut Vec::new())?;
        }
        names.sort();
        let Some(at) = names.iter().position(|n| n == MANIFEST) else {
            return Err(refuse(format!("no {MANIFEST}")));
        };
        let manifest = names.remove(at);
        names.insert(0, manifest);

        let mut collector = Collector::default();
        for name in names {
            let path = dir.join(&name);
            let file = File::open(&path)
                .map_err(|e| refuse(format!("cannot read member {}: {e}", shown(&name))))?;
            let meta = file
                .metadata()
                .map_err(|e| refuse(format!("cannot read member {}: {e}", shown(&name))))?;
            if !meta.is_file() {
                return Err(refuse(format!(
                    "member {} changed while it was read",
                    shown(&name)
                )));
            }
            collector.admit(&name, meta.len())?;
            let mut bytes = Vec::new();
            file.take(meta.len().saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|e| refuse(format!("cannot read member {}: {e}", shown(&name))))?;
            if bytes.len() as u64 != meta.len() {
                return Err(refuse(format!(
                    "member {} changed while it was read",
                    shown(&name)
                )));
            }
            collector.accept(&name, bytes)?;
        }
        collector.finish()
    }

    /// A listed member's bytes, by name (`inputs/<file_sha256>.<ext>`).
    pub fn member(&self, name: &str) -> Option<&[u8]> {
        self.members.get(name).map(Vec::as_slice)
    }

    /// Every member but the manifest, by name, in name order.
    pub fn members(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.members
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
    }

    /// Step `step`'s `invocation.arguments` with a `path` locator beside each image's recorded
    /// `expect`: its member's file under `inputs`, where the replay put the `inputs/` members
    /// (4.8: the arguments, with a locator added to each image, are the call). `None` for a step
    /// the bundle does not have.
    pub fn arguments(&self, step: usize, inputs: &Path) -> Option<Map<String, Value>> {
        let invocation = &self.manifest.steps.get(step)?.invocation;
        let mut arguments = Value::Object(invocation.arguments.clone());
        for located in self.located.get(step)? {
            let file = located
                .member
                .strip_prefix("inputs/")
                .unwrap_or(&located.member);
            let path = inputs.join(file);
            if let Some(Value::Object(image)) = arguments.pointer_mut(&located.pointer) {
                image.insert("path".into(), Value::String(path.to_string_lossy().into()));
            }
        }
        match arguments {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }
}

/// What both forms share once a member's name and size are known: the bounds, the manifest, the
/// listing and the digests.
#[derive(Default)]
struct Collector {
    count: usize,
    total: u64,
    /// The manifest once read, with the SHA-256 of its bytes and each step's located images.
    manifest: Option<(InvocationBundle, String, Vec<Vec<Located>>)>,
    /// Each member the manifest lists, with the SHA-256 its bytes must have.
    listed: BTreeMap<String, String>,
    members: BTreeMap<String, Vec<u8>>,
}

impl Collector {
    /// Checks a member before its bytes are read.
    fn admit(&mut self, name: &str, size: u64) -> Result<()> {
        self.count += 1;
        if self.count > MAX_MEMBERS {
            return Err(refuse(format!("more than {MAX_MEMBERS} members")));
        }
        let limit = if name == MANIFEST {
            MAX_MANIFEST_BYTES
        } else {
            MAX_MEMBER_BYTES
        };
        if size > limit {
            return Err(refuse(format!(
                "member {} is {size} bytes, over the {limit} a member may hold",
                shown(name)
            )));
        }
        self.total = self.total.saturating_add(size);
        if self.total > MAX_TOTAL_BYTES {
            return Err(refuse(format!(
                "its members exceed {MAX_TOTAL_BYTES} bytes together"
            )));
        }
        if name != MANIFEST && !self.listed.contains_key(name) {
            return Err(refuse(format!(
                "member {} is not listed in {MANIFEST}",
                shown(name)
            )));
        }
        Ok(())
    }

    /// Takes a member's bytes: the manifest is checked whole, any other member against its digest.
    fn accept(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        if name == MANIFEST {
            let sha = hex(&Sha256::digest(&bytes));
            let (manifest, listed, located) = manifest(&bytes)?;
            self.manifest = Some((manifest, sha, located));
            self.listed = listed;
            return Ok(());
        }
        let expected = self.listed.get(name);
        let actual = hex(&Sha256::digest(&bytes));
        if expected != Some(&actual) {
            return Err(refuse(format!(
                "member {}: its bytes hash to {actual}, not the file_sha256 it is listed with",
                shown(name)
            )));
        }
        self.members.insert(name.to_string(), bytes);
        Ok(())
    }

    fn finish(self) -> Result<Bundle> {
        let Some((manifest, manifest_sha256, located)) = self.manifest else {
            return Err(refuse(format!("no {MANIFEST}")));
        };
        if let Some(missing) = self.listed.keys().find(|n| !self.members.contains_key(*n)) {
            return Err(refuse(format!(
                "{MANIFEST} lists {} but the bundle has no such member",
                shown(missing)
            )));
        }
        Ok(Bundle {
            manifest,
            manifest_sha256,
            members: self.members,
            located,
        })
    }
}

/// What [`manifest`] makes of `invocation.json`: the manifest, each listed member with the SHA-256
/// its bytes must have, and each step's located images.
type Checked = (
    InvocationBundle,
    BTreeMap<String, String>,
    Vec<Vec<Located>>,
);

/// Parses and checks the manifest; returns it with each listed member and its expected SHA-256.
fn manifest(bytes: &[u8]) -> Result<Checked> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| refuse(format!("{MANIFEST} is not JSON: {e}")))?;
    let validator = schema()
        .as_ref()
        .map_err(|e| Error::Input(format!("bundle: internal: bundle schema: {e}")))?;
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .take(5)
        .map(|e| {
            let at = e.instance_path().to_string();
            format!(
                "at {} (schema {})",
                if at.is_empty() { "/" } else { at.as_str() },
                e.schema_path()
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(refuse(format!(
            "{MANIFEST} does not match the contract's invocation-bundle schema: {}",
            errors.join("; ")
        )));
    }
    let manifest: InvocationBundle = serde_json::from_value(value).map_err(|e| {
        refuse(format!(
            "{MANIFEST} does not read as a bundle manifest: {e}"
        ))
    })?;
    let mut listed = BTreeMap::new();
    let mut located = Vec::new();
    for (i, step) in manifest.steps.iter().enumerate() {
        if step.contract != CONTRACT_VERSION {
            return Err(refuse(format!(
                "steps[{i}] is contract {}; squint replays {CONTRACT_VERSION}",
                step.contract
            )));
        }
        if step.tool.name != "squint" {
            return Err(refuse(format!(
                "steps[{i}] is a call to {}; squint replays only its own steps",
                step.tool.name
            )));
        }
        if let Some(a) = step.artifacts.first() {
            return Err(refuse(format!(
                "steps[{i}] carries an artifact for port {}; squint takes no native inputs",
                a.port
            )));
        }
        if let Some(u) = step.unportable.first() {
            return Err(refuse(format!(
                "steps[{i}] lists an unportable artifact for port {}; squint takes no native inputs",
                u.port
            )));
        }
        if let Some(r) = step.reference_only.first() {
            let slot = r.slot.as_deref().or(r.port.as_deref()).unwrap_or("?");
            return Err(refuse(format!(
                "steps[{i}] left {slot} out (reference_only); squint cannot supply a left-out \
                 input on replay"
            )));
        }
        for (j, input) in step.inputs.iter().enumerate() {
            check_name(&input.member)?;
            let named = input
                .member
                .strip_prefix("inputs/")
                .and_then(|rest| rest.split_once('.'))
                .map(|(sha, _)| sha);
            if named != Some(input.image.file_sha256.as_str()) {
                return Err(refuse(format!(
                    "steps[{i}].inputs[{j}]: member {} does not name its image's file_sha256",
                    shown(&input.member)
                )));
            }
            listed.insert(input.member.clone(), input.image.file_sha256.clone());
        }
        located.push(locate(i, step)?);
    }
    Ok((manifest, listed, located))
}

/// The image arguments of a squint action, as its `params_schema` names them: `candidate` and
/// `source` each one image, `references` a list of them.
const IMAGE_SLOTS: [&str; 3] = ["candidate", "source", "references"];

/// Matches step `i`'s `inputs` to the image arguments of its `invocation` (4.9: "each image a step
/// took, by slot"): every image argument is `{"expect": <reference>}` with no locator (4.8) and
/// has exactly one input of its slot (in order, for `references`) whose reference is its
/// `expect`; every input is by `slot`, never `port` (squint is an images tool), and none is left
/// over. Returns each image argument's pointer with its member.
fn locate(i: usize, step: &BundleStep) -> Result<Vec<Located>> {
    let arguments = &step.invocation.arguments;
    // Each image argument: its slot, its pointer, how a refusal names it, and its value.
    let mut wanted: Vec<(&str, String, String, &Value)> = Vec::new();
    for slot in ["candidate", "source"] {
        if let Some(value) = arguments.get(slot) {
            wanted.push((slot, format!("/{slot}"), slot.to_string(), value));
        }
    }
    match arguments.get("references") {
        None => {}
        Some(Value::Array(references)) => {
            for (k, value) in references.iter().enumerate() {
                wanted.push((
                    "references",
                    format!("/references/{k}"),
                    format!("references[{k}]"),
                    value,
                ));
            }
        }
        Some(_) => {
            return Err(refuse(format!(
                "steps[{i}]: the invocation's references is not a list of images"
            )));
        }
    }
    let mut by_slot: BTreeMap<&str, VecDeque<usize>> = BTreeMap::new();
    for (j, input) in step.inputs.iter().enumerate() {
        let slot = match (&input.slot, &input.port) {
            (Some(slot), None) if IMAGE_SLOTS.contains(&slot.as_str()) => slot.as_str(),
            (Some(slot), None) => {
                return Err(refuse(format!(
                    "steps[{i}].inputs[{j}] is for slot {slot:?}, which no squint action takes"
                )));
            }
            _ => {
                return Err(refuse(format!(
                    "steps[{i}].inputs[{j}] names a port; squint takes images by slot"
                )));
            }
        };
        by_slot.entry(slot).or_default().push_back(j);
    }
    let mut located = Vec::new();
    for (slot, pointer, name, value) in wanted {
        let Value::Object(image) = value else {
            return Err(refuse(format!(
                "steps[{i}]: the invocation's {name} is not an image argument"
            )));
        };
        if let Some(locator) = ["path", "inline"]
            .into_iter()
            .find(|k| image.contains_key(*k))
        {
            return Err(refuse(format!(
                "steps[{i}]: the invocation's {name} carries a locator (`{locator}`); an \
                 invocation's images carry none"
            )));
        }
        let expect = image
            .get("expect")
            .and_then(|e| serde_json::from_value::<ImageReference>(e.clone()).ok())
            .ok_or_else(|| {
                refuse(format!(
                    "steps[{i}]: the invocation's {name} has no expect image reference"
                ))
            })?;
        let Some(j) = by_slot.get_mut(slot).and_then(VecDeque::pop_front) else {
            return Err(refuse(format!(
                "steps[{i}]: the invocation's {name} has no input in the bundle"
            )));
        };
        let input = &step.inputs[j];
        if input.image != expect {
            return Err(refuse(format!(
                "steps[{i}].inputs[{j}] is not the image the invocation's {name} expects"
            )));
        }
        located.push(Located {
            pointer,
            member: input.member.clone(),
        });
    }
    if let Some((slot, j)) = by_slot
        .iter()
        .find_map(|(slot, left)| left.front().map(|j| (slot, j)))
    {
        return Err(refuse(format!(
            "steps[{i}].inputs[{j}] ({slot}) is no image argument of the invocation"
        )));
    }
    Ok(located)
}

/// The contract's bundle schema, its references served from the crate's embedded schemas; built
/// once.
fn schema() -> &'static std::result::Result<jsonschema::Validator, String> {
    static VALIDATOR: OnceLock<std::result::Result<jsonschema::Validator, String>> =
        OnceLock::new();
    VALIDATOR.get_or_init(build_schema)
}

fn build_schema() -> std::result::Result<jsonschema::Validator, String> {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>
        {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("not an embedded table-manners schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(id, _)| *id == "urn:table-manners:core:invocation-bundle")
        .ok_or("not embedded")?;
    let schema: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .map_err(|e| e.to_string())
}

/// A header's name and size, once every canonical field has been checked.
struct Entry {
    name: String,
    size: u64,
}

/// Checks one header block against the canonical layout; `index` is the member's position.
fn parse_header(h: &[u8; BLOCK], index: usize) -> Result<Entry> {
    let stored = octal(&h[148..156]);
    let mut sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
    sum = sum - h[148..156].iter().map(|&b| u64::from(b)).sum::<u64>() + 8 * u64::from(b' ');
    if stored != Some(sum) {
        return Err(refuse(if index == 0 {
            "not a ustar archive (its first block is not a valid header)".to_string()
        } else {
            format!("header {} has a bad checksum (damaged)", index + 1)
        }));
    }
    if &h[257..263] != b"ustar\0" || &h[263..265] != b"00" {
        return Err(refuse(format!(
            "header {} is not a POSIX ustar header",
            index + 1
        )));
    }
    let name =
        text(&h[0..100]).map_err(|why| refuse(format!("header {}: its name {why}", index + 1)))?;
    let name = match std::str::from_utf8(name) {
        Ok(name) => name.to_string(),
        Err(_) => {
            return Err(refuse(format!(
                "member {}: a name that is not UTF-8",
                shown(&String::from_utf8_lossy(name))
            )));
        }
    };
    let non_canonical = |field: &str| {
        refuse(format!(
            "member {}: non-canonical header ({field})",
            shown(&name)
        ))
    };
    if h[345..500].iter().any(|&b| b != 0) {
        return Err(non_canonical("a name prefix"));
    }
    check_name(&name)?;
    match h[156] {
        b'0' => {}
        b'\0' => return Err(non_canonical("type flag NUL instead of '0'")),
        flag => {
            let what = match flag {
                b'1' => "a hard link",
                b'2' => "a symbolic link",
                b'3' => "a character device",
                b'4' => "a block device",
                b'5' => "a directory",
                b'6' => "a FIFO",
                b'7' => "a contiguous file",
                b'x' | b'g' => "a pax extended header",
                b'L' | b'K' => "a GNU long-name header",
                _ => "an unknown member type",
            };
            return Err(refuse(format!(
                "member {} is {what}, not a regular file",
                shown(&name)
            )));
        }
    }
    if octal(&h[100..108]) != Some(MODE) {
        return Err(non_canonical("mode is not 0644"));
    }
    if octal(&h[108..116]) != Some(0) {
        return Err(non_canonical("uid is not 0"));
    }
    if octal(&h[116..124]) != Some(0) {
        return Err(non_canonical("gid is not 0"));
    }
    if octal(&h[136..148]) != Some(0) {
        return Err(non_canonical("mtime is not 0"));
    }
    if h[157..257].iter().any(|&b| b != 0) {
        return Err(non_canonical("a link name"));
    }
    if h[265..297].iter().any(|&b| b != 0) {
        return Err(non_canonical("a user name"));
    }
    if h[297..329].iter().any(|&b| b != 0) {
        return Err(non_canonical("a group name"));
    }
    for (field, range) in [("device major", 329..337), ("device minor", 337..345)] {
        let bytes = &h[range];
        if bytes.iter().any(|&b| b != 0) && octal(bytes) != Some(0) {
            return Err(non_canonical(field));
        }
    }
    if h[500..512].iter().any(|&b| b != 0) {
        return Err(non_canonical("bytes after the header's fields"));
    }
    let size = octal(&h[124..136]).ok_or_else(|| non_canonical("size is not octal"))?;
    Ok(Entry { name, size })
}

/// A NUL-terminated header string; every byte after the terminator must be NUL too.
fn text(field: &[u8]) -> std::result::Result<&[u8], &'static str> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    if field[end..].iter().any(|&b| b != 0) {
        return Err("has bytes after its terminator");
    }
    Ok(&field[..end])
}

/// An octal number field: optional leading spaces, at least one octal digit, then only NUL or
/// space. A base-256 field (the high bit set) is not canonical and reads as `None`.
fn octal(field: &[u8]) -> Option<u64> {
    let rest = field.iter().skip_while(|&&b| b == b' ');
    let mut value: u64 = 0;
    let mut digits = 0;
    let mut ended = false;
    for &b in rest {
        match b {
            b'0'..=b'7' if !ended => {
                value = value.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
                digits += 1;
            }
            b'\0' | b' ' => ended = true,
            _ => return None,
        }
    }
    (digits > 0).then_some(value)
}

/// Checks a member name: plain, relative, and one of the three shapes 4.9 names.
fn check_name(name: &str) -> Result<()> {
    let bad = |why: &str| refuse(format!("member {}: {why}", shown(name)));
    if name.is_empty() {
        return Err(bad("an empty name"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(bad(&format!("a name longer than {MAX_NAME_BYTES} bytes")));
    }
    if name.starts_with('/') || name.starts_with('\\') || name.get(1..2) == Some(":") {
        return Err(bad("an absolute name"));
    }
    if name.split(['/', '\\']).any(|c| c == "..") {
        return Err(bad("a name that climbs out of the bundle (`..`)"));
    }
    let lower_hex =
        |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let ext = |s: &str| !s.is_empty() && s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9'));
    let ok = name == MANIFEST
        || name
            .strip_prefix("inputs/")
            .and_then(|rest| rest.split_once('.'))
            .is_some_and(|(sha, e)| lower_hex(sha) && ext(e))
        || name
            .strip_prefix("artifacts/sha256-")
            .and_then(|rest| rest.split_once('.'))
            .is_some_and(|(sha, exts)| lower_hex(sha) && exts.split('.').all(ext));
    if !ok {
        return Err(bad(&format!(
            "not {MANIFEST}, inputs/<file_sha256>.<ext> or artifacts/<identity>.<ext>"
        )));
    }
    Ok(())
}

/// A member name as an error shows it: quoted, control characters escaped, at most 100 characters.
fn shown(name: &str) -> String {
    let cut: String = name.chars().take(MAX_NAME_BYTES).collect();
    let more = if cut.len() < name.len() { "…" } else { "" };
    format!("`{}{more}`", cut.escape_debug())
}

fn read_block(reader: &mut impl Read, block: &mut [u8; BLOCK]) -> Result<()> {
    match reader.read_exact(block) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Err(refuse(
            "truncated: it ends without the two zero blocks that close an archive",
        )),
        Err(e) => Err(refuse(format!("cannot read it: {e}"))),
    }
}

fn refuse(why: impl std::fmt::Display) -> Error {
    Error::Input(format!("bundle: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_fields_read_as_tar_writes_them() {
        assert_eq!(octal(b"0000644\0"), Some(0o644));
        assert_eq!(octal(b"   644 \0"), Some(0o644));
        assert_eq!(octal(b"00000000012\0"), Some(10));
        assert_eq!(octal(b"\0\0\0\0\0\0\0\0"), None);
        assert_eq!(octal(b"0000\x80644"), None);
        assert_eq!(octal(b"00 0644\0"), None);
        assert_eq!(octal(b"0000648\0"), None);
    }

    #[test]
    fn names_take_only_the_three_shapes() {
        let sha = "a".repeat(64);
        for good in [
            MANIFEST.to_string(),
            format!("inputs/{sha}.png"),
            format!("artifacts/sha256-{sha}.mask.tar"),
        ] {
            assert!(check_name(&good).is_ok(), "{good}");
        }
        for bad in [
            String::new(),
            "/etc/x".into(),
            "C:/x".into(),
            "../x".into(),
            "inputs/../invocation.json".into(),
            format!("inputs/{sha}"),
            format!("inputs/{}.png", "A".repeat(64)),
            format!("inputs/{sha}.PNG"),
            format!("inputs/sub/{sha}.png"),
            format!("artifacts/{sha}.tar"),
            "other.json".into(),
            "x".repeat(MAX_NAME_BYTES + 1),
        ] {
            assert!(check_name(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn members_together_stay_within_the_total() {
        let sha = |i: usize| format!("inputs/{i:064x}.png");
        let mut c = Collector::default();
        for i in 0..5 {
            c.listed.insert(sha(i), String::new());
        }
        c.admit(MANIFEST, MAX_MANIFEST_BYTES).unwrap();
        for i in 0..3 {
            c.admit(&sha(i), MAX_MEMBER_BYTES).unwrap();
        }
        let e = c.admit(&sha(3), MAX_MEMBER_BYTES).unwrap_err().to_string();
        assert!(e.contains("exceed"), "{e}");
    }

    #[test]
    fn shown_names_are_escaped_and_bounded() {
        assert_eq!(shown("a\nb"), "`a\\nb`");
        assert!(shown(&"x".repeat(500)).ends_with("…`"));
    }
}

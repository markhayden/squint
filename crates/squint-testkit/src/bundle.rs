//! A test-only writer of invocation bundles (table-manners 4.9), so tests and fixtures have bundles
//! without squint shipping a builder (no tool builds one). It is written apart from the reader
//! (`squint_core::bundle`) so a round trip checks one against the other.
//!
//! [`canonical`] lays a bundle out as the contract fixes it: an uncompressed POSIX ustar,
//! `invocation.json` first and the rest sorted by name, every header a regular file with mode
//! 0644, uid and gid 0, mtime 0 and no user or group name, numeric fields as zero-padded octal
//! ended by a NUL (as the `tar` crate writes them, so a host using it writes the same bytes), and
//! the two closing zero blocks with nothing after. [`archive`] writes [`Member`]s exactly as given,
//! in the given order, so a test can build any non-canonical archive the reader must refuse.

use std::path::Path;

use serde_json::{Value, json};

const BLOCK: usize = 512;

/// One archive member as the writer lays it out. [`Member::file`] is canonical; every field is
/// open so a test can make it otherwise.
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub bytes: Vec<u8>,
    pub mode: u64,
    pub uid: u64,
    pub gid: u64,
    pub mtime: u64,
    /// The ustar type flag: `b'0'` a regular file, `b'2'` a symbolic link, `b'3'` a device…
    pub typeflag: u8,
    pub linkname: String,
    pub uname: String,
    pub gname: String,
    /// The size the header declares, when it should differ from `bytes.len()`.
    pub declared_size: Option<u64>,
}

impl Member {
    /// A canonical regular file.
    pub fn file(name: &str, bytes: &[u8]) -> Member {
        Member {
            name: name.to_string(),
            bytes: bytes.to_vec(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 0,
            typeflag: b'0',
            linkname: String::new(),
            uname: String::new(),
            gname: String::new(),
            declared_size: None,
        }
    }

    /// This member's header block.
    pub fn header(&self) -> [u8; BLOCK] {
        let mut h = [0u8; BLOCK];
        put_text(&mut h[0..100], &self.name);
        put_octal(&mut h[100..108], self.mode);
        put_octal(&mut h[108..116], self.uid);
        put_octal(&mut h[116..124], self.gid);
        put_octal(
            &mut h[124..136],
            self.declared_size.unwrap_or(self.bytes.len() as u64),
        );
        put_octal(&mut h[136..148], self.mtime);
        h[156] = self.typeflag;
        put_text(&mut h[157..257], &self.linkname);
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        put_text(&mut h[265..297], &self.uname);
        put_text(&mut h[297..329], &self.gname);
        seal(&mut h);
        h
    }
}

/// Recomputes a header block's checksum, as after a test edits a field in place.
pub fn seal(h: &mut [u8; BLOCK]) {
    h[148..156].fill(b' ');
    let sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
    put_octal(&mut h[148..156], sum);
}

/// The members exactly as given, each header followed by its bytes and zero padding, then the two
/// closing zero blocks.
pub fn archive(members: &[Member]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in members {
        out.extend_from_slice(&m.header());
        out.extend_from_slice(&m.bytes);
        out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
    }
    out.resize(out.len() + 2 * BLOCK, 0);
    out
}

/// A canonical bundle: `invocation.json` first, then `others` sorted by name.
pub fn canonical(manifest: &[u8], others: &[(String, Vec<u8>)]) -> Vec<u8> {
    archive(&canonical_members(manifest, others))
}

/// The members of a canonical bundle in their canonical order, for a test to alter one.
pub fn canonical_members(manifest: &[u8], others: &[(String, Vec<u8>)]) -> Vec<Member> {
    let mut sorted: Vec<&(String, Vec<u8>)> = others.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    sorted.dedup_by(|a, b| a.0 == b.0);
    std::iter::once(Member::file("invocation.json", manifest))
        .chain(sorted.into_iter().map(|(n, b)| Member::file(n, b)))
        .collect()
}

/// The unpacked form: the same members as files under `dir`.
pub fn write_dir(dir: &Path, manifest: &[u8], others: &[(String, Vec<u8>)]) {
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
        }
        std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    };
    write("invocation.json", manifest);
    for (name, bytes) in others {
        write(name, bytes);
    }
}

/// A manifest of the given steps, as bytes.
pub fn manifest(steps: Vec<Value>) -> Vec<u8> {
    let doc = json!({ "bundle_version": 1, "steps": steps });
    serde_json::to_vec_pretty(&doc).unwrap_or_else(|e| panic!("manifest: {e}"))
}

/// A step from a squint `run.json` and the image files the call read: each image argument of the
/// run's invocation (`candidate`, `source`, each of `references`) becomes an input by its slot,
/// its member `inputs/<file_sha256>.<ext>` the file whose digest its `expect` names. Returns the
/// step and its members. Panics when a file is missing for an argument.
pub fn step_from_run(result: &Value, files: &[&[u8]]) -> (Value, Vec<(String, Vec<u8>)>) {
    use sha2::{Digest, Sha256};
    let by_sha: Vec<(String, &[u8])> = files
        .iter()
        .map(|f| {
            let digest = Sha256::digest(f);
            let sha: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            (sha, *f)
        })
        .collect();
    let invocation = &result["invocation"];
    let mut images: Vec<(&str, &Value)> = Vec::new();
    for slot in ["candidate", "source"] {
        if let Some(expect) = invocation["arguments"][slot].get("expect") {
            images.push((slot, expect));
        }
    }
    if let Some(refs) = invocation["arguments"]["references"].as_array() {
        for r in refs {
            if let Some(expect) = r.get("expect") {
                images.push(("references", expect));
            }
        }
    }
    let mut inputs = Vec::new();
    let mut members = Vec::new();
    for (slot, expect) in images {
        let sha = expect["file_sha256"].as_str().unwrap_or_default();
        let Some((_, bytes)) = by_sha.iter().find(|(s, _)| s == sha) else {
            panic!("no file given for {slot} ({sha})");
        };
        let ext = image::guess_format(bytes)
            .ok()
            .and_then(|f| f.extensions_str().first().copied())
            .unwrap_or("bin");
        let member = format!("inputs/{sha}.{ext}");
        inputs.push(json!({ "slot": slot, "image": expect, "member": member }));
        members.push((member, bytes.to_vec()));
    }
    let step = json!({
        "contract": result["contract"],
        "tool": result["tool"],
        "invocation": invocation,
        "result": result,
        "inputs": inputs,
        "artifacts": [],
        "reference_only": [],
        "unportable": [],
    });
    (step, members)
}

fn put_text(field: &mut [u8], text: &str) {
    assert!(
        text.len() <= field.len(),
        "`{text}` does not fit a {}-byte header field",
        field.len()
    );
    field[..text.len()].copy_from_slice(text.as_bytes());
}

/// Zero-padded octal ended by a NUL, filling the field.
fn put_octal(field: &mut [u8], value: u64) {
    let digits = format!("{value:0width$o}", width = field.len() - 1);
    assert!(
        digits.len() < field.len(),
        "{value} does not fit a {}-byte octal field",
        field.len()
    );
    field[..digits.len()].copy_from_slice(digits.as_bytes());
    field[digits.len()] = 0;
}

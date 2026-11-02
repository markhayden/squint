//! Reading an invocation bundle (table-manners 4.9, task 10.5a): the testkit writer's bundles read
//! back as archives and directories, and every fault is refused with a precise error, read no
//! further than the header that shows it.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use squint_core::bundle::{Bundle, MAX_MANIFEST_BYTES, MAX_MEMBER_BYTES, MAX_MEMBERS};
use squint_testkit::bundle::{self as writer, Member};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-bundle-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn png(seed: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(4, 3, |x, y| image::Rgb([seed, x as u8, y as u8]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// The completed-run fixture, its candidate and source pointed at two small PNGs.
fn run_with(candidate: &[u8], source: &[u8]) -> Value {
    let text = std::fs::read_to_string(root().join("fixtures/contract/squint/run-completed.json"))
        .unwrap();
    let mut run: Value = serde_json::from_str(&text).unwrap();
    run["invocation"]["arguments"]["candidate"]["expect"]["file_sha256"] = json!(sha(candidate));
    run["invocation"]["arguments"]["source"]["expect"]["file_sha256"] = json!(sha(source));
    run
}

/// A one-step squint bundle: its manifest and its two input members.
fn sample() -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let (candidate, source) = (png(1), png(2));
    let run = run_with(&candidate, &source);
    let (step, members) = writer::step_from_run(&run, &[&candidate, &source]);
    (writer::manifest(vec![step]), members)
}

/// A manifest edited before writing, with the sample's members.
fn sample_with(edit: impl FnOnce(&mut Value)) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let (manifest, members) = sample();
    let mut doc: Value = serde_json::from_slice(&manifest).unwrap();
    edit(&mut doc);
    (serde_json::to_vec_pretty(&doc).unwrap(), members)
}

/// A reader that counts what it hands out, to show where reading stopped.
struct Counting<R> {
    inner: R,
    read: usize,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n;
        Ok(n)
    }
}

/// Reads an archive, returning the refusal and how many bytes were read before it.
fn refused(archive: &[u8]) -> (String, usize) {
    let mut reader = Counting {
        inner: Cursor::new(archive),
        read: 0,
    };
    let err = match Bundle::from_archive(&mut reader) {
        Ok(_) => panic!("accepted a bundle that should be refused"),
        Err(e) => e,
    };
    assert_eq!(err.class(), "input", "{err}");
    (err.to_string(), reader.read)
}

fn refused_dir(dir: &Path) -> String {
    let err = match Bundle::read(dir) {
        Ok(_) => panic!("accepted a directory that should be refused"),
        Err(e) => e,
    };
    assert_eq!(err.class(), "input", "{err}");
    err.to_string()
}

/// The byte offset of each member's header in a canonical archive of these members.
fn header_offsets(members: &[Member]) -> Vec<usize> {
    let mut at = 0;
    members
        .iter()
        .map(|m| {
            let here = at;
            at += 512 + m.bytes.len().div_ceil(512) * 512;
            here
        })
        .collect()
}

fn input_name(members: &[(String, Vec<u8>)], i: usize) -> String {
    let mut names: Vec<&String> = members.iter().map(|(n, _)| n).collect();
    names.sort();
    names[i].clone()
}

#[test]
fn a_written_bundle_reads_back_as_an_archive_and_as_a_directory() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    assert_eq!(
        archive,
        writer::canonical(&manifest, &members),
        "two writes differ"
    );
    assert_eq!(archive.len() % 512, 0);

    let dir = tempdir("roundtrip");
    let file = dir.join("bundle.tar");
    std::fs::write(&file, &archive).unwrap();
    let unpacked = dir.join("unpacked");
    writer::write_dir(&unpacked, &manifest, &members);

    let expected: table_manners::InvocationBundle = serde_json::from_slice(&manifest).unwrap();
    for bundle in [
        Bundle::from_archive(archive.as_slice()).unwrap(),
        Bundle::read(&file).unwrap(),
        Bundle::read(&unpacked).unwrap(),
    ] {
        assert_eq!(bundle.manifest, expected);
        assert_eq!(bundle.manifest_sha256, sha(&manifest));
        let mut want: Vec<(&str, &[u8])> = members
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        want.sort();
        assert_eq!(bundle.members().collect::<Vec<_>>(), want);
        for (name, bytes) in &members {
            assert_eq!(bundle.member(name), Some(bytes.as_slice()));
            assert_eq!(*name, format!("inputs/{}.png", sha(bytes)));
        }
    }
}

#[test]
fn the_writer_lays_headers_out_canonically() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    let h = &archive[..512];
    assert_eq!(&h[..16], b"invocation.json\0");
    assert_eq!(&h[100..108], b"0000644\0");
    assert_eq!(&h[108..116], b"0000000\0");
    assert_eq!(&h[116..124], b"0000000\0");
    assert_eq!(&h[136..148], b"00000000000\0");
    assert_eq!(h[156], b'0');
    assert_eq!(&h[257..265], b"ustar\x0000");
    assert!(h[265..329].iter().all(|&b| b == 0), "user and group names");
    assert!(archive[archive.len() - 1024..].iter().all(|&b| b == 0));
    // The order of `others` given to the writer does not matter.
    let reversed: Vec<_> = members.iter().rev().cloned().collect();
    assert_eq!(archive, writer::canonical(&manifest, &reversed));
}

#[test]
fn any_flipped_byte_outside_the_manifest_is_refused() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    let manifest_data = 512..512 + manifest.len();
    for at in 0..archive.len() {
        if manifest_data.contains(&at) {
            continue;
        }
        let mut bad = archive.clone();
        bad[at] ^= 0x01;
        let mut reader = Counting {
            inner: Cursor::new(bad.as_slice()),
            read: 0,
        };
        assert!(
            Bundle::from_archive(&mut reader).is_err(),
            "a flip at byte {at} was accepted"
        );
    }
}

#[test]
fn a_flipped_byte_in_an_input_names_the_member_and_its_digest() {
    let (manifest, members) = sample();
    let ms = writer::canonical_members(&manifest, &members);
    let offsets = header_offsets(&ms);
    for i in 1..ms.len() {
        for within in [0, ms[i].bytes.len() / 2, ms[i].bytes.len() - 1] {
            let mut bad = writer::canonical(&manifest, &members);
            bad[offsets[i] + 512 + within] ^= 0x80;
            let (e, _) = refused(&bad);
            assert!(e.contains(&ms[i].name), "{e}");
            assert!(e.contains("not the file_sha256 it is listed with"), "{e}");
        }
    }
}

/// The manifest is not self-verifying (the archive's digest names it, 4.9); a flip in it is either
/// refused or reads as a different manifest, never as the same one.
#[test]
fn a_flipped_byte_in_the_manifest_never_reads_as_the_same_bundle() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    let original = sha(&manifest);
    for at in (512..512 + manifest.len()).step_by(7) {
        let mut bad = archive.clone();
        bad[at] ^= 0x01;
        if let Ok(b) = Bundle::from_archive(bad.as_slice()) {
            assert_ne!(b.manifest_sha256, original, "byte {at}");
        }
    }
}

#[test]
fn a_missing_extra_or_renamed_member_is_refused() {
    let (manifest, members) = sample();
    let first = input_name(&members, 0);

    let fewer: Vec<_> = members
        .iter()
        .filter(|(n, _)| *n != first)
        .cloned()
        .collect();
    let (e, _) = refused(&writer::canonical(&manifest, &fewer));
    assert!(
        e.contains(&format!(
            "lists `{first}` but the bundle has no such member"
        )),
        "{e}"
    );

    let extra_name = format!("inputs/{}.png", "0".repeat(64));
    let mut more = members.clone();
    more.push((extra_name.clone(), b"extra".to_vec()));
    let ms = writer::canonical_members(&manifest, &more);
    let at = ms.iter().position(|m| m.name == extra_name).unwrap();
    let (e, read) = refused(&writer::archive(&ms));
    assert!(
        e.contains(&format!("`{extra_name}` is not listed in invocation.json")),
        "{e}"
    );
    assert_eq!(
        read,
        header_offsets(&ms)[at] + 512,
        "read past the unlisted header"
    );

    for renamed in [
        first.replace(".png", ".jpg"),
        format!("inputs/{}.png", "f".repeat(64)),
    ] {
        let swapped: Vec<_> = members
            .iter()
            .map(|(n, b)| {
                let n = if *n == first {
                    renamed.clone()
                } else {
                    n.clone()
                };
                (n, b.clone())
            })
            .collect();
        let (e, _) = refused(&writer::canonical(&manifest, &swapped));
        assert!(e.contains(&format!("`{renamed}` is not listed")), "{e}");
    }

    // A member the manifest lists but no member carries the listed digest.
    let wrong = sample_with(|m| {
        m["steps"][0]["inputs"][0]["image"]["file_sha256"] = json!("e".repeat(64));
    });
    let (e, _) = refused(&writer::canonical(&wrong.0, &wrong.1));
    assert!(e.contains("does not name its image's file_sha256"), "{e}");
}

#[test]
fn an_escaping_or_absolute_name_is_refused_at_its_header() {
    let (manifest, members) = sample();
    for name in [
        "../x",
        "/etc/x",
        "inputs/../../x",
        "inputs/../invocation.json",
    ] {
        let mut ms = writer::canonical_members(&manifest, &members);
        ms.insert(1, Member::file(name, b"never read"));
        let (e, read) = refused(&writer::archive(&ms));
        assert!(e.contains(&format!("member `{name}`")), "{e}");
        assert!(
            e.contains("absolute") || e.contains("climbs out"),
            "{name}: {e}"
        );
        assert_eq!(
            read,
            header_offsets(&ms)[1] + 512,
            "{name}: read past the header"
        );

        // As the very first member too, before any manifest.
        let first = writer::archive(&[Member::file(name, b"x")]);
        let (e, read) = refused(&first);
        assert!(e.contains(&format!("member `{name}`")), "{e}");
        assert_eq!(read, 512);
    }
    let mut ms = writer::canonical_members(&manifest, &members);
    ms.insert(1, Member::file("notes.txt", b"x"));
    let (e, _) = refused(&writer::archive(&ms));
    assert!(
        e.contains("not invocation.json, inputs/<file_sha256>.<ext>"),
        "{e}"
    );
}

#[test]
fn a_member_that_is_not_a_regular_file_is_refused_at_its_header() {
    let (manifest, members) = sample();
    let name = input_name(&members, 0);
    for (flag, what) in [
        (b'1', "a hard link"),
        (b'2', "a symbolic link"),
        (b'3', "a character device"),
        (b'4', "a block device"),
        (b'5', "a directory"),
        (b'6', "a FIFO"),
        (b'x', "a pax extended header"),
        (b'L', "a GNU long-name header"),
    ] {
        let mut ms = writer::canonical_members(&manifest, &members);
        let i = ms.iter().position(|m| m.name == name).unwrap();
        ms[i].typeflag = flag;
        ms[i].bytes.clear();
        let (e, read) = refused(&writer::archive(&ms));
        assert!(
            e.contains(&format!("`{name}` is {what}, not a regular file")),
            "{e}"
        );
        assert_eq!(read, header_offsets(&ms)[i] + 512);
    }
}

#[test]
fn a_truncated_archive_is_refused() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    let ms = writer::canonical_members(&manifest, &members);
    let second = header_offsets(&ms)[1];
    for cut in [
        0,
        100,
        511,
        512 + 10,
        second - 1,
        second + 512 + 5,
        archive.len() - 1024,
        archive.len() - 600,
        archive.len() - 1,
    ] {
        let (e, _) = refused(&archive[..cut]);
        assert!(e.contains("truncated"), "cut at {cut}: {e}");
    }
}

#[test]
fn bytes_after_the_end_are_refused() {
    let (manifest, members) = sample();
    let archive = writer::canonical(&manifest, &members);
    for tail in [vec![0u8], vec![b'x'], vec![0u8; 512], vec![0u8; 8192]] {
        let mut bad = archive.clone();
        bad.extend_from_slice(&tail);
        let (e, read) = refused(&bad);
        assert!(e.contains("bytes after the two zero blocks"), "{e}");
        assert_eq!(read, archive.len() + 1, "read the whole tail");
    }
}

#[test]
fn an_oversized_member_or_manifest_is_refused_before_its_bytes() {
    let (manifest, members) = sample();
    let mut big = Member::file("invocation.json", b"");
    big.declared_size = Some(MAX_MANIFEST_BYTES + 1);
    let (e, read) = refused(&writer::archive(&[big]));
    assert!(
        e.contains("`invocation.json` is 8388609 bytes, over the 8388608"),
        "{e}"
    );
    assert_eq!(read, 512);

    let name = input_name(&members, 0);
    let mut ms = writer::canonical_members(&manifest, &members);
    let i = ms.iter().position(|m| m.name == name).unwrap();
    ms[i].declared_size = Some(MAX_MEMBER_BYTES + 1);
    ms[i].bytes.clear();
    let (e, read) = refused(&writer::archive(&ms));
    assert!(
        e.contains(&format!("`{name}` is 67108865 bytes, over the 67108864")),
        "{e}"
    );
    assert_eq!(read, header_offsets(&ms)[i] + 512);
}

#[test]
fn more_members_than_the_bound_are_refused() {
    let files: Vec<Vec<u8>> = (0..MAX_MEMBERS)
        .map(|i| format!("file {i}").into_bytes())
        .collect();
    let (manifest, _) = sample_with(|m| {
        let inputs: Vec<Value> = files
            .iter()
            .map(|f| {
                let mut image = m["steps"][0]["inputs"][0]["image"].clone();
                image["file_sha256"] = json!(sha(f));
                json!({ "slot": "references", "image": image,
                        "member": format!("inputs/{}.bin", sha(f)) })
            })
            .collect();
        let references: Vec<Value> = inputs
            .iter()
            .map(|i| json!({ "expect": i["image"] }))
            .collect();
        m["steps"][0]["invocation"]["arguments"] = json!({ "references": references });
        m["steps"][0]["inputs"] = json!(inputs);
    });
    let others: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| (format!("inputs/{}.bin", sha(f)), f.clone()))
        .collect();
    let (e, _) = refused(&writer::canonical(&manifest, &others));
    assert!(
        e.contains(&format!("more than {MAX_MEMBERS} members")),
        "{e}"
    );
    // One fewer is within the bound.
    let (manifest, _) = sample_with(|m| {
        let inputs: Vec<Value> = files[1..]
            .iter()
            .map(|f| {
                let mut image = m["steps"][0]["inputs"][0]["image"].clone();
                image["file_sha256"] = json!(sha(f));
                json!({ "slot": "references", "image": image,
                        "member": format!("inputs/{}.bin", sha(f)) })
            })
            .collect();
        let references: Vec<Value> = inputs
            .iter()
            .map(|i| json!({ "expect": i["image"] }))
            .collect();
        m["steps"][0]["invocation"]["arguments"] = json!({ "references": references });
        m["steps"][0]["inputs"] = json!(inputs);
    });
    Bundle::from_archive(writer::canonical(&manifest, &others[1..]).as_slice()).unwrap();
}

#[test]
fn a_non_canonical_header_is_refused_naming_the_field() {
    let (manifest, members) = sample();
    let name = input_name(&members, 0);
    type Edit = fn(&mut Member);
    let cases: [(Edit, &str); 8] = [
        (|m| m.mode = 0o755, "mode is not 0644"),
        (|m| m.uid = 501, "uid is not 0"),
        (|m| m.gid = 20, "gid is not 0"),
        (|m| m.mtime = 1_700_000_000, "mtime is not 0"),
        (|m| m.uname = "root".into(), "a user name"),
        (|m| m.gname = "wheel".into(), "a group name"),
        (|m| m.linkname = "x".into(), "a link name"),
        (|m| m.typeflag = 0, "type flag NUL"),
    ];
    for (edit, field) in cases {
        let mut ms = writer::canonical_members(&manifest, &members);
        let i = ms.iter().position(|m| m.name == name).unwrap();
        edit(&mut ms[i]);
        let (e, read) = refused(&writer::archive(&ms));
        assert!(
            e.contains(&format!("member `{name}`: non-canonical header ({field}")),
            "{e}"
        );
        assert_eq!(read, header_offsets(&ms)[i] + 512);
    }

    // Edits a checksum cannot catch: each header re-sealed after the change.
    let ms = writer::canonical_members(&manifest, &members);
    let at = header_offsets(&ms)[1];
    let resealed = |edit: &dyn Fn(&mut [u8; 512])| {
        let mut archive = writer::canonical(&manifest, &members);
        let mut h: [u8; 512] = archive[at..at + 512].try_into().unwrap();
        edit(&mut h);
        writer::seal(&mut h);
        archive[at..at + 512].copy_from_slice(&h);
        refused(&archive).0
    };
    let e = resealed(&|h| h[257..265].copy_from_slice(b"ustar  \0"));
    assert!(e.contains("not a POSIX ustar header"), "{e}");
    let e = resealed(&|h| h[345] = b'a');
    assert!(e.contains("non-canonical header (a name prefix)"), "{e}");
    let e = resealed(&|h| h[329..337].copy_from_slice(b"0000001\0"));
    assert!(e.contains("non-canonical header (device major)"), "{e}");
    let e = resealed(&|h| h[505] = 1);
    assert!(e.contains("bytes after the header's fields"), "{e}");
    let e = resealed(&|h| h[124] = 0x80);
    assert!(e.contains("size is not octal"), "{e}");
    let e = resealed(&|h| h[99] = b'z');
    assert!(e.contains("its name has bytes after its terminator"), "{e}");
}

#[test]
fn members_out_of_order_are_refused() {
    let (manifest, members) = sample();
    let ms = writer::canonical_members(&manifest, &members);

    let mut late = ms.clone();
    late.rotate_left(1);
    let (e, read) = refused(&writer::archive(&late));
    assert!(e.contains("the first member is `inputs/"), "{e}");
    assert!(e.contains("not invocation.json"), "{e}");
    assert_eq!(read, 512);

    let mut swapped = ms.clone();
    swapped.swap(1, 2);
    let (e, _) = refused(&writer::archive(&swapped));
    assert!(
        e.contains("members after invocation.json are sorted by name"),
        "{e}"
    );

    let mut twice = ms.clone();
    twice.insert(2, ms[1].clone());
    let (e, _) = refused(&writer::archive(&twice));
    assert!(
        e.contains(&format!("member `{}` appears twice", ms[1].name)),
        "{e}"
    );

    let mut again = ms.clone();
    again.push(ms[0].clone());
    let (e, _) = refused(&writer::archive(&again));
    assert!(e.contains("invocation.json appears again"), "{e}");
}

#[test]
fn a_manifest_that_fails_the_schema_is_refused_before_any_member() {
    type Edit = fn(&mut Value);
    let cases: [(Edit, &str); 5] = [
        (|m| m["bundle_version"] = json!(2), "at /bundle_version"),
        (|m| m["extra"] = json!(true), "at /"),
        (|m| m["steps"] = json!([]), "at /steps"),
        (
            |m| m["steps"][0]["inputs"][0]["member"] = json!("inputs/x.png"),
            "at /steps/0/inputs/0/member",
        ),
        (
            |m| {
                m["steps"][0]["inputs"][0]["port"] = json!("image");
            },
            "at /steps/0/inputs/0",
        ),
    ];
    for (edit, at) in cases {
        let (manifest, members) = sample_with(edit);
        let (e, read) = refused(&writer::canonical(&manifest, &members));
        assert!(
            e.contains("does not match the contract's invocation-bundle schema"),
            "{e}"
        );
        assert!(e.contains(at), "{at}: {e}");
        assert_eq!(
            read,
            512 + manifest.len().div_ceil(512) * 512,
            "read past the manifest"
        );
    }
    let (e, _) = refused(&writer::canonical(b"{ not json", &[]));
    assert!(e.contains("invocation.json is not JSON"), "{e}");
}

#[test]
fn a_step_squint_does_not_replay_is_refused_naming_it() {
    let artifact = json!({
        "type": "image", "schema_version": 1, "tool": "rascal",
        "identity": format!("sha256:{}", "a".repeat(64)), "frame": null,
        "resource": format!("rascal://artifact/sha256:{}", "a".repeat(64)),
    });
    let a = artifact.clone();
    type Edit = Box<dyn FnOnce(&mut Value)>;
    let cases: Vec<(Edit, &str)> = vec![
        (
            Box::new(|m| m["steps"][0]["contract"] = json!("0.3.0")),
            "steps[0] is contract 0.3.0; squint replays 0.4.0",
        ),
        (
            Box::new(|m| m["steps"][0]["tool"]["name"] = json!("rascal")),
            "steps[0] is a call to rascal; squint replays only its own steps",
        ),
        (
            Box::new(move |m| {
                m["steps"][0]["artifacts"] = json!([{ "port": "mask", "artifact": a,
                    "member": format!("artifacts/sha256-{}.mask.tar", "a".repeat(64)),
                    "sha256": "a".repeat(64) }]);
            }),
            "steps[0] carries an artifact for port mask",
        ),
        (
            Box::new(move |m| {
                m["steps"][0]["unportable"] =
                    json!([{ "port": "plan", "artifact": artifact, "reason": "portable: none" }]);
            }),
            "steps[0] lists an unportable artifact for port plan",
        ),
        (
            Box::new(|m| {
                m["steps"][0]["reference_only"] =
                    json!([{ "slot": "references", "why": "left out" }]);
            }),
            "steps[0] left references out (reference_only)",
        ),
    ];
    for (edit, want) in cases {
        let (manifest, members) = sample_with(edit);
        let (e, read) = refused(&writer::canonical(&manifest, &members));
        assert!(e.contains(want), "{e}");
        assert_eq!(read, 512 + manifest.len().div_ceil(512) * 512);
    }
    // A later step is named by its index.
    let (manifest, members) = sample_with(|m| {
        let mut second = m["steps"][0].clone();
        second["tool"]["name"] = json!("rascal");
        m["steps"].as_array_mut().unwrap().push(second);
    });
    let (e, _) = refused(&writer::canonical(&manifest, &members));
    assert!(e.contains("steps[1] is a call to rascal"), "{e}");
}

/// Each step's `inputs` are exactly the image arguments of its `invocation` (4.9), by slot and,
/// for `references`, in order; an input that is not, an argument with no input, an input no
/// argument takes and an invocation image with a locator are each refused from the manifest
/// alone, naming the step and the argument.
#[test]
fn inputs_that_are_not_the_invocations_images_are_refused() {
    type Edit = Box<dyn FnOnce(&mut Value)>;
    let cases: Vec<(Edit, &str)> = vec![
        (
            Box::new(|m| m["steps"][0]["inputs"][0]["image"]["width"] = json!(5)),
            "steps[0].inputs[0] is not the image the invocation's candidate expects",
        ),
        (
            Box::new(|m| {
                let source = m["steps"][0]["inputs"][1].clone();
                m["steps"][0]["inputs"][1] = m["steps"][0]["inputs"][0].clone();
                m["steps"][0]["inputs"][1]["slot"] = json!("source");
                m["steps"][0]["inputs"][0] = source;
                m["steps"][0]["inputs"][0]["slot"] = json!("candidate");
            }),
            "steps[0].inputs[0] is not the image the invocation's candidate expects",
        ),
        (
            Box::new(|m| {
                m["steps"][0]["invocation"]["arguments"]
                    .as_object_mut()
                    .unwrap()
                    .remove("source");
            }),
            "steps[0].inputs[1] (source) is no image argument of the invocation",
        ),
        (
            Box::new(|m| {
                let source = m["steps"][0]["invocation"]["arguments"]["source"].clone();
                m["steps"][0]["invocation"]["arguments"]["references"] = json!([source]);
            }),
            "steps[0]: the invocation's references[0] has no input in the bundle",
        ),
        (
            Box::new(|m| {
                m["steps"][0]["invocation"]["arguments"]["candidate"]["path"] = json!("/x.png");
            }),
            "steps[0]: the invocation's candidate carries a locator (`path`)",
        ),
        (
            Box::new(|m| {
                m["steps"][0]["invocation"]["arguments"]["source"] = json!({ "inline": "AAAA" });
            }),
            "steps[0]: the invocation's source carries a locator (`inline`)",
        ),
        (
            Box::new(|m| m["steps"][0]["invocation"]["arguments"]["references"] = json!("x")),
            "steps[0]: the invocation's references is not a list of images",
        ),
        (
            Box::new(|m| m["steps"][0]["inputs"][1]["slot"] = json!("mask")),
            "steps[0].inputs[1] is for slot \"mask\", which no squint action takes",
        ),
    ];
    for (edit, want) in cases {
        let (manifest, members) = sample_with(edit);
        let (e, read) = refused(&writer::canonical(&manifest, &members));
        assert!(e.contains(want), "want {want:?}: {e}");
        assert_eq!(read, 512 + manifest.len().div_ceil(512) * 512, "{want}");
    }
    // An argument whose input the manifest leaves out, its member gone with it.
    let (manifest, members) = sample_with(|m| {
        m["steps"][0]["inputs"].as_array_mut().unwrap().remove(1);
    });
    let (source, _) = &members[1];
    let fewer: Vec<_> = members
        .iter()
        .filter(|(n, _)| n != source)
        .cloned()
        .collect();
    let (e, _) = refused(&writer::canonical(&manifest, &fewer));
    assert!(
        e.contains("steps[0]: the invocation's source has no input in the bundle"),
        "{e}"
    );
}

/// The replay's arguments are the invocation's with a `path` beside each image's `expect`,
/// pointing at its member's file under the directory given, and nothing else changed.
#[test]
fn the_replay_arguments_add_a_path_to_each_image() {
    let (candidate, source, extra) = (png(1), png(2), png(3));
    let mut run = run_with(&candidate, &source);
    let mut reference = run["invocation"]["arguments"]["source"].clone();
    reference["expect"]["file_sha256"] = json!(sha(&extra));
    run["invocation"]["arguments"]["references"] = json!([
        reference,
        run["invocation"]["arguments"]["candidate"].clone()
    ]);
    let (step, members) = writer::step_from_run(&run, &[&candidate, &source, &extra]);
    let manifest = writer::manifest(vec![step]);
    let bundle = Bundle::from_archive(writer::canonical(&manifest, &members).as_slice()).unwrap();
    let dir = Path::new("/replay/inputs");
    let arguments = bundle.arguments(0, dir).unwrap();
    let mut want = run["invocation"]["arguments"].clone();
    for (pointer, bytes) in [
        ("/candidate", &candidate),
        ("/source", &source),
        ("/references/0", &extra),
        ("/references/1", &candidate),
    ] {
        want.pointer_mut(pointer).unwrap()["path"] = json!(dir.join(format!("{}.png", sha(bytes))));
    }
    assert_eq!(Value::Object(arguments), want);
    assert!(bundle.arguments(1, dir).is_none());
}

#[test]
fn something_that_is_not_an_archive_is_refused() {
    let (e, read) = refused(&png(3));
    assert!(
        e.contains("truncated") || e.contains("not a ustar archive"),
        "{e}"
    );
    assert!(read <= 512);
    let mut block = vec![b'A'; 4096];
    block.extend_from_slice(&png(3));
    let (e, read) = refused(&block);
    assert!(e.contains("not a ustar archive"), "{e}");
    assert_eq!(read, 512);
}

#[test]
fn the_directory_form_is_held_to_the_same_checks() {
    let (manifest, members) = sample();
    let first = input_name(&members, 0);
    let fresh = |tag: &str| {
        let dir = tempdir(tag);
        writer::write_dir(&dir, &manifest, &members);
        dir
    };

    let dir = fresh("dir-flip");
    let path = dir.join(&first);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[3] ^= 0x01;
    std::fs::write(&path, bytes).unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains(&format!("`{first}`: its bytes hash to")), "{e}");

    let dir = fresh("dir-missing");
    std::fs::remove_file(dir.join(&first)).unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains(&format!(
            "lists `{first}` but the bundle has no such member"
        )),
        "{e}"
    );

    let dir = fresh("dir-extra");
    let extra = format!("inputs/{}.png", "0".repeat(64));
    std::fs::write(dir.join(&extra), b"extra").unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains(&format!("`{extra}` is not listed in invocation.json")),
        "{e}"
    );

    let dir = fresh("dir-renamed");
    let renamed = first.replace(".png", ".jpg");
    std::fs::rename(dir.join(&first), dir.join(&renamed)).unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains(&format!("`{renamed}` is not listed")), "{e}");

    let dir = fresh("dir-stray");
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains("member `notes.txt`: not invocation.json"), "{e}");

    let dir = fresh("dir-nested");
    std::fs::create_dir_all(dir.join("inputs/sub")).unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains("member `inputs/sub`"), "{e}");

    let dir = tempdir("dir-inputs-file");
    std::fs::write(dir.join("invocation.json"), &manifest).unwrap();
    std::fs::write(dir.join("inputs"), b"x").unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains("`inputs` is a regular file, not a directory"),
        "{e}"
    );

    let dir = fresh("dir-no-manifest");
    std::fs::remove_file(dir.join("invocation.json")).unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains("no invocation.json"), "{e}");

    let (bad, bad_members) = sample_with(|m| m["bundle_version"] = json!(2));
    let dir = tempdir("dir-schema");
    writer::write_dir(&dir, &bad, &bad_members);
    let e = refused_dir(&dir);
    assert!(
        e.contains("invocation-bundle schema: at /bundle_version"),
        "{e}"
    );

    // Over the bound by its length alone: a sparse file is refused before it is read.
    let dir = fresh("dir-oversized");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join(&first))
        .unwrap();
    file.set_len(MAX_MEMBER_BYTES + 1).unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains(&format!("`{first}` is 67108865 bytes, over the")),
        "{e}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn the_directory_form_never_follows_a_symlink() {
    use std::os::unix::fs::symlink;
    let (manifest, members) = sample();
    let first = input_name(&members, 0);
    let outside = tempdir("dir-outside");
    std::fs::write(outside.join("target.png"), &members[0].1).unwrap();

    let dir = tempdir("dir-symlink-member");
    writer::write_dir(&dir, &manifest, &members);
    std::fs::remove_file(dir.join(&first)).unwrap();
    symlink(outside.join("target.png"), dir.join(&first)).unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains(&format!("`{first}` is a symbolic link, not a regular file")),
        "{e}"
    );

    let dir = tempdir("dir-symlink-manifest");
    writer::write_dir(&dir, &manifest, &members);
    std::fs::write(outside.join("invocation.json"), &manifest).unwrap();
    std::fs::remove_file(dir.join("invocation.json")).unwrap();
    symlink(outside.join("invocation.json"), dir.join("invocation.json")).unwrap();
    let e = refused_dir(&dir);
    assert!(e.contains("`invocation.json` is a symbolic link"), "{e}");

    let dir = tempdir("dir-symlink-inputs");
    writer::write_dir(&outside.join("real"), &manifest, &members);
    std::fs::write(dir.join("invocation.json"), &manifest).unwrap();
    symlink(outside.join("real/inputs"), dir.join("inputs")).unwrap();
    let e = refused_dir(&dir);
    assert!(
        e.contains("`inputs` is a symbolic link, not a directory"),
        "{e}"
    );
}

//! Task 7.9: `squint playground` hands off to Wafflery by the launcher agreement (wafflery
//! H1): exactly `wafflery --tool squint`, found at `WAFFLERY_BIN`, on `PATH` or in
//! `~/.local/bin`, its exit code kept; when it is missing, an install offer on a terminal and 69
//! off one. Fake programs record their arguments; nothing here installs anything real.
#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A fresh directory for one test, with a space in its name so every path a test uses has one;
/// the process id keeps concurrent test runs apart.
fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("playground {} {tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir
}

/// An executable shell script at `path` with `body`.
fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake `wafflery` that writes its arguments, one per line, to `argv` beside it and exits
/// with `code`.
fn fake_host(path: &Path, code: i32) {
    script(
        path,
        &format!("printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\nexit {code}"),
    );
}

/// The arguments a fake host was given, or `None` when it never ran.
fn argv_of(host: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(host.parent().unwrap().join("argv")).ok()?;
    Some(text.lines().map(str::to_string).collect())
}

/// `squint playground <extra>` with `HOME` in the scratch directory, `PATH` the given directories
/// and the system's, no `WAFFLERY_BIN` unless set, and `stdin` fed to it.
fn playground(
    dir: &Path,
    path: &[&Path],
    env: &[(&str, &str)],
    extra: &[&str],
    stdin: &str,
) -> Output {
    let mut dirs: Vec<String> = path.iter().map(|p| p.display().to_string()).collect();
    dirs.push("/usr/bin:/bin".into());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_squint"));
    cmd.arg("playground")
        .args(extra)
        .env("HOME", dir.join("home"))
        .env("PATH", dirs.join(":"))
        .env_remove("WAFFLERY_BIN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    {
        use std::io::Write;
        let mut input = child.stdin.take().unwrap();
        let _ = input.write_all(stdin.as_bytes());
    }
    child.wait_with_output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn the_program_on_path_gets_exactly_the_tool_and_its_exit_code_is_kept() {
    let dir = scratch("path");
    let bin = dir.join("bin dir");
    fake_host(&bin.join("wafflery"), 7);
    let o = playground(&dir, &[&bin], &[], &[], "");
    assert_eq!(o.status.code(), Some(7), "{}", stderr(&o));
    assert_eq!(
        argv_of(&bin.join("wafflery")).unwrap(),
        ["--tool", "squint"]
    );
}

#[test]
fn the_launchers_success_is_squints() {
    let dir = scratch("success");
    let bin = dir.join("bin");
    fake_host(&bin.join("wafflery"), 0);
    let o = playground(&dir, &[&bin], &[], &[], "");
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(o.stdout.is_empty(), "squint's stdout stays clean");
}

#[test]
fn an_explicit_override_wins_over_path() {
    let dir = scratch("override");
    let chosen = dir.join("chosen").join("wafflery");
    let on_path = dir.join("bin").join("wafflery");
    fake_host(&chosen, 0);
    fake_host(&on_path, 0);
    let o = playground(
        &dir,
        &[on_path.parent().unwrap()],
        &[("WAFFLERY_BIN", chosen.to_str().unwrap())],
        &[],
        "",
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(argv_of(&chosen).unwrap(), ["--tool", "squint"]);
    assert!(argv_of(&on_path).is_none());
}

#[test]
fn an_invalid_override_fails_and_never_falls_through() {
    let dir = scratch("invalid override");
    let on_path = dir.join("bin").join("wafflery");
    fake_host(&on_path, 0);
    let not_exec = dir.join("plain").join("wafflery");
    std::fs::create_dir_all(not_exec.parent().unwrap()).unwrap();
    std::fs::write(&not_exec, "#!/bin/sh\n").unwrap();
    let missing = dir.join("missing").join("wafflery");
    let a_dir = dir.join("bin");
    for bad in [
        "wafflery",
        missing.to_str().unwrap(),
        a_dir.to_str().unwrap(),
        not_exec.to_str().unwrap(),
    ] {
        let o = playground(
            &dir,
            &[on_path.parent().unwrap()],
            &[("WAFFLERY_BIN", bad)],
            &[],
            "",
        );
        assert_eq!(o.status.code(), Some(69), "{bad}: {}", stderr(&o));
        assert!(stderr(&o).contains("WAFFLERY_BIN"), "{bad}: {}", stderr(&o));
        assert!(argv_of(&on_path).is_none(), "{bad} fell through to PATH");
    }
}

#[test]
fn the_local_bin_is_the_last_place_looked() {
    let dir = scratch("local bin");
    let local = dir.join("home/.local/bin/wafflery");
    fake_host(&local, 0);
    let o = playground(&dir, &[], &[], &[], "");
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(argv_of(&local).unwrap(), ["--tool", "squint"]);

    let on_path = dir.join("bin").join("wafflery");
    fake_host(&on_path, 0);
    std::fs::remove_file(local.parent().unwrap().join("argv")).unwrap();
    let o = playground(&dir, &[on_path.parent().unwrap()], &[], &[], "");
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(argv_of(&on_path).is_some());
    assert!(argv_of(&local).is_none(), "PATH comes before ~/.local/bin");
}

#[test]
fn a_symlinked_program_is_run_and_a_directory_on_path_is_skipped() {
    let dir = scratch("symlink");
    let real = dir.join("cellar").join("wafflery");
    fake_host(&real, 0);
    let linked = dir.join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(&real, linked.join("wafflery")).unwrap();
    let shadow = dir.join("shadow");
    std::fs::create_dir_all(shadow.join("wafflery")).unwrap();
    let o = playground(&dir, &[&shadow, &linked], &[], &[], "");
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // The fake writes beside the name it was run by: the link, not its target.
    assert_eq!(
        argv_of(&linked.join("wafflery")).unwrap(),
        ["--tool", "squint"]
    );
}

#[test]
fn a_launcher_killed_by_a_signal_exits_128_plus_the_signal() {
    let dir = scratch("signal");
    let bin = dir.join("bin");
    script(&bin.join("wafflery"), "kill -TERM $$");
    let o = playground(&dir, &[&bin], &[], &[], "");
    assert_eq!(o.status.code(), Some(143), "{}", stderr(&o));
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let dir = scratch("extra");
    let bin = dir.join("bin");
    fake_host(&bin.join("wafflery"), 0);
    let o = playground(&dir, &[&bin], &[], &["--library", "x"], "");
    assert_eq!(o.status.code(), Some(64), "{}", stderr(&o));
    assert!(argv_of(&bin.join("wafflery")).is_none());
}

#[test]
fn missing_off_a_terminal_exits_69_with_instructions_and_changes_nothing() {
    let dir = scratch("missing");
    let o = playground(&dir, &[], &[], &[], "y\n");
    assert_eq!(o.status.code(), Some(69), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("Wafflery"), "{err}");
    assert!(err.contains("WAFFLERY_BIN"), "{err}");
    assert!(!err.contains("[y/N]"), "no offer off a terminal: {err}");
    assert!(!dir.join("home/.local").exists());
}

#[cfg(not(feature = "testkit"))]
#[test]
fn a_release_build_has_no_installer_seam() {
    let dir = scratch("release seam");
    let o = playground(&dir, &[], &[], &["--installer", "/bin/true"], "");
    assert_eq!(o.status.code(), Some(64), "{}", stderr(&o));
}

/// The install offer, driven through the testkit-only seams: `--terminal` stands in for a
/// terminal and `--installer` for the release artifact no one has published yet.
#[cfg(feature = "testkit")]
mod offer {
    use super::*;

    /// A fake installer that puts a fake host (exiting `code`) at the path it is given.
    fn fake_installer(dir: &Path, code: i32) -> PathBuf {
        let payload = dir.join("payload").join("wafflery");
        fake_host(&payload, code);
        let path = dir.join("installer").join("install");
        script(
            &path,
            &format!("cp {:?} \"$1\"", payload.display().to_string()),
        );
        path
    }

    fn local(dir: &Path) -> PathBuf {
        dir.join("home/.local/bin/wafflery")
    }

    #[test]
    fn no_changes_nothing_and_exits_0() {
        for answer in ["n\n", "no\n", "\n", ""] {
            let dir = scratch("no");
            let installer = fake_installer(&dir, 0);
            let o = playground(
                &dir,
                &[],
                &[],
                &["--terminal", "--installer", installer.to_str().unwrap()],
                answer,
            );
            assert_eq!(o.status.code(), Some(0), "{answer:?}: {}", stderr(&o));
            assert!(stderr(&o).contains("[y/N]"), "{}", stderr(&o));
            assert!(!local(&dir).exists(), "{answer:?} installed");
        }
    }

    #[test]
    fn yes_installs_into_the_local_bin_and_runs_that_path() {
        let dir = scratch("yes");
        let installer = fake_installer(&dir, 5);
        let o = playground(
            &dir,
            &[],
            &[],
            &["--terminal", "--installer", installer.to_str().unwrap()],
            "y\n",
        );
        assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
        assert_eq!(argv_of(&local(&dir)).unwrap(), ["--tool", "squint"]);
    }

    #[test]
    fn a_failed_install_exits_74_and_runs_nothing() {
        let dir = scratch("install fails");
        let failing = dir.join("installer").join("install");
        script(&failing, "exit 1");
        let empty = dir.join("installer").join("noop");
        script(&empty, "exit 0");
        for installer in [failing, empty] {
            let o = playground(
                &dir,
                &[],
                &[],
                &["--terminal", "--installer", installer.to_str().unwrap()],
                "yes\n",
            );
            assert_eq!(o.status.code(), Some(74), "{}", stderr(&o));
            assert!(argv_of(&local(&dir)).is_none());
        }
    }

    #[test]
    fn with_no_published_installer_a_terminal_gets_69_and_no_offer() {
        let dir = scratch("unpublished");
        let o = playground(&dir, &[], &[], &["--terminal"], "y\n");
        assert_eq!(o.status.code(), Some(69), "{}", stderr(&o));
        let err = stderr(&o);
        assert!(!err.contains("[y/N]"), "{err}");
        assert!(err.contains("not available yet"), "{err}");
        assert!(!dir.join("home/.local").exists());
    }
}

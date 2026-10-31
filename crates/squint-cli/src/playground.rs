//! `squint playground`: hands off to Wafflehouse, the shared local program, by the launcher
//! agreement (wafflehouse H1). squint finds the program, runs `wafflehouse --tool squint` and keeps
//! its exit code; the launcher owns everything after that (the host, its locks, the browser).

use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

/// The program's name on `PATH` and in `~/.local/bin`.
const PROGRAM: &str = "wafflehouse";
/// The variable naming the program explicitly: an absolute path, never a fall-through.
const OVERRIDE: &str = "WAFFLEHOUSE_BIN";
/// The program, its release or a terminal to ask on is unavailable (H1).
const EXIT_UNAVAILABLE: i32 = 69;
/// Installation or local I/O failed (H1).
const EXIT_IO: i32 = 74;

#[derive(clap::Args)]
pub struct PlaygroundArgs {
    /// Treat the session as a terminal (testkit only: tests have no terminal to answer on).
    #[cfg(feature = "testkit")]
    #[arg(long, hide = true)]
    pub terminal: bool,
    /// The installer to offer, run as `<installer> <destination>` (testkit only: no release
    /// installer is published yet, so release builds have none).
    #[cfg(feature = "testkit")]
    #[arg(long, hide = true)]
    pub installer: Option<PathBuf>,
}

pub fn run(args: &PlaygroundArgs) -> i32 {
    #[cfg(feature = "testkit")]
    let (terminal, installer) = (args.terminal, args.installer.as_deref());
    #[cfg(not(feature = "testkit"))]
    let (terminal, installer) = {
        let _ = args;
        (false, None::<&Path>)
    };
    let terminal = terminal || (std::io::stdin().is_terminal() && std::io::stderr().is_terminal());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let program = match find(home.as_deref()) {
        Ok(Some(p)) => p,
        Ok(None) => return missing(home.as_deref(), terminal, installer),
        Err(m) => {
            eprintln!("squint: {m}");
            return EXIT_UNAVAILABLE;
        }
    };
    launch(&program)
}

/// The program: `WAFFLEHOUSE_BIN` when set, else the first `wafflehouse` on `PATH`, else
/// `~/.local/bin/wafflehouse`. An unusable override is an error, never a reason to look further.
fn find(home: Option<&Path>) -> Result<Option<PathBuf>, String> {
    if let Some(explicit) = std::env::var_os(OVERRIDE).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(&explicit);
        if !path.is_absolute() {
            return Err(format!(
                "{OVERRIDE}={} is not an absolute path; set it to the program's full path or unset it",
                path.display()
            ));
        }
        if !runnable(&path) {
            return Err(format!(
                "{OVERRIDE}={} is not an executable file; fix it or unset it",
                path.display()
            ));
        }
        return Ok(Some(path));
    }
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join(PROGRAM));
    let local = home.map(local_bin);
    Ok(on_path.chain(local).find(|p| runnable(p)))
}

/// An executable regular file once symlinks are followed.
fn runnable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn local_bin(home: &Path) -> PathBuf {
    home.join(".local/bin").join(PROGRAM)
}

/// No program: explain, then offer to install on a terminal when an installer exists.
fn missing(home: Option<&Path>, terminal: bool, installer: Option<&Path>) -> i32 {
    eprintln!(
        "squint: Wafflehouse is not installed. It is a separate shared local program, the \
         workspace squint opens with `{PROGRAM} --tool squint`.\n\
         Install `{PROGRAM}` on your PATH or in ~/.local/bin, or set {OVERRIDE} to its absolute \
         path, then run `squint playground` again."
    );
    let (Some(installer), Some(home), true) = (installer, home, terminal) else {
        if terminal {
            eprintln!("squint: an installer for Wafflehouse is not available yet.");
        }
        return EXIT_UNAVAILABLE;
    };
    let destination = local_bin(home);
    eprint!("Install Wafflehouse into {}? [y/N] ", destination.display());
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        eprintln!("squint: nothing installed.");
        return 0;
    }
    if let Err(e) = std::fs::create_dir_all(destination.parent().unwrap_or(home)) {
        eprintln!("squint: cannot create {}: {e}", destination.display());
        return EXIT_IO;
    }
    let installed = std::process::Command::new(installer)
        .arg(&destination)
        .status();
    match installed {
        Ok(s) if s.success() && runnable(&destination) => launch(&destination),
        Ok(s) if s.success() => {
            eprintln!(
                "squint: the installer finished but left no program at {}",
                destination.display()
            );
            EXIT_IO
        }
        Ok(s) => {
            eprintln!("squint: the installer failed ({s}); nothing was started");
            EXIT_IO
        }
        Err(e) => {
            eprintln!("squint: cannot run the installer: {e}");
            EXIT_IO
        }
    }
}

/// Runs `<program> --tool squint` with squint's own terminal, waits, and exits with its code: a
/// launcher killed by a signal exits 128 plus that signal.
fn launch(program: &Path) -> i32 {
    match std::process::Command::new(program)
        .args(["--tool", "squint"])
        .status()
    {
        Ok(s) => s
            .code()
            .or_else(|| s.signal().map(|n| 128 + n))
            .unwrap_or(EXIT_IO),
        Err(e) => {
            eprintln!("squint: cannot start {}: {e}", program.display());
            EXIT_UNAVAILABLE
        }
    }
}

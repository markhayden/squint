//! `squint report`: view a decision directory in the static viewer, export it as one page, rebuild
//! `report.json` from its header and journals, or prune decision directories by age.
//!
//! `view` and `export` are the description's two renders (table-manners 3.2): `view` a server
//! whose only stdout is its URL, serving until its stdin closes or a signal arrives, framed by at
//! most the one origin it is given; `export` a document, one self-contained file written outside
//! the decision directory, whose only stdout is that file's absolute path. Neither modifies the
//! decision directory.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use clap::Subcommand;
use squint_core::config::Config;
use squint_core::report::{self, Report};

use crate::EXIT_USAGE;

#[derive(clap::Args)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub command: ReportCommand,
}

#[derive(Subcommand)]
pub enum ReportCommand {
    /// Serve a decision directory to the static viewer on loopback; print only the URL, and serve
    /// until stdin closes or SIGTERM or SIGINT arrives.
    View(ViewArgs),
    /// Write a decision directory as one self-contained HTML page outside it (the viewer with a
    /// timeline of every step, and every image, request and result embedded, to open from disk);
    /// print only the page's absolute path.
    Export(ExportArgs),
    /// Rebuild `report.json` from the decision header and the run journals, complete or partial.
    Rebuild(RebuildArgs),
    /// Remove decision directories at least this old (`30d`, `12h`, `15m`, `0s`).
    Prune(PruneArgs),
}

#[derive(clap::Args)]
pub struct ViewArgs {
    /// A decision directory (`.../decision-<ms>-<pid>`) holding `report.json`.
    pub dir: PathBuf,
    /// The loopback port; 0 picks a free one.
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// The one origin allowed to frame the page (`http://127.0.0.1:4801`: `http` or `https`, a
    /// host, an optional port, no path); without it no page may frame it.
    #[arg(long, value_parser = parse_origin)]
    pub frame_ancestor: Option<String>,
    /// Open the URL in the system browser.
    #[arg(long)]
    pub open: bool,
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// A decision directory (`.../decision-<ms>-<pid>`) holding `report.json`.
    pub dir: PathBuf,
    /// Where to write the page, outside the decision directory;
    /// `$XDG_CACHE_HOME/squint/reports/<run id>.html` (`~/.cache/...`) by default.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Open the page in the system browser.
    #[arg(long)]
    pub open: bool,
}

#[derive(clap::Args)]
pub struct RebuildArgs {
    /// A decision directory (`.../decision-<ms>-<pid>`).
    pub dir: PathBuf,
}

#[derive(clap::Args)]
pub struct PruneArgs {
    /// The output directory holding decision directories; defaults to the config's `[output] dir`.
    #[arg(long, required_unless_present = "config")]
    pub out: Option<PathBuf>,
    /// The config whose `[output] dir` to prune.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Age threshold: `30d`, `12h`, `15m` or `0s`.
    #[arg(long, value_parser = parse_duration)]
    pub older_than: Duration,
}

pub fn run(args: &ReportArgs) -> i32 {
    match &args.command {
        ReportCommand::View(a) => view(a),
        ReportCommand::Export(a) => export(a),
        ReportCommand::Rebuild(a) => rebuild(a),
        ReportCommand::Prune(a) => prune(a),
    }
}

fn export(args: &ExportArgs) -> i32 {
    if !args.dir.join(report::REPORT_FILE).is_file() {
        eprintln!(
            "squint: {} holds no {}; run `squint report rebuild` first",
            args.dir.display(),
            report::REPORT_FILE
        );
        return EXIT_USAGE;
    }
    let dir = match args.dir.canonicalize() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("squint: {}: {e}", args.dir.display());
            return EXIT_USAGE;
        }
    };
    let out = match &args.out {
        Some(out) => out.clone(),
        None => match default_export_path(&dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("squint: {e}; pass --out");
                return EXIT_USAGE;
            }
        },
    };
    let out = match export_target(&dir, &out) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    if let Some(parent) = out.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("squint: cannot create {}: {e}", parent.display());
        return squint_core::run::EXIT_ERROR;
    }
    match crate::viewer::export(&dir, &out) {
        Ok(bytes) => {
            // The only stdout: the page's absolute path (table-manners 3.2).
            println!("{}", out.display());
            eprintln!(
                "squint: wrote {:.1} MB for {}",
                bytes as f64 / 1_000_000.0,
                dir.display()
            );
            if args.open {
                crate::viewer::open_in_browser(&format!("file://{}", out.display()));
            }
            0
        }
        Err(e) => {
            eprintln!("squint: cannot export {}: {e}", args.dir.display());
            squint_core::run::EXIT_ERROR
        }
    }
}

/// `$XDG_CACHE_HOME/squint/reports/<run id>.html`, else under `~/.cache`; the run id is the
/// decision directory's name, as `run.json` gives it.
fn default_export_path(dir: &Path) -> Result<PathBuf, String> {
    let id = dir
        .file_name()
        .ok_or_else(|| format!("{} names no run", dir.display()))?;
    let cache = match std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from) {
        Some(x) if x.is_absolute() => x,
        _ => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cache"))
            .ok_or("neither XDG_CACHE_HOME nor HOME is set")?,
    };
    let mut name = id.to_os_string();
    name.push(".html");
    Ok(cache.join("squint").join("reports").join(name))
}

/// The page's absolute path, refused when it falls inside the decision directory (a render never
/// modifies it). An existing parent is resolved for the check, so a link into the directory is
/// caught too.
fn export_target(dir: &Path, out: &Path) -> Result<PathBuf, String> {
    let out = std::path::absolute(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let name = out
        .file_name()
        .ok_or_else(|| format!("{} names no file", out.display()))?;
    let resolved = match out.parent().map(Path::canonicalize) {
        Some(Ok(parent)) => parent.join(name),
        _ => out.clone(),
    };
    if resolved.starts_with(dir) {
        return Err(format!(
            "{} is inside the decision directory {}, which an export never writes to; choose \
             another --out",
            out.display(),
            dir.display()
        ));
    }
    Ok(out)
}

/// Why the viewer stopped serving.
enum Stop {
    /// Its stdin reached end of file or a signal arrived: the host let go of it (exit 0).
    Released,
    /// The listener failed.
    Failed(std::io::Error),
}

fn view(args: &ViewArgs) -> i32 {
    if !args.dir.join(report::REPORT_FILE).is_file() {
        eprintln!(
            "squint: {} holds no {}; run `squint report rebuild` first",
            args.dir.display(),
            report::REPORT_FILE
        );
        return EXIT_USAGE;
    }
    let server =
        match crate::viewer::Server::bind(&args.dir, args.port, args.frame_ancestor.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("squint: cannot listen on 127.0.0.1:{}: {e}", args.port);
                return EXIT_USAGE;
            }
        };
    let url = match server.addr() {
        Ok(a) => format!("http://127.0.0.1:{}/", a.port()),
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("squint: cannot start the runtime: {e}");
            return EXIT_USAGE;
        }
    };
    // Everything that ends the server is in place before the URL is printed, so a host that
    // signals or closes stdin the moment it reads the line still gets exit 0.
    let signals = {
        let _guard = runtime.enter();
        crate::install_signal_handlers()
    };
    let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
    let eof = stop_tx.clone();
    std::thread::spawn(move || {
        // Whatever arrives is discarded; only its end matters.
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        let _ = eof.send(Stop::Released);
    });
    std::thread::spawn(move || {
        if let Err(e) = server.serve_forever() {
            let _ = stop_tx.send(Stop::Failed(e));
        }
    });
    // The only stdout, ever: the URL line (table-manners 3.2).
    println!("{url}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    eprintln!(
        "squint: viewing {} at {url}; stops when stdin closes or on Ctrl-C",
        args.dir.display()
    );
    if args.open {
        crate::viewer::open_in_browser(&url);
    }
    let stop = runtime.block_on(async {
        tokio::select! {
            _ = until_signal(signals) => Stop::Released,
            stop = stop_rx.recv() => stop.unwrap_or(Stop::Released),
        }
    });
    match stop {
        Stop::Released => 0,
        Stop::Failed(e) => {
            eprintln!("squint: {e}");
            squint_core::run::EXIT_ERROR
        }
    }
}

/// Resolves on SIGINT or SIGTERM (unix), or Ctrl-C elsewhere.
async fn until_signal(signals: crate::Signals) {
    #[cfg(unix)]
    match signals {
        Some((mut int, mut term)) => {
            tokio::select! {
                _ = int.recv() => {}
                _ = term.recv() => {}
            }
        }
        None => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signals;
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `--frame-ancestor`: an `http` or `https` origin (`scheme://host[:port]`, no path, no trailing
/// slash), kept as given; anything else, a CSP keyword or wildcard included, is refused.
pub fn parse_origin(text: &str) -> Result<String, String> {
    let bad = |why: &str| {
        format!("{text:?} is not an origin ({why}); expected e.g. http://127.0.0.1:4801")
    };
    let (scheme, authority) = text.split_once("://").ok_or_else(|| bad("no scheme"))?;
    if scheme != "http" && scheme != "https" {
        return Err(bad("the scheme must be http or https"));
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => {
            let (host, after) = rest
                .split_once(']')
                .ok_or_else(|| bad("an unclosed IPv6 address"))?;
            if host.is_empty()
                || !host
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
            {
                return Err(bad("an invalid IPv6 address"));
            }
            match after {
                "" => (host, None),
                _ => (
                    host,
                    Some(
                        after
                            .strip_prefix(':')
                            .ok_or_else(|| bad("a path or junk after the host"))?,
                    ),
                ),
            }
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let host_ok = authority.starts_with('[')
        || (!host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'));
    if !host_ok {
        return Err(bad(
            "the host may hold only letters, digits, '-' and '.', and no path",
        ));
    }
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || !port.parse::<u16>().is_ok_and(|p| p > 0))
    {
        return Err(bad("the port must be 1 to 65535, with no path after it"));
    }
    Ok(text.to_string())
}

fn rebuild(args: &RebuildArgs) -> i32 {
    let report = match Report::rebuild(&args.dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    if let Err(e) = report.write(&args.dir) {
        eprintln!("squint: {e}");
        return EXIT_USAGE;
    }
    for run in &report.runs {
        let detail = run
            .error
            .as_ref()
            .map(|e| format!(" ({}: {})", e.class, e.message))
            .unwrap_or_default();
        println!("run-{}: {}{detail}", run.run, run.terminal);
    }
    let verdict = report
        .verdict
        .and_then(|v| serde_json::to_string(&v).ok())
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_else(|| "none".into());
    println!("verdict: {verdict}");
    println!("report: {}", args.dir.join(report::REPORT_FILE).display());
    0
}

fn prune(args: &PruneArgs) -> i32 {
    let out = match (&args.out, &args.config) {
        (Some(out), _) => out.clone(),
        (None, Some(config)) => match Config::load(config) {
            Ok(c) => c.output.dir.clone(),
            Err(e) => {
                eprintln!("squint: {e}");
                return EXIT_USAGE;
            }
        },
        (None, None) => {
            eprintln!("squint: pass --out or --config");
            return EXIT_USAGE;
        }
    };
    match report::prune(&out, args.older_than, SystemTime::now()) {
        Ok(removed) => {
            for p in &removed {
                println!("removed {}", p.display());
            }
            println!(
                "pruned {} decision(s) under {}",
                removed.len(),
                out.display()
            );
            0
        }
        Err(e) => {
            eprintln!("squint: {e}");
            EXIT_USAGE
        }
    }
}

/// `30d`, `12h`, `15m`, `0s`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let n: u64 = number
        .parse()
        .map_err(|_| format!("{text:?}: expected a number followed by d, h, m or s"))?;
    let seconds = match unit {
        "d" => n * 86_400,
        "h" => n * 3_600,
        "m" => n * 60,
        "s" => n,
        _ => return Err(format!("{text:?}: the unit must be d, h, m or s")),
    };
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_with_their_units() {
        assert_eq!(parse_duration("30d"), Ok(Duration::from_secs(30 * 86_400)));
        assert_eq!(parse_duration("12h"), Ok(Duration::from_secs(12 * 3_600)));
        assert_eq!(parse_duration("15m"), Ok(Duration::from_secs(900)));
        assert_eq!(parse_duration("0s"), Ok(Duration::ZERO));
        assert!(parse_duration("3w").is_err());
        assert!(parse_duration("d").is_err());
    }

    #[test]
    fn only_an_http_or_https_origin_frames_the_viewer() {
        for good in [
            "http://127.0.0.1:4801",
            "https://example.com",
            "http://localhost:1",
            "http://[::1]:4801",
            "https://a-b.example.org:65535",
        ] {
            assert_eq!(parse_origin(good).as_deref(), Ok(good));
        }
        for bad in [
            "",
            "*",
            "'self'",
            "'none'",
            "127.0.0.1:4801",
            "ftp://example.com",
            "HTTP://example.com",
            "http://",
            "http://127.0.0.1:4801/",
            "http://example.com/path",
            "http://example.com?q",
            "http://*.example.com",
            "http://a b",
            "http://a;script-src *",
            "http://a,http://b",
            "http://user@a",
            "http://a:",
            "http://a:0",
            "http://a:65536",
            "http://a:80:80",
            "http://[::1",
            "http://[::1]/",
            "http://[]",
        ] {
            assert!(parse_origin(bad).is_err(), "{bad:?}");
        }
    }
}

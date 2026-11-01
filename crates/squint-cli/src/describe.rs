//! `squint describe --json`: the contract's description document (table-manners 3, task 7.5). It
//! works with no configuration (3.1, task 9.2): a missing or invalid one is a requirement in each
//! action's readiness, never a failure.

use std::path::PathBuf;

use squint_core::config::{Config, ConfigFile, ConfigState};
use table_manners::Description;

use crate::VERSION;

#[derive(clap::Args)]
pub struct DescribeArgs {
    /// Path to the config file; without it, squint looks for ./squint.toml, then
    /// $XDG_CONFIG_HOME/squint/squint.toml. None found is not an error.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Print the description as JSON (the only form).
    #[arg(long, required = true)]
    pub json: bool,
}

pub fn run(args: &DescribeArgs) -> i32 {
    let file = Config::open(args.config.as_deref(), &[]);
    let text = described(file, false)
        .map_err(|e| e.to_string())
        .and_then(|(_, d)| serde_json::to_string_pretty(&d).map_err(|e| e.to_string()));
    match text {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("squint: {e}");
            squint_core::run::EXIT_ERROR
        }
    }
}

/// The description under the configuration a process started with, and that configuration as
/// the description saw it: one whose rules cannot be resolved (an `extra` file that does not
/// read) is invalid like one that does not parse, so describing never fails on it (3.1). With
/// none loaded, every shipped use case, each needing the configuration.
pub(crate) fn described(
    file: ConfigFile,
    scripted: bool,
) -> squint_core::Result<(ConfigFile, Description)> {
    use squint_core::contract::{self, Executor};
    let file = match file.config() {
        Some(config) => {
            match contract::description(Some(config), VERSION, &crate::executor(config, scripted)) {
                Ok(d) => return Ok((file, d)),
                Err(e) => ConfigFile {
                    path: file.path,
                    state: ConfigState::Invalid {
                        reason: e.to_string(),
                        unset: None,
                    },
                },
            }
        }
        None => file,
    };
    let executor = match contract::config_requirement(&file) {
        Some(config) => Executor::Unconfigured { config },
        None => Executor::Free,
    };
    let description = contract::description(None, VERSION, &executor)?;
    Ok((file, description))
}

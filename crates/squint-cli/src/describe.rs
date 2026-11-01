//! `squint describe --json`: the contract's description document (table-manners 3, task 7.5). It
//! works with no configuration (3.1, task 9.2): a missing or invalid one is a requirement in each
//! action's readiness, never a failure.

use std::path::PathBuf;

use squint_core::config::{Config, ConfigFile, ConfigState};
use squint_core::contract::{self, Executor};
use squint_core::readiness::{self, Answers, Inspection};
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
        .and_then(|d| serde_json::to_string_pretty(&d.description).map_err(|e| e.to_string()));
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

/// What a process that describes squint starts with: the configuration as the description saw
/// it, the description (its readiness that of a fresh session), and the local inspection a
/// server recomputes readiness from on every `describe` (task 9.3).
pub(crate) struct Described {
    pub file: ConfigFile,
    pub description: Description,
    pub inspection: Inspection,
}

/// The description under the configuration a process started with. One whose rules cannot be
/// resolved (an `extra` file that does not read) is invalid like one that does not parse, so
/// describing never fails on it (3.1). With none loaded, every shipped use case, each needing
/// the configuration alone.
pub(crate) fn described(file: ConfigFile, scripted: bool) -> squint_core::Result<Described> {
    let fresh = Answers::default();
    let file = match file.config() {
        Some(config) => {
            let inspection = crate::inspect(config, readiness::config_requirement(&file));
            match contract::description(
                Some(config),
                VERSION,
                crate::executor(config, scripted),
                &inspection.readiness(&fresh),
            ) {
                Ok(description) => {
                    return Ok(Described {
                        file,
                        description,
                        inspection,
                    });
                }
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
    // No configuration chosen yet: a check may spend once one is.
    let inspection = Inspection::config_only(readiness::config_requirement(&file));
    let description =
        contract::description(None, VERSION, Executor::Paid, &inspection.readiness(&fresh))?;
    Ok(Described {
        file,
        description,
        inspection,
    })
}

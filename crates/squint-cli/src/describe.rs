//! `squint describe --json`: the contract's description document (table-manners 3, task 7.5).

use std::path::PathBuf;

use squint_core::config::Config;

use crate::{EXIT_USAGE, VERSION};

#[derive(clap::Args)]
pub struct DescribeArgs {
    /// Path to the config file; its provider profile names the credential variable a check needs.
    #[arg(long)]
    pub config: PathBuf,
    /// Print the description as JSON (the only form).
    #[arg(long, required = true)]
    pub json: bool,
}

pub fn run(args: &DescribeArgs) -> i32 {
    let config = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let executor = crate::executor(&config, false);
    let description = match squint_core::contract::description(&config, VERSION, &executor) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    match serde_json::to_string_pretty(&description) {
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

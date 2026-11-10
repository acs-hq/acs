//! `acs log`: show session records from the ledger (WP4.1).

use std::process::ExitCode;

use clap::Args;

use super::support::{CliError, OutputArgs, finish};

/// Arguments of `acs log`.
#[derive(Debug, Args)]
pub struct LogArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs log`.
#[must_use]
pub fn run(args: &LogArgs) -> ExitCode {
    finish(
        args.output.json,
        Err(CliError::new(
            "not_implemented",
            "`acs log` is not built yet.",
        )),
    )
}

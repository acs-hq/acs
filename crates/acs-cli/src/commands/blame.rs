//! `acs blame`: show which session wrote each line of a file (WP4.2).

use std::process::ExitCode;

use clap::Args;

use super::support::{CliError, OutputArgs, finish};

/// Arguments of `acs blame`.
#[derive(Debug, Args)]
pub struct BlameArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs blame`.
#[must_use]
pub fn run(args: &BlameArgs) -> ExitCode {
    finish(
        args.output.json,
        Err(CliError::new(
            "not_implemented",
            "`acs blame` is not built yet.",
        )),
    )
}

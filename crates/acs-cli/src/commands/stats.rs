//! `acs stats`: count what ACS prevented (WP4.5).

use std::process::ExitCode;

use clap::Args;

use super::support::{CliError, OutputArgs, finish};

/// Arguments of `acs stats`.
#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs stats`.
#[must_use]
pub fn run(args: &StatsArgs) -> ExitCode {
    finish(
        args.output.json,
        Err(CliError::new(
            "not_implemented",
            "`acs stats` is not built yet.",
        )),
    )
}

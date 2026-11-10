//! `acs undo`: undo what one session changed (WP4.3).

use std::process::ExitCode;

use clap::Args;

use super::support::{CliError, OutputArgs, finish};

/// Arguments of `acs undo`.
#[derive(Debug, Args)]
pub struct UndoArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs undo`.
#[must_use]
pub fn run(args: &UndoArgs) -> ExitCode {
    finish(
        args.output.json,
        Err(CliError::new(
            "not_implemented",
            "`acs undo` is not built yet.",
        )),
    )
}

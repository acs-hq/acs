//! `acs verify`: check the ledger's chains and signatures (WP4.4).

use std::process::ExitCode;

use clap::Args;

use super::support::{CliError, OutputArgs, finish};

/// Arguments of `acs verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs verify`.
#[must_use]
pub fn run(args: &VerifyArgs) -> ExitCode {
    finish(
        args.output.json,
        Err(CliError::new(
            "not_implemented",
            "`acs verify` is not built yet.",
        )),
    )
}

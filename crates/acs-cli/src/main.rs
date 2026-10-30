//! The `acs` binary. All logic lives in the `acs_cli` library; this only parses arguments and
//! dispatches.

use std::process::ExitCode;

fn main() -> ExitCode {
    acs_cli::cli::run_from_env()
}

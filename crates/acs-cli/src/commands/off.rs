//! `acs off` and `acs on`: turn the gate off and on for this repository on this machine.

use std::process::ExitCode;

use acs_core::Timestamp;
use acs_store::FLAG_GATE_OFF;
use clap::Args;
use serde_json::json;

use super::support::{CliError, OutputArgs, finish, open_repo, print_json, wrap};
use crate::runtime::OpError;

/// Arguments of `acs off`.
#[derive(Debug, Args)]
#[command(
    after_help = "Sealed paths (.git, .acs, CI workflows, harness hook files, ...) still hold \
                  while the gate is off. `acs status` shows that it is off; `acs on` undoes it."
)]
pub struct OffArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Arguments of `acs on`.
#[derive(Debug, Args)]
pub struct OnArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs off`.
#[must_use]
pub fn run_off(args: &OffArgs) -> ExitCode {
    finish(args.output.json, set(false, args.output.json))
}

/// Runs `acs on`.
#[must_use]
pub fn run_on(args: &OnArgs) -> ExitCode {
    finish(args.output.json, set(true, args.output.json))
}

fn set(gate_on: bool, json: bool) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let db = ctx
        .db()
        .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;
    let was_off = db.flag_is_set(FLAG_GATE_OFF).map_err(OpError::from)?;
    if gate_on {
        db.clear_flag(FLAG_GATE_OFF).map_err(OpError::from)?;
    } else {
        db.set_flag(FLAG_GATE_OFF, "1", Timestamp::now())
            .map_err(OpError::from)?;
    }
    let changed = was_off == gate_on;
    if json {
        print_json(&json!({
            "ok": true,
            "gate_off": !gate_on,
            "changed": changed,
            "sealed_paths_still_hold": true,
        }));
    } else if gate_on {
        let text = if changed {
            "The gate is on again: claims are enforced."
        } else {
            "The gate was already on."
        };
        println!("{text}");
    } else {
        let lead = if changed {
            "The gate is OFF for this repository on this machine."
        } else {
            "The gate was already off."
        };
        println!("{lead}");
        print!(
            "{}",
            wrap(
                "Claims are not enforced and nothing is claimed. Sealed paths still hold: \
                 agents cannot write .git, .acs or the other sealed globs. `acs status` shows \
                 that the gate is off; `acs on` turns it back on.",
                "",
                ""
            )
        );
    }
    Ok(ExitCode::SUCCESS)
}

//! `acs claim <paths...>`: take claims as the human seat.

use std::process::ExitCode;

use acs_core::Timestamp;
use clap::Args;
use serde_json::json;

use super::support::{
    CliError, OutputArgs, ellipsize_start, finish, human_seat, open_repo, print_json, wrap,
};
use crate::runtime::{ClaimOutcome, claim_paths, sync};

/// Arguments of `acs claim`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs claim src/lib.rs src/util.rs\n  acs claim src/payments/\n\n\
                  A directory claims the files under it. Sealed paths and files another session \
                  holds are refused. Owned zones bind agents only, so you may claim there."
)]
pub struct ClaimArgs {
    /// Files or directories to claim, relative to the current directory.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<String>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs claim`. Exits 1 when the claim is refused.
#[must_use]
pub fn run(args: &ClaimArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

/// At most this many claimed files are listed.
const LIST_LIMIT: usize = 20;

fn execute(args: &ClaimArgs) -> Result<ExitCode, CliError> {
    let (ctx, cwd) = open_repo()?;
    let now = Timestamp::now();
    let seat = human_seat(&ctx, now)?;
    let outcome = claim_paths(&ctx, &seat, &args.paths, &cwd, now)?;
    let json = args.output.json;
    match outcome {
        ClaimOutcome::Granted {
            paths,
            claim_ids,
            warnings,
        } => {
            // No hook follows a command-line claim, so nothing else would publish it.
            sync::spawn_now(&ctx);
            if json {
                print_json(&json!({
                    "ok": true,
                    "granted": true,
                    "paths": paths.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                    "claim_ids": claim_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "warnings": warnings,
                }));
            } else {
                let noun = if paths.len() == 1 { "file" } else { "files" };
                println!("Claimed {} {noun} as {}:", paths.len(), seat.seat.pilot);
                for path in paths.iter().take(LIST_LIMIT) {
                    println!("  {}", ellipsize_start(path.as_str(), 76));
                }
                if paths.len() > LIST_LIMIT {
                    println!("  and {} more", paths.len() - LIST_LIMIT);
                }
                for warning in &warnings {
                    print!("{}", wrap(warning, "Note: ", "      "));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ClaimOutcome::Refused {
            reason_code,
            message,
        } => {
            if json {
                print_json(&json!({
                    "ok": false,
                    "granted": false,
                    "reason_code": reason_code.as_str(),
                    "message": message,
                }));
            } else {
                eprint!(
                    "{}",
                    wrap(&format!("Refused ({reason_code}): {message}"), "", "  ")
                );
            }
            Ok(ExitCode::from(1))
        }
    }
}

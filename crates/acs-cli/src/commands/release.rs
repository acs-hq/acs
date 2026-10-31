//! `acs release <paths...>` and `acs release --all`: give up the human seat's claims.

use std::process::ExitCode;

use acs_core::{ReleaseReason, Timestamp};
use clap::Args;
use serde_json::json;

use super::support::{
    CliError, OutputArgs, ellipsize_start, finish, human_seat, open_repo, print_json, wrap,
};
use crate::runtime::{OpError, release_paths, sync};

/// Arguments of `acs release`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  acs release src/lib.rs\n  acs release --all")]
pub struct ReleaseArgs {
    /// Files or directories to release, relative to the current directory.
    #[arg(
        value_name = "PATH",
        required_unless_present = "all",
        conflicts_with = "all"
    )]
    pub paths: Vec<String>,
    /// Release every claim the human seat holds.
    #[arg(long)]
    pub all: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs release`. Releasing a file that is not held is not an error; a path outside the
/// repository is (exit 1).
#[must_use]
pub fn run(args: &ReleaseArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

/// At most this many files are listed per group.
const LIST_LIMIT: usize = 20;

fn execute(args: &ReleaseArgs) -> Result<ExitCode, CliError> {
    let (ctx, cwd) = open_repo()?;
    let now = Timestamp::now();
    let seat = human_seat(&ctx, now)?;
    let (released, not_held, outside): (Vec<String>, Vec<String>, Vec<String>) = if args.all {
        let db = ctx
            .db()
            .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;
        let mine: Vec<_> = db
            .live_claims(now)
            .map_err(OpError::from)?
            .into_iter()
            .filter(|held| held.claim.seat_id == seat.seat.seat_id)
            .collect();
        let ids: Vec<_> = mine.iter().map(|h| h.claim.claim_id.clone()).collect();
        if !ids.is_empty() {
            db.release_claims(&ids, ReleaseReason::Released, now)
                .map_err(OpError::from)?;
        }
        let mut paths: Vec<String> = mine
            .iter()
            .map(|h| h.claim.scope.path().to_string())
            .collect();
        paths.sort();
        paths.dedup();
        (paths, Vec::new(), Vec::new())
    } else {
        let report = release_paths(&ctx, &seat, &args.paths, &cwd, now)?;
        (
            report.released.iter().map(ToString::to_string).collect(),
            report.not_held.iter().map(ToString::to_string).collect(),
            report.outside,
        )
    };
    if !released.is_empty() {
        // No hook follows a command-line release, so nothing else would publish it.
        sync::spawn_now(&ctx);
    }
    let code = if outside.is_empty() { 0 } else { 1 };
    if args.output.json {
        print_json(&json!({
            "ok": outside.is_empty(),
            "released": released,
            "not_held": not_held,
            "outside_repository": outside,
        }));
    } else {
        print_group(&released, |n| format!("Released {n} {}:", plural(n)));
        print_group(&not_held, |n| format!("Not held by you ({n}), left alone:"));
        if !outside.is_empty() {
            eprint!(
                "{}",
                wrap(
                    &format!(
                        "Outside the repository ({}): {}",
                        outside.len(),
                        outside.join(", ")
                    ),
                    "",
                    "  "
                )
            );
        }
        if released.is_empty() && not_held.is_empty() && outside.is_empty() {
            println!("You hold no claims.");
        }
    }
    Ok(ExitCode::from(code))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "file" } else { "files" }
}

fn print_group(items: &[String], title: impl Fn(usize) -> String) {
    if items.is_empty() {
        return;
    }
    println!("{}", title(items.len()));
    for item in items.iter().take(LIST_LIMIT) {
        println!("  {}", ellipsize_start(item, 76));
    }
    if items.len() > LIST_LIMIT {
        println!("  and {} more", items.len() - LIST_LIMIT);
    }
}

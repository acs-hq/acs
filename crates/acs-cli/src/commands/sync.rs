//! `acs sync`: share claims with other machines through the git remote.
//!
//! Hooks run this detached (`runtime::sync`); people run it to see the result right now.

use std::process::ExitCode;

use acs_core::Timestamp;
use acs_git::refs::CustomRefs;
use acs_git::sync::{SyncOptions, SyncReport};
use clap::Args;
use serde_json::json;

use super::support::{CliError, OutputArgs, finish, open_repo, print_json, wrap};
use crate::runtime::{OpError, sync};

/// Arguments of `acs sync`.
#[derive(Debug, Args)]
#[command(
    after_help = "Fetches the claims of other machines, publishes yours, and pushes both over \
                  your git remote (`origin`, or the first remote by name). Nothing else is \
                  involved, and it uses your usual git authentication. It never prompts: if \
                  `git fetch` asks you for credentials, sync fails until they are cached.\n\n\
                  Hooks run this in the background; you rarely need to."
)]
pub struct SyncArgs {
    /// Wait for a sync already running to finish and then sync, instead of leaving it to that one.
    #[arg(long)]
    pub wait: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs sync`. Exits 1 when the remote cannot be reached or rejects the push; with no remote
/// there is nothing to do, which is not an error.
#[must_use]
pub fn run(args: &SyncArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &SyncArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let db = ctx
        .db()
        .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;
    let json = args.output.json;
    let work_tree = &ctx.layout.work_tree;
    let remote = acs_git::repo::default_remote(work_tree)
        .map_err(|err| CliError::new("git", err.to_string()))?;
    let Some(remote) = remote else {
        say(
            json,
            "local_only",
            None,
            "No git remote: ACS is local-only here, so there is nothing to sync.",
        );
        return Ok(ExitCode::SUCCESS);
    };
    let lock = sync::lock(&ctx, args.wait)
        .map_err(|err| CliError::new("lock", format!("cannot take the sync lock: {err}")))?;
    if lock.is_none() {
        say(
            json,
            "already_running",
            Some(&remote),
            "A sync is already running.",
        );
        return Ok(ExitCode::SUCCESS);
    }
    let pilot = acs_git::repo::pilot_email_or_fallback(work_tree);
    let storage = CustomRefs::new(work_tree, &remote);
    let options = SyncOptions {
        pilot: &pilot,
        now: Timestamp::now(),
    };
    let report = acs_git::sync::run(&storage, db, &options).map_err(|err| {
        CliError::new(
            "sync_failed",
            format!(
                "sync with `{remote}` failed: {err}. Claims on this machine are still enforced."
            ),
        )
    })?;
    if json {
        print_json(&json!({
            "ok": true,
            "status": "synced",
            "remote": remote,
            "report": report,
        }));
    } else {
        print!("{}", render(&remote, &report));
    }
    Ok(ExitCode::SUCCESS)
}

fn say(json: bool, status: &str, remote: Option<&str>, text: &str) {
    if json {
        print_json(&json!({"ok": true, "status": status, "remote": remote}));
    } else {
        print!("{}", wrap(text, "", ""));
    }
}

fn render(remote: &str, report: &SyncReport) -> String {
    let mut out = wrap(&format!("Synced with `{remote}`."), "", "");
    let published = if report.refs_pushed == 0 {
        "Published: nothing new.".to_owned()
    } else {
        format!(
            "Published: {} ref{}.",
            report.refs_pushed,
            if report.refs_pushed == 1 { "" } else { "s" }
        )
    };
    out.push_str(&wrap(&published, "  ", "    "));
    out.push_str(&wrap(
        &format!(
            "Other machines: {} claim{} from {} pilot{}.",
            report.remote_claims,
            if report.remote_claims == 1 { "" } else { "s" },
            report.remote_pilots,
            if report.remote_pilots == 1 { "" } else { "s" },
        ),
        "  ",
        "    ",
    ));
    for warning in &report.warnings {
        out.push_str(&wrap(warning, "  Warning: ", "    "));
    }
    out
}

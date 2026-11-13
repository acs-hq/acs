//! `acs verify`: check the ledger's chains and signatures (WP4.4).
//!
//! Reads every pilot's ledger on this machine (written here or fetched) and reports records,
//! signatures and problems. Exit 0 when nothing is wrong; unsigned commits are reported but are
//! only a failure with `--require-signed`. Exit 1 on a broken chain, a bad signature, a wrong
//! author, an invalid record or rewritten history.

use std::process::ExitCode;

use acs_git::refs::SyncStorage;
use acs_git::signing::{PilotVerdict, VerifiedTips, verify_pilot};
use clap::Args;
use serde_json::{Value, json};

use super::support::{CliError, OutputArgs, ellipsize_end, finish, open_repo, print_json, wrap};
use crate::runtime::records::storage;

/// Arguments of `acs verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Check only this pilot's ledger (a git email or a pilot id).
    #[arg(long)]
    pub pilot: Option<String>,
    /// Treat commits that are not signed (or whose signature cannot be checked) as a failure.
    #[arg(long)]
    pub require_signed: bool,
    /// Accept the ledgers as they are now: forget what was verified before, so history that was
    /// rewritten on purpose stops being reported.
    #[arg(long)]
    pub accept_history: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// The file in the repository's cache directory that remembers the verified tips.
const MEMORY_FILE: &str = "verified-ledgers.json";

/// Runs `acs verify`.
#[must_use]
pub fn run(args: &VerifyArgs) -> ExitCode {
    finish(args.output.json, verify(args))
}

fn verify(args: &VerifyArgs) -> Result<ExitCode, CliError> {
    let (ctx, _) = open_repo()?;
    super::support::note_feature_off(
        &ctx,
        acs_core::Feature::History,
        args.output.json,
        "sessions are not recorded any more; this checks the records from before",
    );
    let git = |err: acs_git::GitError| CliError::new("git", format!("cannot verify: {err}"));
    let store = storage(&ctx);
    let mut pilots = store.ledger_pilots().map_err(git)?;
    if let Some(wanted) = &args.pilot {
        let id = if wanted.contains('@') {
            acs_core::pilot_id(wanted)
        } else {
            wanted.trim().to_owned()
        };
        if !pilots.contains(&id) {
            return Err(CliError::new(
                "no_ledger",
                format!("there is no ledger for `{wanted}` on this machine."),
            ));
        }
        pilots = vec![id];
    }
    let remote = acs_git::repo::default_remote(&ctx.layout.work_tree)
        .ok()
        .flatten()
        .unwrap_or_else(|| "origin".to_owned());
    let memory_path = ctx
        .repo_home
        .as_ref()
        .map(|r| r.cache_dir().join(MEMORY_FILE));
    let mut memory = memory_path
        .as_deref()
        .map_or_else(VerifiedTips::in_memory, VerifiedTips::load);
    let mut verdicts = Vec::new();
    for id in &pilots {
        verdicts.push(
            verify_pilot(
                &ctx.layout.work_tree,
                &remote,
                id,
                &mut memory,
                args.accept_history,
            )
            .map_err(git)?,
        );
    }
    let mut warning = None;
    if let Err(err) = memory.save() {
        warning = Some(format!("could not remember the verified tips: {err}"));
    }

    let failed = verdicts.iter().any(|v| failure(v, args.require_signed));
    if args.output.json {
        let items: Vec<Value> = verdicts
            .iter()
            .map(|v| pilot_json(v, args.require_signed))
            .collect();
        print_json(&json!({
            "ok": !failed,
            "require_signed": args.require_signed,
            "pilots": items,
            "warning": warning,
        }));
    } else {
        print!("{}", render(&verdicts, args.require_signed));
        if let Some(warning) = warning {
            eprintln!("acs: {warning}");
        }
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Totals over the chains of a pilot (the same commit is not counted twice: a chain that both
/// copies point at is one chain).
struct Totals {
    commits: usize,
    records: usize,
    joins: usize,
    signed: usize,
    unsigned: usize,
    unchecked: usize,
}

fn totals(verdict: &PilotVerdict) -> Totals {
    let mut sum = Totals {
        commits: 0,
        records: 0,
        joins: 0,
        signed: 0,
        unsigned: 0,
        unchecked: 0,
    };
    // Two different chains share most of their history; the longer one is the picture.
    for chain in &verdict.chains {
        let report = &chain.report;
        if report.commits >= sum.commits {
            sum = Totals {
                commits: report.commits,
                records: report.records,
                joins: report.joins,
                signed: report.signed,
                unsigned: report.unsigned,
                unchecked: report.unchecked,
            };
        }
    }
    sum
}

fn failure(verdict: &PilotVerdict, require_signed: bool) -> bool {
    if verdict.all_problems().next().is_some() {
        return true;
    }
    require_signed
        && verdict
            .chains
            .iter()
            .any(|c| c.report.signed < c.report.commits)
}

fn pilot_json(verdict: &PilotVerdict, require_signed: bool) -> Value {
    let sum = totals(verdict);
    let email = verdict.chains.iter().find_map(|c| c.report.author.clone());
    json!({
        "pilot_id": verdict.pilot_id,
        "pilot": email,
        "ok": !failure(verdict, require_signed),
        "commits": sum.commits,
        "records": sum.records,
        "joins": sum.joins,
        "signed": sum.signed,
        "unsigned": sum.unsigned,
        "unchecked": sum.unchecked,
        "chains": verdict.chains,
        "problems": verdict.all_problems().collect::<Vec<_>>(),
    })
}

fn render(verdicts: &[PilotVerdict], require_signed: bool) -> String {
    if verdicts.is_empty() {
        return "No ledgers to verify: no session has been recorded yet.\n".to_owned();
    }
    let mut out = String::new();
    let bad = verdicts
        .iter()
        .filter(|v| failure(v, require_signed))
        .count();
    out.push_str(&format!(
        "Verified {} ledger(s): {}\n",
        verdicts.len(),
        if bad == 0 {
            "no problems".to_owned()
        } else {
            format!("{bad} with problems")
        }
    ));
    for verdict in verdicts {
        let sum = totals(verdict);
        let who = verdict
            .chains
            .iter()
            .find_map(|c| c.report.author.clone())
            .unwrap_or_else(|| verdict.pilot_id.clone());
        let status = if failure(verdict, require_signed) {
            "PROBLEMS"
        } else {
            "ok"
        };
        out.push_str(&format!("\n  {:<62}  {status}\n", ellipsize_end(&who, 62)));
        out.push_str(&format!(
            "    {} record(s), {} join(s); {} signed, {} unsigned",
            sum.records, sum.joins, sum.signed, sum.unsigned
        ));
        if sum.unchecked > 0 {
            out.push_str(&format!(", {} cannot be checked", sum.unchecked));
        }
        out.push('\n');
        for chain in &verdict.chains {
            for note in &chain.report.notes {
                out.push_str(&wrap(note, "    note: ", "          "));
            }
        }
        if require_signed && sum.signed < sum.commits && verdict.all_problems().next().is_none() {
            out.push_str("    not every commit is signed (--require-signed)\n");
        }
        for problem in verdict.all_problems() {
            let commit = problem
                .commit
                .as_deref()
                .map_or(String::new(), |c| format!("{} ", &c[..c.len().min(8)]));
            out.push_str(&wrap(
                &format!("{commit}{}", problem.detail),
                "    ! ",
                "      ",
            ));
        }
    }
    out
}

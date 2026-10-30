//! `acs status`: who holds what, what is sealed, whether the gate is on.

use std::fmt::Write as _;
use std::process::ExitCode;

use acs_core::Timestamp;
use clap::Args;

use super::support::{
    CliError, OutputArgs, WIDTH, age, ellipsize_start, finish, human_seat, open_repo, print_json,
    row, short_duration, wrap,
};
use crate::runtime::{ClaimView, StatusView, status};

/// Arguments of `acs status`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  acs status\n  acs status --json")]
pub struct StatusArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
    /// List every sealed glob instead of the first few.
    #[arg(long)]
    pub sealed: bool,
}

/// Runs `acs status`.
#[must_use]
pub fn run(args: &StatusArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &StatusArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let now = Timestamp::now();
    // The human seat is created on first use; without a usable database there is no seat and
    // the view says why.
    let seat = human_seat(&ctx, now).ok();
    let view = status(&ctx, seat.as_ref(), now);
    if args.output.json {
        print_json(&view);
    } else {
        print!("{}", render(&view, now, args.sealed));
    }
    Ok(ExitCode::SUCCESS)
}

/// How many sealed globs are listed unless `--sealed` is given.
const SEALED_SHOWN: usize = 4;

/// The human view, every line at most [`WIDTH`] columns.
#[must_use]
pub fn render(view: &StatusView, now: Timestamp, all_sealed: bool) -> String {
    let mut out = String::new();
    let field = |name: &str, value: &str| {
        let room = WIDTH - 12;
        format!("  {name:<10}{}\n", ellipsize_start(value, room))
    };
    out.push_str("ACS status\n");
    if let Some(id) = &view.repo_id {
        out.push_str(&field("repo", &id.chars().take(12).collect::<String>()));
    }
    out.push_str(&field("worktree", &view.worktree));
    let branch = view.branch.as_deref().unwrap_or("(detached HEAD)");
    out.push_str(&field(
        "branch",
        &format!("{branch} (work is integrated into {})", view.target_branch),
    ));
    if let Some(seat) = &view.seat {
        out.push_str(&field("you", &format!("{} ({})", seat.pilot, seat.harness)));
    }

    if view.gate_off {
        out.push('\n');
        out.push_str("!! GATE OFF: claims are not enforced on this machine.\n");
        out.push_str("   Sealed paths still hold. Run `acs on` to enforce claims again.\n");
    }
    if let Some(why) = &view.state_unavailable {
        out.push('\n');
        out.push_str(&wrap(
            &format!(
                "!! The state database is unavailable: {why}. Claims are not listed. \
                 Run `acs doctor`."
            ),
            "",
            "   ",
        ));
    }
    if !view.policy_warnings.is_empty() {
        let _ = writeln!(out, "\nPolicy warnings ({}):", view.policy_warnings.len());
        for warning in &view.policy_warnings {
            out.push_str(&wrap(warning, "  - ", "    "));
        }
    }

    out.push('\n');
    out.push_str(&remote_lines(view, now));

    let mine = view.my_claims.len();
    let _ = writeln!(out, "\nYour claims ({mine}):");
    if mine == 0 {
        out.push_str("  none\n");
    }
    for claim in &view.my_claims {
        let left = claim_target(claim);
        let ttl = claim.expires_at.as_second() - now.as_second();
        let right = format!(
            "held {}, expires in {}",
            age(now, claim.acquired_at),
            short_duration(ttl)
        );
        out.push_str(&row(&left, &right));
        out.push('\n');
    }

    let others = view.other_claims.len();
    let _ = writeln!(out, "\nClaimed by others ({others}):");
    if others == 0 {
        out.push_str("  none\n");
    }
    for claim in &view.other_claims {
        let who = claim.harness.as_deref().map_or_else(
            || claim.pilot.clone(),
            |harness| format!("{harness} {}", claim.pilot),
        );
        let remote = if claim.source == "remote" {
            " [remote]"
        } else {
            ""
        };
        let right = format!("{who}{remote} {}", age(now, claim.acquired_at));
        out.push_str(&row(&claim_target(claim), &right));
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "\nSealed paths no agent may write ({}):",
        view.sealed.len()
    );
    let shown = if all_sealed {
        view.sealed.len()
    } else {
        SEALED_SHOWN.min(view.sealed.len())
    };
    for glob in &view.sealed[..shown] {
        out.push_str(&row(glob, ""));
        out.truncate(out.trim_end().len());
        out.push('\n');
    }
    if shown < view.sealed.len() {
        let _ = writeln!(
            out,
            "  and {} more (`acs status --sealed` lists all)",
            view.sealed.len() - shown
        );
    }
    out
}

fn claim_target(claim: &ClaimView) -> String {
    match &claim.symbol_path {
        Some(symbol) => format!("{}::{symbol}", claim.path),
        None => claim.path.clone(),
    }
}

fn remote_lines(view: &StatusView, now: Timestamp) -> String {
    let remote = &view.remote;
    let mut out = String::new();
    let line = if !remote.configured {
        "Remote: none configured (local-only; claims are not shared)".to_owned()
    } else if !remote.sync_ran {
        "Remote: configured; no sync has run yet, others' claims are not visible".to_owned()
    } else if remote.stale {
        let last = remote.last_success_at.map_or_else(
            || "never fetched".to_owned(),
            |at| format!("last fetch {} ago", age(now, at)),
        );
        format!("Remote: STALE view ({last}); others' claims may be out of date")
    } else {
        let last = remote.last_success_at.map_or_else(String::new, |at| {
            format!(" (last fetch {} ago)", age(now, at))
        });
        format!("Remote: in sync{last}")
    };
    out.push_str(&wrap(&line, "", "        "));
    if let Some(err) = &remote.last_error {
        out.push_str(&wrap(&format!("Last sync error: {err}"), "  ", "    "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RemoteView, SeatSummary};
    use acs_core::ClaimOrigin;

    fn claim(path: &str, harness: &str, pilot: &str, now: Timestamp) -> ClaimView {
        ClaimView {
            path: path.to_owned(),
            symbol_path: None,
            pilot: pilot.to_owned(),
            harness: Some(harness.to_owned()),
            seat_id: "seat_1".to_owned(),
            source: "local",
            origin: ClaimOrigin::Agent,
            acquired_at: now,
            heartbeat_at: now,
            expires_at: now,
        }
    }

    fn view(now: Timestamp) -> StatusView {
        StatusView {
            repo_id: Some("a".repeat(64)),
            worktree: format!("/very/long/{}", "directory/".repeat(12)),
            branch: Some("feature/a-rather-long-branch-name".to_owned()),
            target_branch: "main".to_owned(),
            gate_off: true,
            state_unavailable: Some("x".repeat(150)),
            seat: Some(SeatSummary {
                seat_id: "s".to_owned(),
                session_id: "ses_1".to_owned(),
                harness: "human".to_owned(),
                pilot: "sara@example.com".to_owned(),
                intent: None,
            }),
            my_claims: vec![claim(&"deep/".repeat(30), "human", "sara@example.com", now)],
            other_claims: vec![claim(
                &"nested/".repeat(20),
                "claude-code",
                "someone.with.a.very.long.address@a-long-company-domain.example.com",
                now,
            )],
            sealed: (0..9)
                .map(|i| format!("{}{i}/**", "g".repeat(100)))
                .collect(),
            remote: RemoteView {
                configured: true,
                sync_ran: true,
                stale: true,
                last_success_at: None,
                last_attempt_at: Some(now),
                last_error: Some("fetch failed: ".repeat(12)),
            },
            policy_warnings: vec!["warning ".repeat(30)],
        }
    }

    #[test]
    fn every_line_fits_80_columns() {
        let now = Timestamp::now();
        for all in [false, true] {
            let text = render(&view(now), now, all);
            for line in text.lines() {
                assert!(
                    line.chars().count() <= WIDTH,
                    "{} chars: {line}",
                    line.len()
                );
            }
        }
    }

    #[test]
    fn gate_off_and_staleness_are_prominent() {
        let now = Timestamp::now();
        let text = render(&view(now), now, false);
        assert!(text.contains("!! GATE OFF"), "{text}");
        assert!(text.contains("STALE"), "{text}");
        assert!(text.contains("and 5 more"), "{text}");
    }
}

//! `acs status`: who holds what, what is sealed, whether the gate is on.

use std::fmt::Write as _;
use std::process::ExitCode;

use acs_core::Timestamp;
use clap::Args;

use super::support::{
    CliError, OutputArgs, WIDTH, age, ellipsize_start, finish, human_seat, open_repo, print_json,
    row, short_duration, wrap,
};
use crate::runtime::{COLLISION_WINDOW_HOURS, ClaimView, StatusView, status};

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
/// How many collisions the human view lists.
const COLLISIONS_SHOWN: usize = 5;

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

    out.push_str(&collision_lines(view, now));
    out.push_str(&override_lines(view, now));
    if view.lore_proposals > 0 {
        let n = view.lore_proposals;
        let _ = writeln!(
            out,
            "\nLore proposals from agents: {n} wait{} for review (acs lore review).",
            if n == 1 { "s" } else { "" }
        );
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

/// The overrides of owned territory still in force. Absent when there are none.
fn override_lines(view: &StatusView, now: Timestamp) -> String {
    let mut out = String::new();
    if view.overrides.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "\nOverrides of owned territory ({}):",
        view.overrides.len()
    );
    for item in &view.overrides {
        let mode = match item.mode {
            acs_core::OverrideMode::Once => "once",
            acs_core::OverrideMode::Session => "session",
        };
        let state = match (&item.seat_id, item.expires_at) {
            (Some(seat), _) => format!("held by {seat}"),
            (None, Some(at)) => format!(
                "unused, lapses in {}",
                crate::commands::support::short_duration(
                    at.as_second().saturating_sub(now.as_second())
                )
            ),
            (None, None) => "unused".to_owned(),
        };
        let right = format!("{mode}, {state}");
        out.push_str(&row(&item.pattern, &right));
        out.push('\n');
        out.push_str(&wrap(
            &format!(
                "granted {} ago by {}; `acs override --revoke {}` withdraws it",
                age(now, item.granted_at),
                item.granted_by,
                item.override_id
            ),
            "    ",
            "    ",
        ));
    }
    out
}

/// The refused calls of the last day, newest first. Absent when there were none.
fn collision_lines(view: &StatusView, now: Timestamp) -> String {
    let mut out = String::new();
    let total = view.collisions_total;
    if total == 0 {
        return out;
    }
    let _ = writeln!(
        out,
        "\nRecent collisions ({total} in the last {COLLISION_WINDOW_HOURS} h):"
    );
    for collision in view.collisions.iter().take(COLLISIONS_SHOWN) {
        let mut target = collision.paths.first().cloned().unwrap_or_default();
        if collision.paths.len() > 1 {
            let _ = write!(target, " and {} more", collision.paths.len() - 1);
        }
        let right = format!(
            "{} {} {} ago",
            collision.harness,
            collision.pilot,
            age(now, collision.at)
        );
        out.push_str(&row(&target, &right));
        out.push('\n');
        if let Some(message) = &collision.message {
            out.push_str(&wrap(message, "    ", "    "));
        }
    }
    if (COLLISIONS_SHOWN as u64) < total {
        let _ = writeln!(
            out,
            "  and {} more (`acs status --json` lists them)",
            total - COLLISIONS_SHOWN as u64
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
    use crate::runtime::{CollisionView, RemoteView, SeatSummary};
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
            collisions: (0..7)
                .map(|i| CollisionView {
                    at: now,
                    paths: vec![format!("{}{i}.rs", "deep/".repeat(30)), "b.rs".to_owned()],
                    tool: Some("Edit".to_owned()),
                    pilot: "someone.with.a.very.long.address@example.com".to_owned(),
                    harness: "claude-code".to_owned(),
                    seat_id: "seat_2".to_owned(),
                    message: Some(format!("`{}` is claimed by someone.", "x/".repeat(60))),
                })
                .collect(),
            collisions_total: 9,
            overrides: vec![crate::runtime::OverrideItem {
                override_id: "ovr_abc".to_owned(),
                pattern: format!("{}/**", "payments/".repeat(12)),
                mode: acs_core::OverrideMode::Once,
                granted_by: "someone.with.a.very.long.address@example.com".to_owned(),
                granted_at: now,
                expires_at: Some(now),
                seat_id: None,
            }],
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
            lore_proposals: 12,
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

    #[test]
    fn collisions_are_counted_listed_and_absent_when_there_are_none() {
        let now = Timestamp::now();
        let mut view = view(now);
        let text = render(&view, now, false);
        assert!(
            text.contains("Recent collisions (9 in the last 24 h)"),
            "{text}"
        );
        assert!(text.contains("and 4 more (`acs status --json`"), "{text}");
        view.collisions.clear();
        view.collisions_total = 0;
        let text = render(&view, now, false);
        assert!(!text.contains("collisions"), "{text}");
    }
}

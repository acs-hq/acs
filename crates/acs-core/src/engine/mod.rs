//! The decision engine: [`decide`] turns a [`GateInput`] into a [`GateOutput`].
//!
//! The engine is pure. It does no I/O, reads no clock and generates no ids: everything it needs
//! (resolved paths, shell analysis, a snapshot of the relevant claims, undelivered notices, the
//! time) arrives in the input, and everything it wants done (claims to acquire, pre-images to
//! capture, notices to mark delivered) leaves in the output.
//!
//! # Evaluation
//!
//! For a `pre_tool` event the steps of spec section 9 run in this order, and all of them run:
//!
//! 1. **Paths.** A write or delete target outside the repository, or one that cannot be resolved,
//!    asks (`path_escapes_repo`). Reads outside the repository are fine.
//! 2. **Guards.** Sealed paths (`sealed_path`), banned commands (`banned_command`), history
//!    rewrites on protected branches (`protected_branch`), deletes outside the seat's claims
//!    (`delete_outside_claims`), the per-command delete budget (`destructive_budget`) and
//!    destructive commands with unknown targets (`destructive_unknown_targets`).
//! 3. **Territory and claims.** Every written or deleted repository file in a `shared` zone, an
//!    `owned` zone (treated as `shared` until ownership is enforced) or no zone needs a file claim;
//!    `open` zones need none. A live claim of another seat (decided by
//!    [`claim_conflicts`](crate::gate::claim_conflicts), the only implementation of the conflict
//!    rule) denies with `claim_conflict`; otherwise a [`ClaimRequest`](crate::gate::ClaimRequest)
//!    is returned for each file, even for files the seat already holds, because the store treats
//!    such a request as a heartbeat.
//! 4. **Snapshots.** A [`SnapshotRequest`] for every written or deleted repository path. The engine
//!    cannot know which of them exist: the pipeline skips the ones that do not.
//! 5. **Context.** Pending notices go to `decision.context`. Lore injection (Phase 6) will be added
//!    at the marked seam in `collect_context`.
//!
//! The results combine as deny, then ask, then allow. The reason code and message come from the
//! first denying step (or, if nothing denies, the first asking step) in the order above; within
//! the guards the order is the one listed. A `deny` returns no claim or snapshot requests and
//! delivers no notices; an `ask` returns them as an `allow` would. Warnings from every step are
//! kept whatever the outcome.
//!
//! # Gate states
//!
//! - `gate_off`: only sealed paths are enforced. Everything else is allowed with `gate_off`, no
//!   claims are requested, no notices are delivered, snapshots are still requested.
//! - `state_unavailable`: no claims are checked or requested. Sealed paths, banned commands,
//!   protected branches and path escapes are still enforced. A delete, a destructive shell
//!   command or a history rewrite is denied with `state_unavailable`; anything else is allowed
//!   with that reason and a warning. Snapshots are still requested.
//!
//! # Other phases
//!
//! The engine only gates `pre_tool` events. For any other phase it returns `allow` with `ok`, no
//! requests and no context.

mod messages;
mod plan;

use std::collections::HashSet;

use self::messages::Shown;
use self::plan::Plan;
use crate::decision::{Decision, Outcome, ReasonCode};
use crate::event::{Phase, ToolKind};
use crate::gate::{
    ClaimRequest, GateInput, GateOutput, HeldClaim, PolicyView, ShellAnalysis, claim_conflicts,
};
use crate::ids::NoticeId;
use crate::records::{ClaimOrigin, ClaimScope};
use crate::values::RepoPath;

/// Decides a `pre_tool` event. See the [module documentation](self) for the evaluation order.
#[must_use]
pub fn decide(input: &GateInput<'_>, policy: &dyn PolicyView) -> GateOutput {
    if input.event.phase != Phase::PreTool {
        return GateOutput {
            decision: Decision::new(Outcome::Allow, ReasonCode::Ok, ""),
            claim_requests: Vec::new(),
            snapshot_requests: Vec::new(),
            delivered_notices: Vec::new(),
        };
    }

    let plan = Plan::build(input, policy);
    let off = input.state.gate_off;
    let unavailable = input.state.state_unavailable && !off;
    let mut verdict = Verdict::default();
    if unavailable {
        verdict.warnings.push(messages::state_unavailable_warning());
    }

    if !off {
        path_step(&plan, &mut verdict);
    }
    guards(input, policy, &plan, off, unavailable, &mut verdict);
    let claim_requests = if off || unavailable {
        Vec::new()
    } else {
        claims_step(input, policy, &plan, &mut verdict)
    };

    let Verdict {
        deny,
        ask,
        warnings,
    } = verdict;
    let (outcome, code, message) = match (deny, ask) {
        (Some((code, message)), _) => (Outcome::Deny, code, message),
        (None, Some((code, message))) => (Outcome::Ask, code, message),
        (None, None) => {
            let code = if off {
                ReasonCode::GateOff
            } else if unavailable {
                ReasonCode::StateUnavailable
            } else {
                ReasonCode::Ok
            };
            (Outcome::Allow, code, String::new())
        }
    };

    let mut decision = Decision::new(outcome, code, message);
    decision.warnings = warnings;
    if outcome == Outcome::Deny {
        return GateOutput {
            decision,
            claim_requests: Vec::new(),
            snapshot_requests: Vec::new(),
            delivered_notices: Vec::new(),
        };
    }
    let delivered_notices = if off {
        Vec::new()
    } else {
        collect_context(input, &mut decision.context)
    };
    GateOutput {
        decision,
        claim_requests,
        snapshot_requests: plan.snapshots,
        delivered_notices,
    }
}

/// The first denying and the first asking step, and the warnings of all steps.
#[derive(Default)]
struct Verdict {
    deny: Option<(ReasonCode, String)>,
    ask: Option<(ReasonCode, String)>,
    warnings: Vec<String>,
}

impl Verdict {
    fn deny(&mut self, code: ReasonCode, message: impl FnOnce() -> String) {
        if self.deny.is_none() {
            self.deny = Some((code, message()));
        }
    }

    fn ask(&mut self, code: ReasonCode, message: impl FnOnce() -> String) {
        if self.ask.is_none() {
            self.ask = Some((code, message()));
        }
    }
}

/// Step 1: targets outside the repository.
fn path_step(plan: &Plan<'_, '_>, verdict: &mut Verdict) {
    if !plan.escapes.is_empty() {
        verdict.ask(ReasonCode::PathEscapesRepo, || {
            messages::path_escapes_repo(&plan.escapes)
        });
    }
}

/// Step 2: sealed paths, banned commands, protected branches and destructive budgets. With the
/// gate off only the sealed check runs.
fn guards(
    input: &GateInput<'_>,
    policy: &dyn PolicyView,
    plan: &Plan<'_, '_>,
    off: bool,
    unavailable: bool,
    verdict: &mut Verdict,
) {
    let mut sealed = Shown::new();
    for touch in &plan.touches {
        if let Some(seal) = touch.sealed {
            sealed.push(|| messages::sealed_item(touch.path, seal));
        }
    }
    if !sealed.is_empty() {
        verdict.deny(ReasonCode::SealedPath, || messages::sealed_path(&sealed));
    }
    if off {
        return;
    }

    if let Some(shell) = input.shell {
        banned_commands(policy, shell, verdict);
        protected_branches(input, policy, shell, verdict);
    }

    if unavailable {
        if is_destructive(input, plan) {
            verdict.deny(
                ReasonCode::StateUnavailable,
                messages::state_unavailable_denied,
            );
        }
    } else {
        deletes_outside_claims(input, policy, plan, verdict);
    }
    delete_budget(policy, plan, verdict);

    if plan.unlisted_delete
        || input
            .shell
            .is_some_and(|s| s.is_destructive() && s.has_unknown_targets())
    {
        verdict.ask(
            ReasonCode::DestructiveUnknownTargets,
            messages::destructive_unknown_targets,
        );
    }
}

fn banned_commands(policy: &dyn PolicyView, shell: &ShellAnalysis, verdict: &mut Verdict) {
    for command in &shell.commands {
        let line = command.words.join(" ");
        if let Some(pattern) = policy.banned_match(&line) {
            verdict.deny(ReasonCode::BannedCommand, || {
                messages::banned_command(&line, pattern)
            });
            return;
        }
    }
}

/// A git history rewrite or force push aimed at a protected branch. A push names the branches it
/// updates (the current one when it names none); every other rewrite (`reset`, `rebase`,
/// `commit --amend`, ...) rewrites the current branch, and its refs are only what it rewrites
/// *to*, so `git rebase main` on a feature branch is fine.
fn protected_branches(
    input: &GateInput<'_>,
    policy: &dyn PolicyView,
    shell: &ShellAnalysis,
    verdict: &mut Verdict,
) {
    let current = input.seat.branch.as_deref();
    for command in &shell.commands {
        let Some(git) = command
            .git
            .as_ref()
            .filter(|g| g.rewrites_history || g.force_push)
        else {
            continue;
        };
        let names_refs = matches!(git.subcommand.as_str(), "push" | "branch" | "update-ref");
        let mut hit = None;
        if names_refs {
            hit = git
                .refs
                .iter()
                .map(|r| ref_branch(r))
                .find(|b| policy.is_protected_branch(b));
        }
        if hit.is_none() && (!names_refs || git.refs.is_empty()) {
            hit = current.filter(|b| policy.is_protected_branch(b));
        }
        if let Some(branch) = hit {
            verdict.deny(ReasonCode::ProtectedBranch, || {
                messages::protected_branch(&git.subcommand, branch, git.force_push)
            });
            return;
        }
    }
}

/// The branch a ref or refspec updates: `+src:refs/heads/dst` is `dst`.
fn ref_branch(spec: &str) -> &str {
    let spec = spec.strip_prefix('+').unwrap_or(spec);
    let spec = spec.rsplit_once(':').map_or(spec, |(_, dst)| dst);
    spec.strip_prefix("refs/heads/").unwrap_or(spec)
}

/// Whether the event deletes, moves or rewrites anything.
fn is_destructive(input: &GateInput<'_>, plan: &Plan<'_, '_>) -> bool {
    plan.any_delete
        || input
            .event
            .tool
            .as_ref()
            .is_some_and(|t| matches!(t.kind, ToolKind::Delete | ToolKind::Move))
        || input.shell.is_some_and(|s| {
            s.is_destructive()
                || s.commands.iter().any(|c| {
                    c.git
                        .as_ref()
                        .is_some_and(|g| g.rewrites_history || g.force_push)
                })
        })
}

/// Rule 4, first half: deleted files the seat did not hold a claim on before this event.
fn deletes_outside_claims(
    input: &GateInput<'_>,
    policy: &dyn PolicyView,
    plan: &Plan<'_, '_>,
    verdict: &mut Verdict,
) {
    let mut deletes = plan
        .touches
        .iter()
        .filter(|t| t.delete && !t.dir)
        .peekable();
    if deletes.peek().is_none() {
        return;
    }
    let seat = input.seat;
    let held: HashSet<&RepoPath> = input
        .claims
        .claims
        .iter()
        .filter(|h| {
            h.claim.seat_id == seat.seat_id
                && h.claim.target_branch == seat.target_branch
                && !h.claim.is_expired(input.now)
        })
        .filter_map(|h| match &h.claim.scope {
            ClaimScope::File { file } => Some(file),
            ClaimScope::Symbol { .. } => None,
        })
        .collect();
    let limit = policy.destructive().max_files_deleted_outside_claims;
    let mut outside = Shown::new();
    for touch in deletes.filter(|t| !t.open && !held.contains(t.path)) {
        outside.push(|| format!("`{}`", touch.path));
    }
    if outside.total() > limit as usize {
        verdict.deny(ReasonCode::DeleteOutsideClaims, || {
            messages::delete_outside_claims(&outside, limit)
        });
    }
}

/// Rule 4, second half: the number of files one command deletes.
fn delete_budget(policy: &dyn PolicyView, plan: &Plan<'_, '_>, verdict: &mut Verdict) {
    let count = plan.touches.iter().filter(|t| t.delete && !t.dir).count();
    let limit = policy.destructive().max_files_deleted_per_command;
    if count > limit as usize {
        verdict.ask(ReasonCode::DestructiveBudget, || {
            messages::destructive_budget(count, limit, plan.truncated_delete)
        });
    } else if plan.truncated_delete {
        // The listing stopped early although it should have gone past every budget.
        verdict.ask(
            ReasonCode::DestructiveUnknownTargets,
            messages::destructive_unknown_targets,
        );
    }
}

/// Step 3: claims. Returns the claims to acquire; denies on a conflict.
fn claims_step(
    input: &GateInput<'_>,
    policy: &dyn PolicyView,
    plan: &Plan<'_, '_>,
    verdict: &mut Verdict,
) -> Vec<ClaimRequest> {
    let seat = input.seat;
    let snapshot = input.claims;
    let ttl_secs = policy.config().claim_ttl_secs.get();
    let mut requests = Vec::new();
    let mut conflicts: Vec<(&RepoPath, Vec<&HeldClaim>)> = Vec::new();
    for touch in plan
        .touches
        .iter()
        .filter(|t| !t.dir && !t.open && t.sealed.is_none())
    {
        let scope = ClaimScope::File {
            file: touch.path.clone(),
        };
        let blocks = |held: &&HeldClaim| {
            claim_conflicts(
                held,
                &seat.seat_id,
                &scope,
                &seat.target_branch,
                input.now,
                snapshot.remote_stale,
            )
        };
        if snapshot.claims.iter().any(|h| blocks(&h)) {
            conflicts.push((touch.path, snapshot.claims.iter().filter(blocks).collect()));
        } else {
            requests.push(ClaimRequest {
                scope,
                target_branch: seat.target_branch.clone(),
                ttl_secs,
                origin: ClaimOrigin::Agent,
            });
        }
    }
    if snapshot.remote_stale && (!requests.is_empty() || !conflicts.is_empty()) {
        verdict.warnings.push(messages::remote_stale_warning());
    }
    if !conflicts.is_empty() {
        verdict.deny(ReasonCode::ClaimConflict, || {
            messages::claim_conflict(&conflicts, input.now)
        });
    }
    requests
}

/// Step 5: the context to inject. Returns the ids of the notices delivered.
fn collect_context(input: &GateInput<'_>, context: &mut Vec<String>) -> Vec<NoticeId> {
    // Lore seam (Phase 6): lore entries whose scope the seat enters for the first time in this
    // session are pushed to `context` here, before the notices, once the input carries them.
    let mut delivered = Vec::new();
    for notice in input.notices {
        if notice.delivered_at.is_none() && notice.target_seat == input.seat.seat_id {
            context.push(notice.body.clone());
            delivered.push(notice.notice_id.clone());
        }
    }
    delivered
}

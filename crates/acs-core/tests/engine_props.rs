//! Property tests of the decision engine's two safety guarantees:
//!
//! - an agent write or delete to a sealed path is never allowed or asked, whatever the gate state,
//!   the other targets, the claims and the notices;
//! - two conflicting claims are never both granted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use acs_core::engine::decide;
use acs_core::gate::{
    ClaimRequest, ClaimSource, ClaimsSnapshot, CommandClass, Confidence, HeldClaim, Target,
    TargetOp, blocking_claims,
};
use acs_core::{Claim, ClaimScope, Outcome, ReasonCode, ToolKind, ZoneKind};
use common::{
    FakePolicy, Fixture, TargetExt, analysis, command, notice, now, outside, rp, shell_target,
    write,
};
use proptest::prelude::*;

const NORMAL: &[&str] = &[
    "src/a.rs",
    "src/b.rs",
    "src/lib/c.rs",
    "docs/readme.md",
    "scratch/t.txt",
    "payments/x.rs",
];

/// Paths that [`policy`] seals: by default glob, and by a sealed zone.
const SEALED: &[&str] = &[
    ".env",
    "config/.env.local",
    ".git/hooks/pre-commit",
    ".acs/policy/guards.toml",
    ".github/workflows/ci.yml",
    "vault/keys.txt",
];

fn policy() -> FakePolicy {
    FakePolicy::default()
        .zone_of("vault/**", ZoneKind::Sealed)
        .zone_of("scratch/**", ZoneKind::Open)
        .zone_of("payments/**", ZoneKind::Owned)
        .protecting("main")
        .banning("curl * | sh")
}

fn op_of(n: u8) -> TargetOp {
    [TargetOp::Read, TargetOp::Write, TargetOp::Delete][usize::from(n % 3)]
}

fn pick(pool: &[&str], n: usize) -> String {
    pool[n % pool.len()].to_owned()
}

// ---------------------------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------------------------

/// An arbitrary target over the normal and sealed paths.
#[derive(Debug, Clone)]
struct TargetSpec {
    op: u8,
    location: u8,
    path: usize,
    recursive: bool,
    expansion: Option<(Vec<usize>, bool)>,
    linked: Vec<usize>,
}

fn target_spec() -> impl Strategy<Value = TargetSpec> {
    (
        0u8..3,
        0u8..4,
        0usize..32,
        any::<bool>(),
        proptest::option::of((prop::collection::vec(0usize..32, 0..4), any::<bool>())),
        prop::collection::vec(0usize..32, 0..3),
    )
        .prop_map(
            |(op, location, path, recursive, expansion, linked)| TargetSpec {
                op,
                location,
                path,
                recursive,
                expansion,
                linked,
            },
        )
}

fn any_path(n: usize) -> String {
    let all: Vec<&str> = NORMAL.iter().chain(SEALED).copied().collect();
    pick(&all, n)
}

fn build_target(spec: &TargetSpec) -> Target {
    let op = op_of(spec.op);
    let path = any_path(spec.path);
    let mut target = match spec.location {
        0..=1 => common::in_repo(&path, op),
        2 => outside(&format!("/outside/{path}"), op),
        _ => {
            let mut t = outside(&format!("/outside/{path}"), op);
            t.path.location = acs_core::gate::PathLocation::Unresolvable(path);
            t
        }
    };
    target.recursive = spec.recursive;
    if let Some((files, truncated)) = &spec.expansion {
        let files: Vec<String> = files.iter().map(|n| any_path(*n)).collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        target = target.expanded(&files, *truncated);
    }
    let linked: Vec<String> = spec.linked.iter().map(|n| any_path(*n)).collect();
    let linked: Vec<&str> = linked.iter().map(String::as_str).collect();
    target.linked(&linked)
}

/// An arbitrary live or expired claim in the snapshot.
#[derive(Debug, Clone)]
struct ClaimSpec {
    seat: u8,
    path: usize,
    remote: bool,
    idle_secs: i64,
    branch: u8,
    symbol: bool,
}

fn claim_spec() -> impl Strategy<Value = ClaimSpec> {
    (
        0u8..4,
        0usize..32,
        any::<bool>(),
        0i64..2000,
        0u8..2,
        any::<bool>(),
    )
        .prop_map(
            |(seat, path, remote, idle_secs, branch, symbol)| ClaimSpec {
                seat,
                path,
                remote,
                idle_secs,
                branch,
                symbol,
            },
        )
}

const SEATS: &[&str] = &["seat_me", "seat_x", "seat_y", "seat_c"];
const BRANCHES: &[&str] = &["main", "dev"];

fn build_claim(spec: &ClaimSpec) -> HeldClaim {
    let path = any_path(spec.path);
    let scope = if spec.symbol {
        ClaimScope::Symbol {
            symbol: rp(&path),
            symbol_path: "f".parse().unwrap(),
        }
    } else {
        ClaimScope::File { file: rp(&path) }
    };
    let seat = SEATS[usize::from(spec.seat) % SEATS.len()];
    common::held_scope(
        seat,
        &format!("{seat}@example.com"),
        scope,
        BRANCHES[usize::from(spec.branch) % 2],
        if spec.remote {
            ClaimSource::Remote
        } else {
            ClaimSource::Local
        },
        spec.idle_secs,
    )
}

/// Optional shell analysis: some commands with arbitrary classes and certainty.
fn shell_spec() -> impl Strategy<Value = Option<Vec<(u8, bool, bool)>>> {
    proptest::option::of(prop::collection::vec(
        (0u8..7, any::<bool>(), any::<bool>()),
        0..3,
    ))
}

const CLASSES: [CommandClass; 7] = [
    CommandClass::Read,
    CommandClass::Write,
    CommandClass::Delete,
    CommandClass::Move,
    CommandClass::GitHistoryRewrite,
    CommandClass::Network,
    CommandClass::Unknown,
];

fn apply_shell(fixture: &mut Fixture, spec: &Option<Vec<(u8, bool, bool)>>) {
    if let Some(commands) = spec {
        let commands = commands
            .iter()
            .map(|(class, known, partial)| {
                let class = CLASSES[usize::from(*class) % CLASSES.len()];
                let mut cmd = command(&["tool", "arg"], &[class]);
                cmd.targets_known = *known;
                if *partial {
                    cmd.targets = vec![shell_target("$X", TargetOp::Delete, Confidence::Partial)];
                }
                cmd
            })
            .collect();
        fixture.shell = Some(analysis(commands));
    }
}

fn apply_state(fixture: &mut Fixture, state: u8) {
    fixture.state.gate_off = state & 1 != 0;
    fixture.state.state_unavailable = state & 2 != 0;
}

// ---------------------------------------------------------------------------------------------
// (a) sealed paths are never writable
// ---------------------------------------------------------------------------------------------

/// How the sealed path reaches the event.
fn inject(sealed: &str, how: u8, op: TargetOp) -> Target {
    let op = if op == TargetOp::Read {
        TargetOp::Write
    } else {
        op
    };
    match how % 6 {
        0 => common::in_repo(sealed, op),
        1 => common::in_repo("src/glob", op).expanded(&["src/a.rs", sealed], false),
        2 => common::in_repo("some/dir", op).recursive(&["some/dir/f", sealed]),
        3 => outside("/scratch/copy", op).linked(&[sealed]),
        4 => common::in_repo("some/dir", op).linked(&[sealed]),
        _ => {
            let mut t = common::in_repo(sealed, op);
            t.recursive = true;
            t
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn sealed_paths_are_never_allowed_or_asked(
        sealed in 0usize..SEALED.len(),
        how in 0u8..6,
        delete_it in any::<bool>(),
        others in prop::collection::vec(target_spec(), 0..5),
        position in 0usize..6,
        claims in prop::collection::vec(claim_spec(), 0..6),
        remote_stale in any::<bool>(),
        state in 0u8..4,
        with_notice in any::<bool>(),
        shell in shell_spec(),
        branch in 0usize..3,
    ) {
        let op = if delete_it { TargetOp::Delete } else { TargetOp::Write };
        let mut fixture = Fixture::new(ToolKind::Edit);
        fixture.targets = others.iter().map(build_target).collect();
        let at = position.min(fixture.targets.len());
        fixture.targets.insert(at, inject(SEALED[sealed], how, op));
        fixture.claims = ClaimsSnapshot {
            claims: claims.iter().map(build_claim).collect(),
            remote_stale,
        };
        if with_notice {
            fixture.notices.push(notice("ntc_1", "seat_me", "hello"));
        }
        fixture.seat.branch = Some(["main", "dev", "feature/x"][branch].to_owned());
        apply_shell(&mut fixture, &shell);
        apply_state(&mut fixture, state);

        let out = decide(&fixture.input(), &policy());
        prop_assert_eq!(out.decision.outcome, Outcome::Deny, "{}", out.decision.message);
        prop_assert_eq!(out.decision.reason_code, ReasonCode::SealedPath);
        prop_assert!(out.claim_requests.is_empty());
        prop_assert!(out.snapshot_requests.is_empty());
        prop_assert!(out.delivered_notices.is_empty() && out.decision.context.is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// (b) conflicting claims are never both granted
// ---------------------------------------------------------------------------------------------

/// A seat's event: writes and deletes of normal paths, some expanded.
fn event_specs() -> impl Strategy<Value = Vec<TargetSpec>> {
    prop::collection::vec(
        target_spec().prop_map(|mut t| {
            // Keep the event inside the repository and away from sealed paths so that it actually
            // asks for claims.
            t.location = 0;
            t.path %= NORMAL.len();
            t.linked.clear();
            if let Some((files, _)) = &mut t.expansion {
                files.iter_mut().for_each(|f| *f %= NORMAL.len());
            }
            t
        }),
        1..5,
    )
}

fn granted(request: &ClaimRequest, seat: &str) -> HeldClaim {
    HeldClaim {
        claim: Claim {
            claim_id: "clm_granted".parse().unwrap(),
            seat_id: seat.parse().unwrap(),
            pilot: format!("{seat}@example.com"),
            scope: request.scope.clone(),
            target_branch: request.target_branch.clone(),
            acquired_at: now(),
            heartbeat_at: now(),
            ttl_secs: request.ttl_secs,
            origin: request.origin,
        },
        source: ClaimSource::Local,
        holder_harness: None,
        holder_last_seen_at: None,
    }
}

fn seat_fixture(
    id: &str,
    target_branch: &str,
    specs: &[TargetSpec],
    claims: &ClaimsSnapshot,
    state: u8,
) -> Fixture {
    let mut fixture = Fixture::new(ToolKind::Edit);
    fixture.seat = common::seat(id, &format!("{id}@example.com"), "feature/x", target_branch);
    fixture.targets = specs.iter().map(build_target).collect();
    fixture.claims = claims.clone();
    apply_state(&mut fixture, state);
    fixture
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn conflicting_claims_are_never_both_granted(
        prior in prop::collection::vec(claim_spec(), 0..8),
        remote_stale in any::<bool>(),
        a_events in event_specs(),
        b_events in event_specs(),
        a_branch in 0usize..2,
        b_branch in 0usize..2,
        a_state in 0u8..4,
        b_state in 0u8..4,
    ) {
        let policy = policy();
        let now = now();
        let s0 = ClaimsSnapshot {
            claims: prior.iter().map(build_claim).collect(),
            remote_stale,
        };

        // Seat A decides on the prior claims; its requests are granted.
        let a = seat_fixture("seat_x", BRANCHES[a_branch], &a_events, &s0, a_state);
        let out_a = decide(&a.input(), &policy);
        let a_seat = a.seat.seat_id.clone();
        for request in &out_a.claim_requests {
            let blocking =
                blocking_claims(&s0.claims, &a_seat, &request.scope, &request.target_branch, now, remote_stale);
            prop_assert!(blocking.is_empty(), "A was asked to take {:?} against {:?}", request.scope, blocking);
        }

        // Seat B decides on the claims including A's grants.
        let mut s1 = s0.clone();
        s1.claims.extend(out_a.claim_requests.iter().map(|r| granted(r, "seat_x")));
        let b = seat_fixture("seat_y", BRANCHES[b_branch], &b_events, &s1, b_state);
        let out_b = decide(&b.input(), &policy);
        let b_seat = b.seat.seat_id.clone();
        for request in &out_b.claim_requests {
            let blocking =
                blocking_claims(&s1.claims, &b_seat, &request.scope, &request.target_branch, now, remote_stale);
            prop_assert!(blocking.is_empty(), "B was asked to take {:?} against {:?}", request.scope, blocking);
            for granted_to_a in &out_a.claim_requests {
                prop_assert!(
                    !(granted_to_a.scope.overlaps(&request.scope)
                        && granted_to_a.target_branch == request.target_branch),
                    "A and B were both granted {:?}", request.scope
                );
            }
        }
    }

    #[test]
    fn deny_never_carries_requests_and_requests_are_unique(
        targets in prop::collection::vec(target_spec(), 0..6),
        claims in prop::collection::vec(claim_spec(), 0..6),
        remote_stale in any::<bool>(),
        state in 0u8..4,
        shell in shell_spec(),
    ) {
        let mut fixture = Fixture::new(ToolKind::Shell);
        fixture.targets = targets.iter().map(build_target).collect();
        fixture.claims = ClaimsSnapshot {
            claims: claims.iter().map(build_claim).collect(),
            remote_stale,
        };
        apply_shell(&mut fixture, &shell);
        apply_state(&mut fixture, state);
        let out = decide(&fixture.input(), &policy());
        if out.decision.outcome == Outcome::Deny {
            prop_assert!(out.claim_requests.is_empty() && out.snapshot_requests.is_empty());
            prop_assert!(!out.decision.message.is_empty());
        }
        if fixture.state.gate_off || fixture.state.state_unavailable {
            prop_assert!(out.claim_requests.is_empty());
        }
        let mut seen = std::collections::HashSet::new();
        for request in &out.claim_requests {
            prop_assert!(seen.insert(request.scope.clone()), "duplicate {:?}", request.scope);
        }
        prop_assert!(!matches!(
            out.decision.reason_code,
            ReasonCode::ClaimQueued | ReasonCode::UnknownPayload
        ));
        // Only deny, ask and allow are ever produced.
        prop_assert!(matches!(
            out.decision.outcome,
            Outcome::Allow | Outcome::Ask | Outcome::Deny
        ));
    }
}

#[test]
fn two_seats_racing_on_one_file_is_resolved_by_the_second_decision() {
    // The non-vacuous case behind the property: A takes the file, so B is denied.
    let policy = policy();
    let a = Fixture::new(ToolKind::Edit).with(write("src/a.rs"));
    let out_a = decide(&a.input(), &policy);
    assert_eq!(out_a.claim_requests.len(), 1);

    let mut b = Fixture::new(ToolKind::Edit).with(write("src/a.rs"));
    b.seat = common::seat("seat_b", "sara@example.com", "feature/y", "main");
    b.claims
        .claims
        .push(granted(&out_a.claim_requests[0], "seat_me"));
    let out_b = decide(&b.input(), &policy);
    assert_eq!(out_b.decision.outcome, Outcome::Deny);
    assert_eq!(out_b.decision.reason_code, ReasonCode::ClaimConflict);
    assert!(out_b.claim_requests.is_empty());
}

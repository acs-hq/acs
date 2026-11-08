//! Table-driven tests of the decision engine: every reason code it can return, precedence, claims,
//! snapshots, context and the exact agent-facing messages.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;

use acs_core::engine::decide;
use acs_core::gate::{
    ClaimRequest,
    ClaimSource::{Local, Remote},
    CommandClass, Confidence, GateOutput, SnapshotRequest, TargetOp,
};
use acs_core::{ClaimOrigin, ClaimScope, Outcome, Phase, ReasonCode, ToolKind, ZoneKind};
use common::{
    FakePolicy, Fixture, TargetExt, analysis, command, delete, git_command, held, held_scope,
    notice, outside, read, rp, shell_target, write,
};

fn run(fixture: &Fixture, policy: &FakePolicy) -> GateOutput {
    decide(&fixture.input(), policy)
}

fn rm(paths: &[&str]) -> Fixture {
    let mut cmd = command(&[&["rm"], paths].concat(), &[CommandClass::Delete]);
    cmd.targets = paths
        .iter()
        .map(|p| shell_target(p, TargetOp::Delete, Confidence::Exact))
        .collect();
    Fixture::shell(vec![cmd], paths.iter().map(|p| delete(p)).collect())
}

/// The seat's own live claim on `path`.
fn own(path: &str) -> acs_core::gate::HeldClaim {
    held("seat_me", "omar@example.com", path, Local, 10)
}

/// Another seat's live claim on `path`.
fn theirs(path: &str) -> acs_core::gate::HeldClaim {
    held("seat_sara", "sara@example.com", path, Local, 240)
}

fn file_request(path: &str) -> ClaimRequest {
    ClaimRequest {
        scope: ClaimScope::File { file: rp(path) },
        target_branch: "main".into(),
        ttl_secs: 900,
        origin: ClaimOrigin::Agent,
    }
}

fn snapshot(path: &str, recursive: bool) -> SnapshotRequest {
    SnapshotRequest {
        path: rp(path),
        recursive,
    }
}

struct Case {
    name: &'static str,
    fixture: Fixture,
    policy: FakePolicy,
    outcome: Outcome,
    code: ReasonCode,
}

fn case(
    name: &'static str,
    fixture: Fixture,
    policy: FakePolicy,
    outcome: Outcome,
    code: ReasonCode,
) -> Case {
    Case {
        name,
        fixture,
        policy,
        outcome,
        code,
    }
}

fn cases() -> Vec<Case> {
    use Outcome::{Allow, Ask, Deny};
    use ReasonCode as R;
    let plain = FakePolicy::default;
    let push_force = |refs: &[&str]| {
        let mut words = vec!["git", "push", "--force", "origin"];
        words.extend(refs);
        Fixture::shell(
            vec![git_command(&words, "push", false, true, refs)],
            Vec::new(),
        )
    };
    vec![
        // ok
        case(
            "edit a plain file",
            Fixture::edit("src/a.rs"),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "read only",
            Fixture::new(ToolKind::Read).with(read("src/a.rs")),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "read outside the repository",
            Fixture::new(ToolKind::Read).with(outside("/etc/hosts", TargetOp::Read)),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "read of a sealed path",
            Fixture::new(ToolKind::Read).with(read(".env")),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "delete of a claimed file",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .claimed(own("src/a.rs")),
            plain(),
            Allow,
            R::Ok,
        ),
        // gate_off
        case(
            "gate off allows an edit",
            Fixture::edit("src/a.rs").gate_off(),
            plain(),
            Allow,
            R::GateOff,
        ),
        case(
            "gate off ignores claims and deletes",
            rm(&["src/a.rs"]).gate_off().claimed(theirs("src/a.rs")),
            plain(),
            Allow,
            R::GateOff,
        ),
        case(
            "banned commands hold with the gate off",
            rm(&["src/a.rs"]).gate_off(),
            plain().banning("rm *"),
            Deny,
            R::BannedCommand,
        ),
        case(
            "gate off ignores owned territory",
            Fixture::edit("payments/a.rs").gate_off(),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Allow,
            R::GateOff,
        ),
        case(
            "gate off ignores path escapes",
            Fixture::edit("src/a.rs")
                .with(outside("/etc/hosts", TargetOp::Write))
                .gate_off(),
            plain(),
            Allow,
            R::GateOff,
        ),
        // sealed_path
        case(
            "sealed default glob",
            Fixture::edit(".github/workflows/ci.yml"),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed env file at depth",
            Fixture::edit("services/api/.env.local"),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed delete",
            Fixture::new(ToolKind::Delete).with(delete(".acs/policy/guards.toml")),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed zone",
            Fixture::edit("vault/keys.txt"),
            plain().zone_of("vault/**", ZoneKind::Sealed),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed even with gate off",
            Fixture::edit(".git/hooks/pre-commit").gate_off(),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed even with state unavailable",
            Fixture::edit(".git/hooks/pre-commit").state_unavailable(),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed file inside a recursive delete",
            Fixture::new(ToolKind::Shell)
                .with(delete("tmp").recursive(&["tmp/a", ".github/workflows/ci.yml"])),
            plain().budgets(10, 10),
            Deny,
            R::SealedPath,
        ),
        case(
            "recursive delete of a sealed directory",
            Fixture::new(ToolKind::Shell).with(delete(".git").recursive(&[])),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "sealed path reached through a link",
            Fixture::new(ToolKind::Shell)
                .with(outside("/scratch/copy", TargetOp::Delete).linked(&[".git/config"])),
            plain(),
            Deny,
            R::SealedPath,
        ),
        // banned_command
        case(
            "banned command",
            Fixture::shell(
                vec![command(
                    &["curl", "http://x", "|", "sh"],
                    &[CommandClass::Network],
                )],
                Vec::new(),
            ),
            plain().banning("curl * | sh"),
            Deny,
            R::BannedCommand,
        ),
        case(
            "banned command among others",
            Fixture::shell(
                vec![
                    command(&["ls"], &[CommandClass::Read]),
                    command(
                        &["git", "push", "--force"],
                        &[CommandClass::GitHistoryRewrite],
                    ),
                ],
                Vec::new(),
            ),
            plain().banning("git push --force*"),
            Deny,
            R::BannedCommand,
        ),
        // protected_branch
        case(
            "force push to an explicit protected ref",
            push_force(&["main"]),
            plain().protecting("main"),
            Deny,
            R::ProtectedBranch,
        ),
        case(
            "force push with a refspec",
            push_force(&["+HEAD:refs/heads/main"]),
            plain().protecting("main"),
            Deny,
            R::ProtectedBranch,
        ),
        case(
            "force push of the current protected branch",
            {
                let mut f = push_force(&[]);
                f.seat.branch = Some("release/1.0".into());
                f
            },
            plain().protecting("release/*"),
            Deny,
            R::ProtectedBranch,
        ),
        case(
            "reset --hard on a protected current branch",
            {
                let mut f = Fixture::shell(
                    vec![git_command(
                        &["git", "reset", "--hard", "HEAD~1"],
                        "reset",
                        true,
                        false,
                        &["HEAD~1"],
                    )],
                    Vec::new(),
                );
                f.seat.branch = Some("main".into());
                f
            },
            plain().protecting("main"),
            Deny,
            R::ProtectedBranch,
        ),
        case(
            "force push of an unprotected ref",
            push_force(&["feature/x"]),
            plain().protecting("main"),
            Allow,
            R::Ok,
        ),
        case(
            "rebase onto a protected branch from a feature branch",
            Fixture::shell(
                vec![git_command(
                    &["git", "rebase", "main"],
                    "rebase",
                    true,
                    false,
                    &["main"],
                )],
                Vec::new(),
            ),
            plain().protecting("main"),
            Allow,
            R::Ok,
        ),
        case(
            "force push to an unprotected ref while on a protected branch",
            {
                let mut f = push_force(&["feature/y"]);
                f.seat.branch = Some("main".into());
                f
            },
            plain().protecting("main"),
            Allow,
            R::Ok,
        ),
        // path_escapes_repo
        case(
            "write outside the repository",
            Fixture::new(ToolKind::Write).with(outside("/etc/hosts", TargetOp::Write)),
            plain(),
            Ask,
            R::PathEscapesRepo,
        ),
        case(
            "unresolvable write target",
            Fixture::new(ToolKind::Write).with({
                let mut t = outside("loop/a", TargetOp::Write);
                t.path.location = acs_core::gate::PathLocation::Unresolvable("loop/a".into());
                t
            }),
            plain(),
            Ask,
            R::PathEscapesRepo,
        ),
        case(
            "delete of a scratch copy that links into the repository",
            Fixture::new(ToolKind::Shell)
                .with(outside("/scratch/copy", TargetOp::Delete).linked(&["src/a.rs"]))
                .claimed(own("src/a.rs")),
            plain(),
            Ask,
            R::PathEscapesRepo,
        ),
        // delete_outside_claims
        case(
            "delete of an unclaimed file",
            rm(&["src/a.rs"]),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "delete through a link reaches unclaimed repository files",
            Fixture::new(ToolKind::Shell)
                .with(outside("/scratch/copy", TargetOp::Delete).linked(&["src/a.rs"])),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "delete of a file the seat claimed in the same event does not count",
            Fixture::new(ToolKind::Shell)
                .with(write("src/a.rs"))
                .with(delete("src/a.rs")),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "delete of a file with an expired claim",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .claimed(held("seat_me", "omar@example.com", "src/a.rs", Local, 5000)),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "a symbol claim does not cover deleting the file",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .claimed(held_scope(
                    "seat_me",
                    "omar@example.com",
                    ClaimScope::Symbol {
                        symbol: rp("src/a.rs"),
                        symbol_path: "f".parse().unwrap(),
                    },
                    "main",
                    Local,
                    10,
                )),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "delete in an open zone is not outside claims",
            Fixture::new(ToolKind::Delete).with(delete("scratch/a.txt")),
            plain().zone_of("scratch/**", ZoneKind::Open),
            Allow,
            R::Ok,
        ),
        case(
            "deletes within the outside-claims budget",
            rm(&["src/a.rs", "src/b.rs"]),
            plain().budgets(2, 50),
            Allow,
            R::Ok,
        ),
        case(
            "deletes over the outside-claims budget",
            rm(&["src/a.rs", "src/b.rs", "src/c.rs"]),
            plain().budgets(2, 50),
            Deny,
            R::DeleteOutsideClaims,
        ),
        // destructive_budget
        case(
            "deletes over the per-command budget",
            {
                let mut f = rm(&["src/a.rs", "src/b.rs", "src/c.rs"]);
                for p in ["src/a.rs", "src/b.rs", "src/c.rs"] {
                    f = f.claimed(own(p));
                }
                f
            },
            plain().budgets(0, 2),
            Ask,
            R::DestructiveBudget,
        ),
        case(
            "recursive delete over the per-command budget",
            Fixture::new(ToolKind::Shell)
                .with(delete("old").recursive(&["old/a", "old/b", "old/c"]))
                .claimed(own("old/a"))
                .claimed(own("old/b"))
                .claimed(own("old/c")),
            plain().budgets(0, 2),
            Ask,
            R::DestructiveBudget,
        ),
        // destructive_unknown_targets
        case(
            "rm of a variable",
            {
                let mut cmd = command(&["rm", "-rf", "$DIR/"], &[CommandClass::Delete]);
                cmd.targets = vec![shell_target("$DIR/", TargetOp::Delete, Confidence::Partial)];
                Fixture::shell(vec![cmd], Vec::new())
            },
            plain(),
            Ask,
            R::DestructiveUnknownTargets,
        ),
        case(
            "unknown command next to a delete",
            {
                let mut unknown = command(&["./cleanup.sh"], &[CommandClass::Unknown]);
                unknown.targets_known = false;
                Fixture::shell(
                    vec![command(&["rm", "x"], &[CommandClass::Delete]), unknown],
                    Vec::new(),
                )
            },
            plain(),
            Ask,
            R::DestructiveUnknownTargets,
        ),
        case(
            "recursive delete whose files were not listed",
            Fixture::new(ToolKind::Shell).with({
                let mut t = delete("old");
                t.recursive = true;
                t
            }),
            plain(),
            Ask,
            R::DestructiveUnknownTargets,
        ),
        case(
            "unknown command without a delete is fine",
            {
                let mut unknown = command(&["./build.sh"], &[CommandClass::Unknown]);
                unknown.targets_known = false;
                Fixture::shell(vec![unknown], Vec::new())
            },
            plain(),
            Allow,
            R::Ok,
        ),
        // claim_conflict
        case(
            "file claimed by another seat",
            Fixture::edit("src/a.rs").claimed(theirs("src/a.rs")),
            plain(),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "remote claim of another machine",
            Fixture::edit("src/a.rs").claimed(held(
                "seat_x",
                "lina@example.com",
                "src/a.rs",
                Remote,
                60,
            )),
            plain(),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "symbol claim of another seat in the file",
            Fixture::edit("src/a.rs").claimed(held_scope(
                "seat_sara",
                "sara@example.com",
                ClaimScope::Symbol {
                    symbol: rp("src/a.rs"),
                    symbol_path: "f".parse().unwrap(),
                },
                "main",
                Local,
                60,
            )),
            plain(),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "claim on another target branch is no conflict",
            Fixture::edit("src/a.rs").claimed(held_scope(
                "seat_sara",
                "sara@example.com",
                ClaimScope::File {
                    file: rp("src/a.rs"),
                },
                "experiment",
                Local,
                60,
            )),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "expired claim of another seat is no conflict",
            Fixture::edit("src/a.rs").claimed(held(
                "seat_sara",
                "sara@example.com",
                "src/a.rs",
                Local,
                901,
            )),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "stale remote claim is ignored",
            {
                let mut f = Fixture::edit("src/a.rs").claimed(held(
                    "seat_x",
                    "lina@example.com",
                    "src/a.rs",
                    Remote,
                    60,
                ));
                f.claims.remote_stale = true;
                f
            },
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "own claim is no conflict",
            Fixture::edit("src/a.rs").claimed(own("src/a.rs")),
            plain(),
            Allow,
            R::Ok,
        ),
        case(
            "second seat of the same pilot conflicts",
            Fixture::edit("src/a.rs").claimed(held(
                "seat_other",
                "omar@example.com",
                "src/a.rs",
                Local,
                60,
            )),
            plain(),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "claim conflict on a file inside a recursive delete",
            Fixture::new(ToolKind::Shell)
                .with(delete("old").recursive(&["old/a", "old/b"]))
                .claimed(own("old/a"))
                .claimed(theirs("old/b")),
            plain().budgets(1, 50),
            Deny,
            R::ClaimConflict,
        ),
        // open and owned zones
        case(
            "open zone needs no claim even when claimed by another",
            Fixture::edit("scratch/a.txt").claimed(theirs("scratch/a.txt")),
            plain().zone_of("scratch/**", ZoneKind::Open),
            Allow,
            R::Ok,
        ),
        // owned_territory
        case(
            "owned zone denies a non-owner agent",
            Fixture::edit("payments/a.rs"),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::OwnedTerritory,
        ),
        case(
            "owned zone lets an owner's agent edit",
            Fixture::edit("payments/a.rs"),
            plain().owned_by("payments/**", "Omar@Example.com"),
            Allow,
            R::Ok,
        ),
        case(
            "an owner's agent still needs the claim",
            Fixture::edit("payments/a.rs").claimed(theirs("payments/a.rs")),
            plain().owned_by("payments/**", "omar@example.com"),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "owned territory is decided before claims",
            Fixture::edit("payments/a.rs").claimed(theirs("payments/a.rs")),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::OwnedTerritory,
        ),
        case(
            "an override lets a non-owner agent edit",
            Fixture::edit("payments/a.rs").overridden("payments/**", "ovr_1"),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Allow,
            R::Ok,
        ),
        case(
            "an override does not lift a claim",
            Fixture::edit("payments/a.rs")
                .overridden("payments/**", "ovr_1")
                .claimed(theirs("payments/a.rs")),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "an override does not lift a seal",
            Fixture::edit("payments/.env").overridden("payments/**", "ovr_1"),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::SealedPath,
        ),
        case(
            "an override for other paths does not help",
            Fixture::edit("payments/a.rs").overridden("billing/**", "ovr_1"),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::OwnedTerritory,
        ),
        case(
            "humans are not bound by territory",
            Fixture::edit("payments/a.rs").human(),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Allow,
            R::Ok,
        ),
        case(
            "owned territory holds with state unavailable",
            Fixture::edit("payments/a.rs").state_unavailable(),
            plain().zone_of("payments/**", ZoneKind::Owned),
            Deny,
            R::OwnedTerritory,
        ),
        // state_unavailable
        case(
            "state unavailable allows an ordinary edit",
            Fixture::edit("src/a.rs").state_unavailable(),
            plain(),
            Allow,
            R::StateUnavailable,
        ),
        case(
            "state unavailable does not check claims",
            Fixture::edit("src/a.rs")
                .state_unavailable()
                .claimed(theirs("src/a.rs")),
            plain(),
            Allow,
            R::StateUnavailable,
        ),
        case(
            "state unavailable denies a delete",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .state_unavailable(),
            plain(),
            Deny,
            R::StateUnavailable,
        ),
        case(
            "state unavailable denies a delete even of a claimed file",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .claimed(own("src/a.rs"))
                .state_unavailable(),
            plain(),
            Deny,
            R::StateUnavailable,
        ),
        case(
            "state unavailable denies a destructive shell command",
            rm(&["src/a.rs"]).state_unavailable(),
            plain().budgets(5, 5),
            Deny,
            R::StateUnavailable,
        ),
        case(
            "state unavailable denies a history rewrite",
            Fixture::shell(
                vec![git_command(
                    &["git", "reset", "--hard"],
                    "reset",
                    true,
                    false,
                    &[],
                )],
                Vec::new(),
            )
            .state_unavailable(),
            plain(),
            Deny,
            R::StateUnavailable,
        ),
        case(
            "state unavailable still enforces banned commands",
            Fixture::shell(vec![command(&["x"], &[CommandClass::Read])], Vec::new())
                .state_unavailable(),
            plain().banning("x"),
            Deny,
            R::BannedCommand,
        ),
        case(
            "state unavailable still asks about path escapes",
            Fixture::new(ToolKind::Write)
                .with(outside("/etc/hosts", TargetOp::Write))
                .state_unavailable(),
            plain(),
            Ask,
            R::PathEscapesRepo,
        ),
        case(
            "gate off wins over state unavailable",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .gate_off()
                .state_unavailable(),
            plain(),
            Allow,
            R::GateOff,
        ),
        // precedence
        case(
            "deny beats an earlier ask: sealed after path escape",
            Fixture::new(ToolKind::Write)
                .with(outside("/etc/hosts", TargetOp::Write))
                .with(write(".env")),
            plain(),
            Deny,
            R::SealedPath,
        ),
        case(
            "deny beats an earlier ask: claim conflict after path escape",
            Fixture::new(ToolKind::Write)
                .with(outside("/etc/hosts", TargetOp::Write))
                .with(write("src/a.rs"))
                .claimed(theirs("src/a.rs")),
            plain(),
            Deny,
            R::ClaimConflict,
        ),
        case(
            "deny beats an earlier ask: delete_outside_claims after budget",
            rm(&["src/a.rs", "src/b.rs"]),
            plain().budgets(0, 1),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "first deny wins: sealed before banned",
            Fixture::shell(
                vec![command(&["touch", ".env"], &[CommandClass::Write])],
                vec![write(".env")],
            ),
            plain().banning("touch*"),
            Deny,
            R::SealedPath,
        ),
        case(
            "first deny wins: banned before protected branch",
            Fixture::shell(
                vec![git_command(
                    &["git", "push", "--force", "origin", "main"],
                    "push",
                    false,
                    true,
                    &["main"],
                )],
                Vec::new(),
            ),
            plain().banning("git push --force*").protecting("main"),
            Deny,
            R::BannedCommand,
        ),
        case(
            "first deny wins: protected branch before delete outside claims",
            {
                let mut f = Fixture::shell(
                    vec![
                        git_command(
                            &["git", "push", "--force", "origin", "main"],
                            "push",
                            false,
                            true,
                            &["main"],
                        ),
                        command(&["rm", "src/a.rs"], &[CommandClass::Delete]),
                    ],
                    vec![delete("src/a.rs")],
                );
                f.seat.branch = Some("feature/x".into());
                f
            },
            plain().protecting("main"),
            Deny,
            R::ProtectedBranch,
        ),
        case(
            "first deny wins: delete outside claims before claim conflict",
            Fixture::new(ToolKind::Delete)
                .with(delete("src/a.rs"))
                .claimed(theirs("src/a.rs")),
            plain(),
            Deny,
            R::DeleteOutsideClaims,
        ),
        case(
            "first ask wins: path escape before budget",
            Fixture::new(ToolKind::Shell)
                .with(outside("/tmp/x", TargetOp::Write))
                .with(delete("old").recursive(&["old/a", "old/b"]))
                .claimed(own("old/a"))
                .claimed(own("old/b")),
            plain().budgets(0, 1),
            Ask,
            R::PathEscapesRepo,
        ),
        case(
            "first ask wins: budget before unknown targets",
            {
                let mut f = Fixture::new(ToolKind::Shell)
                    .with(delete("old").recursive(&["old/a", "old/b"]))
                    .claimed(own("old/a"))
                    .claimed(own("old/b"));
                let mut cmd = command(&["rm", "$X"], &[CommandClass::Delete]);
                cmd.targets = vec![shell_target("$X", TargetOp::Delete, Confidence::Partial)];
                f.shell = Some(analysis(vec![cmd]));
                f
            },
            plain().budgets(0, 1),
            Ask,
            R::DestructiveBudget,
        ),
    ]
}

#[test]
fn every_case_yields_its_outcome_and_reason_code() {
    let mut seen = BTreeSet::new();
    for c in cases() {
        let out = run(&c.fixture, &c.policy);
        assert_eq!(
            (out.decision.outcome, out.decision.reason_code),
            (c.outcome, c.code),
            "{}: {}",
            c.name,
            out.decision.message
        );
        if c.outcome == Outcome::Allow {
            assert!(
                out.decision.message.is_empty(),
                "{}: message on allow",
                c.name
            );
        } else {
            assert!(
                !out.decision.message.is_empty(),
                "{}: empty message",
                c.name
            );
        }
        seen.insert(c.code);
    }
    // Every code the engine can return is covered; the others are not produced by the engine.
    let expected: BTreeSet<_> = ReasonCode::ALL
        .into_iter()
        .filter(|c| !matches!(c, ReasonCode::ClaimQueued | ReasonCode::UnknownPayload))
        .collect();
    assert_eq!(seen, expected);
}

#[test]
fn engine_never_emits_codes_reserved_for_later() {
    for c in cases() {
        let code = run(&c.fixture, &c.policy).decision.reason_code;
        assert!(
            !matches!(code, ReasonCode::ClaimQueued | ReasonCode::UnknownPayload),
            "{}",
            c.name
        );
    }
}

#[test]
fn outcomes_match_the_documented_typical_outcomes() {
    for c in cases() {
        let d = run(&c.fixture, &c.policy).decision;
        assert!(
            d.reason_code.typical_outcomes().contains(&d.outcome),
            "{}: {:?} with {}",
            c.name,
            d.outcome,
            d.reason_code
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

#[test]
fn an_edit_requests_a_claim_and_a_snapshot() {
    let out = run(&Fixture::edit("src/a.rs"), &FakePolicy::default());
    assert_eq!(out.claim_requests, vec![file_request("src/a.rs")]);
    assert_eq!(out.snapshot_requests, vec![snapshot("src/a.rs", false)]);
    assert!(out.delivered_notices.is_empty() && out.decision.context.is_empty());
}

#[test]
fn claim_request_uses_policy_ttl_and_seat_target_branch() {
    let mut policy = FakePolicy::default();
    policy.config.claim_ttl_secs = std::num::NonZeroU64::new(60).unwrap();
    let mut f = Fixture::edit("src/a.rs");
    f.seat.target_branch = "release".into();
    let out = run(&f, &policy);
    assert_eq!(out.claim_requests.len(), 1);
    assert_eq!(out.claim_requests[0].ttl_secs, 60);
    assert_eq!(out.claim_requests[0].target_branch, "release");
    assert_eq!(out.claim_requests[0].origin, ClaimOrigin::Agent);
}

#[test]
fn a_held_claim_is_requested_again_as_a_heartbeat() {
    let f = Fixture::edit("src/a.rs").claimed(own("src/a.rs"));
    assert_eq!(
        run(&f, &FakePolicy::default()).claim_requests,
        vec![file_request("src/a.rs")]
    );
}

#[test]
fn targets_are_deduplicated_and_reads_need_nothing() {
    let f = Fixture::new(ToolKind::Shell)
        .with(write("src/a.rs"))
        .with(write("src/a.rs"))
        .with(write("src/b.rs"))
        .with(read("src/c.rs"));
    let out = run(&f, &FakePolicy::default());
    assert_eq!(
        out.claim_requests,
        vec![file_request("src/a.rs"), file_request("src/b.rs")]
    );
    assert_eq!(
        out.snapshot_requests,
        vec![snapshot("src/a.rs", false), snapshot("src/b.rs", false)]
    );
}

#[test]
fn open_zone_is_snapshotted_but_not_claimed() {
    let f = Fixture::edit("scratch/a.txt");
    let out = run(
        &f,
        &FakePolicy::default().zone_of("scratch/**", ZoneKind::Open),
    );
    assert!(out.claim_requests.is_empty());
    assert_eq!(
        out.snapshot_requests,
        vec![snapshot("scratch/a.txt", false)]
    );
}

#[test]
fn glob_expansion_is_claimed_and_snapshotted_per_file() {
    let f = Fixture::new(ToolKind::Shell)
        .with(write("src/*.rs").expanded(&["src/a.rs", "src/b.rs"], false));
    let out = run(&f, &FakePolicy::default());
    assert_eq!(
        out.claim_requests,
        vec![file_request("src/a.rs"), file_request("src/b.rs")]
    );
    assert_eq!(
        out.snapshot_requests,
        vec![snapshot("src/a.rs", false), snapshot("src/b.rs", false)]
    );
}

#[test]
fn recursive_delete_claims_the_files_and_snapshots_the_directory_once() {
    let f = Fixture::new(ToolKind::Shell)
        .with(delete("old").recursive(&["old/a", "old/b"]))
        .claimed(own("old/a"))
        .claimed(own("old/b"));
    let out = run(&f, &FakePolicy::default().budgets(2, 50));
    assert_eq!(out.decision.outcome, Outcome::Allow);
    assert_eq!(
        out.claim_requests,
        vec![file_request("old/a"), file_request("old/b")]
    );
    assert_eq!(out.snapshot_requests, vec![snapshot("old", true)]);
}

#[test]
fn recursive_write_without_expansion_requests_the_path_recursively() {
    let mut target = write("dest");
    target.recursive = true;
    let out = run(
        &Fixture::new(ToolKind::Shell).with(target),
        &FakePolicy::default(),
    );
    assert_eq!(out.claim_requests, vec![file_request("dest")]);
    assert_eq!(out.snapshot_requests, vec![snapshot("dest", true)]);
}

#[test]
fn link_reached_delete_targets_are_claimed_and_snapshotted() {
    let f = Fixture::new(ToolKind::Shell)
        .with(outside("/scratch/copy", TargetOp::Delete).linked(&["src/a.rs"]))
        .claimed(own("src/a.rs"));
    let out = run(&f, &FakePolicy::default());
    assert_eq!(out.decision.reason_code, ReasonCode::PathEscapesRepo);
    assert_eq!(out.claim_requests, vec![file_request("src/a.rs")]);
    assert_eq!(out.snapshot_requests, vec![snapshot("src/a.rs", false)]);
}

#[test]
fn link_reached_paths_conflict_with_other_claims() {
    let f = Fixture::new(ToolKind::Shell)
        .with(outside("/scratch/copy", TargetOp::Delete).linked(&["src/a.rs"]))
        .claimed(theirs("src/a.rs"));
    let out = run(&f, &FakePolicy::default().budgets(1, 50));
    assert_eq!(out.decision.reason_code, ReasonCode::ClaimConflict);
}

#[test]
fn deny_drops_requests_and_notices_but_keeps_warnings() {
    let mut f = Fixture::new(ToolKind::Write)
        .with(write("src/a.rs"))
        .with(write("src/b.rs"))
        .claimed(theirs("src/b.rs"));
    f.claims.remote_stale = true;
    f.notices.push(notice("ntc_1", "seat_me", "hello"));
    let out = run(&f, &FakePolicy::default());
    assert_eq!(out.decision.outcome, Outcome::Deny);
    assert!(out.claim_requests.is_empty() && out.snapshot_requests.is_empty());
    assert!(out.delivered_notices.is_empty() && out.decision.context.is_empty());
    assert_eq!(out.decision.warnings.len(), 1);
    assert!(out.decision.warnings[0].contains("claims view is out of date"));
}

#[test]
fn ask_keeps_requests_as_for_allow() {
    let f = Fixture::new(ToolKind::Write)
        .with(outside("/etc/hosts", TargetOp::Write))
        .with(write("src/a.rs"));
    let out = run(&f, &FakePolicy::default());
    assert_eq!(out.decision.outcome, Outcome::Ask);
    assert_eq!(out.claim_requests, vec![file_request("src/a.rs")]);
    assert_eq!(out.snapshot_requests, vec![snapshot("src/a.rs", false)]);
}

#[test]
fn gate_off_and_state_unavailable_request_snapshots_but_no_claims() {
    for f in [
        Fixture::edit("src/a.rs").gate_off(),
        Fixture::edit("src/a.rs").state_unavailable(),
    ] {
        let out = run(&f, &FakePolicy::default());
        assert_eq!(out.decision.outcome, Outcome::Allow);
        assert!(out.claim_requests.is_empty());
        assert_eq!(out.snapshot_requests, vec![snapshot("src/a.rs", false)]);
    }
}

#[test]
fn state_unavailable_adds_a_warning_and_gate_off_does_not() {
    let out = run(
        &Fixture::edit("src/a.rs").state_unavailable(),
        &FakePolicy::default(),
    );
    assert_eq!(out.decision.warnings.len(), 1);
    assert!(out.decision.warnings[0].contains("acs doctor"));
    let out = run(
        &Fixture::edit("src/a.rs").gate_off(),
        &FakePolicy::default(),
    );
    assert!(out.decision.warnings.is_empty());
}

#[test]
fn stale_remote_view_warns_only_when_claims_were_checked() {
    let mut edit = Fixture::edit("src/a.rs");
    edit.claims.remote_stale = true;
    let out = run(&edit, &FakePolicy::default());
    assert_eq!(out.decision.warnings.len(), 1);
    assert_eq!(out.claim_requests.len(), 1);

    let mut reading = Fixture::new(ToolKind::Read).with(read("src/a.rs"));
    reading.claims.remote_stale = true;
    assert!(
        run(&reading, &FakePolicy::default())
            .decision
            .warnings
            .is_empty()
    );
}

#[test]
fn other_phases_are_allowed_untouched() {
    for phase in [
        Phase::PostTool,
        Phase::SessionStart,
        Phase::PromptSubmit,
        Phase::SessionEnd,
    ] {
        let mut f = Fixture::edit(".env").claimed(theirs(".env"));
        f.event.phase = phase;
        f.notices.push(notice("ntc_1", "seat_me", "hello"));
        let out = run(&f, &FakePolicy::default());
        assert_eq!(out.decision.outcome, Outcome::Allow, "{phase:?}");
        assert_eq!(out.decision.reason_code, ReasonCode::Ok);
        assert!(out.claim_requests.is_empty() && out.snapshot_requests.is_empty());
        assert!(out.delivered_notices.is_empty() && out.decision.context.is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------------

#[test]
fn pending_notices_become_context_and_are_marked_delivered() {
    let mut f = Fixture::edit("src/a.rs");
    f.notices
        .push(notice("ntc_1", "seat_me", "sara released src/b.rs"));
    f.notices
        .push(notice("ntc_2", "seat_me", "interface changed"));
    // Not for this seat, and already delivered: both are skipped.
    f.notices.push(notice("ntc_3", "seat_other", "not mine"));
    let mut done = notice("ntc_4", "seat_me", "old");
    done.delivered_at = Some(common::ago(5));
    f.notices.push(done);
    let out = run(&f, &FakePolicy::default());
    assert_eq!(
        out.decision.context,
        vec!["sara released src/b.rs", "interface changed"]
    );
    assert_eq!(
        out.delivered_notices
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>(),
        vec!["ntc_1", "ntc_2"]
    );
}

#[test]
fn notices_are_delivered_on_ask_and_with_state_unavailable_but_not_with_gate_off() {
    let with_notice = |mut f: Fixture| {
        f.notices.push(notice("ntc_1", "seat_me", "hello"));
        run(&f, &FakePolicy::default())
    };
    let ask = with_notice(Fixture::new(ToolKind::Write).with(outside("/tmp/x", TargetOp::Write)));
    assert_eq!(ask.decision.context, vec!["hello"]);
    let unavailable = with_notice(Fixture::edit("src/a.rs").state_unavailable());
    assert_eq!(unavailable.delivered_notices.len(), 1);
    let off = with_notice(Fixture::edit("src/a.rs").gate_off());
    assert!(off.decision.context.is_empty() && off.delivered_notices.is_empty());
}

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

fn message(fixture: &Fixture, policy: &FakePolicy) -> String {
    run(fixture, policy).decision.message
}

#[test]
fn claim_conflict_message_names_holder_harness_and_age() {
    let f = Fixture::edit("src/payments/refund.ts").claimed(theirs("src/payments/refund.ts"));
    assert_eq!(
        message(&f, &FakePolicy::default()),
        "`src/payments/refund.ts` is claimed by sara@example.com (claude-code, active 4 min ago). \
         Work on other files, or ask your pilot to coordinate."
    );
}

#[test]
fn claim_conflict_message_uses_last_seen_when_known() {
    let mut claim = theirs("src/a.rs");
    claim.holder_last_seen_at = Some(common::ago(7200));
    let f = Fixture::edit("src/a.rs").claimed(claim);
    assert!(message(&f, &FakePolicy::default()).contains("(claude-code, active 2 h ago)"));
}

#[test]
fn claim_conflict_message_for_remote_claims_and_fresh_activity() {
    let mut claim = held("seat_x", "lina@example.com", "src/a.rs", Remote, 5);
    claim.holder_harness = None;
    let f = Fixture::edit("src/a.rs").claimed(claim);
    assert_eq!(
        message(&f, &FakePolicy::default()),
        "`src/a.rs` is claimed by lina@example.com (another machine, active just now). \
         Work on other files, or ask your pilot to coordinate."
    );
}

#[test]
fn claim_conflict_message_with_many_paths_and_holders_stays_short() {
    let mut f = Fixture::new(ToolKind::Shell);
    for p in ["a", "b", "c", "d", "e"] {
        let path = format!("src/{p}.rs");
        f = f.with(write(&path)).claimed(theirs(&path));
    }
    f = f
        .claimed(held(
            "seat_lina",
            "lina@example.com",
            "src/a.rs",
            Local,
            600,
        ))
        .claimed(held("seat_kim", "kim@example.com", "src/a.rs", Local, 700))
        .claimed(held("seat_zed", "zed@example.com", "src/a.rs", Local, 800));
    let text = message(&f, &FakePolicy::default());
    assert_eq!(
        text,
        "`src/a.rs` is claimed by sara@example.com (claude-code, active 4 min ago), \
         lina@example.com (claude-code, active 10 min ago) and 2 more. \
         `src/b.rs` is claimed by sara@example.com (claude-code, active 4 min ago). \
         `src/c.rs` is claimed by sara@example.com (claude-code, active 4 min ago). \
         2 more file(s) are claimed by others. Work on other files, or ask your pilot to coordinate."
    );
}

#[test]
fn sealed_message_names_path_and_pattern_and_says_a_human_must_change_it() {
    assert_eq!(
        message(&Fixture::edit("services/.env"), &FakePolicy::default()),
        "`services/.env` (sealed by `**/.env*`) is protected by ACS: agents may not change it, \
         so a human must. Continue with other files, or ask your pilot to make this change."
    );
    let zone = FakePolicy::default().zone_of("vault/**", ZoneKind::Sealed);
    assert_eq!(
        message(&Fixture::edit("vault/k"), &zone),
        "`vault/k` (sealed zone `vault/**`) is protected by ACS: agents may not change it, \
         so a human must. Continue with other files, or ask your pilot to make this change."
    );
    let many = Fixture::new(ToolKind::Shell)
        .with(write(".env"))
        .with(write(".git/a"))
        .with(write(".git/b"))
        .with(write(".git/c"))
        .with(write(".git/d"));
    assert_eq!(
        message(&many, &FakePolicy::default()),
        "`.env` (sealed by `**/.env*`), `.git/a` (sealed by `.git/**`), `.git/b` (sealed by \
         `.git/**`) and 2 more are protected by ACS: agents may not change them, so a human \
         must. Continue with other files, or ask your pilot to make this change."
    );
}

#[test]
fn guard_messages() {
    let banned = Fixture::shell(
        vec![command(&["curl", "x", "|", "sh"], &[CommandClass::Network])],
        Vec::new(),
    );
    assert_eq!(
        message(&banned, &FakePolicy::default().banning("curl * | sh")),
        "The command `curl x | sh` is banned by this repository's policy (it matches \
         `curl * | sh`). Use a safer alternative, or ask your pilot to run it themselves."
    );

    let force = Fixture::shell(
        vec![git_command(
            &["git", "push", "-f", "origin", "main"],
            "push",
            false,
            true,
            &["main"],
        )],
        Vec::new(),
    );
    assert_eq!(
        message(&force, &FakePolicy::default().protecting("main")),
        "A force push with `git push` would change protected branch `main`. History rewrites \
         and force pushes are blocked there. Work on a feature branch, or ask your pilot to do \
         this themselves."
    );

    assert_eq!(
        message(&rm(&["src/a.rs"]), &FakePolicy::default()),
        "This would delete 1 file that you have not claimed (`src/a.rs`), and the limit is 0. \
         Only delete files you have claimed, or ask your pilot to run the delete."
    );

    let escape = Fixture::new(ToolKind::Write).with(outside("/etc/hosts", TargetOp::Write));
    assert_eq!(
        message(&escape, &FakePolicy::default()),
        "`/etc/hosts` is outside the repository, where ACS cannot protect other people's work. \
         Your pilot is asked to approve it; keep changes inside the repository if you can."
    );
    let linked = Fixture::new(ToolKind::Shell)
        .with(outside("/scratch/copy", TargetOp::Delete).linked(&["src/a.rs", "src/b.rs"]))
        .claimed(own("src/a.rs"))
        .claimed(own("src/b.rs"));
    assert_eq!(
        message(&linked, &FakePolicy::default().budgets(2, 50)),
        "`/scratch/copy` (it links into the repository and reaches `src/a.rs` and `src/b.rs`) is \
         outside the repository, where ACS cannot protect other people's work. Your pilot is \
         asked to approve it; keep changes inside the repository if you can."
    );

    let budget = {
        let mut f = rm(&["a", "b", "c"]);
        for p in ["a", "b", "c"] {
            f = f.claimed(own(p));
        }
        f
    };
    assert_eq!(
        message(&budget, &FakePolicy::default().budgets(0, 2)),
        "This command would delete 3 files, more than the limit of 2 per command. Your pilot is \
         asked to approve it; delete in smaller steps if you can."
    );

    let unknown = {
        let mut cmd = command(&["rm", "-rf", "$D"], &[CommandClass::Delete]);
        cmd.targets_known = false;
        Fixture::shell(vec![cmd], Vec::new())
    };
    assert_eq!(
        message(&unknown, &FakePolicy::default()),
        "This command deletes, moves or rewrites files, but ACS cannot tell which ones (a \
         variable, substitution or script hides the targets). Your pilot is asked to approve \
         it; spell out the exact paths to avoid the question."
    );

    assert_eq!(
        message(&rm(&["a"]).state_unavailable(), &FakePolicy::default()),
        "ACS cannot read its state right now, so destructive actions (deleting or moving files, \
         rewriting history) are blocked. Ordinary edits still work. Ask your pilot to run \
         `acs doctor`."
    );
}

#[test]
fn long_banned_commands_are_truncated_in_the_message() {
    let long = "x".repeat(500);
    let f = Fixture::shell(
        vec![command(&["echo", &long], &[CommandClass::Read])],
        Vec::new(),
    );
    let text = message(&f, &FakePolicy::default().banning("echo *"));
    assert!(text.len() < 400, "{}", text.len());
    assert!(text.contains("..."));
}

#[test]
fn messages_never_suggest_bypassing_acs() {
    for c in cases() {
        let message = run(&c.fixture, &c.policy).decision.message.to_lowercase();
        // Quoted spans name the command that was refused, or a command only the pilot can run.
        let text: String = message.split('`').step_by(2).collect::<Vec<_>>().join(" ");
        for forbidden in ["bypass", "acs off", "--no-verify", "disable", "override"] {
            assert!(!text.contains(forbidden), "{}: {text}", c.name);
        }
    }
}

#[test]
fn decisions_are_deterministic() {
    for c in cases() {
        assert_eq!(
            run(&c.fixture, &c.policy),
            run(&c.fixture, &c.policy),
            "{}",
            c.name
        );
    }
}

//! WP4.5 end to end: `acs stats` equals the event counts of a scripted multi-seat scenario driven
//! through `acs hook --canonical`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::process::{Command, Stdio};
use std::time::Duration;

use acs_core::{Decision, ReasonCode};
use common::{Agent, World};
use serde_json::Value;

fn stats(world: &World, args: &[&str]) -> Value {
    let out = Command::new(common::acs_bin())
        .arg("stats")
        .arg("--json")
        .args(args)
        .current_dir(&world.repo)
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], true);
    value["stats"].clone()
}

/// Counts of the reason codes the hook returned, as `acs stats` buckets them.
#[derive(Default, Debug, PartialEq, Eq)]
struct Expected {
    collisions: u64,
    destructive_blocked: u64,
    destructive_asked: u64,
    sealed: u64,
    other: u64,
}

impl Expected {
    fn see(&mut self, decision: &Decision) {
        match decision.reason_code {
            ReasonCode::ClaimConflict => self.collisions += 1,
            ReasonCode::BannedCommand
            | ReasonCode::ProtectedBranch
            | ReasonCode::DeleteOutsideClaims => self.destructive_blocked += 1,
            ReasonCode::DestructiveBudget | ReasonCode::DestructiveUnknownTargets => {
                self.destructive_asked += 1;
            }
            ReasonCode::SealedPath => self.sealed += 1,
            ReasonCode::OwnedTerritory | ReasonCode::PathEscapesRepo => self.other += 1,
            _ => {}
        }
    }
}

#[test]
fn the_numbers_equal_the_event_counts() {
    let world = World::new();
    let mut expected = Expected::default();
    let a = Agent::new(&world, "claude-code", "stats-a");
    let b = Agent::new(&world, "codex", "stats-b");
    let c = Agent::new(&world, "claude-code", "stats-c");

    a.start();
    b.start();
    c.start();
    for path in ["a.txt", "src/lib.rs"] {
        assert_eq!(a.pre_edit(path).reason_code, ReasonCode::Ok);
        world.write(path, "changed by a\n");
        a.post_edit(path);
    }
    // B runs into A's claims three times, including a retry.
    for path in ["a.txt", "a.txt", "src/lib.rs"] {
        let d = b.pre_edit(path);
        assert_eq!(d.reason_code, ReasonCode::ClaimConflict);
        expected.see(&d);
    }
    // A sealed path, and two commands the guards refuse.
    let d = b.pre_edit(".env");
    assert_eq!(d.reason_code, ReasonCode::SealedPath, "{d:?}");
    expected.see(&d);
    let d = b.pre_shell("git reset --hard HEAD~1");
    assert!(d.outcome != acs_core::Outcome::Allow, "{d:?}");
    expected.see(&d);
    let d = c.pre_shell("rm -rf $BUILD_DIR");
    assert!(d.outcome != acs_core::Outcome::Allow, "{d:?}");
    expected.see(&d);
    assert!(
        expected.destructive_blocked + expected.destructive_asked >= 2,
        "{expected:?}"
    );

    // A lore entry shown to A.
    world
        .db()
        .mark_lore_loaded(
            &a.seat_id(),
            &[
                ("lore_one".parse().unwrap(), false),
                ("lore_two".parse().unwrap(), true),
            ],
            acs_core::Timestamp::now(),
        )
        .unwrap();

    // B waits; A ends; B gets the file.
    std::thread::sleep(Duration::from_millis(1300));
    a.end();
    assert_eq!(b.pre_edit("a.txt").reason_code, ReasonCode::Ok);
    world.write("a.txt", "changed by b\n");
    b.post_edit("a.txt");
    b.end();
    c.end();

    let report = stats(&world, &[]);
    let repo = &report["repository"];
    assert_eq!(repo["sessions"], 3);
    assert_eq!(repo["collisions_prevented"], expected.collisions);
    assert_eq!(repo["destructive_blocked"], expected.destructive_blocked);
    assert_eq!(repo["destructive_asked"], expected.destructive_asked);
    assert_eq!(repo["sealed_path_denials"], expected.sealed);
    assert_eq!(repo["other_denials"], expected.other);
    assert_eq!(repo["lore_served"], 2);
    assert_eq!(
        repo["files_changed"], 3,
        "a.txt and src/lib.rs by A, a.txt by B"
    );
    assert_eq!(expected.collisions, 3);
    assert!(expected.sealed >= 1);

    let pilots = report["pilots"].as_array().unwrap();
    assert_eq!(pilots.len(), 1);
    assert_eq!(pilots[0]["pilot"], "sara@example.com");
    assert_eq!(pilots[0]["sessions"], 3);
    assert_eq!(pilots[0]["collisions_prevented"], expected.collisions);

    let machine = &report["this_machine"];
    assert_eq!(machine["scope"], "this machine");
    assert_eq!(machine["collisions"], 3);
    let waited = machine["waited_on_claims_secs"].as_i64().unwrap();
    assert!((1..30).contains(&waited), "waited {waited}s");

    // Filters.
    assert_eq!(
        stats(&world, &["--pilot", "nobody"])["repository"]["sessions"],
        0
    );
    assert_eq!(
        stats(&world, &["--since", "2099-01-01"])["repository"]["sessions"],
        0
    );
    assert_eq!(
        stats(&world, &["--since", "1h"])["repository"]["sessions"],
        3
    );
    assert_eq!(
        stats(&world, &["--pilot", "sara"])["repository"]["sessions"],
        3
    );

    // The human view fits the screen and says what it counted.
    let out = world.run(&["stats"], b"", &world.repo);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("collisions prevented") && text.contains("this machine"),
        "{text}"
    );
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
}

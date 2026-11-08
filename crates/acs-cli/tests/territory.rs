//! WP7.1 end to end: owned zones deny non-owner agents with the owner named, human overrides let
//! them in and are recorded, humans are not bound, and `acs override` refuses to run without a
//! person at a terminal (decision 0026).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use acs_core::{Outcome, OverrideMode, ReasonCode, Timestamp};
use acs_store::NewOverride;
use common::{Agent, World};
use serde_json::Value;

const TERRITORY: &str = r#"
[[zone]]
paths = ["payments/**"]
kind = "owned"
owners = ["lina@example.com"]

[[zone]]
paths = ["mine/**"]
kind = "owned"
owners = ["team:core"]

[teams]
core = ["Sara@Example.com"]
"#;

fn world() -> World {
    let world = World::new();
    world.write(".acs/policy/territory.toml", TERRITORY);
    for file in [
        "payments/a.rs",
        "payments/b.rs",
        "payments/c.rs",
        "mine/x.rs",
    ] {
        world.write(file, "x\n");
    }
    world
}

fn grant(world: &World, pattern: &str, mode: OverrideMode) -> acs_core::OverrideId {
    let now = Timestamp::now();
    world
        .db()
        .grant_override(&NewOverride {
            pattern,
            mode,
            granted_by: "sara@example.com",
            now,
            expires_at: Timestamp::from_second(now.as_second() + 900).unwrap(),
        })
        .unwrap()
}

fn json(world: &World, args: &[&str]) -> Value {
    let out = world.run(args, b"", &world.repo);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn owned_zones_deny_other_agents_and_name_the_owner() {
    let world = world();
    let agent = Agent::new(&world, "claude-code", "c1");

    let d = agent.pre_edit("payments/a.rs");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::OwnedTerritory)
    );
    assert!(d.message.contains("lina@example.com"), "{}", d.message);
    assert!(d.message.contains("payments/**"), "{}", d.message);
    assert!(agent.claimed().is_empty(), "a denial claims nothing");

    // The pilot (sara) is on the owning team of `mine/**`; team members match case-insensitively.
    let d = agent.pre_edit("mine/x.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::Ok));
    assert_eq!(agent.claimed(), ["mine/x.rs"]);

    // A shell write is judged the same way.
    let d = agent.pre_shell("echo 1 > payments/b.rs");
    assert_eq!(d.reason_code, ReasonCode::OwnedTerritory);
}

#[test]
fn a_once_override_lets_one_call_in_and_is_recorded() {
    let world = world();
    let agent = Agent::new(&world, "claude-code", "c1");
    let id = grant(&world, "payments/**", OverrideMode::Once);

    let status = json(&world, &["status", "--json"]);
    assert_eq!(status["overrides"][0]["override_id"], id.as_str());
    assert_eq!(status["overrides"][0]["mode"], "once");

    let d = agent.pre_edit("payments/a.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::Ok));
    let d = agent.pre_edit("payments/b.rs");
    assert_eq!(d.reason_code, ReasonCode::OwnedTerritory, "used up");

    let uses = world.db().override_uses_for_seat(&agent.seat_id()).unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].override_id, id);
    assert_eq!(uses[0].paths, ["payments/a.rs".parse().unwrap()]);
    assert_eq!(uses[0].granted_by, "sara@example.com");

    let status = json(&world, &["status", "--json"]);
    assert_eq!(status["overrides"], Value::Array(Vec::new()));
}

#[test]
fn a_session_override_belongs_to_the_first_seat_that_uses_it() {
    let world = world();
    let first = Agent::new(&world, "claude-code", "c1");
    let second = Agent::new(&world, "claude-code", "c2");
    grant(&world, "payments/**", OverrideMode::Session);

    assert_eq!(first.pre_edit("payments/a.rs").outcome, Outcome::Allow);
    assert_eq!(first.pre_edit("payments/b.rs").outcome, Outcome::Allow);
    let d = second.pre_edit("payments/c.rs");
    assert_eq!(d.reason_code, ReasonCode::OwnedTerritory);

    let status = json(&world, &["status", "--json"]);
    assert_eq!(
        status["overrides"][0]["seat_id"],
        first.seat_id().as_str(),
        "status shows who holds it"
    );
    first.end();
    let status = json(&world, &["status", "--json"]);
    assert_eq!(status["overrides"], Value::Array(Vec::new()));
}

#[test]
fn an_override_can_be_revoked_and_humans_are_not_bound() {
    let world = world();
    let agent = Agent::new(&world, "claude-code", "c1");
    let id = grant(&world, "payments/**", OverrideMode::Session);
    let revoked = json(&world, &["override", "--revoke", id.as_str(), "--json"]);
    assert_eq!(revoked["revoked"], true);
    assert_eq!(
        agent.pre_edit("payments/a.rs").reason_code,
        ReasonCode::OwnedTerritory
    );

    let claimed = json(&world, &["claim", "payments/c.rs", "--json"]);
    assert_eq!(claimed["ok"], true, "{claimed}");
}

#[test]
fn granting_needs_a_person_at_a_terminal() {
    let world = world();
    // Piped confirmations do not count: stdin is not a terminal.
    let out = world.run(
        &["override", "--path", "payments/**", "--once", "--json"],
        b"ABCD\nyes\n",
        &world.repo,
    );
    assert!(!out.status.success());
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "not_interactive", "{body}");
    assert!(
        world
            .db()
            .active_overrides(Timestamp::now())
            .unwrap()
            .is_empty()
    );

    let out = world.run(
        &["override", "--path", "/abs/**", "--once", "--json"],
        b"",
        &world.repo,
    );
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid", "{body}");
}

#[test]
fn with_the_gate_off_territory_lapses_but_bans_and_self_protection_hold() {
    let world = world();
    let agent = Agent::new(&world, "claude-code", "c1");
    world
        .db()
        .set_flag(acs_store::FLAG_GATE_OFF, "1", Timestamp::now())
        .unwrap();

    let d = agent.pre_edit("payments/a.rs");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Allow, ReasonCode::GateOff)
    );
    let d = agent.pre_shell("git reset --hard HEAD~1");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::BannedCommand),
        "issue 16: bans fail closed, always"
    );
    let d = agent.pre_shell("acs on");
    assert_eq!(d.outcome, Outcome::Deny);
}

#[test]
fn file_tools_cannot_write_acs_state() {
    let world = world();
    let agent = Agent::new(&world, "claude-code", "c1");
    let target = world.home.join("repos").join("x.db");
    let d = agent.pre_tool("Write", "write", &[target.to_str().unwrap()]);
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::SealedPath),
        "{}",
        d.message
    );
    assert!(d.message.contains("ACS state"), "{}", d.message);

    let elsewhere = world.tmp.path().join("notes.txt");
    let d = agent.pre_tool("Write", "write", &[elsewhere.to_str().unwrap()]);
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Allow, ReasonCode::Ok),
        "other paths outside the repository are not ACS's business by default"
    );
}

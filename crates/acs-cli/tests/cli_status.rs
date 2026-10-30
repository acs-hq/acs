//! `acs status`, `acs claim`, `acs release`, `acs off` and `acs on`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod cli_common;
mod common;

use acs_core::{DEFAULT_SEALED, Outcome, ReasonCode};
use cli_common::{
    acs, acs_in, assert_failure, assert_fits_80, assert_success, json, stderr, stdout,
};
use common::{Agent, World};
use serde_json::Value;

fn paths(view: &Value, key: &str) -> Vec<String> {
    let mut paths: Vec<String> = view[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect();
    paths.sort();
    paths
}

#[test]
fn status_shows_my_claims_and_other_agents_in_text_and_json() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    assert_eq!(agent.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    assert_success(&acs(&world, &["claim", "a.txt"]));

    let out = acs(&world, &["status", "--json"]);
    assert_success(&out);
    let view = json(&out);
    assert_eq!(view["gate_off"], false);
    assert_eq!(view["branch"], "main");
    assert_eq!(view["seat"]["harness"], "human");
    assert_eq!(view["seat"]["pilot"], "sara@example.com");
    assert_eq!(paths(&view, "my_claims"), vec!["a.txt"]);
    assert_eq!(view["my_claims"][0]["origin"], "human");
    assert_eq!(paths(&view, "other_claims"), vec!["src/lib.rs"]);
    let other = &view["other_claims"][0];
    assert_eq!(other["harness"], "claude-code");
    assert_eq!(other["pilot"], "sara@example.com");
    assert_eq!(other["source"], "local");
    assert!(
        view["sealed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == ".git/**")
    );
    assert_eq!(view["remote"]["configured"], false);
    assert!(view["policy_warnings"].as_array().unwrap().is_empty());
    assert!(view["repo_id"].is_string() && view["worktree"].is_string());

    let out = acs(&world, &["status"]);
    assert_success(&out);
    let text = stdout(&out);
    assert_fits_80(&text);
    assert!(text.contains("Your claims (1):"), "{text}");
    assert!(text.contains("Claimed by others (1):"), "{text}");
    let line = text.lines().find(|l| l.contains("src/lib.rs")).unwrap();
    assert!(
        line.contains("claude-code") && line.contains("sara@example.com"),
        "{line}"
    );
    let total = DEFAULT_SEALED.len();
    assert!(
        text.contains(&format!("Sealed paths no agent may write ({total}):")),
        "{text}"
    );
    assert!(text.contains(&format!("and {} more", total - 4)), "{text}");
    let all = stdout(&acs(&world, &["status", "--sealed"]));
    for glob in DEFAULT_SEALED {
        assert!(all.contains(glob), "{glob} missing from {all}");
    }
    assert_fits_80(&all);
}

#[test]
fn status_fits_80_columns_with_long_paths_and_names() {
    let world = World::new();
    let dirs = "deeply/nested/".repeat(8);
    let long = format!("{dirs}file_with_a_long_name.rs");
    let long2 = format!("{dirs}another_file_with_a_long_name.rs");
    world.write(&long, "x\n");
    world.write(&long2, "x\n");
    let agent = Agent::new(&world, "antigravity", "long-session");
    agent.start();
    assert_eq!(agent.pre_edit(&long).outcome, Outcome::Allow);
    assert_success(&acs(&world, &["claim", &long2]));
    let text = stdout(&acs(&world, &["status", "--sealed"]));
    assert_fits_80(&text);
    assert!(text.contains("file_with_a_long_name.rs"), "{text}");
    assert!(text.contains("another_file_with_a_long_name.rs"), "{text}");
}

#[test]
fn a_refused_claim_names_the_agent_holding_the_file() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    assert_eq!(agent.pre_edit("src/lib.rs").outcome, Outcome::Allow);

    let out = acs(&world, &["claim", "src/lib.rs"]);
    assert_failure(&out);
    assert_eq!(out.status.code(), Some(1));
    let message = stderr(&out);
    assert!(message.contains("Refused (claim_conflict)"), "{message}");
    assert!(message.contains("src/lib.rs"), "{message}");
    assert!(message.contains("claude-code"), "{message}");
    assert!(message.contains("sara@example.com"), "{message}");
    assert_fits_80(&message);

    let out = acs(&world, &["claim", "--json", "src/lib.rs"]);
    assert_failure(&out);
    let doc = json(&out);
    assert_eq!(doc["granted"], false);
    assert_eq!(doc["reason_code"], "claim_conflict");
    assert!(doc["message"].as_str().unwrap().contains("claude-code"));
    // Nothing was claimed by the refused call.
    let view = json(&acs(&world, &["status", "--json"]));
    assert!(view["my_claims"].as_array().unwrap().is_empty());
}

#[test]
fn claiming_a_sealed_path_is_refused_and_a_directory_claims_its_files() {
    let world = World::new();
    let out = acs(&world, &["claim", "--json", ".git/config"]);
    assert_failure(&out);
    assert_eq!(json(&out)["reason_code"], "sealed_path");

    let out = acs(&world, &["claim", "--json", "src"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(doc["granted"], true);
    let mut granted: Vec<&str> = doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    granted.sort_unstable();
    assert_eq!(granted, vec!["src/lib.rs", "src/util.rs"]);
    assert_eq!(doc["claim_ids"].as_array().unwrap().len(), 2);
}

#[test]
fn claims_resolve_relative_to_the_current_directory_and_block_agents() {
    let world = World::new();
    let out = acs_in(&world, &world.repo.join("src"), &["claim", "lib.rs"]);
    assert_success(&out);
    assert!(stdout(&out).contains("src/lib.rs"), "{}", stdout(&out));

    // The human's claim is judged like any other: an agent is told who holds it.
    let agent = Agent::new(&world, "antigravity", "g1");
    agent.start();
    let decision = agent.pre_edit("src/lib.rs");
    assert_eq!(decision.outcome, Outcome::Deny);
    assert_eq!(decision.reason_code, ReasonCode::ClaimConflict);
    assert!(
        decision.message.contains("sara@example.com"),
        "{}",
        decision.message
    );
    assert_eq!(agent.pre_edit("src/util.rs").outcome, Outcome::Allow);
}

#[test]
fn release_gives_claims_back_and_is_forgiving() {
    let world = World::new();
    assert_success(&acs(&world, &["claim", "a.txt", "src/lib.rs"]));

    let out = acs(&world, &["release", "a.txt"]);
    assert_success(&out);
    assert!(
        stdout(&out).contains("Released 1 file:"),
        "{}",
        stdout(&out)
    );
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    assert_eq!(agent.pre_edit("a.txt").outcome, Outcome::Allow);

    let out = acs(&world, &["release", "--json", "a.txt", "src/util.rs"]);
    assert_success(&out);
    let doc = json(&out);
    assert!(doc["released"].as_array().unwrap().is_empty());
    assert_eq!(doc["not_held"].as_array().unwrap().len(), 2);

    let out = acs(&world, &["release", "--json", "--all"]);
    assert_success(&out);
    assert_eq!(json(&out)["released"], serde_json::json!(["src/lib.rs"]));
    let view = json(&acs(&world, &["status", "--json"]));
    assert!(view["my_claims"].as_array().unwrap().is_empty());
    // The agent's own claim is not the human's to release.
    assert_eq!(paths(&view, "other_claims"), vec!["a.txt"]);

    assert_failure(&acs(&world, &["release"]));
    assert_failure(&acs(&world, &["release", "--all", "a.txt"]));
}

#[test]
fn releasing_a_path_outside_the_repository_fails() {
    let world = World::new();
    let outside = world.tmp.path().join("elsewhere.txt");
    let out = acs(&world, &["release", "--json", outside.to_str().unwrap()]);
    assert_failure(&out);
    assert_eq!(
        json(&out)["outside_repository"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn off_and_on_show_in_status_and_in_decisions() {
    let world = World::new();
    let holder = Agent::new(&world, "claude-code", "c1");
    holder.start();
    assert_eq!(holder.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    let other = Agent::new(&world, "antigravity", "g1");
    other.start();
    assert_eq!(other.pre_edit("src/lib.rs").outcome, Outcome::Deny);

    let out = acs(&world, &["off"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("OFF"), "{text}");
    assert!(text.contains("Sealed paths still hold"), "{text}");
    assert!(text.contains("acs status"), "{text}");
    assert_fits_80(&text);

    let view = json(&acs(&world, &["status", "--json"]));
    assert_eq!(view["gate_off"], true);
    let text = stdout(&acs(&world, &["status"]));
    assert!(text.contains("!! GATE OFF"), "{text}");
    assert_fits_80(&text);

    // The canonical pre_tool decision says why it allows, and sealed paths still hold.
    let decision = other.pre_edit("src/lib.rs");
    assert_eq!(decision.outcome, Outcome::Allow);
    assert_eq!(decision.reason_code, ReasonCode::GateOff);
    let sealed = other.pre_edit(".git/config");
    assert_eq!(sealed.outcome, Outcome::Deny);
    assert_eq!(sealed.reason_code, ReasonCode::SealedPath);

    let out = acs(&world, &["off", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(doc["gate_off"], true);
    assert_eq!(doc["changed"], false);

    let out = acs(&world, &["on", "--json"]);
    assert_success(&out);
    let doc = json(&out);
    assert_eq!(doc["gate_off"], false);
    assert_eq!(doc["changed"], true);
    assert_eq!(json(&acs(&world, &["status", "--json"]))["gate_off"], false);
    assert_eq!(
        other.pre_edit("src/lib.rs").reason_code,
        ReasonCode::ClaimConflict
    );
    assert!(stdout(&acs(&world, &["on"])).contains("already on"));
}

#[test]
fn commands_report_errors_as_json_objects_outside_a_repository() {
    let world = World::new();
    let plain = world.tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    for args in [
        vec!["status", "--json"],
        vec!["claim", "--json", "x"],
        vec!["release", "--json", "x"],
        vec!["off", "--json"],
        vec!["on", "--json"],
    ] {
        let out = acs_in(&world, &plain, &args);
        assert_failure(&out);
        let error = json(&out);
        assert_eq!(error["ok"], false, "{args:?}");
        assert_eq!(error["error"]["code"], "not_a_repo", "{args:?}");
        assert!(stderr(&out).is_empty(), "{args:?}: {}", stderr(&out));
    }
    let out = acs_in(&world, &plain, &["status"]);
    assert_failure(&out);
    assert!(stderr(&out).starts_with("acs: "), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

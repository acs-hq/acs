//! Team sync through the `acs` binary: a bare remote, clones that do not share `ACS_HOME`, and
//! simulated agents. No network.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

use acs_core::{Outcome, ReasonCode};
use common::git;
use common::team::{Team, wait_for};
use serde_json::{Value, json};

#[test]
fn a_claim_in_one_clone_blocks_an_edit_in_another_after_one_sync_interval() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");
    let lina = team.clone("lina", "lina@example.com");
    for clone in [&sara, &omar, &lina] {
        assert_eq!(clone.json(&["init"])["ok"], true);
    }

    // Sara's agent edits a file. Nobody ran `acs sync`: hooks start it, detached.
    sara.hook("session_start", "s1", json!({}));
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    sara.after_edit("s1", "src/lib.rs");
    omar.hook("session_start", "o1", json!({}));
    lina.hook("session_start", "l1", json!({}));

    // Omar's next post-tool events pull the claim in within an interval or two. The edit waits
    // for that: tried any earlier, it would be granted, as nothing says otherwise yet.
    wait_for("the claim to reach omar", || {
        sara.after_edit("s1", "src/lib.rs");
        omar.after_edit("o1", "src/util.rs");
        omar.sees_remote_claim("src/lib.rs")
    });
    let denied = omar.edit("o1", "src/lib.rs");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    assert_eq!(denied.reason_code, ReasonCode::ClaimConflict);
    assert!(denied.message.contains("sara@example.com"), "{denied:?}");

    // A third clone is held back the same way, and free to work elsewhere.
    assert_eq!(lina.json(&["sync"])["status"], "synced");
    assert_eq!(lina.edit("l1", "src/lib.rs").outcome, Outcome::Deny);
    assert_eq!(lina.edit("l1", "src/util.rs").outcome, Outcome::Allow);
}

#[test]
fn of_two_claims_taken_inside_one_sync_window_the_earlier_keeps_the_file() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");

    // Neither machine has heard of the other, so both are granted the file.
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(omar.edit("o1", "src/lib.rs").outcome, Outcome::Allow);

    // Each publishes its claim and fetches the other's.
    sara.json(&["sync"]);
    omar.json(&["sync"]);
    sara.json(&["sync"]);
    assert!(sara.sees_remote_claim("src/lib.rs"));
    assert!(omar.sees_remote_claim("src/lib.rs"));

    // Both machines reach the same answer on their own: no standstill.
    let kept = sara.edit("s1", "src/lib.rs");
    assert_eq!(kept.outcome, Outcome::Allow, "{kept:?}");
    let denied = omar.edit("o1", "src/lib.rs");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    assert!(denied.message.contains("sara@example.com"), "{denied:?}");

    // Omar's claim was never taken away: it holds the file once Sara's ends.
    sara.hook("session_end", "s1", json!({}));
    wait_for("the file to come to omar", || {
        omar.json(&["sync"]);
        omar.edit("o1", "src/lib.rs").outcome == Outcome::Allow
    });
}

#[test]
fn ending_a_session_publishes_the_release_without_any_further_event() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(sara.json(&["sync"])["report"]["refs_pushed"], 1);
    omar.json(&["sync"]);
    assert_eq!(omar.edit("o1", "src/lib.rs").outcome, Outcome::Deny);

    sara.hook("session_end", "s1", json!({}));
    wait_for("the release to reach omar", || {
        omar.json(&["sync"]);
        omar.edit("o1", "src/lib.rs").outcome == Outcome::Allow
    });
}

#[test]
fn a_command_line_claim_is_published_by_itself() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");
    assert!(sara.acs(&["claim", "src/lib.rs"]).status.success());
    wait_for("the human claim to reach omar", || {
        omar.json(&["sync"]);
        omar.sees_remote_claim("src/lib.rs")
    });
    let denied = omar.edit("o1", "src/lib.rs");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    let view = omar.status();
    assert_eq!(view["other_claims"][0]["origin"], "human");
}

#[test]
fn claims_on_another_integration_branch_do_not_conflict() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");
    fs::write(
        omar.repo.join(".acs/policy/config.toml"),
        "spec = \"acs/0.1\"\nsync_interval_secs = 5\ndefault_target_branch = \"develop\"\n",
    )
    .unwrap();
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    sara.json(&["sync"]);
    omar.json(&["sync"]);
    assert_eq!(omar.edit("o1", "src/lib.rs").outcome, Outcome::Allow);
}

#[test]
fn offline_and_unreachable_remotes_degrade_as_the_spec_says() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");

    // No remote at all: local-only, not an error.
    git(
        &sara.repo,
        &sara.git_config,
        &["remote", "remove", "origin"],
    );
    assert_eq!(sara.json(&["sync"])["status"], "local_only");
    assert_eq!(sara.status()["remote"]["sync_ran"], false);

    // A remote that cannot be reached: a failure that says so, and local claims still hold.
    let gone = team.base.join("gone.git");
    git(
        &sara.repo,
        &sara.git_config,
        &["remote", "add", "origin", gone.to_str().unwrap()],
    );
    let out = sara.acs(&["sync"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("still enforced"), "{text}");
    let view = sara.status();
    assert_eq!(view["remote"]["stale"], true, "{view}");
    assert!(view["remote"]["last_error"].is_string(), "{view}");

    let first = sara.edit("s1", "src/lib.rs");
    assert_eq!(first.outcome, Outcome::Allow, "stale remotes fail open");
    assert!(
        first.warnings.iter().any(|w| w.contains("out of date")),
        "{first:?}"
    );
    assert_eq!(sara.edit("s2", "src/lib.rs").outcome, Outcome::Deny);
}

#[test]
fn a_sync_needs_neither_init_nor_a_quiet_moment() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);

    // Never ran `acs init`, so the refspec is not configured; the claim gets out anyway.
    let report = sara.json(&["sync"]);
    assert_eq!(report["remote"], "origin");
    assert_eq!(report["report"]["documents_published"], 1);
    let text = String::from_utf8_lossy(&sara.acs(&["sync"]).stdout).into_owned();
    assert!(text.contains("Synced with `origin`"), "{text}");
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");

    // While the lock is held, a sync steps aside.
    let ctx =
        acs_cli::runtime::RepoContext::open_with(&sara.repo, acs_store::AcsHome::at(&sara.home))
            .unwrap();
    let lock = acs_cli::runtime::sync::lock(&ctx, false).unwrap().unwrap();
    assert_eq!(sara.json(&["sync"])["status"], "already_running");
    assert!(acs_cli::runtime::sync::lock(&ctx, false).unwrap().is_none());
    drop(lock);
    assert_eq!(sara.json(&["sync"])["status"], "synced");
}

#[test]
fn post_tool_only_syncs_once_the_interval_has_passed() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    fs::write(
        sara.repo.join(".acs/policy/config.toml"),
        "spec = \"acs/0.1\"\nsync_interval_secs = 3600\n",
    )
    .unwrap();
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    // The first event has no attempt to be measured from, so it starts one...
    sara.after_edit("s1", "src/lib.rs");
    let mut first = Value::Null;
    wait_for("the first sync", || {
        first = sara.sync_attempts();
        first.is_string()
    });
    // ...and later ones within the hour leave it alone. A sync started here would finish well
    // within this wait.
    sara.after_edit("s1", "src/lib.rs");
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(sara.sync_attempts(), first);
}

#[test]
fn hooks_publish_nothing_from_a_repository_that_never_adopted_acs() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    fs::remove_dir_all(sara.repo.join(".acs")).unwrap();

    sara.hook("session_start", "s1", json!({}));
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    sara.after_edit("s1", "src/lib.rs");
    sara.hook("session_end", "s1", json!({}));
    assert!(sara.acs(&["claim", "src/util.rs"]).status.success());

    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(sara.status()["remote"]["sync_ran"], false);
    let refs = Command::new("git")
        .arg("-C")
        .arg(&team.remote)
        .args(["for-each-ref", "refs/acs"])
        .output()
        .unwrap();
    assert!(refs.stdout.is_empty(), "{refs:?}");

    // Typing the command is an explicit choice, and works.
    assert_eq!(sara.json(&["sync"])["report"]["documents_published"], 1);
}

/// A remote that hangs for five seconds on every connection, reached through a fake ssh.
#[test]
fn a_hook_does_not_wait_for_the_sync_it_starts() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    git(
        &sara.repo,
        &sara.git_config,
        &["remote", "set-url", "origin", "ssh://example.invalid/r.git"],
    );
    git(
        &sara.repo,
        &sara.git_config,
        &["config", "core.sshCommand", "sleep 5; false"],
    );
    let started = Instant::now();
    sara.hook("session_start", "s1", json!({}));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the hook took {:?}",
        started.elapsed()
    );
}

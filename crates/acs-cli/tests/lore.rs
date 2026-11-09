//! Phase 6 end to end (decision 0029): lore is shown to an agent the first time it works in an
//! entry's scope in a session, most specific first and within the cap; stale entries ask to be
//! verified; agents can neither write lore nor accept proposals; `acs lore review` commits an
//! accepted proposal on its own branch without touching the work tree.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::process::Command;

use acs_core::{Decision, Outcome, ReasonCode, Timestamp};
use acs_lore::{Proposal, ProposalStore, Proposer};
use common::{Agent, World, git};
use serde_json::Value;

fn head(world: &World) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git_out(world: &World, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
    )
}

fn lore(id: &str, scope: &str, since: &str, body: &str) -> String {
    format!(
        "+++\nid = \"{id}\"\nscope = [\"{scope}\"]\nowner = \"lina@example.com\"\n\
         written_against = \"{since}\"\n+++\n\n{body}\n"
    )
}

fn commit_all(world: &World, message: &str) {
    git(&world.repo, &world.git_config, &["add", "-A"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", message],
    );
}

/// A world with payments code and three lore entries, all written against the commit that
/// adds the code.
fn world() -> World {
    let world = World::new();
    let ten: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    for file in [
        "payments/refund.ts",
        "payments/api/keys.ts",
        "docs/guide.md",
    ] {
        world.write(file, &ten);
    }
    commit_all(&world, "code");
    let since = head(&world);
    world.write(
        ".acs/policy/lore/payments/money.md",
        &lore(
            "lore_pay",
            "payments/**",
            &since,
            "Amounts are integer cents.",
        ),
    );
    world.write(
        ".acs/policy/lore/payments/api/keys.md",
        &lore(
            "lore_api",
            "payments/api/**",
            &since,
            "Every call takes an idempotency key.",
        ),
    );
    world.write(
        ".acs/policy/lore/docs.md",
        &lore(
            "lore_docs",
            "docs/**",
            &since,
            "Plain English, short sentences.",
        ),
    );
    world.write(".acs/policy/lore/README.md", "How to write lore.");
    commit_all(&world, "lore");
    world
}

fn lore_ids(decision: &Decision) -> Vec<String> {
    decision
        .context
        .iter()
        .filter(|c| c.starts_with("ACS lore for "))
        .map(|c| {
            let start = c.find("(lore_").unwrap() + 1;
            let end = start + c[start..].find(',').unwrap();
            c[start..end].to_owned()
        })
        .collect()
}

#[test]
fn lore_comes_once_per_session_most_specific_first() {
    let world = world();
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    sara.start();

    // The first read in the scope brings both payments entries, the narrower one first.
    let read = sara.pre_tool("Read", "read", &["payments/api/keys.ts"]);
    assert_eq!(read.outcome, Outcome::Allow);
    assert_eq!(
        lore_ids(&read),
        ["lore_api", "lore_pay"],
        "{:?}",
        read.context
    );
    assert!(
        read.context[0].contains("Every call takes an idempotency key."),
        "{:?}",
        read.context
    );
    assert!(read.context[0].contains("owner lina@example.com"));

    // Not again in this session, whatever the tool or file in the scope.
    assert!(lore_ids(&sara.pre_edit("payments/api/keys.ts")).is_empty());
    assert!(lore_ids(&sara.pre_edit("payments/refund.ts")).is_empty());
    assert!(lore_ids(&sara.pre_tool("Read", "read", &["src/lib.rs"])).is_empty());
    // Another scope brings its own entry.
    assert_eq!(lore_ids(&sara.pre_edit("docs/guide.md")), ["lore_docs"]);

    // Another session is shown it afresh (a read: Sara holds the file).
    let omar = Agent::new(&world, "claude-code", "cc-omar");
    assert_eq!(
        lore_ids(&omar.pre_tool("Read", "read", &["payments/refund.ts"])),
        ["lore_pay"]
    );

    // Recorded per seat for session records.
    let loaded = world.db().lore_loaded_for_seat(&sara.seat_id()).unwrap();
    let ids: Vec<&str> = loaded.iter().map(|l| l.lore_id.as_str()).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"lore_api") && ids.contains(&"lore_docs"));
    assert!(loaded.iter().all(|l| !l.stale));
}

#[test]
fn shell_commands_bring_lore_for_the_files_they_name() {
    let world = world();
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    let decision = sara.pre_shell("cat docs/guide.md");
    assert_eq!(lore_ids(&decision), ["lore_docs"], "{:?}", decision.context);
}

#[test]
fn a_denied_call_brings_no_lore_and_the_next_allowed_one_does() {
    let world = world();
    let lina = Agent::new(&world, "claude-code", "cc-lina");
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    assert_eq!(lina.pre_edit("payments/refund.ts").outcome, Outcome::Allow);
    let denied = sara.pre_edit("payments/refund.ts");
    assert_eq!(denied.reason_code, ReasonCode::ClaimConflict);
    assert!(lore_ids(&denied).is_empty());
    assert_eq!(
        lore_ids(&sara.pre_tool("Read", "read", &["payments/refund.ts"])),
        ["lore_pay"]
    );
}

#[test]
fn the_cap_holds_back_what_does_not_fit_until_the_next_call() {
    let world = world();
    world.write(
        ".acs/policy/config.toml",
        "spec = \"acs/0.1\"\n\n[lore]\nmax_inject_chars = 120\n",
    );
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    let first = sara.pre_tool("Read", "read", &["payments/api/keys.ts"]);
    assert_eq!(lore_ids(&first), ["lore_api"], "{:?}", first.context);
    let shown: usize = first.context.iter().map(|c| c.chars().count()).sum();
    assert!(first.context[0].contains("1 more lore entry applies here did not fit"));
    // The note about what is held back is the only text over the cap.
    assert!(shown < 120 + 200, "{shown}");
    let second = sara.pre_tool("Read", "read", &["payments/api/keys.ts"]);
    assert_eq!(lore_ids(&second), ["lore_pay"]);
    assert!(lore_ids(&sara.pre_tool("Read", "read", &["payments/api/keys.ts"])).is_empty());
}

#[test]
fn stale_entries_ask_to_be_verified() {
    let world = world();
    // Rewrite most of refund.ts: well over 30% of the lines under payments/ change.
    world.write("payments/refund.ts", "rewritten 1\nrewritten 2\n");
    world.write(
        ".acs/policy/lore/old.md",
        &lore(
            "lore_old",
            "docs/**",
            "0123456789ab",
            "Written somewhere else.",
        ),
    );
    commit_all(&world, "rewrite");
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    let decision = sara.pre_edit("payments/refund.ts");
    let pay = decision
        .context
        .iter()
        .find(|c| c.contains("lore_pay"))
        .unwrap();
    assert!(pay.contains("may be out of date"), "{pay}");
    assert!(pay.contains("of the lines in its scope changed"), "{pay}");
    let docs = sara.pre_edit("docs/guide.md");
    let old = docs
        .context
        .iter()
        .find(|c| c.contains("lore_old"))
        .unwrap();
    assert!(old.contains("which this clone does not have"), "{old}");
    let fresh = docs
        .context
        .iter()
        .find(|c| c.contains("lore_docs"))
        .unwrap();
    assert!(!fresh.contains("out of date"), "{fresh}");
    let loaded = world.db().lore_loaded_for_seat(&sara.seat_id()).unwrap();
    assert!(
        loaded
            .iter()
            .find(|l| l.lore_id.as_str() == "lore_pay")
            .unwrap()
            .stale
    );

    // `acs lore stale` lists the same two.
    let out = world.run(&["lore", "stale", "--json"], b"", &world.repo);
    let stale: Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut ids: Vec<&str> = stale["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["lore_old", "lore_pay"]);
    assert_eq!(stale["threshold_percent"], 30);
}

#[test]
fn agents_cannot_write_lore_or_accept_proposals() {
    let world = world();
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    let edit = sara.pre_edit(".acs/policy/lore/payments/money.md");
    assert_eq!(edit.outcome, Outcome::Deny);
    assert_eq!(edit.reason_code, ReasonCode::SealedPath);
    let write = sara.pre_tool("Write", "write", &[".acs/policy/lore/new.md"]);
    assert_eq!(write.outcome, Outcome::Deny);
    let shell = sara.pre_shell("echo x > .acs/policy/lore/new.md");
    assert_eq!(shell.outcome, Outcome::Deny);

    let review = sara.pre_shell("acs lore review --accept prop_abc");
    assert_eq!(review.outcome, Outcome::Deny);
    assert_eq!(review.reason_code, ReasonCode::BannedCommand);
    assert!(
        review.message.contains("acs_lore_propose"),
        "{}",
        review.message
    );
    for read_only in [
        "acs lore list",
        "acs lore show payments/refund.ts",
        "acs lore stale",
    ] {
        assert_eq!(
            sara.pre_shell(read_only).outcome,
            Outcome::Allow,
            "{read_only}"
        );
    }
}

#[test]
fn broken_lore_files_are_listed_and_do_not_stop_the_others() {
    let world = world();
    world.write(".acs/policy/lore/broken.md", "no front matter");
    let sara = Agent::new(&world, "claude-code", "cc-sara");
    assert_eq!(lore_ids(&sara.pre_edit("docs/guide.md")), ["lore_docs"]);
    let out = world.run(&["lore", "list", "--json"], b"", &world.repo);
    let list: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["entries"].as_array().unwrap().len(), 3);
    assert_eq!(list["problems"][0]["file"], ".acs/policy/lore/broken.md");
    let text = world.run(&["lore", "list"], b"", &world.repo);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("3 lore entries:"), "{text}");
    assert!(text.contains("broken.md"), "{text}");
    assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
}

#[test]
fn lore_show_prints_what_an_agent_gets() {
    let world = world();
    let out = world.run(&["lore", "show", "payments/api/keys.ts"], b"", &world.repo);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let api = text.find("lore_api").unwrap();
    let pay = text.find("lore_pay").unwrap();
    assert!(api < pay, "{text}");
    let out = world.run(&["lore", "show", "src/lib.rs", "--json"], b"", &world.repo);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["entries"].as_array().unwrap().is_empty());
    // Nothing was marked as shown: `show` is for people.
    assert!(
        world
            .db()
            .live_seats()
            .unwrap()
            .iter()
            .all(|s| s.seat.harness == "human")
    );
}

fn propose(world: &World, id: &str, scope: &str, body: &str, replaces: Option<&str>) {
    let store = ProposalStore::new(world.repo_home().pending_dir().join("lore"));
    store
        .save(&Proposal {
            proposal_id: id.to_owned(),
            replaces: replaces.map(|r| r.parse().unwrap()),
            scope: vec![scope.to_owned()],
            body: body.to_owned(),
            why: Some("found while fixing refunds".to_owned()),
            proposed_by: Proposer {
                pilot: "omar@example.com".to_owned(),
                harness: "claude-code".to_owned(),
                session_id: "ses_test".to_owned(),
            },
            proposed_at: Timestamp::now(),
            head: None,
        })
        .unwrap();
}

fn run_json(world: &World, args: &[&str]) -> Value {
    let out = world.run(args, b"", &world.repo);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn review_accepts_onto_a_branch_and_leaves_the_work_tree_alone() {
    let world = world();
    world.write(
        ".acs/policy/territory.toml",
        "[[zone]]\npaths = [\"src/**\"]\nkind = \"owned\"\nowners = [\"lina@example.com\"]\n",
    );
    commit_all(&world, "territory");
    world.write("src/lib.rs", "uncommitted edit\n");
    let before = head(&world);
    propose(&world, "prop_new", "src/**", "Errors use thiserror.", None);
    propose(
        &world,
        "prop_fix",
        "payments/**",
        "Amounts are integer cents, always i64.",
        Some("lore_pay"),
    );
    propose(&world, "prop_bad", "x/**", "Nonsense.", None);

    let listed = run_json(&world, &["lore", "review", "--list", "--json"]);
    assert_eq!(listed["proposals"].as_array().unwrap().len(), 3);

    let accepted = run_json(
        &world,
        &["lore", "review", "--accept", "prop_new", "--json"],
    );
    assert_eq!(accepted["ok"], true, "{accepted}");
    let lore_id = accepted["lore_id"].as_str().unwrap();
    let branch = accepted["branch"].as_str().unwrap();
    let file = accepted["file"].as_str().unwrap();
    assert_eq!(branch, format!("acs/lore-{}", &lore_id[5..]));
    assert_eq!(file, format!(".acs/policy/lore/src/{}.md", &lore_id[5..]));
    // The zone's owner is written in; Sara (the reviewer) is told she is not the owner.
    assert_eq!(accepted["owner"], "lina@example.com");
    assert!(accepted["note"].as_str().unwrap().contains("not you"));

    let (ok, text) = git_out(&world, &["show", &format!("{branch}:{file}")]);
    assert!(ok, "{text}");
    let entry = acs_lore::LoreEntry::parse(file, &text, false).unwrap();
    assert_eq!(entry.body, "Errors use thiserror.");
    assert_eq!(entry.front.written_against, before);
    let (_, message) = git_out(&world, &["log", "-1", "--format=%B", branch]);
    assert!(
        message.contains("Proposed by omar@example.com"),
        "{message}"
    );
    assert!(
        message.contains("Why: found while fixing refunds"),
        "{message}"
    );

    // HEAD, the index and the uncommitted edit are untouched.
    assert_eq!(head(&world), before);
    let (_, status) = git_out(&world, &["status", "--porcelain"]);
    assert_eq!(status, "M src/lib.rs");

    // A correction replaces the entry in place, keeping its id and owner, with edited text.
    let edited = world.tmp.path().join("edited.md");
    std::fs::write(&edited, "Amounts are i64 cents.\n").unwrap();
    let fixed = run_json(
        &world,
        &[
            "lore",
            "review",
            "--accept",
            "prop_fix",
            "--body-file",
            edited.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(fixed["lore_id"], "lore_pay");
    assert_eq!(fixed["branch"], "acs/lore-pay");
    assert_eq!(fixed["file"], ".acs/policy/lore/payments/money.md");
    let (_, text) = git_out(
        &world,
        &["show", "acs/lore-pay:.acs/policy/lore/payments/money.md"],
    );
    assert!(text.contains("Amounts are i64 cents."), "{text}");
    assert!(text.contains("owner = \"lina@example.com\""), "{text}");

    // Rejecting deletes; unknown ids are errors.
    let rejected = run_json(
        &world,
        &["lore", "review", "--reject", "prop_bad", "--json"],
    );
    assert_eq!(rejected["rejected"], "prop_bad");
    let missing = run_json(
        &world,
        &["lore", "review", "--accept", "prop_bad", "--json"],
    );
    assert_eq!(missing["error"]["code"], "not_found");
    let listed = run_json(&world, &["lore", "review", "--list", "--json"]);
    assert!(listed["proposals"].as_array().unwrap().is_empty());

    // Without a terminal, the interactive review explains the flags.
    propose(&world, "prop_more", "src/**", "More.", None);
    let out = world.run(&["lore", "review"], b"", &world.repo);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--accept"));
}

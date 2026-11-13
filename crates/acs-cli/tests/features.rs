//! The feature switches of `config.toml`'s `[features]` table, end to end (decision 0033): each
//! switch turns its part of ACS off and leaves the others working, ACS's protection of itself
//! stays on whatever is off, and only a person can flip a switch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use acs_core::{Outcome, ReasonCode, RepoPath};
use common::{Agent, World, acs_bin, git};
use serde_json::Value;

/// A world whose policy switches `features` (lines of the `[features]` table).
fn world_with(features: &str) -> World {
    let world = World::new();
    world.write(
        ".acs/policy/config.toml",
        &format!("spec = \"acs/0.1\"\n\n[features]\n{features}\n"),
    );
    world
}

fn json(world: &World, args: &[&str]) -> Value {
    let out = world.run(args, b"", &world.repo);
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn git_with_hooks(world: &World, args: &[&str]) -> Output {
    let acs_dir = Path::new(acs_bin()).parent().unwrap().to_path_buf();
    let mut paths = vec![acs_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(args)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CLAUDECODE")
        .env_remove("ANTIGRAVITY_CONVERSATION_ID")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap()
}

fn install_hooks(world: &World) {
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
}

fn head_message(world: &World) -> String {
    let out = git_with_hooks(world, &["log", "-1", "--format=%B"]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn allowed(decision: &acs_core::Decision) -> bool {
    decision.outcome == Outcome::Allow
}

#[test]
fn everything_but_sync_is_on_by_default() {
    let world = World::new();
    world.write(".acs/policy/config.toml", "spec = \"acs/0.1\"\n");
    let list = json(&world, &["features", "--json"]);
    let states: Vec<(String, bool)> = list["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["feature"].as_str().unwrap().to_owned(),
                f["on"].as_bool().unwrap(),
            )
        })
        .collect();
    let off: Vec<&str> = states
        .iter()
        .filter(|(_, on)| !on)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(states.len(), 7);
    assert_eq!(off, ["sync"]);
    let status = json(&world, &["status", "--json"]);
    assert_eq!(status["features_off"], serde_json::json!(["sync"]));
    assert_eq!(status["remote"]["enabled"], false);
}

#[test]
fn claims_off_blocks_nobody_and_claims_nothing() {
    let world = world_with("claims = false");
    let first = Agent::new(&world, "claude-code", "c1");
    let second = Agent::new(&world, "claude-code", "c2");
    let context = first.start().context.join("\n");
    assert!(context.contains("claims switched off"), "{context}");
    assert!(!context.contains("You hold"), "{context}");

    let d = first.pre_edit("src/lib.rs");
    assert_eq!((d.outcome, d.reason_code), (Outcome::Allow, ReasonCode::Ok));
    assert!(d.claims_acquired.is_empty());
    assert!(first.claimed().is_empty());
    assert!(allowed(&second.pre_edit("src/lib.rs")));

    // The rule about deleting files outside one's claims is a claims rule.
    assert!(allowed(&first.pre_shell("rm a.txt")));

    let refused = world.run(&["claim", "src/lib.rs"], b"", &world.repo);
    assert!(!refused.status.success());
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("Claims are switched off"), "{text}");

    // The other features are untouched: sealed paths and bans still hold.
    let d = first.pre_edit(".env");
    assert_eq!(d.reason_code, ReasonCode::SealedPath);
    let d = first.pre_shell("git push --force origin main");
    assert_eq!(d.reason_code, ReasonCode::BannedCommand);
}

#[test]
fn guards_off_keeps_only_acs_own_protection() {
    let world = world_with("guards = false");
    world.write(
        ".acs/policy/guards.toml",
        "sealed = [\"secrets/**\"]\nprotected_branches = [\"main\"]\n",
    );
    world.write("secrets/key.txt", "k\n");
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();

    for path in [".env", "secrets/key.txt", ".github/workflows/ci.yml"] {
        assert!(allowed(&agent.pre_edit(path)), "{path}");
    }
    for command in [
        "git push --force origin main",
        "git reset --hard HEAD~1",
        "rm -rf src",
    ] {
        let d = agent.pre_shell(command);
        assert!(allowed(&d), "{command}: {}", d.message);
    }

    // Never off: ACS's own files, its commands, and the git hooks.
    for path in [
        ".acs/policy/config.toml",
        ".git/hooks/pre-commit",
        ".claude/settings.json",
    ] {
        let d = agent.pre_edit(path);
        assert_eq!(
            (d.outcome, d.reason_code),
            (Outcome::Deny, ReasonCode::SealedPath),
            "{path}"
        );
    }
    for command in [
        "acs off",
        "acs features guards on",
        "git commit --no-verify -m x",
        "echo x > .acs/policy/config.toml",
    ] {
        let d = agent.pre_shell(command);
        assert_eq!(d.outcome, Outcome::Deny, "{command}");
    }

    // Claims still work.
    let other = Agent::new(&world, "claude-code", "c2");
    assert_eq!(agent.pre_edit("src/lib.rs").claims_acquired.len(), 1);
    assert_eq!(
        other.pre_edit("src/lib.rs").reason_code,
        ReasonCode::ClaimConflict
    );
}

#[test]
fn territory_off_makes_every_file_shared() {
    let world = world_with("territory = false");
    world.write(
        ".acs/policy/territory.toml",
        "[[zone]]\npaths = [\"payments/**\"]\nkind = \"owned\"\nowners = [\"lina@example.com\"]\n\n\
         [[zone]]\npaths = [\"vault/**\"]\nkind = \"sealed\"\n\n\
         [[zone]]\npaths = [\"scratch/**\"]\nkind = \"open\"\n",
    );
    for file in ["payments/a.rs", "vault/v.txt", "scratch/s.txt"] {
        world.write(file, "x\n");
    }
    let agent = Agent::new(&world, "claude-code", "c1");
    for file in ["payments/a.rs", "vault/v.txt", "scratch/s.txt"] {
        let d = agent.pre_edit(file);
        assert!(allowed(&d), "{file}: {}", d.message);
    }
    // With no zones an `open` zone's file needs a claim like any other.
    assert_eq!(
        agent.claimed(),
        ["payments/a.rs", "scratch/s.txt", "vault/v.txt"]
    );
    let out = world.run(
        &["override", "--path", "payments/**", "--once", "--json"],
        b"",
        &world.repo,
    );
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "feature_off", "{reply}");
}

#[test]
fn lore_off_shows_nothing_and_takes_no_proposals() {
    let entry = "+++\nid = \"lore_refunds\"\nscope = [\"src/**\"]\nowner = \"lina@example.com\"\n\
                 written_against = \"HEAD\"\n+++\n\nRefunds are idempotent.\n";
    let on = World::new();
    on.write(".acs/policy/config.toml", "spec = \"acs/0.1\"\n");
    on.write(".acs/policy/lore/refunds.md", entry);
    let d = Agent::new(&on, "claude-code", "c1").pre_edit("src/lib.rs");
    assert!(d.context.join("\n").contains("Refunds are idempotent"));

    let off = world_with("lore = false");
    off.write(".acs/policy/lore/refunds.md", entry);
    let d = Agent::new(&off, "claude-code", "c1").pre_edit("src/lib.rs");
    assert!(allowed(&d));
    assert!(!d.context.join("\n").contains("Refunds"), "{:?}", d.context);
}

#[test]
fn history_off_keeps_no_copies_and_adds_no_trailer() {
    let run = |features: &str| {
        let world = world_with(features);
        install_hooks(&world);
        let agent = Agent::new(&world, "claude-code", "c1");
        agent.start();
        assert!(allowed(&agent.pre_edit("src/lib.rs")));
        world.write("src/lib.rs", "pub fn changed() {}\n");
        agent.post_edit("src/lib.rs");
        let path: RepoPath = "src/lib.rs".parse().unwrap();
        let attributed = !world.db().path_attributions(&path).unwrap().is_empty();
        let session = agent.info().session_id;
        let has_copy = world
            .snapshots()
            .pre_image(&session, &path)
            .unwrap()
            .is_some();
        agent.pre_shell("git commit -am work");
        let out = git_with_hooks(&world, &["commit", "-q", "-am", "work"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        (
            attributed,
            has_copy,
            head_message(&world).contains("ACS-Session"),
        )
    };
    assert_eq!(run("history = true"), (true, true, true));
    assert_eq!(run("history = false"), (false, false, false));
}

#[test]
fn commit_checks_off_lets_every_commit_through() {
    let run = |features: &str| {
        let world = world_with(features);
        install_hooks(&world);
        let holder = Agent::new(&world, "claude-code", "c1");
        let committer = Agent::new(&world, "antigravity", "g1");
        holder.start();
        committer.start();
        assert_eq!(holder.pre_edit("src/lib.rs").claims_acquired.len(), 1);
        committer.pre_shell("git commit -m work");
        world.write("src/lib.rs", "pub fn changed() {}\n");
        git(&world.repo, &world.git_config, &["add", "src/lib.rs"]);
        git_with_hooks(&world, &["commit", "-q", "-m", "work"])
            .status
            .success()
    };
    assert!(
        !run("commit_checks = true"),
        "rejected while the check is on"
    );
    assert!(run("commit_checks = false"));
}

#[test]
fn sync_is_off_until_a_person_switches_it_on() {
    let world = World::new();
    let remote = world.repo.parent().unwrap().join("remote.git");
    git(
        world.repo.parent().unwrap(),
        &world.git_config,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &world.repo,
        &world.git_config,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let fetch_specs = || {
        let out = Command::new("git")
            .arg("-C")
            .arg(&world.repo)
            .args(["config", "--get-all", "remote.origin.fetch"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let init = json(&world, &["init", "--json"]);
    assert_eq!(init["features_off"], serde_json::json!(["sync"]));
    assert_eq!(init["remotes"], serde_json::json!([]));
    assert!(!fetch_specs().contains("refs/acs"), "{}", fetch_specs());
    let config = fs::read_to_string(world.repo.join(".acs/policy/config.toml")).unwrap();
    assert!(config.contains("[features]\n"), "{config}");
    assert!(config.contains("sync = false"), "{config}");

    let synced = json(&world, &["sync", "--json"]);
    assert_eq!(synced["status"], "sync_off");
    let refs = Command::new("git")
        .arg("-C")
        .arg(&remote)
        .args(["for-each-ref", "refs/acs"])
        .output()
        .unwrap();
    assert!(refs.stdout.is_empty(), "nothing was published");

    // An agent cannot switch it on; a person can, and the comments of the file survive.
    let agent = Agent::new(&world, "claude-code", "c1");
    assert_eq!(
        agent.pre_shell("acs features sync on").outcome,
        Outcome::Deny
    );
    assert!(allowed(&agent.pre_shell("acs features")));
    let switched = json(&world, &["features", "sync", "on", "--json"]);
    assert_eq!(switched["changed"], true, "{switched}");
    let edited = fs::read_to_string(world.repo.join(".acs/policy/config.toml")).unwrap();
    assert_eq!(edited, config.replace("sync = false", "sync = true"));
    assert!(fetch_specs().contains("refs/acs"), "{}", fetch_specs());
    assert_eq!(
        json(&world, &["status", "--json"])["remote"]["enabled"],
        true
    );
    let synced = json(&world, &["sync", "--wait", "--json"]);
    assert_eq!(synced["status"], "synced", "{synced}");

    let again = json(&world, &["features", "sync", "on", "--json"]);
    assert_eq!(again["changed"], false);
    let unknown = world.run(&["features", "telepathy", "on"], b"", &world.repo);
    assert!(!unknown.status.success());
}

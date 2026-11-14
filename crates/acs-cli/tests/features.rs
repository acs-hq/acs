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

/// Decision 0034: a denial points at a tool only where that tool is listed.
#[test]
fn denials_name_no_tool_whose_feature_is_off() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    let d = agent.pre_shell("acs claim src/lib.rs");
    assert_eq!(d.outcome, Outcome::Deny);
    assert!(d.message.contains("`acs_claim`"), "{}", d.message);
    let d = agent.pre_shell("acs lore review --list");
    assert_eq!(d.outcome, Outcome::Deny);
    assert!(d.message.contains("`acs_lore_propose`"), "{}", d.message);

    let world = world_with("claims = false\nlore = false");
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    let d = agent.pre_shell("acs claim src/lib.rs");
    assert_eq!(d.outcome, Outcome::Deny);
    assert!(!d.message.contains("acs_claim"), "{}", d.message);
    assert!(
        d.message.contains("Claims are switched off"),
        "{}",
        d.message
    );
    let d = agent.pre_shell("acs lore review --list");
    assert_eq!(d.outcome, Outcome::Deny);
    assert!(!d.message.contains("acs_lore_propose"), "{}", d.message);
    assert!(d.message.contains("lore is switched off"), "{}", d.message);
}

// Decision 0035: a tool event that no switched-on feature needs ends early.

fn read_event(agent: &Agent<'_>, phase: &str) -> acs_core::Decision {
    agent.send(
        phase,
        serde_json::json!({"tool": {"name": "Read", "kind": "read"}, "paths": ["src/lib.rs"]}),
    )
}

/// How many events of `phase` the seat of `agent` has recorded.
fn recorded(agent: &Agent<'_>, phase: acs_core::Phase) -> usize {
    let events = agent.world.db().seat_events(&agent.seat_id()).unwrap();
    events.iter().filter(|e| e.phase == phase).count()
}

fn has_seat(agent: &Agent<'_>) -> bool {
    let db = agent.world.db();
    db.seat_by_session(agent.harness, &agent.session)
        .unwrap()
        .is_some()
}

fn tool_markers(world: &World) -> usize {
    fs::read_dir(world.repo_home().pending_dir().join("tool")).map_or(0, Iterator::count)
}

#[test]
fn a_read_is_recorded_only_where_a_feature_needs_it() {
    use acs_core::Phase::{PostTool, PreTool};
    // (switches, pre-tool read recorded, post-tool read recorded)
    for (features, before, after) in [
        ("", 1, 1),
        ("lore = false", 0, 1),
        ("claims = false", 1, 0),
        ("lore = false\nclaims = false", 0, 0),
        // Sync checks after a read whether it is due.
        ("claims = false\nsync = true", 1, 1),
    ] {
        let world = world_with(features);
        let agent = Agent::new(&world, "claude-code", "c1");
        agent.start();
        assert!(allowed(&read_event(&agent, "pre_tool")), "{features}");
        assert!(allowed(&read_event(&agent, "post_tool")), "{features}");
        assert_eq!(recorded(&agent, PreTool), before, "pre-tool, {features:?}");
        assert_eq!(recorded(&agent, PostTool), after, "post-tool, {features:?}");
    }
}

#[test]
fn a_skipped_event_creates_no_seat() {
    let world = world_with("lore = false\nclaims = false");
    let agent = Agent::new(&world, "antigravity", "g1");
    assert!(allowed(&agent.send("prompt_submit", serde_json::json!({}))));
    assert!(allowed(&read_event(&agent, "pre_tool")));
    assert!(allowed(&read_event(&agent, "post_tool")));
    assert!(!has_seat(&agent), "nothing needed a seat yet");
    // The first event that is never skipped creates it.
    assert!(allowed(&agent.pre_edit("src/lib.rs")));
    assert!(has_seat(&agent));

    // With every switch at its default a prompt alone creates no seat either: nothing can be
    // waiting for a session that has none.
    let world = World::new();
    let agent = Agent::new(&world, "antigravity", "g1");
    assert!(allowed(&agent.send("prompt_submit", serde_json::json!({}))));
    assert!(!has_seat(&agent));
}

#[test]
fn after_a_write_or_a_command_nothing_is_recorded_with_history_claims_and_sync_off() {
    use acs_core::Phase::PostTool;
    let run = |features: &str| {
        let world = world_with(features);
        let agent = Agent::new(&world, "claude-code", "c1");
        agent.start();
        assert!(allowed(&agent.pre_edit("src/lib.rs")));
        agent.post_edit("src/lib.rs");
        assert!(allowed(&agent.pre_shell("./build.sh")));
        assert_eq!(tool_markers(&world), 1, "the running command, {features:?}");
        agent.post_shell("./build.sh");
        // The marker is used up either way, or `acs watch` would see a command that never ends.
        assert_eq!(tool_markers(&world), 0, "{features:?}");
        recorded(&agent, PostTool)
    };
    assert_eq!(run("history = false\nclaims = false"), 0);
    assert_eq!(
        run("history = false"),
        2,
        "claims refresh after a tool call"
    );
    assert_eq!(run("claims = false"), 2, "history attributes the change");
}

#[test]
fn self_protection_and_the_status_message_are_never_skipped() {
    let world = world_with(
        "claims = false\nguards = false\nterritory = false\nlore = false\nhistory = false\n\
         commit_checks = false",
    );
    let agent = Agent::new(&world, "claude-code", "c1");
    let context = agent.start().context.join("\n");
    assert!(context.contains("switched off"), "{context}");
    assert!(has_seat(&agent));
    let d = agent.pre_edit(".acs/policy/config.toml");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::SealedPath)
    );
    assert_eq!(agent.pre_shell("acs off").outcome, Outcome::Deny);
    // What `acs_why` reads is still recorded: the denied calls and the last decision.
    assert_eq!(recorded(&agent, acs_core::Phase::PreTool), 2);
    let last = world.db().last_decision(&agent.seat_id()).unwrap().unwrap();
    assert_eq!(last.outcome, Outcome::Deny);
}

#[test]
fn session_end_always_ends_the_seat() {
    use acs_core::Phase::SessionEnd;
    let run = |features: &str| {
        let world = world_with(features);
        let agent = Agent::new(&world, "claude-code", "c1");
        agent.start();
        assert!(allowed(&agent.pre_shell("./build.sh")));
        agent.end();
        assert!(agent.info().ended_at.is_some(), "{features:?}");
        assert!(world.db().live_seats().unwrap().is_empty(), "{features:?}");
        assert_eq!(tool_markers(&world), 0, "{features:?}");
        recorded(&agent, SessionEnd)
    };
    assert_eq!(run(""), 1);
    assert_eq!(run("claims = false"), 1, "history records the session");
    assert_eq!(run("claims = false\nhistory = false"), 0);
}

/// A session that only reads, in a repository where no feature needs a read, must not look
/// dead: after an hour of silence `acs sync` would record it as crashed, and the MCP server
/// would stop taking it for the most recently active session.
#[test]
fn a_session_whose_events_are_skipped_stays_alive() {
    use acs_core::{Phase, Timestamp};
    let world = world_with("lore = false\nclaims = false");
    let adapter = acs_adapters::by_name("canonical").unwrap();
    let agent = Agent::new(&world, "claude-code", "c1");
    let send = |phase: Phase, name: &str, extra: Value, at: Timestamp| {
        let env = acs_cli::hook::HookEnv {
            process_cwd: world.repo.clone(),
            home: Ok(world.acs_home()),
            scan: acs_git::status::ScanOptions::default(),
            now: at,
        };
        let payload = agent.event(name, extra).to_string();
        let _ = acs_cli::hook::run(adapter, Some(phase), payload.as_bytes(), env);
    };
    let read = serde_json::json!({"tool": {"name": "Read", "kind": "read"}, "paths": ["a.txt"]});
    let start = Timestamp::now().as_second();
    let at = |minutes: i64| Timestamp::from_second(start + minutes * 60).unwrap();
    let last_seen = || agent.info().seat.last_seen_at.as_second();

    send(
        Phase::SessionStart,
        "session_start",
        serde_json::json!({}),
        at(0),
    );
    // Reading for seventy minutes, one file every ten.
    for minutes in (10..=70).step_by(10) {
        send(Phase::PreTool, "pre_tool", read.clone(), at(minutes));
        send(Phase::PostTool, "post_tool", read.clone(), at(minutes));
    }
    assert_eq!(
        recorded(&agent, Phase::PreTool),
        0,
        "the reads were skipped"
    );
    assert_eq!(
        recorded(&agent, Phase::PostTool),
        0,
        "the reads were skipped"
    );
    assert_eq!(last_seen(), at(70).as_second());
    let crashed_before = Timestamp::from_second(at(70).as_second() - 3600).unwrap();
    let stale = world.db().stale_live_seats(crashed_before).unwrap();
    assert!(
        stale.is_empty(),
        "the session is not taken for a crashed one"
    );

    // The seat is not written on every skipped event: within five minutes it is left alone.
    send(Phase::PreTool, "pre_tool", read.clone(), at(72));
    assert_eq!(last_seen(), at(70).as_second());
    send(
        Phase::PromptSubmit,
        "prompt_submit",
        serde_json::json!({}),
        at(74),
    );
    assert_eq!(last_seen(), at(70).as_second());
    send(Phase::PostTool, "post_tool", read, at(75));
    assert_eq!(last_seen(), at(75).as_second());
}

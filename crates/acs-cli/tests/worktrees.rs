//! A session that works in another work tree of the repository than the one it started in: the
//! harness reports the session's directory, and the files are somewhere else (a linked worktree
//! under `.claude/worktrees/`, as Claude Code's subagents use). Each call must be judged in the
//! work tree that holds its files.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::PathBuf;

use acs_cli::runtime::seats::current_human_seat;
use acs_cli::runtime::{ClaimOutcome, RepoContext, claim_paths, release_paths};
use acs_core::{Outcome, ReasonCode, RepoPath, Timestamp};
use common::{Agent, World, git};

/// Adds a linked worktree inside the main checkout and returns its canonical path.
fn nested_worktree(world: &World) -> PathBuf {
    git(
        &world.repo,
        &world.git_config,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            ".claude/worktrees/wp",
        ],
    );
    dunce::canonicalize(world.repo.join(".claude/worktrees/wp")).unwrap()
}

fn slashes(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn an_edit_in_a_nested_worktree_claims_the_file_as_that_worktree_names_it() {
    let world = World::new();
    let wt = nested_worktree(&world);
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();

    // By absolute path and by a path relative to the session's directory.
    let decision = agent.pre_edit(&wt.join("src/lib.rs").to_string_lossy());
    assert_eq!(decision.outcome, Outcome::Allow, "{decision:?}");
    let decision = agent.pre_edit(".claude/worktrees/wp/src/util.rs");
    assert_eq!(decision.outcome, Outcome::Allow, "{decision:?}");
    assert_eq!(agent.claimed(), ["src/lib.rs", "src/util.rs"]);

    // The seat still belongs to the work tree the session runs in.
    assert_eq!(agent.info().seat.worktree, world.repo.to_string_lossy());

    // The claim is the same file everywhere: a second session in the main checkout is stopped.
    let other = Agent::new(&world, "claude-code", "c2");
    other.start();
    let denied = other.pre_edit("src/lib.rs");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    assert_eq!(denied.reason_code, ReasonCode::ClaimConflict);

    // The change is recorded against the file in the worktree, not the main checkout's copy.
    fs::write(wt.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    agent.post_edit(".claude/worktrees/wp/src/lib.rs");
    let path = RepoPath::try_from("src/lib.rs".to_owned()).unwrap();
    let rows = world.db().path_attributions(&path).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].post_hash.is_some());
}

#[test]
fn a_session_started_in_a_worktree_is_checked_when_it_edits_the_main_checkout() {
    let world = World::new();
    let wt = nested_worktree(&world);
    let holder = Agent::new(&world, "claude-code", "c1");
    holder.start();
    assert_eq!(holder.pre_edit("a.txt").claims_acquired.len(), 1);

    // An event whose `cwd` is the worktree and whose path is in the main checkout.
    let event = serde_json::json!({
        "spec": "acs/0.1", "harness": "claude-code", "harness_session_id": "w1",
        "phase": "pre_tool", "cwd": wt,
        "tool": {"name": "Edit", "kind": "edit"},
        "paths": [world.repo.join("a.txt")],
    });
    let denied = world.hook("pre_tool", &event);
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    assert_eq!(denied.reason_code, ReasonCode::ClaimConflict);
}

#[test]
fn sealed_paths_hold_inside_a_nested_worktree() {
    let world = World::new();
    nested_worktree(&world);
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    let denied = agent.pre_edit(".claude/worktrees/wp/.github/workflows/ci.yml");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
    assert_eq!(denied.reason_code, ReasonCode::SealedPath);
    let denied = agent.pre_shell("rm .claude/worktrees/wp/.github/workflows/ci.yml");
    assert_eq!(denied.outcome, Outcome::Deny, "{denied:?}");
}

#[test]
fn a_shell_command_in_a_nested_worktree_claims_that_worktrees_files() {
    let world = World::new();
    let wt = nested_worktree(&world);
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();

    let decision = agent.pre_shell("cd .claude/worktrees/wp && echo x > a.txt");
    assert_ne!(decision.outcome, Outcome::Deny, "{decision:?}");
    assert_eq!(agent.claimed(), ["a.txt"]);

    let command = format!("echo x > {}/src/util.rs", slashes(&wt));
    let decision = agent.pre_shell(&command);
    assert_ne!(decision.outcome, Outcome::Deny, "{decision:?}");
    assert_eq!(agent.claimed(), ["a.txt", "src/util.rs"]);

    // What the command changed is found in the worktree and recorded under the same names.
    fs::write(wt.join("src/util.rs"), "pub fn changed() {}\n").unwrap();
    agent.post_shell(&command);
    let path = RepoPath::try_from("src/util.rs".to_owned()).unwrap();
    assert_eq!(world.db().path_attributions(&path).unwrap().len(), 1);

    // Removing the worktree itself is an operation on the checkout around it: its files are
    // named from there, so another session's claim on `a.txt` is not what stops it.
    let other = Agent::new(&world, "claude-code", "c2");
    other.start();
    let decision = other.pre_shell("rm -rf .claude/worktrees/wp");
    assert_ne!(
        decision.reason_code,
        ReasonCode::ClaimConflict,
        "{decision:?}"
    );
    assert!(
        decision.message.is_empty() || decision.message.contains(".claude/worktrees/wp/"),
        "{decision:?}"
    );
}

#[test]
fn claims_and_releases_from_the_main_checkout_reach_files_in_a_worktree() {
    let world = World::new();
    let wt = nested_worktree(&world);
    let ctx = RepoContext::open_with(&world.repo, Ok(world.acs_home())).unwrap();
    let now = Timestamp::now();
    let seat = current_human_seat(&ctx, ctx.db().unwrap(), now)
        .unwrap()
        .info;

    let paths = vec![
        "a.txt".to_owned(),
        ".claude/worktrees/wp/src/lib.rs".to_owned(),
        wt.join("src/util.rs").to_string_lossy().into_owned(),
    ];
    let outcome = claim_paths(&ctx, &seat, &paths, &world.repo, now).unwrap();
    let ClaimOutcome::Granted { paths: granted, .. } = outcome else {
        panic!("expected a grant: {outcome:?}");
    };
    let mut names: Vec<&str> = granted.iter().map(RepoPath::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.txt", "src/lib.rs", "src/util.rs"]);

    // Released by either spelling: the worktree's path or the plain repository path.
    let report = release_paths(
        &ctx,
        &seat,
        &[
            ".claude/worktrees/wp/src/lib.rs".to_owned(),
            "src/util.rs".to_owned(),
        ],
        &world.repo,
        now,
    )
    .unwrap();
    let released: Vec<&str> = report.released.iter().map(RepoPath::as_str).collect();
    assert_eq!(released, ["src/lib.rs", "src/util.rs"]);
    assert!(report.not_held.is_empty() && report.outside.is_empty());
}

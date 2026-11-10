//! `acs undo`: the dry run, `--apply`, committed changes, conflicts, other seats and live seats.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::process::Output;

use common::{Agent, World, git};
use serde_json::Value;

/// One file-tool call: the hook before, the change, the hook after.
fn edit(agent: &Agent<'_>, path: &str, text: &str) {
    agent.pre_edit(path);
    agent.world.write(path, text);
    agent.post_edit(path);
}

fn session_id(agent: &Agent<'_>) -> String {
    agent.info().session_id.to_string()
}

fn undo(world: &World, args: &[&str]) -> Output {
    let mut full = vec!["undo"];
    full.extend_from_slice(args);
    world.run(&full, b"", &world.repo)
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn read(world: &World, path: &str) -> String {
    fs::read_to_string(world.repo.join(path)).unwrap()
}

fn git_out(world: &World, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A finished session that edited `a.txt` and created `new.txt`.
fn finished_session(world: &World) -> String {
    let agent = Agent::new(world, "claude-code", "a");
    agent.start();
    edit(&agent, "a.txt", "a changed by the session\n");
    edit(&agent, "new.txt", "created by the session\n");
    agent.end();
    session_id(&agent)
}

#[test]
fn a_dry_run_prints_the_plan_and_changes_nothing() {
    let world = World::new();
    let sid = finished_session(&world);
    let out = undo(&world, &[&sid]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out);
    assert!(shown.contains("a.txt"), "{shown}");
    assert!(shown.contains("restore"), "{shown}");
    assert!(shown.contains("new.txt"), "{shown}");
    assert!(shown.contains("delete"), "{shown}");
    assert!(shown.contains("dry run"), "{shown}");
    assert!(shown.lines().all(|l| l.chars().count() <= 80), "{shown}");
    assert_eq!(read(&world, "a.txt"), "a changed by the session\n");
    assert!(world.repo.join("new.txt").exists());
}

#[test]
fn applying_puts_the_files_back() {
    let world = World::new();
    let sid = finished_session(&world);
    let out = undo(&world, &[&sid, "--apply"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(read(&world, "a.txt"), "a\n");
    assert!(!world.repo.join("new.txt").exists());
    let shown = text(&out);
    assert!(shown.contains("(done)"), "{shown}");
    assert!(shown.lines().all(|l| l.chars().count() <= 80), "{shown}");
    // A second run finds nothing left to do.
    let again = undo(&world, &[&sid, "--apply"]);
    assert!(again.status.success());
    assert!(text(&again).contains("already undone"), "{}", text(&again));
}

#[test]
fn a_unique_prefix_names_the_session() {
    let world = World::new();
    let sid = finished_session(&world);
    let out = undo(&world, &[&sid[..8], "--json"]);
    assert!(out.status.success(), "{out:?}");
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["session"], sid.as_str());
    assert_eq!(view["applied"], false);
    assert_eq!(view["files"].as_array().unwrap().len(), 2, "{view}");
    assert_eq!(view["summary"]["restore"], 1, "{view}");
    assert_eq!(view["summary"]["delete"], 1, "{view}");
}

#[test]
fn an_unknown_session_says_undo_is_local() {
    let world = World::new();
    finished_session(&world);
    let out = undo(&world, &["ses_nowhere"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("known on this machine"), "{err}");
    assert!(err.contains("local"), "{err}");

    let out = undo(&world, &["ses_nowhere", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["error"]["code"], "unknown_session");
}

#[test]
fn an_ambiguous_prefix_is_refused() {
    let world = World::new();
    finished_session(&world);
    let other = Agent::new(&world, "claude-code", "b");
    other.start();
    other.end();
    let out = undo(&world, &["ses_"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("matches 2 sessions"),
        "{out:?}"
    );
}

#[test]
fn a_live_session_can_be_planned_but_not_applied() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "live");
    agent.start();
    edit(&agent, "a.txt", "changed\n");
    let sid = session_id(&agent);
    let dry = undo(&world, &[&sid]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(
        text(&dry).contains("may still be running"),
        "{}",
        text(&dry)
    );
    let applied = undo(&world, &[&sid, "--apply"]);
    assert_eq!(applied.status.code(), Some(1));
    let err = String::from_utf8_lossy(&applied.stderr).into_owned();
    assert!(err.contains("may still be running"), "{err}");
    assert_eq!(read(&world, "a.txt"), "changed\n");
    agent.end();
    assert!(undo(&world, &[&sid, "--apply"]).status.success());
    assert_eq!(read(&world, "a.txt"), "a\n");
}

#[test]
fn changes_others_made_afterwards_are_kept() {
    let world = World::new();
    let before: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    world.write("m.txt", &before);
    let agent = Agent::new(&world, "claude-code", "a");
    agent.start();
    edit(&agent, "m.txt", &format!("{before}added by the session\n"));
    agent.end();
    let sid = session_id(&agent);
    // Someone else edits the top of the file afterwards.
    world.write(
        "m.txt",
        &format!("edited by someone else\n{before}added by the session\n"),
    );
    let out = undo(&world, &[&sid, "--apply"]);
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out).contains("merge"), "{}", text(&out));
    assert_eq!(
        read(&world, "m.txt"),
        format!("edited by someone else\n{before}")
    );
}

#[test]
fn overlapping_later_changes_are_a_conflict_and_exit_1() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "a");
    agent.start();
    edit(&agent, "a.txt", "changed by the session\n");
    edit(&agent, "src/lib.rs", "pub fn session() {}\n");
    agent.end();
    let sid = session_id(&agent);
    world.write("a.txt", "changed by someone else\n");
    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out).contains("conflict"), "{}", text(&out));
    assert_eq!(read(&world, "a.txt"), "changed by someone else\n");
    // The other file is still undone.
    assert_eq!(read(&world, "src/lib.rs"), "pub fn lib() {}\n");
}

#[test]
fn a_file_held_by_another_live_seat_is_left_alone() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "a");
    agent.start();
    edit(&agent, "a.txt", "changed by a\n");
    edit(&agent, "src/lib.rs", "pub fn a() {}\n");
    agent.end();
    let sid = session_id(&agent);
    let holder = Agent::new(&world, "codex", "holder");
    holder.start();
    assert_eq!(
        holder.pre_edit("a.txt").outcome,
        acs_core::Outcome::Allow,
        "the ended seat's claim is gone"
    );
    let out = undo(&world, &[&sid, "--apply"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out);
    assert!(shown.contains("held by another seat"), "{shown}");
    assert!(shown.contains("claimed by codex"), "{shown}");
    assert_eq!(read(&world, "a.txt"), "changed by a\n");
    assert_eq!(read(&world, "src/lib.rs"), "pub fn lib() {}\n");
}

#[test]
fn committed_changes_are_undone_with_a_new_commit() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "a");
    agent.start();
    edit(&agent, "a.txt", "committed change\n");
    edit(&agent, "new.txt", "committed file\n");
    let sid = session_id(&agent);
    git(&world.repo, &world.git_config, &["add", "a.txt", "new.txt"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", &format!("work\n\nACS-Session: {sid}")],
    );
    // A later change in the work tree only, and an unrelated staged change of the person.
    edit(&agent, "src/lib.rs", "pub fn later() {}\n");
    agent.end();
    world.write("src/util.rs", "pub fn staged() {}\n");
    git(&world.repo, &world.git_config, &["add", "src/util.rs"]);
    let log_before = git_out(&world, &["rev-list", "--count", "HEAD"]);

    let out = undo(&world, &[&sid, "--apply"]);
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out).contains("new commit"), "{}", text(&out));
    // History was not rewritten: one more commit on top.
    let log_after = git_out(&world, &["rev-list", "--count", "HEAD"]);
    assert_eq!(
        log_after.parse::<u32>().unwrap(),
        log_before.parse::<u32>().unwrap() + 1
    );
    assert!(git_out(&world, &["log", "-1", "--format=%s"]).starts_with("Undo session ses_"));
    assert_eq!(
        git_out(&world, &["show", "--name-only", "--format=", "HEAD"])
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>(),
        vec!["a.txt", "new.txt"]
    );
    assert_eq!(read(&world, "a.txt"), "a\n");
    assert!(!world.repo.join("new.txt").exists());
    // The uncommitted change was undone in the work tree only, and the staged file stays staged.
    assert_eq!(read(&world, "src/lib.rs"), "pub fn lib() {}\n");
    assert_eq!(
        git_out(&world, &["diff", "--cached", "--name-only"]),
        "src/util.rs"
    );
}

#[test]
fn a_rejected_undo_commit_leaves_the_files_restored_and_exits_1() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "a");
    agent.start();
    edit(&agent, "a.txt", "committed change\n");
    agent.end();
    let sid = session_id(&agent);
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", &format!("work\n\nACS-Session: {sid}")],
    );
    let hook = world.repo.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho 'no commits today' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out).contains("commit failed"), "{}", text(&out));
    assert_eq!(read(&world, "a.txt"), "a\n");
}

#[test]
fn json_lists_every_file_with_its_outcome() {
    let world = World::new();
    let sid = finished_session(&world);
    let out = undo(&world, &[&sid, "--apply", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(view["applied"], true);
    assert_eq!(view["commit"]["status"], "none");
    for file in view["files"].as_array().unwrap() {
        assert_eq!(file["applied"], true, "{file}");
    }
}

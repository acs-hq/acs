//! A repository that already uses husky adopts ACS with `acs init`, and three seats commit in it:
//! the backstop stops what it must, and the team's own hook keeps running.

use std::fs;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::common::{Agent, World, git};

/// What `npx husky init` leaves behind, version 9: stubs in `.husky/_` that run `.husky/<hook>`.
fn husky_repo() -> World {
    let world = World::new();
    let stubs = world.repo.join(".husky/_");
    fs::create_dir_all(&stubs).unwrap();
    fs::write(
        stubs.join("h"),
        "#!/usr/bin/env sh\nn=$(basename \"$0\")\ns=$(dirname \"$(dirname \"$0\")\")/$n\n\
         [ ! -f \"$s\" ] && exit 0\nsh -e \"$s\" \"$@\"\nc=$?\nexit $c\n",
    )
    .unwrap();
    for name in ["pre-commit", "prepare-commit-msg", "pre-push"] {
        let path = stubs.join(name);
        fs::write(&path, "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::write(
        world.repo.join(".husky/pre-commit"),
        "echo ran >> husky-ran.txt\n",
    )
    .unwrap();
    git(
        &world.repo,
        &world.git_config,
        &["config", "core.hooksPath", ".husky/_"],
    );
    world
}

fn commit(world: &World, message: &str) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["commit", "-q", "-m", message])
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stage(world: &World, path: &str, text: &str) {
    world.write(path, text);
    git(&world.repo, &world.git_config, &["add", path]);
}

#[test]
fn seats_commit_through_acs_and_husky_together() {
    let world = husky_repo();
    let out = world.run(&["init", "--json"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], true, "{report}");

    // Installing twice changes nothing: the team's hook still calls ACS exactly once.
    let again = world.run(&["init", "--json"], b"", &world.repo);
    assert!(again.status.success(), "{again:?}");
    let hook = fs::read_to_string(world.repo.join(".husky/pre-commit")).unwrap();
    assert_eq!(
        hook.matches("acs git-hook pre-commit \"$@\"").count(),
        1,
        "{hook}"
    );
    assert!(hook.contains("husky-ran.txt"), "{hook}");

    let holder = Agent::new(&world, "claude-code", "holder");
    let committer = Agent::new(&world, "claude-code", "committer");
    let third = Agent::new(&world, "claude-code", "third");
    for agent in [&holder, &committer, &third] {
        agent.start();
    }

    // An agent's commit of a file another seat holds stops at the backstop.
    assert_eq!(
        holder.pre_edit("src/lib.rs").outcome,
        acs_core::Outcome::Allow
    );
    committer.pre_shell("git commit -m stolen");
    stage(&world, "src/lib.rs", "pub fn stolen() {}\n");
    let out = commit(&world, "stolen");
    assert!(!out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("`src/lib.rs` is claimed by"), "{text}");
    git(&world.repo, &world.git_config, &["reset", "-q"]);
    world.write("src/lib.rs", "pub fn lib() {}\n");

    // The rejected commit counts as a collision.
    let status = world.run(&["status", "--json"], b"", &world.repo);
    let view: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(view["collisions_total"], 1, "{view}");
    assert_eq!(view["collisions"][0]["paths"][0], "src/lib.rs", "{view}");

    // A commit of its own file goes through both: husky's script runs, ACS adds its trailer.
    assert_eq!(
        third.pre_edit("src/util.rs").outcome,
        acs_core::Outcome::Allow
    );
    third.pre_shell("git commit -m own");
    stage(&world, "src/util.rs", "pub fn own() {}\n");
    let out = commit(&world, "own");
    assert!(out.status.success(), "{out:?}");
    assert!(
        world.repo.join("husky-ran.txt").exists(),
        "husky's hook ran"
    );
    let message = Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["log", "-1", "--format=%B"])
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .output()
        .unwrap();
    let message = String::from_utf8_lossy(&message.stdout);
    let session = third.info().session_id;
    assert!(
        message.contains(&format!("ACS-Session: {session}")),
        "{message}"
    );
}

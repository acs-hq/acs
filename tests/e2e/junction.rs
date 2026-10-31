//! The shape of the 48,000-file incident: a scratch copy holds a junction (a symlink on Unix) into
//! the live tree, and a recursive delete aimed at the copy would go through it. M1 proves the gate
//! stops that delete. Recovering from one that happened anyway needs `acs undo` and arrives with M2.

use std::fs;
use std::path::Path;

use acs_core::{Outcome, ReasonCode};

use crate::common::{Agent, World};

fn dir_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success(), "mklink /J failed: {out:?}");
    }
}

fn assert_live_tree_is_intact(world: &World) {
    assert_eq!(
        fs::read_to_string(world.repo.join("src/lib.rs")).unwrap(),
        "pub fn lib() {}\n"
    );
}

#[test]
fn a_recursive_delete_of_a_scratch_copy_inside_the_repository_is_stopped() {
    let world = World::new();
    let copy = world.repo.join("scratch").join("copy");
    fs::create_dir_all(&copy).unwrap();
    fs::write(copy.join("notes.txt"), "scratch\n").unwrap();
    dir_link(&world.repo.join("src"), &copy.join("live"));

    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let d = agent.pre_shell("rm -rf scratch");
    assert_eq!(d.outcome, Outcome::Deny, "{d:?}");
    assert_eq!(d.reason_code, ReasonCode::DeleteOutsideClaims);
    assert!(d.message.contains("src"), "{}", d.message);
    assert_live_tree_is_intact(&world);
}

#[test]
fn a_recursive_delete_of_a_scratch_copy_outside_the_repository_is_stopped() {
    let world = World::new();
    let copy = world.tmp.path().join("scratch-copy");
    fs::create_dir_all(&copy).unwrap();
    dir_link(&world.repo.join("src"), &copy.join("live"));

    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let d = agent.pre_shell("rm -rf ../scratch-copy");
    assert_ne!(d.outcome, Outcome::Allow, "{d:?}");
    assert_live_tree_is_intact(&world);
}

#[test]
fn a_delete_through_a_junction_needs_the_claim_of_the_file_it_reaches() {
    let world = World::new();
    let copy = world.repo.join("scratch");
    fs::create_dir_all(&copy).unwrap();
    dir_link(&world.repo.join("src"), &copy.join("live"));

    let holder = Agent::new(&world, "claude-code", "holder");
    let other = Agent::new(&world, "claude-code", "other");
    holder.start();
    other.start();
    assert_eq!(holder.pre_edit("src/lib.rs").outcome, Outcome::Allow);
    let d = other.pre_shell("rm scratch/live/lib.rs");
    assert_eq!(d.outcome, Outcome::Deny, "{d:?}");
    assert_live_tree_is_intact(&world);
}

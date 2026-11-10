//! Interleaved undo: session A edits files and ends, session B edits the same files elsewhere in
//! them, then A is undone. B's work stays byte for byte; A's is gone.

use std::fs;

use serde_json::json;

use crate::common::{Agent, World};

fn numbered(prefix: &str) -> String {
    (1..=40).map(|n| format!("{prefix} {n}\n")).collect()
}

fn read(world: &World, path: &str) -> String {
    fs::read_to_string(world.repo.join(path)).unwrap()
}

/// One file-tool call: the hook before, the change, the hook after.
fn write(agent: &Agent<'_>, path: &str, text: &str) {
    agent.pre_edit(path);
    agent.world.write(path, text);
    agent.post_edit(path);
}

fn delete(agent: &Agent<'_>, path: &str) {
    let event = json!({"tool": {"name": "Delete", "kind": "delete"}, "paths": [path]});
    // A delete needs the file's claim, which an edit's pre-tool step takes.
    assert_eq!(agent.pre_edit(path).outcome, acs_core::Outcome::Allow);
    let decision = agent.send("pre_tool", event.clone());
    assert_eq!(decision.outcome, acs_core::Outcome::Allow, "{decision:?}");
    fs::remove_file(agent.world.repo.join(path)).unwrap();
    agent.send("post_tool", event);
}

#[test]
fn undoing_one_session_keeps_the_work_of_the_session_that_came_after() {
    let world = World::new();
    let original = numbered("line");
    world.write("shared.txt", &original);
    world.write("notes.md", "# notes\n");
    world.write("gone.txt", "delete me, undo brings me back\n");

    let a = Agent::new(&world, "claude-code", "session-a");
    a.start();
    // A edits the top of the shared file, edits its own file, creates one, deletes one.
    write(&a, "shared.txt", &format!("A was here\n{original}"));
    write(&a, "src/lib.rs", "pub fn a() {}\n");
    write(&a, "a_new.txt", "created by A\n");
    delete(&a, "gone.txt");
    a.end();

    let b = Agent::new(&world, "codex", "session-b");
    b.start();
    // B edits the bottom of the same shared file, adds its own file, and edits notes.md.
    let current = read(&world, "shared.txt");
    write(&b, "shared.txt", &format!("{current}B was here\n"));
    write(&b, "b_new.txt", "created by B\n");
    write(&b, "notes.md", "# notes\nB's notes\n");
    b.end();
    let b_files = ["b_new.txt", "notes.md"].map(|p| (p, read(&world, p)));

    let sid = a.info().session_id.to_string();
    let dry = world.run(&["undo", &sid], b"", &world.repo);
    assert!(dry.status.success(), "{dry:?}");
    let plan = String::from_utf8_lossy(&dry.stdout).into_owned();
    for expected in ["merge", "restore", "delete", "recreate"] {
        assert!(plan.contains(expected), "{expected} missing: {plan}");
    }
    assert!(plan.lines().all(|l| l.chars().count() <= 80), "{plan}");
    assert!(read(&world, "shared.txt").starts_with("A was here\n"));

    let out = world.run(&["undo", &sid, "--apply"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");

    // A's changes are gone ...
    assert_eq!(
        read(&world, "shared.txt"),
        format!("{original}B was here\n")
    );
    assert_eq!(read(&world, "src/lib.rs"), "pub fn lib() {}\n");
    assert!(!world.repo.join("a_new.txt").exists());
    assert_eq!(read(&world, "gone.txt"), "delete me, undo brings me back\n");
    // ... and B's stay as they were.
    assert_eq!(read(&world, "b_new.txt"), "created by B\n");
    assert_eq!(b_files[0].1, "created by B\n");
    assert_eq!(read(&world, "notes.md"), b_files[1].1);

    // Undoing B afterwards is clean too: the files it touched go back to what they were.
    let sid_b = b.info().session_id.to_string();
    let out = world.run(&["undo", &sid_b, "--apply"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(read(&world, "shared.txt"), original);
    assert!(!world.repo.join("b_new.txt").exists());
    assert_eq!(read(&world, "notes.md"), "# notes\n");
}

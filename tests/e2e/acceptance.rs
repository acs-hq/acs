//! The v0.1 acceptance test: three seats with overlapping tasks produce zero silent overwrites, and
//! every collision is visible in `acs status`. The real-agent version is `real/three_sessions.sh`.

use std::fs;
use std::sync::Barrier;

use acs_core::{Outcome, ReasonCode};
use serde_json::Value;

use crate::common::{Agent, World};

/// The files all three tasks want.
const SHARED: [&str; 2] = ["src/lib.rs", "CHANGELOG.md"];
/// The most rounds a task may need. One seat finishes each round, so three are enough.
const ROUNDS: usize = 3;

/// What one seat wants to write: a line of its own in each file, and how often it was refused.
struct Task {
    name: &'static str,
    pending: Vec<&'static str>,
    denied: usize,
}

impl Task {
    fn new(name: &'static str, own_file: &'static str) -> Self {
        let mut pending = SHARED.to_vec();
        pending.push(own_file);
        Self {
            name,
            pending,
            denied: 0,
        }
    }

    fn line(&self, path: &str) -> String {
        format!("work of {} in {path}", self.name)
    }

    /// One session: tries everything still pending, as an agent does: the hook runs first, and only
    /// then does the tool read the file and write it back.
    fn session(&mut self, world: &World, round: usize) {
        let agent = Agent::new(world, "claude-code", &format!("{}-{round}", self.name));
        agent.start();
        let mut left = Vec::new();
        for path in std::mem::take(&mut self.pending) {
            let decision = agent.pre_edit(path);
            if decision.outcome == Outcome::Allow {
                append(world, path, &self.line(path));
                agent.post_edit(path);
            } else {
                assert_eq!(decision.reason_code, ReasonCode::ClaimConflict, "{path}");
                assert!(decision.message.contains(path), "{}", decision.message);
                self.denied += 1;
                left.push(path);
            }
        }
        self.pending = left;
        agent.end();
    }
}

fn append(world: &World, path: &str, line: &str) {
    let file = world.repo.join(path);
    let mut text = fs::read_to_string(&file).unwrap_or_default();
    text.push_str(line);
    text.push('\n');
    fs::write(file, text).unwrap();
}

fn status(world: &World) -> Value {
    let out = world.run(&["status", "--json"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn three_seats_with_overlapping_tasks_lose_nothing_and_every_collision_shows() {
    let world = World::new();
    world.write("CHANGELOG.md", "# Changelog\n");
    let mut tasks = [
        Task::new("discount", "src/discount.rs"),
        Task::new("tax", "src/tax.rs"),
        Task::new("rounding", "src/rounding.rs"),
    ];
    for round in 0..ROUNDS {
        let barrier = Barrier::new(tasks.len());
        std::thread::scope(|scope| {
            for task in &mut tasks {
                let (world, barrier) = (&world, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    task.session(world, round);
                });
            }
        });
    }

    // Zero silent overwrites: every write that was allowed is in the file, and all work landed.
    for task in &tasks {
        assert!(
            task.pending.is_empty(),
            "{} left {:?}",
            task.name,
            task.pending
        );
        for path in SHARED.iter().copied().chain([task_file(task)]) {
            let text = fs::read_to_string(world.repo.join(path)).unwrap();
            assert!(text.contains(&task.line(path)), "{path} lost: {text}");
        }
    }

    // Every collision is visible, by seat and by file.
    let denied: usize = tasks.iter().map(|t| t.denied).sum();
    assert!(
        denied > 0,
        "the tasks overlapped, so someone must have been refused"
    );
    let view = status(&world);
    assert_eq!(view["collisions_total"], denied, "{view}");
    let collisions = view["collisions"].as_array().unwrap();
    assert_eq!(collisions.len(), denied, "{view}");
    for collision in collisions {
        let path = collision["paths"][0].as_str().unwrap();
        assert!(SHARED.contains(&path), "{collision}");
        assert_eq!(collision["harness"], "claude-code");
        assert_eq!(collision["pilot"], "sara@example.com");
        let message = collision["message"].as_str().unwrap();
        assert!(message.contains("is claimed by"), "{collision}");
    }
    let human = world.run(&["status"], b"", &world.repo);
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.contains(&format!("Recent collisions ({denied}")),
        "{text}"
    );
    assert!(
        text.lines().all(|line| line.chars().count() <= 80),
        "{text}"
    );
}

fn task_file(task: &Task) -> &'static str {
    match task.name {
        "discount" => "src/discount.rs",
        "tax" => "src/tax.rs",
        _ => "src/rounding.rs",
    }
}

#[test]
fn a_collision_is_still_visible_after_both_seats_have_gone() {
    let world = World::new();
    let holder = Agent::new(&world, "claude-code", "holder");
    let other = Agent::new(&world, "claude-code", "other");
    holder.start();
    other.start();
    assert_eq!(holder.pre_edit("a.txt").outcome, Outcome::Allow);
    assert_eq!(other.pre_edit("a.txt").outcome, Outcome::Deny);
    holder.end();
    other.end();
    let view = status(&world);
    assert_eq!(view["other_claims"].as_array().unwrap().len(), 0, "{view}");
    assert_eq!(view["collisions_total"], 1, "{view}");
    assert_eq!(view["collisions"][0]["paths"][0], "a.txt", "{view}");
}

/// Keeps the two tests above honest: the same two-step write, with the gate switched off, loses a
/// line that the file then silently lacks. If this stopped failing to keep the line, the assertions
/// above would prove nothing.
#[test]
fn without_the_gate_the_same_writes_overwrite_each_other() {
    let world = World::new();
    world.write("CHANGELOG.md", "# Changelog\n");
    let writes_after_reading = |gated: bool| {
        let a = Agent::new(&world, "claude-code", "a");
        let b = Agent::new(&world, "claude-code", "b");
        a.start();
        b.start();
        let path = "CHANGELOG.md";
        assert_eq!(a.pre_edit(path).outcome, Outcome::Allow);
        let refused = b.pre_edit(path).outcome != Outcome::Allow;
        assert_eq!(refused, gated);
        let read_by_a = fs::read_to_string(world.repo.join(path)).unwrap();
        let read_by_b = fs::read_to_string(world.repo.join(path)).unwrap();
        fs::write(world.repo.join(path), format!("{read_by_a}from a\n")).unwrap();
        if !refused {
            fs::write(world.repo.join(path), format!("{read_by_b}from b\n")).unwrap();
        }
        a.end();
        b.end();
        fs::read_to_string(world.repo.join(path)).unwrap()
    };

    assert!(writes_after_reading(true).contains("from a"));
    assert!(world.run(&["off"], b"", &world.repo).status.success());
    world.write("CHANGELOG.md", "# Changelog\n");
    let ungated = writes_after_reading(false);
    assert!(!ungated.contains("from a"), "{ungated}");
}

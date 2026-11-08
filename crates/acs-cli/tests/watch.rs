//! `acs watch` (WP7.4): changes made outside any agent are claimed for the person, changes agents
//! make are left to them, gitignored files are never claimed, and claims are released after the
//! idle window.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use acs_core::{Outcome, ReasonCode};
use common::{Agent, World, acs_bin};
use serde_json::Value;

/// A running `acs watch --json` and the events it has printed so far.
struct Watch {
    child: Child,
    events: mpsc::Receiver<Value>,
    seen: Vec<Value>,
}

impl Watch {
    fn start(world: &World, idle_secs: u64) -> Self {
        let mut child = Command::new(acs_bin())
            .args([
                "watch",
                "--json",
                "--settle-ms",
                "300",
                "--idle",
                &idle_secs.to_string(),
            ])
            .current_dir(&world.repo)
            .env("ACS_HOME", &world.home)
            .env("GIT_CONFIG_GLOBAL", &world.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        let mut watch = Self {
            child,
            events,
            seen: Vec::new(),
        };
        watch.wait_for("the watcher to start", |e| e["event"] == "watching");
        watch
    }

    /// Waits for an event matching `matches`, keeping every event seen.
    fn wait_for(&mut self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) => {
                    self.seen.push(event.clone());
                    if matches(&event) {
                        return event;
                    }
                }
                Err(_) => panic!("timed out waiting for {what}; seen: {:?}", self.seen),
            }
        }
    }

    /// Collects whatever arrives in the next `wait`.
    fn drain(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        while let Ok(event) = self
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(event);
        }
    }

    /// Every path the watcher claimed so far.
    fn claimed(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter(|e| e["event"] == "claimed")
            .flat_map(|e| e["paths"].as_array().cloned().unwrap_or_default())
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect()
    }

    /// Writes `a.txt` and waits for the watcher to claim it: everything changed before has then
    /// been judged too.
    fn sentinel(&mut self, world: &World) {
        world.write("a.txt", &format!("sentinel {:?}\n", Instant::now()));
        self.wait_for("the sentinel claim", |e| {
            e["event"] == "claimed" && e["paths"].as_array().unwrap().contains(&"a.txt".into())
        });
        self.drain(Duration::from_millis(800));
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn claimed_by(path: &Value, file: &str) -> bool {
    path["event"] == "claimed" && path["paths"].as_array().unwrap().contains(&file.into())
}

#[test]
fn a_person_editing_a_file_gets_a_human_claim_that_blocks_agents() {
    let world = World::new();
    let mut watch = Watch::start(&world, 120);
    world.write("src/lib.rs", "pub fn lib() { /* edited by hand */ }\n");
    watch.wait_for("the claim on src/lib.rs", |e| claimed_by(e, "src/lib.rs"));

    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let d = agent.pre_edit("src/lib.rs");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::ClaimConflict)
    );
    assert!(
        d.message.contains("sara@example.com") && d.message.contains("human"),
        "the denial names the person: {}",
        d.message
    );
}

#[test]
fn an_agents_file_tool_edit_is_left_to_the_agent() {
    let world = World::new();
    let mut watch = Watch::start(&world, 120);
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    assert_eq!(agent.pre_edit("src/util.rs").outcome, Outcome::Allow);
    world.write("src/util.rs", "pub fn util() { /* by the agent */ }\n");
    agent.post_edit("src/util.rs");
    watch.sentinel(&world);

    assert!(
        !watch.claimed().contains(&"src/util.rs".to_owned()),
        "an agent's edit is not claimed for the person: {:?}",
        watch.seen
    );
    assert_eq!(agent.claimed(), vec!["src/util.rs".to_owned()]);
}

#[test]
fn a_running_agent_shell_command_is_waited_for() {
    let world = World::new();
    let mut watch = Watch::start(&world, 120);
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    // Its targets cannot be predicted, so nothing is claimed before it runs.
    assert_eq!(agent.pre_shell("python gen.py").outcome, Outcome::Allow);
    world.write("gen.txt", "generated\n");
    // Well past the settle window: the watcher must hold the change back while the command runs.
    std::thread::sleep(Duration::from_millis(2000));
    agent.post_shell("python gen.py");
    watch.sentinel(&world);

    assert!(
        !watch.claimed().contains(&"gen.txt".to_owned()),
        "a change made during an agent's shell command is left to it: {:?}",
        watch.seen
    );
    let attributed = world
        .db()
        .path_attributions(&"gen.txt".to_owned().try_into().unwrap())
        .unwrap();
    assert!(
        attributed.iter().any(|a| a.seat_id == agent.seat_id()),
        "the shell command's post-tool step attributed the file to the agent"
    );
}

#[test]
fn gitignored_files_are_never_claimed() {
    let world = World::new();
    let mut watch = Watch::start(&world, 120);
    world.write("target/debug/out.bin", "binary\n");
    world.write("build.log", "log\n");
    watch.sentinel(&world);

    let claimed = watch.claimed();
    assert!(
        !claimed
            .iter()
            .any(|p| p.starts_with("target/") || p == "build.log"),
        "ignored files are not claimed: {claimed:?}"
    );
}

#[test]
fn claims_are_released_after_the_idle_window() {
    let world = World::new();
    let mut watch = Watch::start(&world, 1);
    world.write("src/util.rs", "pub fn util() { /* by hand */ }\n");
    watch.wait_for("the claim", |e| claimed_by(e, "src/util.rs"));
    watch.wait_for("the release", |e| {
        e["event"] == "released"
            && e["paths"]
                .as_array()
                .unwrap()
                .contains(&"src/util.rs".into())
    });

    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    assert_eq!(agent.pre_edit("src/util.rs").outcome, Outcome::Allow);
}

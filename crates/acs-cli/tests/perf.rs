//! Latency budgets of plan section 4.7, measured against the built `acs` binary, process start
//! included. Ignored by default (timing depends on the machine); run with a release build:
//!
//! ```text
//! cargo test --release -p acs-cli --test perf -- --ignored --nocapture
//! ACS_PERF_FILES=100000 cargo test --release -p acs-cli --test perf -- --ignored --nocapture
//! ```
//!
//! `ACS_PERF_FILES` sets the size of the repository for the post-tool measurement (default
//! 10,000). `ACS_PERF_NO_ASSERT=1` prints the numbers without asserting the budgets.
//!
//! Budgets: pre-tool file edit, warm: p50 < 15 ms, p99 < 40 ms. Post-tool shell, large repo:
//! p50 < 60 ms, p99 < 200 ms.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{World, git};
use serde_json::{Value, json};

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Times one `acs hook --canonical <phase>` call, from spawn to exit.
fn timed_hook(world: &World, phase: &str, event: &Value) -> Duration {
    let payload = event.to_string();
    let started = Instant::now();
    let mut child = Command::new(common::acs_bin())
        .args(["hook", "--canonical", phase])
        .current_dir(&world.repo)
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    assert!(out.status.success());
    elapsed
}

fn event(world: &World, phase: &str, extra: Value) -> Value {
    let mut base = json!({
        "spec": "acs/0.1", "harness": "claude-code", "harness_session_id": "perf",
        "phase": phase, "cwd": world.repo,
    });
    if let (Some(map), Value::Object(more)) = (base.as_object_mut(), extra) {
        map.extend(more);
    }
    base
}

fn report(label: &str, samples: &mut [Duration], p50_budget: f64, p99_budget: f64) {
    samples.sort();
    let (p50, p99) = (ms(percentile(samples, 0.50)), ms(percentile(samples, 0.99)));
    println!(
        "{label}: n={} p50 {p50:.1} ms (budget {p50_budget}) p99 {p99:.1} ms (budget {p99_budget}) \
         min {:.1} max {:.1}",
        samples.len(),
        ms(samples[0]),
        ms(samples[samples.len() - 1]),
    );
    if std::env::var_os("ACS_PERF_NO_ASSERT").is_none() {
        assert!(
            p50 < p50_budget,
            "{label}: p50 {p50:.1} ms over {p50_budget} ms"
        );
        assert!(
            p99 < p99_budget,
            "{label}: p99 {p99:.1} ms over {p99_budget} ms"
        );
    }
}

#[test]
#[ignore = "timing test: run with --release --ignored"]
fn pre_tool_file_edit_warm_budget() {
    let world = World::new();
    for i in 0..50 {
        world.write(&format!("src/f{i}.rs"), &format!("pub fn f{i}() {{}}\n"));
    }
    let start = event(&world, "session_start", json!({}));
    timed_hook(&world, "session_start", &start);
    let edit = |path: &str| {
        event(
            &world,
            "pre_tool",
            json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]}),
        )
    };
    // Warm-up: the first calls create claims, the policy cache and the snapshot blobs.
    for i in 0..50 {
        timed_hook(&world, "pre_tool", &edit(&format!("src/f{i}.rs")));
    }
    let mut samples = Vec::new();
    for round in 0..300 {
        let path = format!("src/f{}.rs", round % 50);
        samples.push(timed_hook(&world, "pre_tool", &edit(&path)));
    }
    report("pre-tool file edit (warm)", &mut samples, 15.0, 40.0);
}

#[test]
#[ignore = "timing test: run with --release --ignored"]
fn post_tool_shell_budget() {
    let files: usize = std::env::var("ACS_PERF_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let world = World::new();
    // `files` source files in directories of 100.
    let per_dir = 100;
    for d in 0..files.div_ceil(per_dir) {
        let dir = world.repo.join(format!("pkg{}/mod{}/src", d / 10, d % 10));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..per_dir.min(files - d * per_dir) {
            fs::write(dir.join(format!("file{f}.rs")), format!("// {d} {f}\n")).unwrap();
        }
    }
    git(&world.repo, &world.git_config, &["add", "-A"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", "big"],
    );
    println!("repository: {files} generated files plus the fixture's");

    let shell = |phase: &str| {
        event(
            &world,
            phase,
            json!({"tool": {"name": "Bash", "kind": "shell"}, "command": "./build.sh"}),
        )
    };
    timed_hook(
        &world,
        "session_start",
        &event(&world, "session_start", json!({})),
    );
    let (mut pre, mut post) = (Vec::new(), Vec::new());
    for round in 0..120 {
        let p = timed_hook(&world, "pre_tool", &shell("pre_tool"));
        std::thread::sleep(Duration::from_millis(30));
        fs::write(
            world.repo.join("pkg0/mod0/src/file0.rs"),
            format!("// edit {round}\n"),
        )
        .unwrap();
        fs::write(world.repo.join(format!("generated{round}.txt")), "new\n").unwrap();
        let q = timed_hook(&world, "post_tool", &shell("post_tool"));
        if round >= 20 {
            pre.push(p);
            post.push(q);
        }
    }
    report("pre-tool shell", &mut pre, 15.0, 40.0);
    report(
        &format!("post-tool shell ({files} files)"),
        &mut post,
        60.0,
        200.0,
    );
}

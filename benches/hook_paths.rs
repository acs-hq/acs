//! The latency budgets of plan section 4.7, measured against the built `acs` binary with process
//! start included:
//!
//! ```text
//! cargo bench -p acs-cli --bench hook_paths
//! ```
//!
//! Criterion gives the trends; it reports means, so the percentiles the budgets are written in come
//! from the individual calls recorded here. The run fails when a budget is missed.
//!
//! - `ACS_PERF_FILES`: files in the repository of the post-tool path (default 100,000).
//! - `ACS_PERF_BASE_BIN`: a build of an earlier commit. Each call is then repeated with it, against
//!   the same repository and with an `ACS_HOME` of its own, and the run fails when this build's p99
//!   is over 20% worse and that costs over 15% of the path's p99 budget. Taking turns on one machine
//!   keeps most runner noise out of the comparison.
//!
//! The calls of decision 0035 (tool calls that no switched-on feature needs) are measured too,
//! without budgets.
//!
//! - `ACS_PERF_NO_ASSERT=1`: print the numbers without failing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../crates/acs-cli/tests/common/mod.rs"]
mod common;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use common::{World, git};
use criterion::{Criterion, SamplingMode};
use serde_json::{Value, json};

/// How much worse a p99 may get against `ACS_PERF_BASE_BIN`.
const MAX_REGRESSION: f64 = 1.20;

/// The share of a path's p99 budget a regression must also cost to count. A p99 of a few hundred
/// calls is one of the slowest few, and the same code moves it by more than 20% between runs.
const MIN_REGRESSION_OF_BUDGET: f64 = 0.15;

/// The budget of a path that has none: it is measured to be compared, not judged.
const NO_BUDGET: f64 = f64::INFINITY;

/// A build of `acs` under test, with its own state.
struct Build {
    bin: PathBuf,
    home: PathBuf,
}

/// What one path measured.
struct Measured {
    label: String,
    p50_budget_ms: f64,
    p99_budget_ms: f64,
    head: Vec<Duration>,
    base: Vec<Duration>,
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index].as_secs_f64() * 1000.0
}

/// Times one `acs` call, from spawn to exit.
fn timed(world: &World, build: &Build, args: &[&str], stdin: &[u8]) -> Duration {
    let started = Instant::now();
    let mut child = Command::new(&build.bin)
        .args(args)
        .current_dir(&world.repo)
        .env("ACS_HOME", &build.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    assert!(out.status.success(), "acs {args:?}");
    elapsed
}

fn event(world: &World, session: &str, phase: &str, extra: Value) -> Vec<u8> {
    let mut base = json!({
        "spec": "acs/0.1", "harness": "claude-code", "harness_session_id": session,
        "phase": phase, "cwd": world.repo,
    });
    if let (Some(map), Value::Object(more)) = (base.as_object_mut(), extra) {
        map.extend(more);
    }
    base.to_string().into_bytes()
}

fn hook(world: &World, build: &Build, session: &str, phase: &str, extra: Value) -> Duration {
    let payload = event(world, session, phase, extra);
    timed(world, build, &["hook", "--canonical", phase], &payload)
}

fn edit(path: &str) -> Value {
    json!({"tool": {"name": "Edit", "kind": "edit"}, "paths": [path]})
}

fn shell() -> Value {
    json!({"tool": {"name": "Bash", "kind": "shell"}, "command": "./build.sh"})
}

/// This run's build, plus `ACS_PERF_BASE_BIN` when set. Each gets its own `ACS_HOME`, so a build
/// with a newer schema cannot get in the way of an older one.
fn builds(world: &World) -> Vec<Build> {
    let mut builds = vec![Build {
        bin: common::acs_bin().into(),
        home: world.home.clone(),
    }];
    if let Some(bin) = std::env::var_os("ACS_PERF_BASE_BIN") {
        builds.push(Build {
            bin: bin.into(),
            home: world.tmp.path().join("acs-home-base"),
        });
    }
    builds
}

/// Runs `call` once for each build and returns the durations in `builds` order. Who goes first
/// changes every round, so that neither build always follows the other.
fn turn_about(
    builds: &[Build],
    round: usize,
    mut call: impl FnMut(&Build) -> Duration,
) -> Vec<Duration> {
    let mut durations = vec![Duration::ZERO; builds.len()];
    for offset in 0..builds.len() {
        let index = (round + offset) % builds.len();
        durations[index] = call(&builds[index]);
    }
    durations
}

/// Runs one criterion iteration per sample, each from [`turn_about`], and keeps every duration.
/// The warm-up runs through here too, so only the last `samples` calls are kept at the end.
fn bench(
    criterion: &mut Criterion,
    label: &str,
    (p50_budget_ms, p99_budget_ms): (f64, f64),
    samples: usize,
    builds: &[Build],
    mut iteration: impl FnMut(&[Build]) -> Vec<Duration>,
) -> Measured {
    let (mut head, mut base) = (Vec::new(), Vec::new());
    let mut group = criterion.benchmark_group("hook_paths");
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(samples)
        .warm_up_time(Duration::from_millis(500))
        // Short on purpose: each sample is then one call, and `samples` sets the run's length.
        .measurement_time(Duration::from_millis(100));
    group.bench_function(label, |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let durations = iteration(builds);
                total += durations[0];
                head.push(durations[0]);
                base.extend(durations.get(1).copied());
            }
            total
        });
    });
    group.finish();
    let keep = |all: Vec<Duration>| -> Vec<Duration> {
        let skip = all.len().saturating_sub(samples);
        let mut kept = all[skip..].to_vec();
        kept.sort();
        kept
    };
    Measured {
        label: label.to_owned(),
        p50_budget_ms,
        p99_budget_ms,
        head: keep(head),
        base: keep(base),
    }
}

fn pre_tool_file_edit(criterion: &mut Criterion) -> Measured {
    let world = World::new();
    for i in 0..50 {
        world.write(&format!("src/f{i}.rs"), &format!("pub fn f{i}() {{}}\n"));
    }
    let builds = builds(&world);
    for build in &builds {
        hook(&world, build, "perf", "session_start", json!({}));
        // The first calls create the claims, the policy cache and the snapshot blobs.
        for i in 0..50 {
            let path = format!("src/f{i}.rs");
            hook(&world, build, "perf", "pre_tool", edit(&path));
        }
    }
    let mut round = 0;
    bench(
        criterion,
        "pre-tool file edit, warm",
        (15.0, 40.0),
        300,
        &builds,
        |builds| {
            round += 1;
            let path = format!("src/f{}.rs", round % 50);
            turn_about(builds, round, |build| {
                hook(&world, build, "perf", "pre_tool", edit(&path))
            })
        },
    )
}

fn post_tool_shell(criterion: &mut Criterion) -> Measured {
    let files: usize = std::env::var("ACS_PERF_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let world = World::new();
    let per_dir = 100;
    for d in 0..files.div_ceil(per_dir) {
        let dir = world.repo.join(format!("pkg{}/mod{}/src", d / 10, d % 10));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..per_dir.min(files - d * per_dir) {
            fs::write(dir.join(format!("file{f}.rs")), format!("// {d} {f}\n")).unwrap();
        }
    }
    git(&world.repo, &world.git_config, &["add", "-A"]);
    // No automatic repack: it would take minutes, and the budgets are about the hook.
    git(
        &world.repo,
        &world.git_config,
        &["-c", "gc.auto=0", "commit", "-q", "-m", "big"],
    );

    let builds = builds(&world);
    for build in &builds {
        hook(&world, build, "perf", "session_start", json!({}));
    }
    let (mut round, mut change) = (0, 0);
    bench(
        criterion,
        &format!("post-tool shell, {files} files"),
        (60.0, 200.0),
        100,
        &builds,
        |builds| {
            round += 1;
            // Each build gets a change of its own: sharing one would let the second reuse the
            // first's refresh of the git index.
            turn_about(builds, round, |build| {
                hook(&world, build, "perf", "pre_tool", shell());
                std::thread::sleep(Duration::from_millis(30));
                change += 1;
                let changed = format!("// edit {change}\n");
                fs::write(world.repo.join("pkg0/mod0/src/file0.rs"), changed).unwrap();
                fs::write(world.repo.join(format!("generated{change}.txt")), "new\n").unwrap();
                hook(&world, build, "perf", "post_tool", shell())
            })
        },
    )
}

fn status(criterion: &mut Criterion) -> Measured {
    let world = World::new();
    let builds = builds(&world);
    for build in &builds {
        for seat in ["one", "two", "three"] {
            hook(&world, build, seat, "session_start", json!({}));
        }
        for i in 0..50 {
            let path = format!("src/f{i}.rs");
            hook(&world, build, "one", "pre_tool", edit(&path));
            hook(&world, build, "two", "pre_tool", edit(&path));
        }
    }
    let mut round = 0;
    bench(
        criterion,
        "status, 50 claims and 50 collisions",
        (50.0, 150.0),
        200,
        &builds,
        |builds| {
            round += 1;
            turn_about(builds, round, |build| {
                timed(&world, build, &["status", "--json"], b"")
            })
        },
    )
}

fn read() -> Value {
    json!({"tool": {"name": "Read", "kind": "read"}, "paths": ["src/lib.rs"]})
}

/// Tool calls that no switched-on feature needs (decision 0035), next to what they are compared
/// with: starting the process, and the same read where every switch is at its default. They have
/// no budgets. With `ACS_PERF_BASE_BIN` the base build shows what the call cost before.
fn unneeded_calls(criterion: &mut Criterion) -> Vec<Measured> {
    const SAMPLES: usize = 200;
    let budget = (NO_BUDGET, NO_BUDGET);
    let world_with = |features: &str| {
        let world = World::new();
        world.write(
            ".acs/policy/config.toml",
            &format!(
                "spec = \"acs/0.1\"

[features]
{features}
"
            ),
        );
        let builds = builds(&world);
        for build in &builds {
            hook(&world, build, "perf", "session_start", json!({}));
            // The first call writes the policy cache.
            hook(&world, build, "perf", "pre_tool", edit("src/util.rs"));
        }
        (world, builds)
    };
    let mut measured = Vec::new();
    let mut round = 0;

    let (world, builds) = world_with("");
    measured.push(bench(
        criterion,
        "process start (acs --version)",
        budget,
        SAMPLES,
        &builds,
        |builds| {
            round += 1;
            turn_about(builds, round, |build| {
                timed(&world, build, &["--version"], b"")
            })
        },
    ));
    for phase in ["pre_tool", "post_tool"] {
        measured.push(bench(
            criterion,
            &format!("{phase} read, every switch at its default"),
            budget,
            SAMPLES,
            &builds,
            |builds| {
                round += 1;
                turn_about(builds, round, |build| {
                    hook(&world, build, "perf", phase, read())
                })
            },
        ));
    }

    let (world, builds) = world_with(
        "lore = false
claims = false",
    );
    for phase in ["pre_tool", "post_tool"] {
        measured.push(bench(
            criterion,
            &format!("{phase} read, lore and claims off"),
            budget,
            SAMPLES,
            &builds,
            |builds| {
                round += 1;
                turn_about(builds, round, |build| {
                    hook(&world, build, "perf", phase, read())
                })
            },
        ));
    }

    let (world, builds) = world_with(
        "history = false
claims = false",
    );
    measured.push(bench(
        criterion,
        "post_tool shell, history and claims off",
        budget,
        SAMPLES,
        &builds,
        |builds| {
            round += 1;
            turn_about(builds, round, |build| {
                hook(&world, build, "perf", "pre_tool", shell());
                hook(&world, build, "perf", "post_tool", shell())
            })
        },
    ));
    measured
}

/// Prints one path and returns what is wrong with it.
fn judge(path: &Measured) -> Vec<String> {
    let (p50, p99) = (percentile(&path.head, 0.50), percentile(&path.head, 0.99));
    let label = &path.label;
    let (p50_budget, p99_budget) = (path.p50_budget_ms, path.p99_budget_ms);
    if p50_budget.is_finite() {
        println!(
            "{label}: n={} p50 {p50:.1} ms (budget {p50_budget}) p99 {p99:.1} ms (budget {p99_budget})",
            path.head.len()
        );
    } else {
        println!(
            "{label}: n={} p50 {p50:.1} ms p99 {p99:.1} ms",
            path.head.len()
        );
    }
    let mut problems = Vec::new();
    if p50 >= p50_budget {
        problems.push(format!("{label}: p50 {p50:.1} ms is over {p50_budget} ms"));
    }
    if p99 >= p99_budget {
        problems.push(format!("{label}: p99 {p99:.1} ms is over {p99_budget} ms"));
    }
    if !path.base.is_empty() {
        let (base_p50, base) = (percentile(&path.base, 0.50), percentile(&path.base, 0.99));
        println!("  base build: p50 {base_p50:.1} ms p99 {base:.1} ms");
        let floor = p99_budget * MIN_REGRESSION_OF_BUDGET;
        if p99 > base * MAX_REGRESSION && p99 - base > floor {
            problems.push(format!(
                "{label}: p99 {p99:.1} ms is over 20% and {floor:.0} ms worse than the base \
                 build's {base:.1} ms"
            ));
        }
    }
    problems
}

fn main() -> ExitCode {
    let mut criterion = Criterion::default().configure_from_args();
    let mut measured = vec![
        pre_tool_file_edit(&mut criterion),
        post_tool_shell(&mut criterion),
        status(&mut criterion),
    ];
    measured.extend(unneeded_calls(&mut criterion));
    criterion.final_summary();
    let problems: Vec<String> = measured.iter().flat_map(judge).collect();
    for problem in &problems {
        println!("FAILED {problem}");
    }
    if problems.is_empty() || std::env::var("ACS_PERF_NO_ASSERT").is_ok_and(|v| v == "1") {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

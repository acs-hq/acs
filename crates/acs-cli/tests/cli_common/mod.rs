//! Helpers for the `cli_*` tests: running `acs` with the `acs` binary on `PATH` (or with a
//! `PATH` that has `git` only), reading its output, and checking the 80-column rule.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

pub use crate::common::path_with_acs;
use crate::common::{World, acs_bin};

/// The directory holding the built `acs` binary.
pub fn bin_dir() -> PathBuf {
    Path::new(acs_bin()).parent().unwrap().to_path_buf()
}

/// A directory with only a symlink to `git` in it: a `PATH` where `acs` does not resolve.
#[cfg(unix)]
pub fn git_only_path(world: &World) -> std::ffi::OsString {
    let dir = world.tmp.path().join("git-only-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH");
    let link = dir.join("git");
    if !link.exists() {
        std::os::unix::fs::symlink(git, link).unwrap();
    }
    dir.into_os_string()
}

/// Runs `acs <args>` in `cwd` with `PATH` set to `path`.
pub fn run_with_path(world: &World, cwd: &Path, path: &std::ffi::OsStr, args: &[&str]) -> Output {
    Command::new(acs_bin())
        .args(args)
        .current_dir(cwd)
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", path)
        // Keep the harness-settings lookups inside the test sandbox.
        .env("HOME", world.tmp.path().join("fake-home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// Runs `acs <args>` in the world's repository, `acs` on `PATH`.
pub fn acs(world: &World, args: &[&str]) -> Output {
    run_with_path(world, &world.repo, &path_with_acs(), args)
}

/// Runs `acs <args>` in `cwd`, `acs` on `PATH`.
pub fn acs_in(world: &World, cwd: &Path, args: &[&str]) -> Output {
    run_with_path(world, cwd, &path_with_acs(), args)
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Parses stdout as the one JSON document `--json` promises.
pub fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not one JSON document ({err}):\n{}\nstderr: {}",
            stdout(out),
            stderr(out)
        )
    })
}

/// Fails if any line of `text` is wider than 80 characters.
pub fn assert_fits_80(text: &str) {
    for line in text.lines() {
        assert!(
            line.chars().count() <= 80,
            "{} columns: {line}\n--- in:\n{text}",
            line.chars().count()
        );
    }
}

pub fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout(out),
        stderr(out)
    );
}

pub fn assert_failure(out: &Output) {
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        stdout(out),
        stderr(out)
    );
}

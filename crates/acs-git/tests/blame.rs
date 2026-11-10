//! `git blame` reads: commits, original line numbers and trailers, against a real repository.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use acs_git::blame::blame_file;
use acs_git::trailers::SESSION_TOKEN;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn lines_get_their_commit_original_line_trailers_and_uncommitted_ones_none() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "first"]);
    fs::write(dir.join("f.txt"), "one\nnew\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "commit",
            "-q",
            "-m",
            "second\n\nACS-Session: ses_a\nACS-Session: ses_b",
        ],
    );
    fs::write(dir.join("f.txt"), "one\nnew\ntwo\nlocal\n").unwrap();

    let blame = blame_file(dir, "f.txt", '#').unwrap();
    assert_eq!(blame.lines.len(), 4);
    let texts: Vec<&str> = blame.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "new", "two", "local"]);
    let (first, second) = (
        blame.lines[0].commit.clone().unwrap(),
        blame.lines[1].commit.clone().unwrap(),
    );
    assert_ne!(first, second);
    assert_eq!(blame.lines[2].commit.as_deref(), Some(first.as_str()));
    assert_eq!(
        blame.lines[2].original_line, 2,
        "two was line 2 when written"
    );
    assert_eq!(blame.lines[1].original_line, 2);
    assert_eq!(blame.lines[3].commit, None);
    assert_eq!(blame.lines[3].line, 4);

    assert!(blame.trailers[&first].is_empty());
    let ids: Vec<&str> = blame.trailers[&second]
        .iter()
        .filter(|t| t.token == SESSION_TOKEN)
        .map(|t| t.value.as_str())
        .collect();
    assert_eq!(ids, ["ses_a", "ses_b"]);
}

#[test]
fn an_untracked_file_is_all_uncommitted_and_a_missing_one_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main", "."]);
    fs::write(dir.join("new.txt"), "a\nb\n").unwrap();
    let blame = blame_file(dir, "new.txt", '#').unwrap();
    assert_eq!(blame.lines.len(), 2);
    assert!(blame.lines.iter().all(|l| l.commit.is_none()));
    assert!(blame_file(dir, "missing.txt", '#').is_err());
}

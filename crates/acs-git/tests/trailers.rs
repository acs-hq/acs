//! Our trailer rules against `git interpret-trailers`, which they re-implement.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::process::{Command, Stdio};

use acs_git::trailers::{Trailer, add_trailers};

fn git_says(message: &str, trailers: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .args([
            "interpret-trailers",
            "--if-exists",
            "addIfDifferent",
            "--no-divider",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    for trailer in trailers {
        command.arg("--trailer").arg(trailer);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(message.as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
}

// A message without a final newline is left out: git joins the trailer to the subject there, which
// is not what a commit message file ever looks like.
#[test]
fn matches_git_interpret_trailers_on_comment_free_messages() {
    let cases = [
        "fix\n",
        "fix\n\nbody text\n",
        "fix\n\nbody text\n\nSigned-off-by: A <a@x.org>\n",
        "fix\n\nbody\n\nFixes: #1\nReviewed-by: B <b@x.org>\n",
        "fix: colon in subject\n",
        "fix: colon in subject\n\nFixes: 12\n",
        "fix\n\nACS-Session: ses_one\n",
        "fix\n\nACS-Session: ses_two\n",
        "fix\n\nprose line\nSigned-off-by: A <a@x.org>\n",
        "fix\n\nprose line\nnot a trailer: but has a colon\nmore prose\n",
        "fix\n\nFixes: 1\n  continued over two lines\n",
        "fix\n\npara one\n\npara two\n",
        "fix\n\n(cherry picked from commit abc123)\n",
    ];
    for case in cases {
        let ours = add_trailers(case, &[Trailer::session("ses_one")], '#')
            .unwrap_or_else(|| case.to_owned());
        let theirs = git_says(case, &["ACS-Session: ses_one"]);
        assert_eq!(ours, theirs, "message: {case:?}");
    }
}

//! The write-paths audit before first testers (`docs/security/write-paths.md`): every place ACS
//! itself writes to a person's work tree, git history, git config, hooks or remote.
//!
//! Each test reproduces one finding of the report, named in its comment (W1, W2, ...). A test for a
//! finding that is still open fails on purpose; the report lists which ones.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use acs_lore::{Proposal, ProposalStore, Proposer};
use common::{Agent, World, git};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// One file-tool call: the hook before, the change, the hook after.
fn edit(agent: &Agent<'_>, path: &str, text: &str) {
    agent.pre_edit(path);
    agent.world.write(path, text);
    agent.post_edit(path);
}

fn session_id(agent: &Agent<'_>) -> String {
    agent.info().session_id.to_string()
}

fn undo(world: &World, args: &[&str]) -> Output {
    let mut full = vec!["undo"];
    full.extend_from_slice(args);
    world.run(&full, b"", &world.repo)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A git command as a person in a plain terminal would run it: `acs` on `PATH`, no harness
/// environment (this test may itself run inside Claude Code).
fn git_cmd(world: &World, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("PATH", common::path_with_acs())
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CLAUDECODE")
        .env_remove("ANTIGRAVITY_CONVERSATION_ID")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null());
    command
}

fn git_run(world: &World, args: &[&str]) -> Output {
    git_cmd(world, &world.repo, args).output().unwrap()
}

fn git_out(world: &World, args: &[&str]) -> String {
    let out = git_run(world, args);
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A world whose repository is set up for ACS (`.acs/policy`) with the hooks installed.
fn world_with_hooks() -> World {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".acs/policy")).unwrap();
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    world
}

/// Adds a linked worktree inside the main checkout (as Claude Code's subagents use) and returns
/// its canonical path.
fn nested_worktree(world: &World) -> PathBuf {
    git(
        &world.repo,
        &world.git_config,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            ".claude/worktrees/wp",
        ],
    );
    dunce::canonicalize(world.repo.join(".claude/worktrees/wp")).unwrap()
}

// ---------------------------------------------------------------------------------------------
// 1. `acs undo --apply`
// ---------------------------------------------------------------------------------------------

/// W1 (data loss). A file that existed before the session, but of which ACS took no pre-image,
/// must not be taken for a file the session created. The pre-image is missing whenever the
/// pre-tool step did not capture one while the post-tool step still recorded the change: the
/// pre-tool hook failed open or timed out (Claude Code then runs the tool anyway), the state was
/// busy, or `history` was switched on between the two events. Today the planner treats a tool
/// change with no before-hash as a creation and `--apply` deletes the person's file.
#[test]
fn w1_undo_never_deletes_a_file_that_existed_before_the_session() {
    let world = World::new();
    // The person's own notes: untracked, so git cannot bring them back.
    world.write("notes.txt", "my own notes, never committed\n");
    let agent = Agent::new(&world, "claude-code", "w1");
    agent.start();
    // The pre-tool event never reached ACS (the hook failed open); the edit and the post-tool
    // event did.
    world.write(
        "notes.txt",
        "my own notes, never committed\nplus a line by the agent\n",
    );
    agent.post_edit("notes.txt");
    agent.end();
    let sid = session_id(&agent);

    let plan = undo(&world, &[&sid, "--json"]);
    let view: Value = serde_json::from_slice(&plan.stdout).unwrap();
    let out = undo(&world, &[&sid, "--apply"]);
    assert!(
        world.repo.join("notes.txt").exists(),
        "undo deleted a file that existed before the session (plan: {view}); output: {}",
        stdout(&out)
    );
    assert_ne!(view["files"][0]["action"], "delete", "{view}");
}

/// W1, the case the rule exists for: a file the session really created, then edited, is still
/// deleted by undo (it must keep working after the fix).
#[test]
fn w1_a_file_the_session_created_and_then_edited_is_still_deleted() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "w1b");
    agent.start();
    edit(&agent, "made.txt", "first version\n");
    edit(&agent, "made.txt", "second version\n");
    agent.end();
    let sid = session_id(&agent);
    let out = undo(&world, &[&sid, "--apply"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!world.repo.join("made.txt").exists(), "{}", stdout(&out));
}

/// W2 (wrong target). Since decision 0032 a session started in the main checkout that edits a
/// file in a linked worktree records it under the worktree's own path (`src/lib.rs`). Undo
/// resolves recorded paths against the work tree it is run in, so `acs undo` run where the session
/// ran (the main checkout) looks at the main checkout's `src/lib.rs`: it reports "already undone"
/// and leaves the worktree's file changed (or, when the main copy differs, merges the reverse
/// change into a file the session never touched).
#[test]
fn w2_undo_acts_on_the_worktree_the_session_changed() {
    let world = World::new();
    let wt = nested_worktree(&world);
    let agent = Agent::new(&world, "claude-code", "w2");
    agent.start();
    let target = wt.join("src/lib.rs");
    agent.pre_edit(&target.to_string_lossy());
    fs::write(&target, "pub fn changed_in_the_worktree() {}\n").unwrap();
    agent.post_edit(".claude/worktrees/wp/src/lib.rs");
    agent.end();
    let sid = session_id(&agent);

    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "pub fn lib() {}\n",
        "the worktree's file was not undone; output: {}",
        stdout(&out)
    );
    assert_eq!(
        fs::read_to_string(world.repo.join("src/lib.rs")).unwrap(),
        "pub fn lib() {}\n"
    );
}

/// A session started in the main checkout that changes `src/lib.rs` of the worktree
/// `.claude/worktrees/wp`; returns the worktree's root and the session id.
fn w2_session(world: &World) -> (PathBuf, String) {
    let wt = nested_worktree(world);
    let agent = Agent::new(world, "claude-code", "w2");
    agent.start();
    let target = wt.join("src/lib.rs");
    agent.pre_edit(&target.to_string_lossy());
    fs::write(
        &target,
        "pub fn changed_in_the_worktree() {}
",
    )
    .unwrap();
    agent.post_edit(".claude/worktrees/wp/src/lib.rs");
    agent.end();
    (wt, session_id(&agent))
}

/// W2, run from the worktree itself: the same file is undone, and the main checkout's copy is
/// left alone.
#[test]
fn w2_undo_run_in_the_worktree_acts_on_its_file() {
    let world = World::new();
    let (wt, sid) = w2_session(&world);
    let out = world.run(&["undo", &sid, "--apply"], b"", &wt);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        fs::read_to_string(wt.join("src/lib.rs")).unwrap(),
        "pub fn lib() {}
",
        "{}",
        stdout(&out)
    );
    assert_eq!(
        fs::read_to_string(world.repo.join("src/lib.rs")).unwrap(),
        "pub fn lib() {}
"
    );
}

/// W2, the worktree removed since: undo writes nothing, not even into the main checkout's file
/// of the same name (here equal to what the session left, which undo used to "restore"), and
/// says why.
#[test]
fn w2_undo_leaves_files_of_a_removed_worktree_alone() {
    let world = World::new();
    let (_wt, sid) = w2_session(&world);
    git(
        &world.repo,
        &world.git_config,
        &["worktree", "remove", "--force", ".claude/worktrees/wp"],
    );
    let main_copy = world.repo.join("src/lib.rs");
    fs::write(
        &main_copy,
        "pub fn changed_in_the_worktree() {}
",
    )
    .unwrap();

    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(
        fs::read_to_string(&main_copy).unwrap(),
        "pub fn changed_in_the_worktree() {}
",
        "{}",
        stdout(&out)
    );
    let text = stdout(&out);
    assert!(text.contains(".claude/worktrees/wp/src/lib.rs"), "{text}");
    assert!(text.contains("no longer a worktree"), "{text}");
}

/// W3 (surprising). With `core.autocrlf = true` (the Git for Windows default) the work tree has
/// CRLF while `HEAD` holds LF. Undo decides whether a restored file can go into the undo commit
/// by comparing the raw work-tree bytes with the `HEAD` blob, so committed session changes are
/// restored in the work tree but never committed, and the output says nothing about a commit.
#[test]
fn w3_committed_changes_get_an_undo_commit_under_autocrlf() {
    let world = World::new();
    fs::write(
        &world.git_config,
        "[user]\n\temail = sara@example.com\n\tname = Sara\n[core]\n\tautocrlf = true\n",
    )
    .unwrap();
    // Check `a.txt` out again so the work tree has CRLF, as on a Windows clone.
    fs::remove_file(world.repo.join("a.txt")).unwrap();
    git(&world.repo, &world.git_config, &["checkout", "--", "a.txt"]);
    assert_eq!(fs::read(world.repo.join("a.txt")).unwrap(), b"a\r\n");

    let agent = Agent::new(&world, "claude-code", "w3");
    agent.start();
    edit(&agent, "a.txt", "committed change\r\n");
    agent.end();
    let sid = session_id(&agent);
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", &format!("work\n\nACS-Session: {sid}")],
    );

    let out = undo(&world, &[&sid, "--apply", "--json"]);
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(fs::read(world.repo.join("a.txt")).unwrap(), b"a\r\n");
    assert_eq!(view["commit"]["status"], "committed", "{view}");
    assert_eq!(git_out(&world, &["show", "HEAD:a.txt"]), "a");
}

/// W4 (surprising). When a hook of the person rejects the undo commit, the files stay restored
/// (decision 0030), but `commit_only` has already run `git add` on them: the index now holds the
/// restored versions, and the person's next `git commit` takes them along.
#[test]
fn w4_a_rejected_undo_commit_leaves_the_index_as_it_was() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "w4");
    agent.start();
    edit(&agent, "a.txt", "committed change\n");
    agent.end();
    let sid = session_id(&agent);
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", &format!("work\n\nACS-Session: {sid}")],
    );
    let hook = world.repo.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho 'no commits today' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(fs::read_to_string(world.repo.join("a.txt")).unwrap(), "a\n");
    assert_eq!(
        git_out(&world, &["diff", "--cached", "--name-only"]),
        "",
        "the failed undo commit left files staged"
    );
}

fn dir_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let out = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success(), "mklink /J failed: {out:?}");
    }
}

/// Issue 77. A folder that became a link (a junction on Windows) after the session: undo would
/// write the old content through it, to wherever it points, outside the repository included.
#[test]
fn undo_never_writes_through_a_folder_that_became_a_link() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "link-dir");
    agent.start();
    edit(&agent, "src/lib.rs", "pub fn changed() {}\n");
    agent.end();
    let sid = session_id(&agent);

    let outside = world.repo.parent().unwrap().join("outside");
    fs::rename(world.repo.join("src"), &outside).unwrap();
    dir_link(&outside, &world.repo.join("src"));

    let out = undo(&world, &[&sid, "--apply"]);
    assert_eq!(
        fs::read_to_string(outside.join("lib.rs")).unwrap(),
        "pub fn changed() {}\n",
        "undo wrote through the link; output: {}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("link"), "{}", stdout(&out));
}

/// Issue 77 (Unix only: symlinks need developer mode on Windows). A change made through a
/// symlinked file: undo must never replace the link with a regular file. Either it restores the
/// target (the session recorded the real path) or it reports the link and leaves it alone.
#[cfg(unix)]
#[test]
fn undo_never_replaces_a_symlink_with_a_file() {
    let world = World::new();
    let link = world.repo.join("link.txt");
    std::os::unix::fs::symlink("a.txt", &link).unwrap();
    let agent = Agent::new(&world, "claude-code", "link-file");
    agent.start();
    agent.pre_edit("link.txt");
    fs::write(&link, "changed through the link\n").unwrap();
    agent.post_edit("link.txt");
    agent.end();
    let sid = session_id(&agent);

    let out = undo(&world, &[&sid, "--apply"]);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced; output: {}",
        stdout(&out)
    );
    let target = fs::read_to_string(world.repo.join("a.txt")).unwrap();
    assert!(
        target == "a\n" || stdout(&out).contains("link"),
        "the target kept the change and nothing was reported: {}",
        stdout(&out)
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Git hook install and removal
// ---------------------------------------------------------------------------------------------

/// W5 (a person's commit rejected; Unix only). A hook that is not a shell script is moved to
/// `<hook>.acs-orig` and replaced by a wrapper that `exec`s it. The wrapper is always made
/// executable, so a non-executable hook (one the person switched off with `chmod -x`) goes from
/// "skipped by git" to "run, and `exec` fails with 126": every commit is rejected.
#[cfg(unix)]
#[test]
fn w5_a_disabled_non_shell_hook_stays_disabled_after_install() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    fs::create_dir_all(world.repo.join(".acs/policy")).unwrap();
    let hook = world.repo.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/usr/bin/env python3\nraise SystemExit(1)\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
    // Before ACS: git skips the hook and the commit goes through.
    world.write("a.txt", "one\n");
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    let before = git_run(&world, &["commit", "-q", "-m", "before acs"]);
    assert!(before.status.success(), "{}", stderr(&before));

    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    world.write("a.txt", "two\n");
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    let after = git_run(&world, &["commit", "-q", "-m", "after acs"]);
    assert!(
        after.status.success(),
        "installing ACS turned a disabled hook into a failing one: {}",
        stderr(&after)
    );
}

/// W6 (surprising, harmless). A hook whose last line has no newline gets one when ACS inserts its
/// block after the shebang, and removing the block keeps it: `acs init` then `acs uninstall` does
/// not leave the file byte for byte as it was (decision 0039 promises that, with one stated
/// exception for a hook holding only `#!/bin/sh`).
#[test]
fn w6_removing_the_block_restores_a_hook_without_a_final_newline() {
    let world = World::new();
    let hook = world.repo.join(".git/hooks/pre-commit");
    let original = "#!/usr/bin/env bash";
    fs::write(&hook, original).unwrap();
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    acs_git::hooks::uninstall(&layout).unwrap();
    assert_eq!(fs::read_to_string(&hook).unwrap(), original);
}

/// W6, the same with CRLF line endings (a hook committed from Windows): install and removal must
/// leave it byte for byte. This one passes; it guards the CRLF handling.
#[test]
fn w6_removing_the_block_restores_a_crlf_hook() {
    let world = World::new();
    let hook = world.repo.join(".git/hooks/pre-commit");
    let original = "#!/bin/sh\r\nnpm test\r\n";
    fs::write(&hook, original).unwrap();
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    assert!(fs::read_to_string(&hook).unwrap().contains("acs git-hook"));
    acs_git::hooks::uninstall(&layout).unwrap();
    assert_eq!(fs::read_to_string(&hook).unwrap(), original);
}

// ---------------------------------------------------------------------------------------------
// 4. What `acs sync` publishes
// ---------------------------------------------------------------------------------------------

/// W7 (unexpected publication). Session records go to the remote (`refs/acs/ledger/*`) and the
/// intent is redacted with the hook's rules first. The rules miss common shapes: Stripe keys
/// (`sk_live_...`, the list has `sk-` only) and a token after `user:` (`curl -u sara:ghp_...`,
/// the prefix is only checked at the start of a word).
#[test]
fn w7_the_record_that_is_pushed_carries_no_tokens_from_the_intent() {
    let world = World::new();
    let agent = Agent::new(&world, "claude-code", "w7");
    agent.start();
    edit(&agent, "a.txt", "changed\n");
    world
        .db()
        .set_seat_intent(
            &agent.seat_id(),
            "Rotate sk_live_51HxQabcdefghijklmnop then check with curl -u sara:ghp_abcdefghijklmnop12345",
        )
        .unwrap();
    agent.end();
    let out = world.run(&["log", "--json"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");
    let records: Value = serde_json::from_slice(&out.stdout).unwrap();
    let intent = records[0]["intent"].as_str().unwrap().to_owned();
    assert!(!intent.contains("sk_live_51HxQ"), "{intent}");
    assert!(!intent.contains("ghp_abcdef"), "{intent}");
}

// ---------------------------------------------------------------------------------------------
// 5. `acs lore review --accept`
// ---------------------------------------------------------------------------------------------

fn propose_replacement(world: &World, id: &str, body: &str) {
    let store = ProposalStore::new(world.repo_home().pending_dir().join("lore"));
    store
        .save(&Proposal {
            proposal_id: id.to_owned(),
            replaces: Some("lore_pay".parse().unwrap()),
            scope: vec!["payments/**".to_owned()],
            body: body.to_owned(),
            why: None,
            proposed_by: Proposer {
                pilot: "omar@example.com".to_owned(),
                harness: "claude-code".to_owned(),
                session_id: "ses_test".to_owned(),
            },
            proposed_at: acs_core::Timestamp::now(),
            head: None,
        })
        .unwrap();
}

/// W8 (surprising). Accepting commits on `acs/lore-<id>` with `update-ref`, which is safe only
/// while that branch is not checked out. A reviewer who checked the branch out (to edit the entry
/// before merging) and then accepts a second proposal for the same entry has their current branch
/// moved under them: the index and work tree still hold the old commit, so `git status` shows the
/// new entry as a staged reversal and the next `git commit -a` undoes it.
#[test]
fn w8_accepting_lore_never_moves_a_checked_out_branch() {
    let world = World::new();
    let head = git_out(&world, &["rev-parse", "HEAD"]);
    world.write("payments/refund.ts", "line\n");
    world.write(
        ".acs/policy/lore/payments/money.md",
        &format!(
            "+++\nid = \"lore_pay\"\nscope = [\"payments/**\"]\nowner = \"lina@example.com\"\n\
             written_against = \"{head}\"\n+++\n\nAmounts are integer cents.\n"
        ),
    );
    git(&world.repo, &world.git_config, &["add", "-A"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", "lore"],
    );

    propose_replacement(&world, "prop_one", "Amounts are i64 cents.");
    let first = world.run(
        &["lore", "review", "--accept", "prop_one", "--json"],
        b"",
        &world.repo,
    );
    assert!(first.status.success(), "{first:?}");
    git(
        &world.repo,
        &world.git_config,
        &["checkout", "-q", "acs/lore-pay"],
    );
    let before = git_out(&world, &["rev-parse", "HEAD"]);

    propose_replacement(&world, "prop_two", "Amounts are i64 cents, never floats.");
    let _second = world.run(
        &["lore", "review", "--accept", "prop_two", "--json"],
        b"",
        &world.repo,
    );
    assert_eq!(
        git_out(&world, &["rev-parse", "HEAD"]),
        before,
        "the checked-out branch moved"
    );
    assert_eq!(git_out(&world, &["status", "--porcelain"]), "");
}

// ---------------------------------------------------------------------------------------------
// 6. The pending-commit marker and the prepare-commit-msg trailer
// ---------------------------------------------------------------------------------------------

/// W9 (a person's commit altered). An agent's shell command that contains `git commit` leaves a
/// pending-commit marker for ten minutes, and only a commit consumes it. When the commit never
/// runs (`cargo test && git commit ...` with failing tests, the common case), the person's next
/// commit in that work tree is taken for the agent's: it gets the agent's `ACS-Session` trailer.
/// Issue 38 describes the rarer trigger (another hook rejecting the agent's commit).
#[test]
#[ignore = "W9 is still open (issue 38); remove this when it is fixed"]
fn w9_a_commit_the_agent_never_ran_does_not_mark_the_persons_commit() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "w9");
    agent.start();
    agent.pre_shell("false && git commit -m wip");
    // `false` failed, so git never ran. The person commits a minute later.
    world.write("a.txt", "the person's change\n");
    git(&world.repo, &world.git_config, &["add", "a.txt"]);
    let out = git_run(&world, &["commit", "-q", "-m", "mine"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let message = git_out(&world, &["log", "-1", "--format=%B"]);
    assert!(
        !message.contains("ACS-Session"),
        "the person's commit was attributed to the agent: {message}"
    );
}

/// W9, the rejection side: with the stale marker, the person's commit is judged as an agent's,
/// and sealed paths (which only bind agents) reject it.
#[test]
#[ignore = "W9 is still open (issue 38); remove this when it is fixed"]
fn w9_a_stale_marker_does_not_reject_a_persons_commit_of_a_sealed_path() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "w9b");
    agent.start();
    agent.pre_shell("false && git commit -m wip");
    world.write(".acs/policy/config.toml", "spec = \"acs/0.1\"\n");
    git(
        &world.repo,
        &world.git_config,
        &["add", ".acs/policy/config.toml"],
    );
    let out = git_run(
        &world,
        &["commit", "-q", "-m", "policy change by the person"],
    );
    assert!(
        out.status.success(),
        "a person's commit was rejected as an agent's: {}",
        stderr(&out)
    );
}

//! The git backstop end to end: real `git commit` and `git push` with the hooks installed and the
//! built `acs` binary on `PATH`, next to simulated agents (canonical events).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use acs_core::{Claim, ClaimOrigin, ClaimScope, RepoPath, Timestamp};
use common::{Agent, World, acs_bin};
use serde_json::json;

/// A git command as a person in a plain terminal would run it: `acs` on `PATH`, no harness
/// environment (this test may itself run inside Claude Code).
fn git_cmd(world: &World, dir: &Path, args: &[&str]) -> Command {
    let acs_dir = Path::new(acs_bin()).parent().unwrap().to_path_buf();
    let mut paths = vec![acs_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CLAUDECODE")
        .env_remove("ANTIGRAVITY_CONVERSATION_ID")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE");
    command
}

fn git(world: &World, dir: &Path, args: &[&str]) -> Output {
    git_cmd(world, dir, args).output().unwrap()
}

fn git_ok(world: &World, dir: &Path, args: &[&str]) -> String {
    let out = git(world, dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A world whose repository is set up for ACS (`.acs/policy`) with the hooks installed.
fn world_with_hooks() -> World {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".acs/policy")).unwrap();
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    world
}

fn stage(world: &World, rel: &str, text: &str) {
    world.write(rel, text);
    git_ok(world, &world.repo, &["add", "--", rel]);
}

fn message_of_head(world: &World, dir: &Path) -> String {
    git_ok(world, dir, &["log", "-1", "--format=%B"])
}

fn head(world: &World, dir: &Path) -> String {
    git_ok(world, dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

#[test]
fn an_agent_commit_touching_another_seats_claim_is_rejected_with_a_clear_message() {
    let world = world_with_hooks();
    let holder = Agent::new(&world, "claude-code", "c1");
    let committer = Agent::new(&world, "antigravity", "g1");
    holder.start();
    committer.start();
    assert_eq!(holder.pre_edit("src/lib.rs").claims_acquired.len(), 1);
    // The committing agent announces its commit; then plain git runs.
    committer.pre_shell("git commit -m work");
    let before = head(&world, &world.repo);
    stage(&world, "src/lib.rs", "pub fn changed() {}\n");
    stage(&world, "a.txt", "fine\n");

    let out = git(&world, &world.repo, &["commit", "-q", "-m", "work"]);
    assert!(!out.status.success(), "the commit must be rejected");
    let text = stderr(&out);
    assert!(text.contains("commit rejected"), "{text}");
    assert!(text.contains("agent session ses_"), "{text}");
    assert!(
        text.contains("`src/lib.rs` is claimed by sara@example.com"),
        "{text}"
    );
    assert!(text.contains("claude-code"), "{text}");
    assert!(text.contains("claimed just now"), "{text}");
    assert!(text.contains("git restore --staged"), "{text}");
    assert!(
        !text.contains("--no-verify"),
        "agents are not pointed at the override: {text}"
    );
    assert!(
        !text.contains("a.txt"),
        "only the offending path is named: {text}"
    );
    assert_eq!(head(&world, &world.repo), before);

    // The rejected attempt consumed the marker: the next commit is a person's, and the claim of
    // an agent working in this very work tree does not stop a person.
    let human = git(&world, &world.repo, &["commit", "-q", "-m", "mine"]);
    assert!(human.status.success(), "{}", stderr(&human));
}

#[test]
fn an_agent_may_commit_its_own_claims_and_gets_its_session_trailer() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    assert_eq!(agent.pre_edit("src/lib.rs").claims_acquired.len(), 1);
    agent.pre_shell("git commit -am work");
    stage(&world, "src/lib.rs", "pub fn mine() {}\n");

    let out = git(&world, &world.repo, &["commit", "-q", "-m", "work"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let message = message_of_head(&world, &world.repo);
    let session = agent.info().session_id;
    assert!(
        message.contains(&format!("\n\nACS-Session: {session}")),
        "{message}"
    );

    // The marker is gone: a person's next commit gets no trailer for it.
    stage(&world, "a.txt", "human edit\n");
    let human = git(&world, &world.repo, &["commit", "-q", "-m", "human"]);
    assert!(human.status.success(), "{}", stderr(&human));
    assert!(!message_of_head(&world, &world.repo).contains("ACS-Session"));
}

#[test]
fn a_human_commit_of_an_agents_work_in_this_worktree_succeeds_with_a_trailer() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    assert_eq!(agent.pre_edit("src/lib.rs").claims_acquired.len(), 1);
    world.write("src/lib.rs", "pub fn agent_wrote_this() {}\n");
    agent.post_edit("src/lib.rs");
    git_ok(&world, &world.repo, &["add", "src/lib.rs"]);

    let out = git(
        &world,
        &world.repo,
        &["commit", "-q", "-m", "take the agent's change"],
    );
    assert!(
        out.status.success(),
        "claimed by an agent in this worktree: {}",
        stderr(&out)
    );
    let session = agent.info().session_id;
    let message = message_of_head(&world, &world.repo);
    assert!(
        message.contains(&format!("ACS-Session: {session}")),
        "{message}"
    );
    assert_eq!(message.matches("ACS-Session").count(), 1);

    // Already committed: committing something else does not repeat the trailer.
    stage(&world, "a.txt", "unrelated\n");
    git_ok(&world, &world.repo, &["commit", "-q", "-m", "unrelated"]);
    assert!(!message_of_head(&world, &world.repo).contains("ACS-Session"));
}

#[test]
fn a_human_who_edits_on_top_of_an_agents_change_still_gets_the_trailer() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    agent.pre_edit("src/util.rs");
    world.write("src/util.rs", "pub fn agent() {}\n");
    agent.post_edit("src/util.rs");
    // The person adds to it before committing: the staged content is no longer the agent's.
    world.write("src/util.rs", "pub fn agent() {}\npub fn human() {}\n");
    git_ok(&world, &world.repo, &["add", "src/util.rs"]);
    git_ok(&world, &world.repo, &["commit", "-q", "-m", "both"]);
    let session = agent.info().session_id;
    assert!(message_of_head(&world, &world.repo).contains(&format!("ACS-Session: {session}")));
}

#[test]
fn two_sessions_in_one_commit_give_two_trailers() {
    let world = world_with_hooks();
    let one = Agent::new(&world, "claude-code", "c1");
    let two = Agent::new(&world, "antigravity", "g1");
    for (agent, file) in [(&one, "src/lib.rs"), (&two, "src/util.rs")] {
        agent.start();
        agent.pre_edit(file);
        world.write(file, &format!("// {}\n", agent.harness));
        agent.post_edit(file);
    }
    git_ok(&world, &world.repo, &["add", "-A"]);
    git_ok(&world, &world.repo, &["commit", "-q", "-m", "both agents"]);
    let message = message_of_head(&world, &world.repo);
    assert!(
        message.contains(&format!("ACS-Session: {}", one.info().session_id)),
        "{message}"
    );
    assert!(
        message.contains(&format!("ACS-Session: {}", two.info().session_id)),
        "{message}"
    );
}

#[test]
fn sealed_paths_stop_an_agent_commit_but_not_a_person() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    agent.pre_shell("git commit -m secrets");
    stage(&world, ".env", "TOKEN=1\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "secrets"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("`.env` is sealed by `**/.env*`"), "{text}");
    assert!(text.contains("ask your pilot to commit them"), "{text}");

    // A person can commit sealed paths: sealing is for agents.
    let human = git(&world, &world.repo, &["commit", "-q", "-m", "config"]);
    assert!(human.status.success(), "{}", stderr(&human));
}

#[test]
fn a_human_commit_of_a_file_claimed_in_another_worktree_is_rejected() {
    let world = world_with_hooks();
    git_ok(
        &world,
        &world.repo,
        &["worktree", "add", "-q", "-b", "side", "../wt2"],
    );
    let wt2: PathBuf = dunce::canonicalize(world.repo.parent().unwrap().join("wt2")).unwrap();
    // An agent working in the other worktree claims `a.txt` (events carry that cwd).
    let event = json!({
        "spec": "acs/0.1", "harness": "claude-code", "harness_session_id": "w2",
        "phase": "pre_tool", "cwd": wt2,
        "tool": {"name": "Edit", "kind": "edit"}, "paths": ["a.txt"],
    });
    let decision = world.hook("pre_tool", &event);
    assert_eq!(decision.claims_acquired.len(), 1, "{decision:?}");

    // A person in the main worktree may not commit it...
    stage(&world, "a.txt", "main worktree edit\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "mine"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("by a person"), "{text}");
    assert!(
        text.contains("`a.txt` is claimed by sara@example.com (claude-code"),
        "{text}"
    );
    assert!(text.contains("git commit --no-verify"), "{text}");
    // ...unless they override explicitly.
    assert!(
        git(
            &world,
            &world.repo,
            &["commit", "-q", "--no-verify", "-m", "mine"]
        )
        .status
        .success()
    );

    // In the other worktree, the same claim is the normal agent-edits-human-commits flow, and the
    // hooks (shared through the common directory) ran there too.
    fs::write(wt2.join("a.txt"), "agent edit\n").unwrap();
    git_ok(&world, &wt2, &["add", "a.txt"]);
    let there = git(&world, &wt2, &["commit", "-q", "-m", "theirs"]);
    assert!(there.status.success(), "{}", stderr(&there));
}

/// A world with a committed policy (so a linked worktree's checkout has it and gets the git hook
/// checks) and a worktree inside the main checkout, as Claude Code's subagents use.
fn world_with_nested_worktree() -> (World, PathBuf) {
    let world = World::new();
    world.write(".acs/policy/guards.toml", "");
    git_ok(&world, &world.repo, &["add", "-A"]);
    git_ok(&world, &world.repo, &["commit", "-q", "-m", "policy"]);
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    git_ok(
        &world,
        &world.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            ".claude/worktrees/wp",
        ],
    );
    let wt = dunce::canonicalize(world.repo.join(".claude/worktrees/wp")).unwrap();
    (world, wt)
}

/// A session that runs in the main checkout edits a file in the worktree and commits there, with
/// `commit` as its shell command. The commit must be the agent's own, trailer included.
fn commit_from_the_main_checkout(commit: impl Fn(&Path) -> String) {
    let (world, wt) = world_with_nested_worktree();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    let file = wt.join("src/lib.rs");
    assert_eq!(
        agent
            .pre_edit(&file.to_string_lossy())
            .claims_acquired
            .len(),
        1
    );
    fs::write(&file, "pub fn mine() {}\n").unwrap();
    git_ok(&world, &wt, &["add", "src/lib.rs"]);

    let command = commit(&wt);
    agent.pre_shell(&command);
    let out = git(&world, &wt, &["commit", "-q", "-m", "work"]);
    assert!(out.status.success(), "`{command}`: {}", stderr(&out));
    let session = agent.info().session_id;
    let message = message_of_head(&world, &wt);
    assert!(
        message.contains(&format!("\n\nACS-Session: {session}")),
        "`{command}`: {message}"
    );
}

#[test]
fn an_agent_commit_after_cd_into_a_worktree_is_the_agents_own() {
    commit_from_the_main_checkout(|_| "cd .claude/worktrees/wp && git commit -m work".to_owned());
}

#[test]
fn an_agent_commit_after_cd_to_an_absolute_worktree_path_is_the_agents_own() {
    commit_from_the_main_checkout(|wt| {
        let dir = wt.to_string_lossy().replace('\\', "/");
        format!("cd \"{dir}\" && git add -A && git commit -m work")
    });
}

#[test]
fn an_agent_commit_with_git_dash_c_into_a_worktree_is_the_agents_own() {
    commit_from_the_main_checkout(|_| "git -C .claude/worktrees/wp commit -m work".to_owned());
}

#[test]
fn a_commit_from_an_agents_shell_that_acs_cannot_place_is_not_told_to_skip_the_hooks() {
    let (world, wt) = world_with_nested_worktree();
    let agent = Agent::new(&world, "claude-code", "c1");
    agent.start();
    let file = wt.join("src/lib.rs");
    agent.pre_edit(&file.to_string_lossy());
    fs::write(&file, "pub fn mine() {}\n").unwrap();
    git_ok(&world, &wt, &["add", "src/lib.rs"]);

    // The directory is not visible in the command, so the marker stays with the session's work
    // tree and the hook in the worktree cannot tell whose commit this is.
    agent.pre_shell("cd \"$WT\" && git commit -m work");
    let out = git_cmd(&world, &wt, &["commit", "-q", "-m", "work"])
        .env("CLAUDECODE", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("from an agent's shell"), "{text}");
    assert!(text.contains("acs_release"), "{text}");
    assert!(!text.contains("--no-verify"), "{text}");

    // The same rejection for a person keeps the advice a person can follow.
    let out = git(&world, &wt, &["commit", "-q", "-m", "work"]);
    let text = stderr(&out);
    assert!(text.contains("by a person"), "{text}");
    assert!(text.contains("git commit --no-verify"), "{text}");
}

/// A claim on `src/util.rs` held on another machine since `acquired_at`, as a sync stores it, with
/// a fresh fetch.
fn claim_on_another_machine(world: &World, acquired_at: Timestamp) {
    let db = world.db();
    let now = Timestamp::now();
    let claim = Claim {
        claim_id: "clm_remote1".parse().unwrap(),
        seat_id: "seat_remote".parse().unwrap(),
        pilot: "omar@example.com".into(),
        scope: ClaimScope::File {
            file: RepoPath::try_from("src/util.rs".to_owned()).unwrap(),
        },
        target_branch: "main".into(),
        acquired_at,
        heartbeat_at: acquired_at,
        ttl_secs: 900,
        origin: ClaimOrigin::Agent,
    };
    db.replace_remote_claims("0de61b1110462435", &[claim], now)
        .unwrap();
    db.record_sync_success(now).unwrap();
}

#[test]
fn a_commit_keeps_a_file_only_if_its_claim_came_before_the_other_machines() {
    let world = world_with_hooks();
    let agent = Agent::new(&world, "claude-code", "c1");
    assert_eq!(
        agent.pre_edit("src/util.rs").outcome,
        acs_core::Outcome::Allow
    );

    // The other machine claimed the file after this one: the gate lets this seat go on, and the
    // commit must not stop it.
    claim_on_another_machine(&world, Timestamp::now());
    stage(&world, "src/util.rs", "pub fn util2() {}\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "x"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // Had it claimed the file first, the commit would be the one stopped.
    let earlier = Timestamp::from_second(Timestamp::now().as_second() - 600).unwrap();
    claim_on_another_machine(&world, earlier);
    stage(&world, "src/util.rs", "pub fn util3() {}\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "y"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("is claimed by omar@example.com"));
}

#[test]
fn a_human_commit_of_a_file_claimed_on_another_machine_is_rejected() {
    let world = world_with_hooks();
    claim_on_another_machine(&world, Timestamp::now());

    stage(&world, "src/util.rs", "pub fn util2() {}\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "x"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(
        text.contains("`src/util.rs` is claimed by omar@example.com (another machine"),
        "{text}"
    );
}

#[test]
fn husky_v9_repo_still_runs_its_hook_and_the_backstop_rejects() {
    let world = World::new();
    fs::create_dir_all(world.repo.join(".acs/policy")).unwrap();
    let stubs = world.repo.join(".husky/_");
    fs::create_dir_all(&stubs).unwrap();
    fs::write(
        stubs.join("h"),
        "#!/usr/bin/env sh\nn=$(basename \"$0\")\ns=$(dirname \"$(dirname \"$0\")\")/$n\n\
         [ ! -f \"$s\" ] && exit 0\nsh -e \"$s\" \"$@\"\nc=$?\nexit $c\n",
    )
    .unwrap();
    for name in ["pre-commit", "prepare-commit-msg", "pre-push"] {
        let path = stubs.join(name);
        fs::write(&path, "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::write(world.repo.join(".husky/pre-commit"), "touch husky-ran\n").unwrap();
    git_ok(
        &world,
        &world.repo,
        &["config", "core.hooksPath", ".husky/_"],
    );
    let layout = acs_git::discover(&world.repo).unwrap();
    let report = acs_git::hooks::install(&layout).unwrap();
    assert_eq!(report.location.kind, acs_git::hooks::LocationKind::HuskyV9);

    let holder = Agent::new(&world, "claude-code", "c1");
    let committer = Agent::new(&world, "antigravity", "g1");
    holder.start();
    committer.start();
    holder.pre_edit("src/lib.rs");
    committer.pre_shell("git commit -m work");
    stage(&world, "src/lib.rs", "pub fn x() {}\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "work"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("`src/lib.rs` is claimed by sara@example.com"),
        "{}",
        stderr(&out)
    );
    assert!(
        !world.repo.join("husky-ran").exists(),
        "ACS runs first, so husky's script did not"
    );

    // An allowed commit runs both: ACS and the user's husky script.
    git_ok(&world, &world.repo, &["reset", "-q"]);
    stage(&world, "a.txt", "ok\n");
    let ok = git(&world, &world.repo, &["commit", "-q", "-m", "ok"]);
    assert!(ok.status.success(), "{}", stderr(&ok));
    assert!(world.repo.join("husky-ran").exists());
}

#[test]
fn a_repository_without_acs_policy_is_left_alone_silently() {
    let world = World::new();
    let layout = acs_git::discover(&world.repo).unwrap();
    acs_git::hooks::install(&layout).unwrap();
    stage(&world, ".env", "x\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "x"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn unavailable_state_allows_with_a_warning() {
    let world = world_with_hooks();
    stage(&world, "a.txt", "x\n");
    // ACS_HOME below a regular file cannot be created.
    let blocker = world.repo.parent().unwrap().join("blocker");
    fs::write(&blocker, "file").unwrap();
    let out = git_cmd(&world, &world.repo, &["commit", "-q", "-m", "x"])
        .env("ACS_HOME", blocker.join("home"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("acs: warning: ACS state is unavailable"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn misuse_of_the_git_hook_command_never_blocks_git() {
    let world = World::new();
    for args in [&["git-hook"][..], &["git-hook", "no-such-hook"][..]] {
        let out = Command::new(acs_bin())
            .args(args)
            .current_dir(&world.repo)
            .env("ACS_HOME", &world.home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: exit {:?}",
            out.status.code()
        );
    }
}

// ---- pre-push -----------------------------------------------------------------------------

/// A bare remote with `main` pushed, and a second commit locally that rewrites history.
struct PushWorld {
    world: World,
}

impl PushWorld {
    fn new() -> Self {
        let world = world_with_hooks();
        let remote = world.repo.parent().unwrap().join("remote.git");
        let out = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(out.status.success());
        git_ok(
            &world,
            &world.repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_ok(&world, &world.repo, &["push", "-q", "origin", "main"]);
        // Diverge: replace the pushed commit.
        world.write("a.txt", "rewritten\n");
        git_ok(&world, &world.repo, &["add", "a.txt"]);
        git_ok(
            &world,
            &world.repo,
            &["commit", "-q", "--amend", "-m", "rewritten init"],
        );
        Self { world }
    }

    fn push(&self, extra_env: &[(&str, &str)], args: &[&str]) -> Output {
        let mut command = git_cmd(&self.world, &self.world.repo, args);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.output().unwrap()
    }
}

#[test]
fn a_force_push_to_a_protected_branch_from_an_agent_session_is_rejected() {
    let pw = PushWorld::new();
    let agent = Agent::new(&pw.world, "claude-code", "c1");
    agent.start();
    // The agent's plain push is allowed by the gate and leaves the marker...
    assert_eq!(
        agent.pre_shell("git push origin main").outcome,
        acs_core::Outcome::Allow
    );
    // ...so the force push that follows (however it got there) is recognised as the agent's.
    let out = pw.push(&[], &["push", "--force", "-q", "origin", "main"]);
    assert!(!out.status.success(), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(text.contains("push rejected (agent session ses_"), "{text}");
    assert!(
        text.contains("a force push (not a fast-forward) to protected branch `main`"),
        "{text}"
    );
    assert!(text.contains("ask your pilot"), "{text}");
}

#[test]
fn the_harness_environment_also_marks_an_agent_push() {
    let pw = PushWorld::new();
    let out = pw.push(
        &[("CLAUDECODE", "1")],
        &["push", "--force", "-q", "origin", "main"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("protected branch `main`"),
        "{}",
        stderr(&out)
    );
    // Deleting the protected branch is blocked too.
    let del = pw.push(&[("CLAUDECODE", "1")], &["push", "-q", "origin", ":main"]);
    assert!(!del.status.success());
    assert!(
        stderr(&del).contains("deleting protected branch `main`"),
        "{}",
        stderr(&del)
    );
}

#[test]
fn agents_may_push_fast_forwards_and_force_push_other_branches() {
    let pw = PushWorld::new();
    let env = [("CLAUDECODE", "1")];
    let feature = pw.push(&env, &["push", "--force", "-q", "origin", "main:feature"]);
    assert!(feature.status.success(), "{}", stderr(&feature));
    // A fast-forward to main is fine for an agent.
    git_ok(
        &pw.world,
        &pw.world.repo,
        &["push", "--force", "-q", "origin", "main"],
    );
    pw.world.write("b.txt", "b\n");
    git_ok(&pw.world, &pw.world.repo, &["add", "b.txt"]);
    git_ok(&pw.world, &pw.world.repo, &["commit", "-q", "-m", "next"]);
    let ff = pw.push(&env, &["push", "-q", "origin", "main"]);
    assert!(ff.status.success(), "{}", stderr(&ff));
}

#[test]
fn a_person_may_force_push_a_protected_branch() {
    let pw = PushWorld::new();
    let out = pw.push(&[], &["push", "--force", "-q", "origin", "main"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn a_stale_push_marker_does_not_outlive_its_push() {
    let pw = PushWorld::new();
    let agent = Agent::new(&pw.world, "claude-code", "c1");
    agent.start();
    agent.pre_shell("git push origin feature");
    // The agent's push consumes the marker (here a push of another branch)...
    let first = pw.push(&[], &["push", "-q", "origin", "main:feature"]);
    assert!(first.status.success(), "{}", stderr(&first));
    // ...so a person's force push right after is a person's.
    let second = pw.push(&[], &["push", "--force", "-q", "origin", "main"]);
    assert!(second.status.success(), "{}", stderr(&second));
}

#[test]
fn renames_and_commit_dash_a_are_checked_too() {
    let world = world_with_hooks();
    let holder = Agent::new(&world, "claude-code", "c1");
    let committer = Agent::new(&world, "antigravity", "g1");
    holder.start();
    committer.start();
    holder.pre_edit("src/util.rs");
    holder.pre_edit("src/lib.rs");

    // A rename moves the claimed file away: the old path is a deletion in the staged diff.
    committer.pre_shell("git commit -m mv");
    git_ok(&world, &world.repo, &["mv", "src/util.rs", "src/util2.rs"]);
    let out = git(&world, &world.repo, &["commit", "-q", "-m", "mv"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("`src/util.rs` is claimed by"),
        "{}",
        stderr(&out)
    );
    git_ok(&world, &world.repo, &["reset", "-q", "--hard"]);

    // `git commit -a` stages through a temporary index; the hook must read that one.
    committer.pre_shell("git commit -am edit");
    world.write("src/lib.rs", "pub fn edited() {}\n");
    let out = git(&world, &world.repo, &["commit", "-q", "-a", "-m", "edit"]);
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("`src/lib.rs` is claimed by"),
        "{}",
        stderr(&out)
    );
}

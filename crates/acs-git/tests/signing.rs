//! Ledger verification: chains, authorship, signatures and rewritten history, against real
//! repositories. The signed cases need `ssh-keygen` and are skipped without it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use acs_git::refs::{CustomRefs, Pilot, RECORD_FILE, SyncStorage, ledger_ref};
use acs_git::signing::{
    ProblemKind, SigningSetup, VerifiedTips, signing_setup, verify_chain, verify_pilot,
};
use serde_json::json;
use tempfile::TempDir;

const SARA: &str = "sara@example.com";
const OMAR: &str = "omar@example.com";

fn git_in(dir: &Path, args: &[&str], stdin: Option<&[u8]>, env: &[(&str, &str)]) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(bytes) = stdin {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_in(dir, args, None, &[])
}

/// A valid session record of `pilot`'s session `n`, as bytes.
fn record(pilot: &str, n: u32) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "spec": "acs/0.1",
        "session_id": format!("ses_{n:08}"),
        "seat": {
            "seat_id": format!("seat_{n:08}"),
            "pilot": pilot,
            "harness": "claude-code",
            "harness_session_id": format!("h{n}"),
            "worktree": "/w",
            "target_branch": "main",
            "started_at": "2026-10-29T14:02:11Z",
            "last_seen_at": "2026-10-29T14:31:02Z"
        },
        "claims": [],
        "files_changed": [],
        "guard_decisions": {"counts": {}, "notable": []},
        "commits": [],
        "lore_loaded": [],
        "started_at": "2026-10-29T14:02:11Z",
        "ended_at": "2026-10-29T14:31:02Z",
        "ended_reason": "clean",
        "acs_version": "0.1.0"
    }))
    .unwrap()
}

/// A bare remote with one commit, and clones of it.
struct Team {
    _tmp: TempDir,
    base: PathBuf,
    remote: PathBuf,
}

struct Machine {
    repo: PathBuf,
    pilot: Pilot,
}

impl Team {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let remote = base.join("remote.git");
        fs::create_dir(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main", "."]);
        let seed = base.join("seed");
        fs::create_dir(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main", "."]);
        fs::write(seed.join("a.txt"), "a\n").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "init"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "-q", "origin", "main"]);
        Self {
            _tmp: tmp,
            base,
            remote,
        }
    }

    fn machine(&self, name: &str, pilot: &str) -> Machine {
        let repo = self.base.join(name);
        git(
            &self.base,
            &[
                "clone",
                "-q",
                self.remote.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        git(&repo, &["config", "user.email", pilot]);
        Machine {
            repo,
            pilot: Pilot::new(pilot),
        }
    }

    /// A key pair in the team's directory; `None` when `ssh-keygen` is missing.
    fn key(&self, name: &str) -> Option<PathBuf> {
        let key = self.base.join(name);
        let made = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if made.is_ok_and(|status| status.success()) {
            Some(key)
        } else {
            eprintln!("skipped: ssh-keygen is not available");
            None
        }
    }
}

impl Machine {
    fn storage(&self) -> CustomRefs {
        CustomRefs::new(&self.repo, "origin")
    }

    fn append(&self, n: u32) -> String {
        let pilot = &self.pilot;
        self.storage()
            .append_ledger(pilot, &format!("ses_{n:08}"), &record(&pilot.email, n))
            .unwrap()
            .commit
    }

    fn ledger(&self) -> String {
        git(&self.repo, &["rev-parse", &ledger_ref(&self.pilot.id)])
    }

    fn verify(&self, memory: &mut VerifiedTips) -> acs_git::signing::PilotVerdict {
        verify_pilot(&self.repo, "origin", &self.pilot.id, memory, false).unwrap()
    }

    fn sign_with(&self, key: &Path, allowed: bool) {
        git(&self.repo, &["config", "commit.gpgsign", "true"]);
        git(&self.repo, &["config", "gpg.format", "ssh"]);
        git(
            &self.repo,
            &["config", "user.signingkey", key.to_str().unwrap()],
        );
        if allowed {
            let allowed_file = self.repo.join("..").join("allowed_signers");
            let public = fs::read_to_string(format!("{}.pub", key.display())).unwrap();
            fs::write(&allowed_file, format!("{} {public}", self.pilot.email)).unwrap();
            git(
                &self.repo,
                &[
                    "config",
                    "gpg.ssh.allowedSignersFile",
                    allowed_file.to_str().unwrap(),
                ],
            );
        }
    }

    /// Re-creates the chain of `tip` with `edit` applied to the commit at `at` (oldest first):
    /// every commit from there on gets the new parent, nothing is signed again. With `keep_sig`
    /// the old signatures stay in the commit headers, as a forger who has no key would leave them.
    fn rewrite(&self, at: usize, record: &[u8], keep_sig: bool) -> String {
        let name = ledger_ref(&self.pilot.id);
        let listing = git(
            &self.repo,
            &["rev-list", "--reverse", "--topo-order", &name],
        );
        let ids: Vec<&str> = listing.lines().collect();
        let mut previous: Option<String> = None;
        for (index, id) in ids.iter().enumerate() {
            let raw = git(&self.repo, &["cat-file", "commit", id]);
            let mut out = Vec::new();
            let mut in_sig = false;
            for line in raw.lines() {
                if let Some(rest) = line.strip_prefix("tree ")
                    && index == at
                {
                    let _ = rest;
                    let tree = self.record_tree(record);
                    out.push(format!("tree {tree}"));
                    continue;
                }
                if line.starts_with("parent ") {
                    if let Some(parent) = &previous {
                        out.push(format!("parent {parent}"));
                    }
                    continue;
                }
                if line.starts_with("gpgsig ") {
                    in_sig = true;
                    if keep_sig {
                        out.push(line.to_owned());
                    }
                    continue;
                }
                if in_sig && line.starts_with(' ') {
                    if keep_sig {
                        out.push(line.to_owned());
                    }
                    continue;
                }
                in_sig = false;
                out.push(line.to_owned());
            }
            let text = format!("{}\n", out.join("\n"));
            let new = git_in(
                &self.repo,
                &["hash-object", "-t", "commit", "-w", "--stdin"],
                Some(text.as_bytes()),
                &[],
            );
            previous = Some(new);
        }
        let tip = previous.unwrap();
        git(&self.repo, &["update-ref", &name, &tip]);
        tip
    }

    fn record_tree(&self, record: &[u8]) -> String {
        self.tree_of(&[(RECORD_FILE, record)])
    }

    fn tree_of(&self, files: &[(&str, &[u8])]) -> String {
        let mut listing = String::new();
        for (name, bytes) in files {
            let blob = git_in(
                &self.repo,
                &["hash-object", "-w", "--stdin"],
                Some(bytes),
                &[],
            );
            listing.push_str(&format!("100644 blob {blob}\t{name}\n"));
        }
        git_in(&self.repo, &["mktree"], Some(listing.as_bytes()), &[])
    }

    /// A commit by `author` on top of the ledger, which becomes the new tip.
    fn forge(&self, tree: &str, author: &str) -> String {
        let name = ledger_ref(&self.pilot.id);
        let parent = self.ledger();
        let id = git_in(
            &self.repo,
            &["commit-tree", tree, "-p", &parent, "-m", "forged"],
            None,
            &[
                ("GIT_AUTHOR_NAME", "x"),
                ("GIT_AUTHOR_EMAIL", author),
                ("GIT_COMMITTER_NAME", "x"),
                ("GIT_COMMITTER_EMAIL", author),
            ],
        );
        git(&self.repo, &["update-ref", &name, &id]);
        id
    }
}

fn kinds(verdict: &acs_git::signing::PilotVerdict) -> Vec<ProblemKind> {
    verdict.all_problems().map(|p| p.kind).collect()
}

#[test]
fn a_clean_unsigned_chain_is_reported_unsigned_and_not_wrong() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    for n in 1..=3 {
        sara.append(n);
    }
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).is_empty(), "{verdict:?}");
    let report = &verdict.chains[0].report;
    assert_eq!(
        (report.commits, report.records, report.joins),
        (3, 3, 0),
        "{report:?}"
    );
    assert_eq!((report.signed, report.unsigned), (0, 3));
    assert_eq!(report.author.as_deref(), Some(SARA));
}

#[test]
fn a_signed_chain_verifies() {
    let team = Team::new();
    let Some(key) = team.key("key") else {
        return;
    };
    let sara = team.machine("sara", SARA);
    sara.sign_with(&key, true);
    for n in 1..=3 {
        assert!(
            sara.storage()
                .append_ledger(&sara.pilot, &format!("ses_{n:08}"), &record(SARA, n))
                .unwrap()
                .signed
        );
    }
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).is_empty(), "{verdict:?}");
    let report = &verdict.chains[0].report;
    assert_eq!(
        (report.signed, report.unsigned, report.unchecked),
        (3, 0, 0)
    );
    assert!(report.notes.is_empty());
    assert!(signing_setup(&sara.repo).unwrap().allowed_signers);
}

#[test]
fn signatures_git_cannot_check_are_counted_not_failed() {
    let team = Team::new();
    let Some(key) = team.key("key") else {
        return;
    };
    let sara = team.machine("sara", SARA);
    sara.sign_with(&key, false);
    sara.append(1);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).is_empty(), "{verdict:?}");
    let report = &verdict.chains[0].report;
    assert_eq!((report.signed, report.unchecked), (0, 1));
    assert!(
        report.notes[0].contains("allowedSignersFile"),
        "{:?}",
        report.notes
    );
    assert_eq!(
        signing_setup(&sara.repo).unwrap(),
        SigningSetup {
            enabled: true,
            format: "ssh".to_owned(),
            allowed_signers: false
        }
    );
}

#[test]
fn editing_a_signed_record_breaks_its_signature() {
    let team = Team::new();
    let Some(key) = team.key("key") else {
        return;
    };
    let sara = team.machine("sara", SARA);
    sara.sign_with(&key, true);
    for n in 1..=3 {
        sara.append(n);
    }
    let mut memory = VerifiedTips::in_memory();
    assert!(kinds(&sara.verify(&mut memory)).is_empty());

    // The forger edits the middle record and re-creates the chain; the old signatures stay in
    // the headers, and cannot match.
    sara.rewrite(1, &record(SARA, 99), true);
    let verdict = sara.verify(&mut memory);
    let kinds = kinds(&verdict);
    assert!(kinds.contains(&ProblemKind::BadSignature), "{verdict:?}");
    assert!(
        kinds.contains(&ProblemKind::RewrittenHistory),
        "{verdict:?}"
    );
    let report = &verdict.chains[0].report;
    assert_eq!(
        report.signed, 1,
        "only the commit before the edit is intact"
    );

    // Even with a fresh memory (a clone that never verified), the signatures say it.
    let fresh = sara.verify(&mut VerifiedTips::in_memory());
    assert!(
        fresh
            .all_problems()
            .any(|p| p.kind == ProblemKind::BadSignature)
    );
}

#[test]
fn a_rewritten_unsigned_chain_is_caught_by_the_remembered_tip() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    for n in 1..=3 {
        sara.append(n);
    }
    let file = team.base.join("verified.json");
    let mut memory = VerifiedTips::load(&file);
    assert!(kinds(&sara.verify(&mut memory)).is_empty());
    memory.save().unwrap();

    // Nothing is signed, so the rewritten chain is perfect on its own.
    let tip = sara.rewrite(1, &record(SARA, 99), false);
    let alone = verify_chain(&sara.repo, &sara.pilot.id, &tip).unwrap();
    assert!(alone.problems.is_empty(), "{alone:?}");

    // The memory is what shows it, also after a restart.
    let mut memory = VerifiedTips::load(&file);
    let verdict = sara.verify(&mut memory);
    assert_eq!(kinds(&verdict), [ProblemKind::RewrittenHistory]);
    // The baseline is kept: the next run still says so.
    memory.save().unwrap();
    let again = sara.verify(&mut VerifiedTips::load(&file));
    assert_eq!(kinds(&again), [ProblemKind::RewrittenHistory]);

    // A person who intended it accepts the present history.
    let accepted = verify_pilot(
        &sara.repo,
        "origin",
        &sara.pilot.id,
        &mut VerifiedTips::load(&file),
        true,
    )
    .unwrap();
    assert!(kinds(&accepted).is_empty());
}

#[test]
fn appending_after_a_verification_is_not_a_rewrite() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    sara.append(1);
    let mut memory = VerifiedTips::in_memory();
    assert!(kinds(&sara.verify(&mut memory)).is_empty());
    sara.append(2);
    assert!(kinds(&sara.verify(&mut memory)).is_empty());
}

#[test]
fn extra_files_a_wrong_author_and_an_invalid_record_are_reported() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    sara.append(1);
    let good = record(SARA, 2);

    let tree = sara.tree_of(&[(RECORD_FILE, &good), ("extra.txt", b"x")]);
    let extra = sara.forge(&tree, SARA);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    let found: Vec<_> = verdict
        .all_problems()
        .filter(|p| p.kind == ProblemKind::Structure)
        .collect();
    assert_eq!(found.len(), 1, "{verdict:?}");
    assert_eq!(found[0].commit.as_deref(), Some(extra.as_str()));
    assert!(found[0].detail.contains("extra.txt"), "{:?}", found[0]);

    sara.forge(&sara.record_tree(&good), OMAR);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).contains(&ProblemKind::WrongAuthor));

    sara.forge(&sara.record_tree(b"{not json"), SARA);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).contains(&ProblemKind::InvalidRecord));

    // A well-formed record of another pilot's seat in this ledger.
    sara.forge(&sara.record_tree(&record(OMAR, 7)), SARA);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&verdict).contains(&ProblemKind::WrongPilot));
}

#[test]
fn a_record_commit_with_two_parents_or_an_empty_one_with_one_is_a_structure_problem() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let first = sara.append(1);
    sara.append(2);
    let tip = sara.ledger();
    let empty = sara.tree_of(&[]);
    // An empty commit with one parent is not a join.
    sara.forge(&empty, SARA);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(
        kinds(&verdict).contains(&ProblemKind::Structure),
        "{verdict:?}"
    );
    // A record commit with two parents.
    let name = ledger_ref(&sara.pilot.id);
    let tree = sara.record_tree(&record(SARA, 2));
    let id = git_in(
        &sara.repo,
        &["commit-tree", &tree, "-p", &first, "-p", &tip, "-m", "two"],
        None,
        &[
            ("GIT_AUTHOR_NAME", "x"),
            ("GIT_AUTHOR_EMAIL", SARA),
            ("GIT_COMMITTER_NAME", "x"),
            ("GIT_COMMITTER_EMAIL", SARA),
        ],
    );
    git(&sara.repo, &["update-ref", &name, &id]);
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(
        kinds(&verdict).contains(&ProblemKind::Structure),
        "{verdict:?}"
    );
}

#[test]
fn join_commits_of_one_pilot_on_two_machines_are_accepted() {
    let team = Team::new();
    let desktop = team.machine("desktop", SARA);
    let laptop = team.machine("laptop", SARA);
    desktop.append(1);
    desktop
        .storage()
        .push(&[], &[desktop.pilot.clone()])
        .unwrap();
    laptop.storage().fetch().unwrap();
    laptop.storage().reconcile_ledger(&laptop.pilot).unwrap();
    laptop.append(2);
    desktop.append(3);
    laptop.storage().push(&[], &[laptop.pilot.clone()]).unwrap();
    desktop.storage().fetch().unwrap();
    desktop.storage().reconcile_ledger(&desktop.pilot).unwrap();

    let mut memory = VerifiedTips::in_memory();
    let verdict = desktop.verify(&mut memory);
    assert!(kinds(&verdict).is_empty(), "{verdict:?}");
    let report = &verdict.chains[0].report;
    assert_eq!((report.records, report.joins, report.commits), (3, 1, 4));

    // The fetched copy is behind the join; that is fine, and so is the laptop's unjoined view.
    let before = laptop.verify(&mut VerifiedTips::in_memory());
    assert!(kinds(&before).is_empty(), "{before:?}");
}

#[test]
fn a_fetched_chain_that_dropped_records_or_was_rewritten_is_caught() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let first = sara.append(1);
    sara.append(2);
    sara.append(3);
    sara.storage().push(&[], &[sara.pilot.clone()]).unwrap();
    sara.storage().fetch().unwrap();
    let mut memory = VerifiedTips::in_memory();
    assert!(kinds(&sara.verify(&mut memory)).is_empty());

    // Someone with push access resets the remote ledger to its first commit.
    let name = ledger_ref(&sara.pilot.id);
    git(&team.remote, &["update-ref", &name, &first]);
    sara.storage().fetch().unwrap();
    let verdict = sara.verify(&mut memory);
    let rewritten: Vec<_> = verdict
        .all_problems()
        .filter(|p| p.kind == ProblemKind::RewrittenHistory)
        .collect();
    assert_eq!(rewritten.len(), 1, "{verdict:?}");
    assert!(rewritten[0].detail.contains("fetched"), "{rewritten:?}");

    // Or replaces it with a rewritten chain of the same sessions: the two copies conflict.
    let original = sara.ledger();
    sara.rewrite(1, &record(SARA, 98), false);
    let rewritten_tip = sara.ledger();
    git(
        &team.remote,
        &["fetch", "-q", sara.repo.to_str().unwrap(), &name],
    );
    git(&team.remote, &["update-ref", &name, &rewritten_tip]);
    git(&sara.repo, &["update-ref", &name, &original]);
    sara.storage().fetch().unwrap();
    let verdict = sara.verify(&mut VerifiedTips::in_memory());
    assert!(
        verdict
            .all_problems()
            .any(|p| p.kind == ProblemKind::RewrittenHistory
                && p.detail.contains("both hold a record")),
        "{verdict:?}"
    );
}

#[test]
fn signing_is_off_by_default_and_follows_the_configuration() {
    let team = Team::new();
    let sara = team.machine("sara", SARA);
    let setup = signing_setup(&sara.repo).unwrap();
    assert!(!setup.enabled);
    assert_eq!(setup.format, "openpgp");
    git(&sara.repo, &["config", "commit.gpgsign", "true"]);
    assert!(signing_setup(&sara.repo).unwrap().enabled);
}

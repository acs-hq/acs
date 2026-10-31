//! Where shared state lives in git: the [`SyncStorage`] interface and its implementation over
//! custom refs (SPEC section 8).
//!
//! Everything that reads or writes shared state goes through [`SyncStorage`], and the interface
//! speaks of pilots and documents, never of refs. A forge that rejects `refs/acs/*` is the risk
//! this guards against: the fallback (a dedicated `acs/state` branch, plan section 9.2) is one
//! more implementation of the trait, and nothing above it changes.
//!
//! [`CustomRefs`] keeps a pilot's own state under `refs/acs/` and other machines' state under
//! `refs/remotes/<remote>/acs/`, so a fetch can never overwrite what this machine wrote. Network
//! and signing go through the user's `git`, so existing authentication and signing configuration
//! just work; a call here never prompts, and never runs longer than [`NETWORK_LIMIT`].

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::GitError;
use crate::repo::{git_command, objects};

/// The file holding a pilot's `ClaimsDocument` in the tree of their claims commit.
pub const CLAIMS_FILE: &str = "claims.json";
/// The file holding a `SessionRecord` in the tree of a ledger commit.
pub const RECORD_FILE: &str = "record.json";
/// The longest a fetch or a push may take. `git` has no timeout of its own, and a detached sync
/// stuck on a dead connection would hold the sync lock for good.
pub const NETWORK_LIMIT: Duration = Duration::from_secs(120);

/// The error text kept from a failed `git` process, in characters.
const MAX_DETAIL_CHARS: usize = 400;
/// How often a running `git` is checked on.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long the output of an exited `git` is waited for. A background process it left behind
/// (an ssh control master, for example) can keep the pipes open long after it is gone.
const DRAIN_LIMIT: Duration = Duration::from_secs(1);

/// The fetch refspec for `remote`: `+refs/acs/*:refs/remotes/<remote>/acs/*`.
#[must_use]
pub fn fetch_refspec(remote: &str) -> String {
    format!("+refs/acs/*:refs/remotes/{remote}/acs/*")
}

/// The ref of a pilot's claims.
#[must_use]
pub fn claims_ref(pilot_id: &str) -> String {
    format!("refs/acs/claims/{pilot_id}")
}

/// The ref of a pilot's ledger.
#[must_use]
pub fn ledger_ref(pilot_id: &str) -> String {
    format!("refs/acs/ledger/{pilot_id}")
}

/// A pilot: their git email, and the id derived from it (`acs_core::pilot_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pilot {
    /// The git email. It is the author of the commits written for this pilot.
    pub email: String,
    /// The pilot id.
    pub id: String,
}

impl Pilot {
    /// The pilot with this email.
    #[must_use]
    pub fn new(email: &str) -> Self {
        Self {
            email: email.to_owned(),
            id: acs_core::pilot_id(email),
        }
    }
}

/// One pilot's published `claims.json`, as fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PilotDocument {
    /// The pilot id the document is published under.
    pub pilot_id: String,
    /// The file's bytes, not yet validated: anyone who can push wrote them.
    pub bytes: Vec<u8>,
}

/// One session record in a ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// The commit holding it.
    pub commit: String,
    /// The record's bytes.
    pub record: Vec<u8>,
}

/// The result of appending to a ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerAppend {
    /// The new commit.
    pub commit: String,
    /// Whether it is signed. `false` when signing is not configured, or when it failed: a session
    /// record is worth more than its signature, and `acs verify` reports unsigned commits.
    pub signed: bool,
}

/// The shared state of a repository. See the module documentation.
pub trait SyncStorage {
    /// Brings every pilot's published state from the remote to this machine.
    ///
    /// # Errors
    ///
    /// [`GitError`] when the remote cannot be reached or `git` fails.
    fn fetch(&self) -> Result<(), GitError>;

    /// Every pilot's claims document as of the last [`SyncStorage::fetch`], this machine's pilot
    /// included: the same person may hold claims on another machine.
    ///
    /// # Errors
    ///
    /// [`GitError`] when `git` fails.
    fn fetched_claims(&self) -> Result<Vec<PilotDocument>, GitError>;

    /// Replaces `pilot`'s claims document on this machine. [`SyncStorage::push`] publishes it.
    ///
    /// # Errors
    ///
    /// [`GitError`] when `git` fails.
    fn write_claims(&self, pilot: &Pilot, document: &[u8]) -> Result<(), GitError>;

    /// Appends one session record to `pilot`'s ledger on this machine. Safe to call from
    /// concurrent processes: no append is lost.
    ///
    /// # Errors
    ///
    /// [`GitError`] when `git` fails.
    fn append_ledger(
        &self,
        pilot: &Pilot,
        session_id: &str,
        record: &[u8],
    ) -> Result<LedgerAppend, GitError>;

    /// The session records in the pilot's ledger, oldest first: this machine's ledger when it has
    /// one, otherwise the fetched one.
    ///
    /// # Errors
    ///
    /// [`GitError`] when `git` fails.
    fn ledger_entries(&self, pilot_id: &str) -> Result<Vec<LedgerEntry>, GitError>;

    /// Joins `pilot`'s ledger on this machine with the fetched one, so that it can be pushed as
    /// a fast-forward. They differ when one pilot works on two machines.
    ///
    /// # Errors
    ///
    /// [`GitError`] when `git` fails.
    fn reconcile_ledger(&self, pilot: &Pilot) -> Result<(), GitError>;

    /// Publishes the claims of `claims_of`, replacing what is on the remote, and the ledgers of
    /// `ledgers_of`, as a fast-forward. Returns how many refs it pushed; with nothing new it does
    /// not touch the network.
    ///
    /// # Errors
    ///
    /// [`GitError`] when the remote rejects the push or cannot be reached.
    fn push(&self, claims_of: &[Pilot], ledgers_of: &[Pilot]) -> Result<usize, GitError>;
}

/// [`SyncStorage`] over `refs/acs/*` on one remote.
#[derive(Debug, Clone)]
pub struct CustomRefs {
    work_tree: PathBuf,
    remote: String,
}

impl CustomRefs {
    /// The storage of the repository around `work_tree`, shared through `remote`.
    #[must_use]
    pub fn new(work_tree: &Path, remote: &str) -> Self {
        Self {
            work_tree: work_tree.to_path_buf(),
            remote: remote.to_owned(),
        }
    }

    /// Where the fetched copy of the local ref `name` (`refs/acs/...`) lives.
    fn fetched_ref(&self, name: &str) -> String {
        let rest = name.strip_prefix("refs/").unwrap_or(name);
        format!("refs/remotes/{}/{rest}", self.remote)
    }

    /// A `git` for talking to the remote: it never asks anyone anything. Nobody is there to
    /// answer a detached sync, and a prompt would hang it until [`NETWORK_LIMIT`].
    fn network_git(&self) -> Command {
        let mut command = git_command(&self.work_tree);
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never");
        command
    }
}

impl SyncStorage for CustomRefs {
    fn fetch(&self) -> Result<(), GitError> {
        let spec = fetch_refspec(&self.remote);
        let mut command = self.network_git();
        // Not FETCH_HEAD (it is the user's, and they may be about to merge it), not tags, and no
        // maintenance: only ACS's refs move. The refspec is given here so sync works in a clone
        // `acs init` never ran in.
        command.args([
            "fetch",
            "--quiet",
            "--prune",
            "--no-tags",
            "--no-auto-gc",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            &self.remote,
            &spec,
        ]);
        run_limited(command, NETWORK_LIMIT, "fetch").map(drop)
    }

    fn fetched_claims(&self) -> Result<Vec<PilotDocument>, GitError> {
        let prefix = self.fetched_ref(&claims_ref(""));
        let listing = git_text(
            &self.work_tree,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                prefix.trim_end_matches('/'),
            ],
        )?;
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for line in listing.lines() {
            let Some((name, oid)) = line.split_once(' ') else {
                continue;
            };
            let Some(id) = name.strip_prefix(&prefix) else {
                continue;
            };
            ids.push(id.to_owned());
            names.push(format!("{oid}:{CLAIMS_FILE}"));
        }
        let blobs = objects(&self.work_tree, &names)?;
        Ok(ids
            .into_iter()
            .zip(blobs)
            .filter_map(|(pilot_id, blob)| {
                Some(PilotDocument {
                    pilot_id,
                    bytes: blob?,
                })
            })
            .collect())
    }

    fn write_claims(&self, pilot: &Pilot, document: &[u8]) -> Result<(), GitError> {
        let tree = file_tree(&self.work_tree, CLAIMS_FILE, document)?;
        // No parent: the ref is one commit (SPEC section 8), and old claims are of no interest.
        let commit = commit(&self.work_tree, &tree, &[], "claims", pilot, false)?;
        git_text(
            &self.work_tree,
            &["update-ref", &claims_ref(&pilot.id), &commit],
        )
        .map(drop)
    }

    fn append_ledger(
        &self,
        pilot: &Pilot,
        session_id: &str,
        record: &[u8],
    ) -> Result<LedgerAppend, GitError> {
        crate::ledger::append(&self.work_tree, pilot, session_id, record)
    }

    fn ledger_entries(&self, pilot_id: &str) -> Result<Vec<LedgerEntry>, GitError> {
        let local = ledger_ref(pilot_id);
        let tip = match resolve_ref(&self.work_tree, &local)? {
            Some(oid) => Some(oid),
            None => resolve_ref(&self.work_tree, &self.fetched_ref(&local))?,
        };
        match tip {
            Some(tip) => crate::ledger::entries(&self.work_tree, &tip),
            None => Ok(Vec::new()),
        }
    }

    fn reconcile_ledger(&self, pilot: &Pilot) -> Result<(), GitError> {
        let local = ledger_ref(&pilot.id);
        crate::ledger::reconcile(&self.work_tree, pilot, &local, &self.fetched_ref(&local))
    }

    fn push(&self, claims_of: &[Pilot], ledgers_of: &[Pilot]) -> Result<usize, GitError> {
        let mut specs: Vec<String> = claims_of
            .iter()
            .map(|pilot| {
                let name = claims_ref(&pilot.id);
                format!("+{name}:{name}")
            })
            .collect();
        for pilot in ledgers_of {
            let name = ledger_ref(&pilot.id);
            let Some(tip) = resolve_ref(&self.work_tree, &name)? else {
                continue;
            };
            if resolve_ref(&self.work_tree, &self.fetched_ref(&name))?.as_ref() != Some(&tip) {
                specs.push(format!("{name}:{name}"));
            }
        }
        if specs.is_empty() {
            return Ok(0);
        }
        let mut command = self.network_git();
        // `--no-verify`: pre-push hooks are for the user's pushes of code, and one that runs a
        // test suite must not run every thirty seconds.
        command.args([
            "push",
            "--quiet",
            "--no-verify",
            "--no-follow-tags",
            &self.remote,
        ]);
        command.args(&specs);
        run_limited(command, NETWORK_LIMIT, "push")?;
        Ok(specs.len())
    }
}

// ---------------------------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------------------------

fn command_error(args: &str, detail: impl Into<String>) -> GitError {
    GitError::Command {
        args: args.to_owned(),
        detail: detail.into(),
    }
}

/// The last characters of a process's error output: the end is where git says what went wrong.
fn detail_of(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(MAX_DETAIL_CHARS);
    text.chars().skip(skip).collect()
}

/// Runs a local `git` command to completion, feeding it `stdin`.
pub(crate) fn git_output(
    work_tree: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<Output, GitError> {
    let label = args.first().copied().unwrap_or("git");
    let mut command = git_command(work_tree);
    command
        .args(args)
        .envs(env.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| command_error(label, e.to_string()))?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Every command fed here reads all of its input before it writes anything.
        pipe.write_all(bytes)
            .map_err(|e| command_error(label, e.to_string()))?;
    }
    child
        .wait_with_output()
        .map_err(|e| command_error(label, e.to_string()))
}

/// Runs a local `git` command that must succeed; returns its output, trimmed.
pub(crate) fn git_text(work_tree: &Path, args: &[&str]) -> Result<String, GitError> {
    checked(args, git_output(work_tree, args, None, &[])?)
}

/// The trimmed output of a finished command that must have succeeded.
pub(crate) fn checked(args: &[&str], output: Output) -> Result<String, GitError> {
    let label = args.first().copied().unwrap_or("git");
    if !output.status.success() {
        return Err(command_error(label, detail_of(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The object `name` points to, or `None` when the ref does not exist.
pub(crate) fn resolve_ref(work_tree: &Path, name: &str) -> Result<Option<String>, GitError> {
    let output = git_output(
        work_tree,
        &["rev-parse", "--verify", "--quiet", name],
        None,
        &[],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!oid.is_empty()).then_some(oid))
}

/// A tree with one file, `name`, holding `bytes`. Returns the tree's id.
pub(crate) fn file_tree(work_tree: &Path, name: &str, bytes: &[u8]) -> Result<String, GitError> {
    let args = ["hash-object", "-w", "--stdin"];
    let blob = checked(&args, git_output(work_tree, &args, Some(bytes), &[])?)?;
    let entry = format!("100644 blob {blob}\t{name}\n");
    let args = ["mktree"];
    checked(
        &args,
        git_output(work_tree, &args, Some(entry.as_bytes()), &[])?,
    )
}

/// Writes a commit authored by `pilot` and returns its id. The identity is given here so that a
/// machine with no `user.name` can still sync. With `sign`, git signs it as the user's
/// configuration says; signing can wait on a passphrase, so it runs under [`SIGN_LIMIT`].
pub(crate) fn commit(
    work_tree: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
    pilot: &Pilot,
    sign: bool,
) -> Result<String, GitError> {
    let mut command = git_command(work_tree);
    command.arg("commit-tree").arg(tree);
    for parent in parents {
        command.arg("-p").arg(parent);
    }
    if sign {
        command.arg("-S");
    }
    command.arg("-m").arg(message);
    for (name, value) in [
        ("GIT_AUTHOR_NAME", "ACS"),
        ("GIT_AUTHOR_EMAIL", pilot.email.as_str()),
        ("GIT_COMMITTER_NAME", "ACS"),
        ("GIT_COMMITTER_EMAIL", pilot.email.as_str()),
    ] {
        command.env(name, value);
    }
    run_limited(command, SIGN_LIMIT, "commit-tree")
}

/// The longest a commit may take, signing included.
const SIGN_LIMIT: Duration = Duration::from_secs(30);

/// Reads `pipe` to the end on a thread of its own.
fn drain(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// Runs `command` with no stdin, for at most `limit`, and returns its output, trimmed. A command
/// that overruns is killed and reported as failed.
fn run_limited(mut command: Command, limit: Duration, label: &str) -> Result<String, GitError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| command_error(label, e.to_string()))?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let collect = |pipe: &Option<Receiver<Vec<u8>>>| {
        pipe.as_ref()
            .and_then(|pipe| pipe.recv_timeout(DRAIN_LIMIT).ok())
            .unwrap_or_default()
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(command_error(
                    label,
                    format!("gave up after {} seconds", limit.as_secs()),
                ));
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => return Err(command_error(label, e.to_string())),
        }
    };
    let out = collect(&stdout);
    if !status.success() {
        return Err(command_error(label, detail_of(&collect(&stderr))));
    }
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

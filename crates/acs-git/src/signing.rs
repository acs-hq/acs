//! Ledger signatures: whether signing is configured, and checking the chain and the signatures of
//! a pilot's ledger (WP4.4).
//!
//! A ledger is a chain of commits under `refs/acs/ledger/<pilot id>`. [`verify_chain`] walks every
//! commit reachable from a tip and checks, for each one:
//!
//! - **structure**: a record commit has exactly one file, `record.json`, and at most one parent;
//!   a join commit (see the `ledger` module) has an empty tree and two parents and no record;
//! - **authorship**: the author's email belongs to the ref's pilot (`acs_core::pilot_id`), the
//!   record parses as a `SessionRecord`, and its seat's pilot is the same pilot;
//! - **signature**: git itself checks it (`%G?`), against `gpg.ssh.allowedSignersFile` for SSH
//!   and the keyring for GPG.
//!
//! A chain rewritten consistently (every commit re-created, nothing signed) is structurally
//! perfect, so the chain alone cannot show it. [`VerifiedTips`] remembers the last tip that was
//! verified, and [`check_remembered`] reports a tip that no longer descends from it;
//! [`check_local_vs_fetched`] does the same between this machine's copy of a ledger and the
//! fetched one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::GitError;
use crate::refs::{RECORD_FILE, git_output, git_text, ledger_ref, resolve_ref};
use crate::repo::git_command;

/// Whether the user's configuration asks for signed commits (`commit.gpgsign`).
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn signing_configured(work_tree: &Path) -> Result<bool, GitError> {
    Ok(
        config_value(work_tree, &["--type=bool", "--get", "commit.gpgsign"])?.as_deref()
            == Some("true"),
    )
}

/// The signing configuration, for messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SigningSetup {
    /// `commit.gpgsign` is true.
    pub enabled: bool,
    /// `gpg.format`: `openpgp` (the default), `ssh` or `x509`.
    pub format: String,
    /// `gpg.ssh.allowedSignersFile` is set (only SSH signatures need it).
    pub allowed_signers: bool,
}

/// Reads [`SigningSetup`].
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn signing_setup(work_tree: &Path) -> Result<SigningSetup, GitError> {
    Ok(SigningSetup {
        enabled: signing_configured(work_tree)?,
        format: config_value(work_tree, &["--get", "gpg.format"])?
            .unwrap_or_else(|| "openpgp".to_owned()),
        allowed_signers: config_value(work_tree, &["--get", "gpg.ssh.allowedSignersFile"])?
            .is_some_and(|value| !value.is_empty()),
    })
}

fn config_value(work_tree: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    let mut full = vec!["config"];
    full.extend_from_slice(args);
    let output = git_output(work_tree, &full, None, &[])?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

// ---------------------------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------------------------

/// What git says about one commit's signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureState {
    /// A valid signature by a key git trusts (`G`), or a valid one whose key has unknown trust
    /// (`U`, what every GPG signature is without a trust database).
    Good,
    /// The signature does not match the commit (`B`): the commit was changed after signing.
    Bad,
    /// A valid signature, but the signature or its key has expired (`X`, `Y`).
    Expired,
    /// A valid signature by a revoked key (`R`).
    Revoked,
    /// The commit is signed but git cannot check it (`E`, or `N` for a commit that has a
    /// signature): the key is not known, or no allowed-signers file is configured.
    Unchecked,
    /// The commit carries no signature.
    Unsigned,
}

/// The kind of a [`Problem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// The commit is not shaped like a ledger commit (extra files, wrong parents).
    Structure,
    /// The author is not the pilot the ledger belongs to.
    WrongAuthor,
    /// The commit's `record.json` is not a valid `SessionRecord`.
    InvalidRecord,
    /// The record's seat belongs to another pilot than the ledger.
    WrongPilot,
    /// The signature does not match the commit.
    BadSignature,
    /// The signature or its key has expired.
    ExpiredSignature,
    /// The signing key was revoked.
    RevokedSignature,
    /// History that was verified before is no longer there.
    RewrittenHistory,
    /// This machine's copy and the fetched copy cannot be joined.
    Diverged,
}

/// Something wrong with a ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// What kind of problem.
    pub kind: ProblemKind,
    /// The commit it concerns, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// A sentence saying what is wrong.
    pub detail: String,
}

impl Problem {
    fn on(kind: ProblemKind, commit: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            commit: Some(commit.to_owned()),
            detail: detail.into(),
        }
    }

    fn general(kind: ProblemKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            commit: None,
            detail: detail.into(),
        }
    }
}

/// The result of checking one chain from its tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    /// The tip that was checked.
    pub tip: String,
    /// Every commit reachable from the tip.
    pub commits: usize,
    /// Commits that hold a valid session record.
    pub records: usize,
    /// Join commits.
    pub joins: usize,
    /// Commits with a [`SignatureState::Good`] signature.
    pub signed: usize,
    /// Commits with no signature.
    pub unsigned: usize,
    /// Signed commits git cannot check.
    pub unchecked: usize,
    /// The author email of the first commit that has one (for display).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Remarks that are not failures (why signatures cannot be checked).
    pub notes: Vec<String>,
    /// Everything wrong with the chain.
    pub problems: Vec<Problem>,
}

// ---------------------------------------------------------------------------------------------
// Reading the chain
// ---------------------------------------------------------------------------------------------

/// What `cat-file` says about one object: its type and bytes.
type Object = Option<(String, Vec<u8>)>;

/// The objects named, via one `git cat-file --batch`. A name that does not resolve yields `None`.
fn read_objects(work_tree: &Path, names: &[String]) -> Result<Vec<Object>, GitError> {
    let fail = |detail: String| GitError::Command {
        args: "cat-file --batch".to_owned(),
        detail,
    };
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git_command(work_tree)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail(e.to_string()))?;
    let mut input = names.join("\n");
    input.push('\n');
    let writer = child.stdin.take().map(|mut stdin| {
        // From a thread: a long chain outgrows the pipe buffers.
        std::thread::spawn(move || stdin.write_all(input.as_bytes()))
    });
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut out)
            .map_err(|e| fail(e.to_string()))?;
    }
    let _ = child.wait();
    if let Some(writer) = writer {
        match writer.join() {
            Ok(written) => written.map_err(|e| fail(e.to_string()))?,
            Err(_) => return Err(fail("the writer thread panicked".to_owned())),
        }
    }
    let mut rest = out.as_slice();
    let mut objects = Vec::with_capacity(names.len());
    for _ in names {
        let Some(newline) = rest.iter().position(|&b| b == b'\n') else {
            return Err(fail("truncated output".to_owned()));
        };
        let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
        rest = &rest[newline + 1..];
        let mut words = header.split(' ');
        let (_, kind, size) = (words.next(), words.next(), words.next());
        match (kind, size.and_then(|s| s.parse::<usize>().ok())) {
            (Some(kind), Some(size)) if rest.len() > size => {
                objects.push(Some((kind.to_owned(), rest[..size].to_vec())));
                rest = &rest[size + 1..];
            }
            (Some("missing"), _) | (_, None) => objects.push(None),
            _ => return Err(fail("truncated object".to_owned())),
        }
    }
    Ok(objects)
}

/// A commit object, parsed as far as verification needs.
struct RawCommit {
    tree: String,
    parents: Vec<String>,
    author_email: Option<String>,
    signed: bool,
}

fn parse_commit(bytes: &[u8]) -> Option<RawCommit> {
    let text = String::from_utf8_lossy(bytes);
    let header = text.split("\n\n").next().unwrap_or_default();
    let mut commit = RawCommit {
        tree: String::new(),
        parents: Vec::new(),
        author_email: None,
        signed: false,
    };
    for line in header.lines() {
        if let Some(tree) = line.strip_prefix("tree ") {
            commit.tree = tree.trim().to_owned();
        } else if let Some(parent) = line.strip_prefix("parent ") {
            commit.parents.push(parent.trim().to_owned());
        } else if let Some(author) = line.strip_prefix("author ") {
            let open = author.rfind('<')?;
            let close = author.rfind('>')?;
            commit.author_email = (open < close).then(|| author[open + 1..close].to_owned());
        } else if line.starts_with("gpgsig ") || line.starts_with("gpgsig-sha256 ") {
            commit.signed = true;
        }
    }
    (!commit.tree.is_empty()).then_some(commit)
}

/// The entries of a tree object as `(mode, name, object id)`. `None` when it is malformed.
fn parse_tree(mut bytes: &[u8], id_len: usize) -> Option<Vec<(String, String, String)>> {
    let mut entries = Vec::new();
    while !bytes.is_empty() {
        let space = bytes.iter().position(|&b| b == b' ')?;
        let nul = bytes.iter().position(|&b| b == 0)?;
        if nul < space || bytes.len() < nul + 1 + id_len {
            return None;
        }
        let mode = String::from_utf8_lossy(&bytes[..space]).into_owned();
        let name = String::from_utf8_lossy(&bytes[space + 1..nul]).into_owned();
        let id: String = bytes[nul + 1..nul + 1 + id_len]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        entries.push((mode, name, id));
        bytes = &bytes[nul + 1 + id_len..];
    }
    Some(entries)
}

fn signature_states(work_tree: &Path, tip: &str) -> Result<HashMap<String, char>, GitError> {
    // git prints its verification errors on stderr and still exits 0; what matters is the code.
    let output = git_output(work_tree, &["log", "--format=%H %G?", tip, "--"], None, &[])?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (id, code) = line.split_once(' ')?;
            Some((id.to_owned(), code.chars().next()?))
        })
        .collect())
}

fn state_of(signed: bool, code: Option<char>) -> SignatureState {
    if !signed {
        return SignatureState::Unsigned;
    }
    match code {
        Some('G' | 'U') => SignatureState::Good,
        Some('B') => SignatureState::Bad,
        Some('X' | 'Y') => SignatureState::Expired,
        Some('R') => SignatureState::Revoked,
        _ => SignatureState::Unchecked,
    }
}

/// Checks every commit reachable from `tip` of the ledger of `pilot_id`. See the module
/// documentation for what is checked.
///
/// # Errors
///
/// [`GitError::Command`] when `git` fails to list or read the commits.
pub fn verify_chain(work_tree: &Path, pilot_id: &str, tip: &str) -> Result<ChainReport, GitError> {
    let listing = git_text(work_tree, &["rev-list", "--topo-order", "--reverse", tip])?;
    let ids: Vec<String> = listing.lines().map(str::to_owned).collect();
    let mut report = ChainReport {
        tip: tip.to_owned(),
        commits: ids.len(),
        records: 0,
        joins: 0,
        signed: 0,
        unsigned: 0,
        unchecked: 0,
        author: None,
        notes: Vec::new(),
        problems: Vec::new(),
    };
    let raw = read_objects(work_tree, &ids)?;
    let mut commits: Vec<Option<RawCommit>> = raw
        .iter()
        .map(|object| match object {
            Some((kind, bytes)) if kind == "commit" => parse_commit(bytes),
            _ => None,
        })
        .collect();
    let trees: Vec<String> = commits
        .iter()
        .map(|c| c.as_ref().map(|c| c.tree.clone()).unwrap_or_default())
        .collect();
    let tree_objects = read_objects(work_tree, &trees)?;

    // Record blobs of the commits whose tree is exactly `record.json`.
    let mut record_names = Vec::new();
    let mut record_owner = Vec::new();
    for (at, id) in ids.iter().enumerate() {
        let Some(commit) = &commits[at] else {
            report.problems.push(Problem::on(
                ProblemKind::Structure,
                id,
                "the commit cannot be read",
            ));
            continue;
        };
        let id_len = id.len() / 2;
        let entries = match &tree_objects[at] {
            Some((kind, bytes)) if kind == "tree" => parse_tree(bytes, id_len),
            _ => None,
        };
        let Some(entries) = entries else {
            report.problems.push(Problem::on(
                ProblemKind::Structure,
                id,
                "the commit's tree cannot be read",
            ));
            continue;
        };
        let record = match entries.as_slice() {
            [] => {
                if commit.parents.len() == 2 {
                    report.joins += 1;
                } else {
                    report.problems.push(Problem::on(
                        ProblemKind::Structure,
                        id,
                        format!(
                            "an empty commit must join two chains, but it has {} parent(s)",
                            commit.parents.len()
                        ),
                    ));
                }
                None
            }
            [(mode, name, blob)] if name == RECORD_FILE && mode == "100644" => {
                if commit.parents.len() > 1 {
                    report.problems.push(Problem::on(
                        ProblemKind::Structure,
                        id,
                        format!(
                            "a record commit has {} parents, at most one is expected",
                            commit.parents.len()
                        ),
                    ));
                }
                Some(blob.clone())
            }
            other => {
                let names: Vec<&str> = other.iter().map(|(_, name, _)| name.as_str()).collect();
                report.problems.push(Problem::on(
                    ProblemKind::Structure,
                    id,
                    format!(
                        "the tree must hold only {RECORD_FILE}, it holds: {}",
                        names.join(", ")
                    ),
                ));
                None
            }
        };
        if let Some(blob) = record {
            record_names.push(blob);
            record_owner.push(at);
        }
    }

    let blobs = read_objects(work_tree, &record_names)?;
    for (at, blob) in record_owner.iter().zip(blobs) {
        let id = &ids[*at];
        let parsed = blob
            .filter(|(kind, _)| kind == "blob")
            .and_then(|(_, bytes)| serde_json::from_slice::<acs_core::SessionRecord>(&bytes).ok());
        match parsed {
            None => report.problems.push(Problem::on(
                ProblemKind::InvalidRecord,
                id,
                format!("{RECORD_FILE} is not a valid session record"),
            )),
            Some(record) => {
                report.records += 1;
                if acs_core::pilot_id(&record.seat.pilot) != pilot_id {
                    report.problems.push(Problem::on(
                        ProblemKind::WrongPilot,
                        id,
                        format!(
                            "the record is of session {} of {}, not of this ledger's pilot",
                            record.session_id, record.seat.pilot
                        ),
                    ));
                }
            }
        }
    }

    for (at, id) in ids.iter().enumerate() {
        let Some(commit) = commits[at].as_ref() else {
            continue;
        };
        match commit.author_email.as_deref() {
            Some(email) if acs_core::pilot_id(email) == pilot_id => {
                report.author.get_or_insert_with(|| email.to_owned());
            }
            Some(email) => report.problems.push(Problem::on(
                ProblemKind::WrongAuthor,
                id,
                format!("authored by {email}, who is not this ledger's pilot"),
            )),
            None => report.problems.push(Problem::on(
                ProblemKind::WrongAuthor,
                id,
                "the commit has no author email",
            )),
        }
    }

    let any_signed = commits.iter().flatten().any(|c| c.signed);
    let codes = if any_signed {
        signature_states(work_tree, tip)?
    } else {
        HashMap::new()
    };
    for (at, id) in ids.iter().enumerate() {
        let Some(commit) = commits[at].take() else {
            continue;
        };
        match state_of(commit.signed, codes.get(id).copied()) {
            SignatureState::Good => report.signed += 1,
            SignatureState::Unsigned => report.unsigned += 1,
            SignatureState::Unchecked => report.unchecked += 1,
            SignatureState::Bad => report.problems.push(Problem::on(
                ProblemKind::BadSignature,
                id,
                "the signature does not match the commit: it was changed after signing",
            )),
            SignatureState::Expired => report.problems.push(Problem::on(
                ProblemKind::ExpiredSignature,
                id,
                "the signature or its key has expired",
            )),
            SignatureState::Revoked => report.problems.push(Problem::on(
                ProblemKind::RevokedSignature,
                id,
                "the signing key was revoked",
            )),
        }
    }
    if report.unchecked > 0 {
        let setup = signing_setup(work_tree)?;
        let why = if setup.format == "ssh" && !setup.allowed_signers {
            "gpg.ssh.allowedSignersFile is not set"
        } else {
            "the signing key is not known here"
        };
        report.notes.push(format!(
            "{} signed commit(s) cannot be checked: {why}",
            report.unchecked
        ));
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// Rewrites
// ---------------------------------------------------------------------------------------------

/// Whether `ancestor` is in the history of `descendant` (or is it). A commit git does not have
/// is not an ancestor of anything.
fn is_ancestor(work_tree: &Path, ancestor: &str, descendant: &str) -> Result<bool, GitError> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    Ok(git_output(work_tree, &args, None, &[])?.status.success())
}

/// The tips of one pilot's ledger on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerTips {
    /// `refs/acs/ledger/<id>`: what this machine wrote (or adopted from the fetched copy).
    pub local: Option<String>,
    /// `refs/remotes/<remote>/acs/ledger/<id>`: the copy as of the last fetch.
    pub fetched: Option<String>,
}

/// The local and the fetched tip of `pilot_id`'s ledger, for the remote `remote`.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn ledger_tips(work_tree: &Path, remote: &str, pilot_id: &str) -> Result<LedgerTips, GitError> {
    let local = ledger_ref(pilot_id);
    let rest = local.strip_prefix("refs/").unwrap_or(&local);
    Ok(LedgerTips {
        local: resolve_ref(work_tree, &local)?,
        fetched: resolve_ref(work_tree, &format!("refs/remotes/{remote}/{rest}"))?,
    })
}

/// The last tip verified for each ledger, kept in a small JSON file so that a chain rewritten
/// consistently is noticed. Keys are `local/<pilot id>` and `fetched/<pilot id>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedTips {
    tips: BTreeMap<String, String>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl VerifiedTips {
    /// Reads the file at `path`. A missing or unreadable file is an empty memory: nothing is
    /// remembered, so nothing can be reported as rewritten.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let mut memory: Self = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        memory.path = Some(path.to_path_buf());
        memory
    }

    /// A memory that is never written (no state directory to keep it in).
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// The remembered tip of `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.tips.get(key).map(String::as_str)
    }

    /// Remembers `tip` for `key`.
    pub fn set(&mut self, key: &str, tip: &str) {
        self.tips.insert(key.to_owned(), tip.to_owned());
    }

    /// Writes the memory back, when it has a file. The write goes through a temporary file so a
    /// crash cannot leave half a file.
    ///
    /// # Errors
    ///
    /// [`GitError::Io`] when the file cannot be written.
    pub fn save(&self) -> Result<(), GitError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let io = |e| GitError::io("write", path, e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| io(e.into()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, bytes).map_err(io)?;
        std::fs::rename(&temp, path).map_err(io)
    }
}

/// A [`ProblemKind::RewrittenHistory`] problem when `remembered` (the last tip verified for this
/// ledger) is not in the history of `tip`.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn check_remembered(
    work_tree: &Path,
    remembered: Option<&str>,
    tip: &str,
    what: &str,
) -> Result<Option<Problem>, GitError> {
    let Some(old) = remembered else {
        return Ok(None);
    };
    if old == tip || is_ancestor(work_tree, old, tip)? {
        return Ok(None);
    }
    Ok(Some(Problem::general(
        ProblemKind::RewrittenHistory,
        format!(
            "the {what} ledger was rewritten: its tip {} was verified before and is no longer in \
             its history",
            &old[..old.len().min(12)]
        ),
    )))
}

/// The session ids of the records in the commits reachable from `include` and not from
/// `exclude`.
fn sessions_between(
    work_tree: &Path,
    include: &str,
    exclude: &str,
) -> Result<HashSet<String>, GitError> {
    let listing = git_text(work_tree, &["rev-list", include, "--not", exclude])?;
    let names: Vec<String> = listing
        .lines()
        .map(|id| format!("{id}:{RECORD_FILE}"))
        .collect();
    Ok(read_objects(work_tree, &names)?
        .into_iter()
        .flatten()
        .filter(|(kind, _)| kind == "blob")
        .filter_map(|(_, bytes)| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter_map(|value| value.get("session_id")?.as_str().map(str::to_owned))
        .collect())
}

/// Compares this machine's copy of a ledger with the fetched one. They are fine when one is in
/// the history of the other, or when they diverged the way one pilot working on two machines
/// diverges (`reconcile` joins them). They are not when they share no history, or when both
/// sides carry a record of the same session: a session runs on one machine, so one side rewrote
/// it.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run.
pub fn check_local_vs_fetched(
    work_tree: &Path,
    local: &str,
    fetched: &str,
) -> Result<Option<Problem>, GitError> {
    if local == fetched
        || is_ancestor(work_tree, local, fetched)?
        || is_ancestor(work_tree, fetched, local)?
    {
        return Ok(None);
    }
    let base = git_output(work_tree, &["merge-base", local, fetched], None, &[])?;
    if !base.status.success() {
        return Ok(Some(Problem::general(
            ProblemKind::Diverged,
            "this machine's ledger and the fetched one share no history",
        )));
    }
    let ours = sessions_between(work_tree, local, fetched)?;
    let theirs = sessions_between(work_tree, fetched, local)?;
    let mut both: Vec<&String> = ours.intersection(&theirs).collect();
    both.sort();
    if both.is_empty() {
        return Ok(None);
    }
    let names: Vec<&str> = both.iter().take(3).map(|s| s.as_str()).collect();
    Ok(Some(Problem::general(
        ProblemKind::RewrittenHistory,
        format!(
            "this machine's ledger and the fetched one both hold a record of {} and cannot be \
             joined: one of them was rewritten",
            names.join(", ")
        ),
    )))
}

// ---------------------------------------------------------------------------------------------
// One pilot
// ---------------------------------------------------------------------------------------------

/// Which copies of a ledger a [`ChainVerdict`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainSource {
    /// This machine's ledger.
    Local,
    /// The fetched copy.
    Fetched,
    /// Both point at the same commit.
    Both,
}

/// One checked chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainVerdict {
    /// Which copy it is.
    pub source: ChainSource,
    /// What was found.
    pub report: ChainReport,
}

/// Everything found about one pilot's ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PilotVerdict {
    /// The pilot id.
    pub pilot_id: String,
    /// The chains that exist: one, or two when this machine's copy and the fetched one differ.
    pub chains: Vec<ChainVerdict>,
    /// Problems between the copies and against what was verified before.
    pub problems: Vec<Problem>,
}

impl PilotVerdict {
    /// Every problem, of the chains and between them.
    pub fn all_problems(&self) -> impl Iterator<Item = &Problem> {
        self.chains
            .iter()
            .flat_map(|chain| chain.report.problems.iter())
            .chain(self.problems.iter())
    }
}

/// Checks the ledger of `pilot_id` on this machine: its local and fetched chains, each against
/// the tip remembered in `memory`, and the two against each other. Without `accept_history`, a
/// chain with no problems is remembered as verified; with it, the problems about rewritten
/// history are dropped and the present tips are remembered, which is how a person accepts a
/// rewrite that was intended.
///
/// # Errors
///
/// [`GitError::Command`] when `git` cannot be run or read the commits.
pub fn verify_pilot(
    work_tree: &Path,
    remote: &str,
    pilot_id: &str,
    memory: &mut VerifiedTips,
    accept_history: bool,
) -> Result<PilotVerdict, GitError> {
    let tips = ledger_tips(work_tree, remote, pilot_id)?;
    let mut verdict = PilotVerdict {
        pilot_id: pilot_id.to_owned(),
        chains: Vec::new(),
        problems: Vec::new(),
    };
    let mut copies: Vec<(ChainSource, &str, &str)> = Vec::new();
    match (&tips.local, &tips.fetched) {
        (Some(local), Some(fetched)) if local == fetched => {
            copies.push((ChainSource::Both, local, "local"));
        }
        (local, fetched) => {
            if let Some(local) = local {
                copies.push((ChainSource::Local, local, "local"));
            }
            if let Some(fetched) = fetched {
                copies.push((ChainSource::Fetched, fetched, "fetched"));
            }
        }
    }
    let mut remember = Vec::new();
    for (source, tip, what) in copies {
        let report = verify_chain(work_tree, pilot_id, tip)?;
        let keys: &[&str] = match source {
            ChainSource::Both => &["local", "fetched"],
            ChainSource::Local => &["local"],
            ChainSource::Fetched => &["fetched"],
        };
        let mut clean = report.problems.is_empty();
        if !accept_history {
            for key in keys {
                let name = format!("{key}/{pilot_id}");
                let label = if *key == "local" { what } else { "fetched" };
                if let Some(problem) = check_remembered(work_tree, memory.get(&name), tip, label)? {
                    verdict.problems.push(problem);
                    clean = false;
                }
            }
        }
        if clean {
            remember.extend(keys.iter().map(|key| (format!("{key}/{pilot_id}"), tip)));
        }
        verdict.chains.push(ChainVerdict { source, report });
    }
    if !accept_history
        && let (Some(local), Some(fetched)) = (&tips.local, &tips.fetched)
        && let Some(problem) = check_local_vs_fetched(work_tree, local, fetched)?
    {
        verdict.problems.push(problem);
        remember.clear();
    }
    for (key, tip) in remember {
        memory.set(&key, tip);
    }
    Ok(verdict)
}

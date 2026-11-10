//! The ledger: an append-only commit chain per pilot, one commit per session record.
//!
//! A commit's tree holds the record as `record.json`, and the hash chain comes free from the
//! commit parents. When one pilot works on two machines their chains diverge; a push must stay a
//! fast-forward, so [`reconcile`] joins them with a merge commit whose tree is empty. No record
//! is rewritten and no signature is lost. Readers skip the merge commits.

use std::path::Path;

use crate::GitError;
use crate::refs::{
    LedgerAppend, LedgerEntry, Pilot, RECORD_FILE, checked, commit, file_tree, git_output,
    git_text, ledger_ref, resolve_ref,
};
use crate::repo::objects;
pub(crate) use crate::signing::signing_configured;

/// How many times [`append`] retries when another process appended first.
const APPEND_ATTEMPTS: usize = 64;

/// A commit, signed when `sign` is set and signing works. Returns the id and whether it is
/// signed.
fn commit_signed_if_possible(
    work_tree: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
    pilot: &Pilot,
    sign: bool,
) -> Result<(String, bool), GitError> {
    if sign && let Ok(id) = commit(work_tree, tree, parents, message, pilot, true) {
        return Ok((id, true));
    }
    Ok((
        commit(work_tree, tree, parents, message, pilot, false)?,
        false,
    ))
}

/// Points `name` at `new` if it still points at `expected` (`None`: if it does not exist).
/// Returns whether it did.
fn compare_and_set(
    work_tree: &Path,
    name: &str,
    new: &str,
    expected: Option<&str>,
) -> Result<bool, GitError> {
    let output = git_output(
        work_tree,
        &["update-ref", name, new, expected.unwrap_or("")],
        None,
        &[],
    )?;
    Ok(output.status.success())
}

/// See [`crate::refs::SyncStorage::append_ledger`].
pub(crate) fn append(
    work_tree: &Path,
    pilot: &Pilot,
    session_id: &str,
    record: &[u8],
) -> Result<LedgerAppend, GitError> {
    let name = ledger_ref(&pilot.id);
    let tree = file_tree(work_tree, RECORD_FILE, record)?;
    let sign = signing_configured(work_tree)?;
    let message = format!("session {session_id}");
    for _ in 0..APPEND_ATTEMPTS {
        let parent = resolve_ref(work_tree, &name)?;
        let parents: Vec<&str> = parent.iter().map(String::as_str).collect();
        let (id, signed) =
            commit_signed_if_possible(work_tree, &tree, &parents, &message, pilot, sign)?;
        if compare_and_set(work_tree, &name, &id, parent.as_deref())? {
            return Ok(LedgerAppend { commit: id, signed });
        }
    }
    Err(GitError::Command {
        args: "update-ref".to_owned(),
        detail: format!("{name} kept moving; gave up after {APPEND_ATTEMPTS} attempts"),
    })
}

/// The records reachable from `tip`, oldest first.
pub(crate) fn entries(work_tree: &Path, tip: &str) -> Result<Vec<LedgerEntry>, GitError> {
    let listing = git_text(work_tree, &["rev-list", "--topo-order", "--reverse", tip])?;
    let commits: Vec<&str> = listing.lines().collect();
    let names: Vec<String> = commits
        .iter()
        .map(|id| format!("{id}:{RECORD_FILE}"))
        .collect();
    let blobs = objects(work_tree, &names)?;
    Ok(commits
        .into_iter()
        .zip(blobs)
        .filter_map(|(id, blob)| {
            Some(LedgerEntry {
                commit: id.to_owned(),
                record: blob?,
            })
        })
        .collect())
}

/// Whether `ancestor` is in the history of `descendant`.
fn is_ancestor(work_tree: &Path, ancestor: &str, descendant: &str) -> Result<bool, GitError> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let output = git_output(work_tree, &args, None, &[])?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => checked(&args, output).map(|_| false),
    }
}

/// See [`crate::refs::SyncStorage::reconcile_ledger`]. `local` is the machine's ref and `fetched`
/// the fetched copy. If the local ref moves meanwhile (an append), this one is left to the next
/// sync: the push then stays a fast-forward or is rejected, and either way nothing is lost.
pub(crate) fn reconcile(
    work_tree: &Path,
    pilot: &Pilot,
    local: &str,
    fetched: &str,
) -> Result<(), GitError> {
    let Some(theirs) = resolve_ref(work_tree, fetched)? else {
        return Ok(());
    };
    let Some(ours) = resolve_ref(work_tree, local)? else {
        compare_and_set(work_tree, local, &theirs, None)?;
        return Ok(());
    };
    if ours == theirs || is_ancestor(work_tree, &theirs, &ours)? {
        return Ok(());
    }
    if is_ancestor(work_tree, &ours, &theirs)? {
        compare_and_set(work_tree, local, &theirs, Some(&ours))?;
        return Ok(());
    }
    let args = ["mktree"];
    let empty = checked(&args, git_output(work_tree, &args, Some(b""), &[])?)?;
    let sign = signing_configured(work_tree)?;
    let (merge, _) = commit_signed_if_possible(
        work_tree,
        &empty,
        &[&ours, &theirs],
        "join chains",
        pilot,
        sign,
    )?;
    compare_and_set(work_tree, local, &merge, Some(&ours))?;
    Ok(())
}

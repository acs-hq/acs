//! Starting a sync: the lock that keeps one running per repository, and the detached child the
//! hooks and commands leave behind them.
//!
//! The child must never slow the process that started it. A harness reads a hook's output to the
//! end, so a child holding those pipes would make it wait for the whole sync: the child's stdio is
//! null, and on Windows, where that alone does not keep the hook's handles out of it, they are
//! made non-inheritable first. It is not waited on either, and it gets its own process group so
//! that a harness cleaning up after a hook does not take it down.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use acs_core::Timestamp;
use acs_store::StateDb;

use super::RepoContext;

/// Held while a sync runs; dropping it, or the process ending in any way, lets the next one in.
#[derive(Debug)]
pub struct SyncLock(#[allow(dead_code)] File);

fn lock_path(ctx: &RepoContext) -> Option<PathBuf> {
    Some(ctx.repo_home.as_ref()?.pending_dir().join("sync.lock"))
}

/// Takes the sync lock. With `wait`, blocks until it is free; otherwise `None` says that a sync
/// is already running.
///
/// # Errors
///
/// The underlying error when the lock file cannot be opened or locked.
pub fn lock(ctx: &RepoContext, wait: bool) -> io::Result<Option<SyncLock>> {
    let path = lock_path(ctx)
        .ok_or_else(|| io::Error::other("the repository's state directory is unavailable"))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    if wait {
        file.lock()?;
        return Ok(Some(SyncLock(file)));
    }
    match file.try_lock() {
        Ok(()) => Ok(Some(SyncLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// Whether hooks may sync this repository on their own: it has a remote, and it adopted ACS.
///
/// A plugin enabled for a user runs its hooks in every repository they open. Sync publishes file
/// paths and the pilot's email, so it takes the repository's `.acs/policy/` to start it unasked;
/// without one, only someone typing `acs sync` does.
fn may_sync(ctx: &RepoContext) -> bool {
    ctx.has_remote && ctx.layout.work_tree.join(".acs").join("policy").is_dir()
}

/// Starts a detached `acs sync` if the repository takes part in sync, `sync_interval_secs` has
/// passed since the last attempt, and none is running. Never fails: a sync that does not start is
/// a stale view, which SPEC section 11 tolerates.
pub fn spawn_if_due(ctx: &RepoContext, db: &StateDb, now: Timestamp) {
    if !may_sync(ctx) {
        return;
    }
    let interval = ctx.policy.policy.files().config.sync_interval_secs.get();
    let Ok(status) = db.remote_sync_status() else {
        return;
    };
    let due = status.last_attempt_at.is_none_or(|at| {
        let waited = now.as_second().saturating_sub(at.as_second());
        waited >= i64::try_from(interval).unwrap_or(i64::MAX)
    });
    if due && matches!(lock(ctx, false), Ok(Some(_))) {
        spawn(ctx, false);
    }
}

/// Starts a detached `acs sync` now, whatever the interval says, for a change other machines
/// should hear about although this may be the last event for a while (a session ended, a claim
/// was made from the command line). It queues behind a sync already running, which may have read
/// the claims too early.
pub fn spawn_now(ctx: &RepoContext) {
    if may_sync(ctx) {
        spawn(ctx, true);
    }
}

/// Starts a detached `acs sync --wait` when a session ended, so its record is written to the
/// ledger without waiting for the next command. Unlike [`spawn_now`] this needs no remote (the
/// record lands in the local ledger), only that the repository adopted ACS (`.acs/policy/`): a
/// plugin runs its hooks in every repository its user opens.
pub fn spawn_for_records(ctx: &RepoContext) {
    if ctx.layout.work_tree.join(".acs").join("policy").is_dir() {
        spawn(ctx, true);
    }
}

fn spawn(ctx: &RepoContext, wait: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .arg("sync")
        .current_dir(&ctx.layout.work_tree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if wait {
        command.arg("--wait");
    }
    detach(&mut command);
    acs_os::keep_stdio_from_children();
    let _ = command.spawn();
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // Not DETACHED_PROCESS: `git` is a console program, and with no console to share each one
    // would open a window of its own.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

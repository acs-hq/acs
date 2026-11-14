//! Session records in the ledger: reading every pilot's records (this module) and, from WP4.1,
//! assembling and appending this machine's.
//!
//! [`ledger_records`] is the one way the read-only commands (`acs log`, `acs blame`, `acs stats`,
//! `acs verify`) see the ledger. It reads through [`SyncStorage`], never refs directly, so a
//! fallback storage changes nothing here.

use std::collections::HashMap;

use acs_core::{EndedReason, SessionRecord, Timestamp};
use acs_git::refs::{CustomRefs, Pilot, SyncStorage};
use acs_store::records::{RecordInputs, is_worth_recording};

use super::{OpError, RepoContext, sync};
use crate::hook::redact::redact_text;

/// How long an agent seat must have been silent, without ending, before its session counts as
/// crashed and gets a record.
pub const CRASH_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

/// The environment variable that overrides [`CRASH_AFTER`], in seconds (tests, and anyone whose
/// harness is known to go quiet for longer).
pub const CRASH_AFTER_ENV: &str = "ACS_CRASH_AFTER_SECS";

/// How long a silent agent seat is given before it counts as crashed, in seconds.
pub(crate) fn crash_after_secs() -> i64 {
    std::env::var(CRASH_AFTER_ENV)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map_or_else(
            || i64::try_from(CRASH_AFTER.as_secs()).unwrap_or(i64::MAX),
            i64::from,
        )
}

/// How much earlier than a seat's start a commit may be dated and still count as its own (clock
/// skew between the commit and the seat).
const COMMIT_SLACK_SECS: i64 = 3600;

/// What [`finalize`] did.
#[derive(Debug, Clone, Default)]
pub struct FinalizeReport {
    /// Records appended to the ledger (clean and crashed together).
    pub written: usize,
    /// Of those, sessions recorded as crashed.
    pub crashed: usize,
    /// Seats with nothing worth recording, remembered so they are not looked at again.
    pub skipped: usize,
    /// The pilots whose ledgers got a record: their refs need pushing.
    pub pilots: Vec<Pilot>,
    /// Seats that could not be recorded now (they are retried next time).
    pub warnings: Vec<String>,
}

/// One session record as read from a ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRecord {
    /// The pilot id of the ledger it came from.
    pub pilot_id: String,
    /// The ledger commit holding it.
    pub commit: String,
    /// The record.
    pub record: SessionRecord,
}

/// What [`ledger_records`] read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerRead {
    /// The latest record of each session, ordered by `started_at`, then `session_id`.
    pub records: Vec<LedgerRecord>,
    /// Ledger entries that are not a valid `SessionRecord` (anyone who can push wrote them), as
    /// `(pilot id, commit)`. They are skipped, not fatal.
    pub invalid: Vec<(String, String)>,
}

/// The storage of this repository's shared state. Without a remote nothing has been fetched, so
/// the remote name only decides where fetched refs would be looked for.
#[must_use]
pub fn storage(ctx: &RepoContext) -> CustomRefs {
    let remote = acs_git::repo::default_remote(&ctx.layout.work_tree)
        .ok()
        .flatten()
        .unwrap_or_else(|| "origin".to_owned());
    CustomRefs::new(&ctx.layout.work_tree, &remote)
}

/// Every pilot's session records on this machine (written here or fetched).
///
/// A session can have more than one record: a seat that comes back after its record was written
/// (a resumed session, or one first recorded as crashed) gets a new one with the same
/// `session_id`. Records are append-only, so the one appended last in its pilot's ledger wins.
///
/// # Errors
///
/// [`OpError::Invalid`] when `git` fails to list or read the ledgers.
pub fn ledger_records(ctx: &RepoContext) -> Result<LedgerRead, OpError> {
    read_all(&storage(ctx))
}

/// [`ledger_records`] over any storage.
///
/// # Errors
///
/// [`OpError::Invalid`] when the storage fails.
pub fn read_all(storage: &dyn SyncStorage) -> Result<LedgerRead, OpError> {
    let git = |err: acs_git::GitError| OpError::Invalid(format!("cannot read the ledger: {err}"));
    let mut read = LedgerRead::default();
    let mut latest: HashMap<String, usize> = HashMap::new();
    for pilot_id in storage.ledger_pilots().map_err(git)? {
        for entry in storage.ledger_entries(&pilot_id).map_err(git)? {
            let Ok(record) = serde_json::from_slice::<SessionRecord>(&entry.record) else {
                read.invalid.push((pilot_id.clone(), entry.commit));
                continue;
            };
            let item = LedgerRecord {
                pilot_id: pilot_id.clone(),
                commit: entry.commit,
                record,
            };
            match latest.get(item.record.session_id.as_str()) {
                Some(&at) => read.records[at] = item,
                None => {
                    latest.insert(item.record.session_id.to_string(), read.records.len());
                    read.records.push(item);
                }
            }
        }
    }
    read.records.sort_by(|a, b| {
        (a.record.started_at, a.record.session_id.as_str())
            .cmp(&(b.record.started_at, b.record.session_id.as_str()))
    });
    Ok(read)
}

/// Appends the session records that are due to the ledger: seats that ended, and agent seats not
/// seen for [`CRASH_AFTER`] (recorded as `crashed`, ended at their last sign of life; they are not
/// ended, their claims lapse by TTL). A seat whose record is already up to date is left alone; one
/// that came back since gets a second record with the same session id.
///
/// Callers hold the sync lock so two processes never record one seat twice.
///
/// # Errors
///
/// [`OpError::Unavailable`] without a state database, [`OpError::Store`] when it fails. A ledger
/// that cannot be written is a warning on the report, not an error.
pub fn finalize(ctx: &RepoContext, now: Timestamp) -> Result<FinalizeReport, OpError> {
    let db = ctx
        .db()
        .map_err(|why| OpError::Unavailable(why.to_owned()))?;
    let crashed_before = Timestamp::from_second(now.as_second().saturating_sub(crash_after_secs()))
        .map_err(|err| OpError::Invalid(err.to_string()))?;
    let mut report = FinalizeReport::default();
    if !ctx.features().history {
        // Sessions stay due: if history is switched back on, what they left is recorded then.
        return Ok(report);
    }
    let due = db.records_due(crashed_before)?;
    if due.is_empty() {
        return Ok(report);
    }

    // One `git log` for every due seat. If it fails the records are written without commits: a
    // record that lacks them is worth more than none, and sync fails the same way next.
    let since = due
        .iter()
        .map(|d| d.info.seat.started_at.as_second())
        .min()
        .map(|secs| secs - COMMIT_SLACK_SECS);
    let mut commits: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(found) = acs_git::trailers::commits_with_sessions(&ctx.layout.work_tree, since) {
        for item in found.into_iter().rev() {
            for session in item.sessions {
                let list = commits.entry(session).or_default();
                if !list.contains(&item.commit) {
                    list.push(item.commit.clone());
                }
            }
        }
    }

    let storage = storage(ctx);
    let redact = |text: &str| redact_text(text, usize::MAX);
    for item in due {
        let info = &item.info;
        let seen = info.seat.last_seen_at;
        let ended_at = match item.reason {
            EndedReason::Clean => info.ended_at.unwrap_or(seen),
            EndedReason::Crashed => seen,
        };
        let record = db.assemble_record(
            info,
            RecordInputs {
                commits: commits
                    .get(info.session_id.as_str())
                    .cloned()
                    .unwrap_or_default(),
                ended_at,
                ended_reason: item.reason,
                acs_version: env!("CARGO_PKG_VERSION"),
                redact: &redact,
            },
        )?;
        if !is_worth_recording(&record) {
            db.mark_recorded(info, seen, item.reason, None, false, now)?;
            report.skipped += 1;
            continue;
        }
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| OpError::Invalid(format!("cannot encode a session record: {err}")))?;
        let pilot = Pilot::new(&info.seat.pilot);
        match storage.append_ledger(&pilot, info.session_id.as_str(), &bytes) {
            Ok(appended) => {
                db.mark_recorded(
                    info,
                    seen,
                    item.reason,
                    Some(&appended.commit),
                    appended.signed,
                    now,
                )?;
                report.written += 1;
                report.crashed += usize::from(item.reason == EndedReason::Crashed);
                if !report.pilots.iter().any(|p| p.id == pilot.id) {
                    report.pilots.push(pilot);
                }
            }
            Err(err) => report.warnings.push(format!(
                "could not record session {}: {err}",
                info.session_id
            )),
        }
    }
    Ok(report)
}

/// [`finalize`] for commands that only read the ledger (`acs log`, `acs stats`): runs under the
/// sync lock and does nothing, quietly, when the lock is busy (another process is recording, or
/// syncing, and will) or the state is unavailable.
pub fn finalize_if_free(ctx: &RepoContext, now: Timestamp) -> Option<FinalizeReport> {
    ctx.db().ok()?;
    let _lock = sync::lock(ctx, false).ok()??;
    finalize(ctx, now).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use acs_git::GitError;
    use acs_git::refs::{LedgerAppend, LedgerEntry, PilotDocument};

    use super::*;

    #[derive(Default)]
    struct Memory {
        ledgers: BTreeMap<String, Vec<Vec<u8>>>,
    }

    impl SyncStorage for Memory {
        fn fetch(&self) -> Result<(), GitError> {
            Ok(())
        }
        fn fetched_claims(&self) -> Result<Vec<PilotDocument>, GitError> {
            Ok(Vec::new())
        }
        fn write_claims(&self, _: &Pilot, _: &[u8]) -> Result<(), GitError> {
            Ok(())
        }
        fn append_ledger(&self, _: &Pilot, _: &str, _: &[u8]) -> Result<LedgerAppend, GitError> {
            Err(GitError::Command {
                args: "append".into(),
                detail: "read-only".into(),
            })
        }
        fn ledger_entries(&self, pilot_id: &str) -> Result<Vec<LedgerEntry>, GitError> {
            Ok(self.ledgers[pilot_id]
                .iter()
                .enumerate()
                .map(|(i, record)| LedgerEntry {
                    commit: format!("{pilot_id}-{i}"),
                    record: record.clone(),
                })
                .collect())
        }
        fn ledger_pilots(&self) -> Result<Vec<String>, GitError> {
            Ok(self.ledgers.keys().cloned().collect())
        }
        fn reconcile_ledger(&self, _: &Pilot) -> Result<(), GitError> {
            Ok(())
        }
        fn push(&self, _: &[Pilot], _: &[Pilot]) -> Result<usize, GitError> {
            Ok(0)
        }
    }

    fn record(session: &str, started: i64, reason: &str) -> Vec<u8> {
        let at = |secs: i64| {
            Timestamp::from_second(1_800_000_000 + secs)
                .unwrap()
                .to_string()
        };
        serde_json::json!({
            "spec": "acs/0.1",
            "session_id": session,
            "seat": {
                "seat_id": "seat_1", "pilot": "sara@example.com", "harness": "claude-code",
                "harness_session_id": "h", "worktree": "repo", "target_branch": "main",
                "started_at": at(started), "last_seen_at": at(started + 10),
            },
            "claims": [], "files_changed": [],
            "guard_decisions": {"counts": {}, "notable": []},
            "commits": [], "lore_loaded": [],
            "started_at": at(started), "ended_at": at(started + 10),
            "ended_reason": reason, "acs_version": "0.1.0",
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_latest_record_of_a_session_wins_and_invalid_entries_are_listed() {
        let mut memory = Memory::default();
        memory.ledgers.insert(
            "pilot-a".into(),
            vec![
                record("ses_one", 0, "crashed"),
                b"not json".to_vec(),
                record("ses_two", 5, "clean"),
                record("ses_one", 0, "clean"),
            ],
        );
        memory.ledgers.insert(
            "pilot-b".into(),
            vec![record("ses_three", 2, "clean"), Vec::new()],
        );

        let read = read_all(&memory).unwrap();
        let seen: Vec<_> = read
            .records
            .iter()
            .map(|r| {
                (
                    r.record.session_id.to_string(),
                    r.record.ended_reason,
                    r.commit.clone(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (
                    "ses_one".to_owned(),
                    EndedReason::Clean,
                    "pilot-a-3".to_owned()
                ),
                (
                    "ses_three".to_owned(),
                    EndedReason::Clean,
                    "pilot-b-0".to_owned()
                ),
                (
                    "ses_two".to_owned(),
                    EndedReason::Clean,
                    "pilot-a-2".to_owned()
                ),
            ],
            "ordered by start time; the later record of ses_one replaced the crashed one"
        );
        assert_eq!(
            read.invalid,
            vec![
                ("pilot-a".to_owned(), "pilot-a-1".to_owned()),
                ("pilot-b".to_owned(), "pilot-b-1".to_owned()),
            ]
        );
    }
}

//! Session records in the ledger: reading every pilot's records (this module) and, from WP4.1,
//! assembling and appending this machine's.
//!
//! [`ledger_records`] is the one way the read-only commands (`acs log`, `acs blame`, `acs stats`,
//! `acs verify`) see the ledger. It reads through [`SyncStorage`], never refs directly, so a
//! fallback storage changes nothing here.

use std::collections::HashMap;

use acs_core::SessionRecord;
use acs_git::refs::{CustomRefs, SyncStorage};

use super::{OpError, RepoContext};

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

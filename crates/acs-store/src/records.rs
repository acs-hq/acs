//! Session records (WP4.1): assembling a `SessionRecord` from a seat's stored history, and
//! remembering which seats already have one in the ledger.
//!
//! Assembly is the git-free part: the commits that carry the session's trailer are looked up by the
//! caller and passed in, and so is the redaction applied to text that came from an agent (intent,
//! command lines). Records are pushed to the git remote, so they hold structural data only.
//!
//! The table `session_records` has one row per seat: the seat's `last_seen_at` when its record was
//! written, why it ended, and the ledger commit (absent when the seat had nothing worth
//! recording). A seat that moved past `recorded_last_seen_at` is due again and gets a new record
//! with the same session id; readers take the last record of a session.

use std::collections::HashMap;

use acs_core::{
    EndedReason, FileChange, GuardDecisions, NotableDecision, Outcome, RepoPath, SeatId,
    SessionRecord, SpecVersion, Timestamp,
};
use rusqlite::{OptionalExtension, params};

use crate::db::{StateDb, StoreError, enum_from_db, enum_to_db, from_db, id_from_db, to_db};
use crate::seats::SeatInfo;

/// The most characters of intent a record keeps.
pub const MAX_INTENT_CHARS: usize = 500;
/// The most notable decisions a record lists.
pub const MAX_NOTABLE: usize = 50;
/// The most characters of a notable decision's path or command.
pub const MAX_NOTABLE_CHARS: usize = 200;

/// What the caller supplies when assembling a record.
pub struct RecordInputs<'a> {
    /// Full hashes of the commits that carry the session's `ACS-Session` trailer.
    pub commits: Vec<String>,
    /// When the session ended (for a crashed one, the seat's `last_seen_at`).
    pub ended_at: Timestamp,
    /// How it ended.
    pub ended_reason: EndedReason,
    /// The version of the binary writing the record.
    pub acs_version: &'a str,
    /// Redacts secrets from text an agent produced (intent, command lines). Applied to intent and
    /// to every notable decision's text, whatever redaction the hook applied before.
    pub redact: &'a dyn Fn(&str) -> String,
}

/// A seat whose session record is due, and why it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueRecord {
    /// The seat.
    pub info: SeatInfo,
    /// `clean` for a seat that ended, `crashed` for one that went quiet.
    pub reason: EndedReason,
}

/// What `session_records` holds for a seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSeat {
    /// The seat's `last_seen_at` when the record was written.
    pub recorded_last_seen_at: Timestamp,
    /// Why the recorded session had ended.
    pub ended_reason: EndedReason,
    /// The ledger commit; `None` when the seat had nothing worth recording.
    pub ledger_commit: Option<String>,
    /// Whether the ledger commit is signed.
    pub signed: bool,
}

/// Whether a record says anything: a session that claimed nothing, changed nothing, was never
/// denied or asked, declared no intent, loaded no lore, used no override and left no commit is not
/// worth a ledger commit.
#[must_use]
pub fn is_worth_recording(record: &SessionRecord) -> bool {
    record.intent.is_some()
        || !record.claims.is_empty()
        || !record.files_changed.is_empty()
        || !record.guard_decisions.notable.is_empty()
        || !record.overrides.is_empty()
        || !record.lore_loaded.is_empty()
        || !record.commits.is_empty()
}

/// The last component of a worktree path: the folder name, without the user names an absolute path
/// carries. Handles both `/` and `\` whatever the platform.
#[must_use]
pub fn worktree_name(worktree: &str) -> String {
    let trimmed = worktree.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// How long one pilot's seats on this machine waited on other seats' claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimWait {
    /// The pilot of the seats that were refused.
    pub pilot: String,
    /// How many claim-conflict denials their seats got.
    pub conflicts: u64,
    /// Seconds from a seat's first refusal until it next changed or claimed the path (or its
    /// session ended); overlapping waits of one seat count once.
    pub waited_secs: i64,
}

impl StateDb {
    /// Time spent waiting on claims, per pilot, from the events on this machine since `since`.
    ///
    /// For each claim-conflict denial the wait lasts until the same seat next changed one of the
    /// denial's paths (a post-tool event) or was allowed to claim one (an allowed pre-tool event
    /// of a tool that is not a read), or until the seat ended, or, for a seat that did neither,
    /// until it was last seen. Retries inside one wait count once. Ordered by pilot.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn claim_waits(&self, since: Timestamp) -> Result<Vec<ClaimWait>, StoreError> {
        use acs_core::{Phase, ReasonCode, ToolKind};
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT e.seat_id FROM events e              WHERE e.reason_code = 'claim_conflict' AND e.at >= ?1",
        )?;
        let seat_ids: Vec<String> = stmt
            .query_map(params![to_db(since)], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut by_pilot: std::collections::BTreeMap<String, ClaimWait> =
            std::collections::BTreeMap::new();
        for seat_id in seat_ids {
            let seat_id: SeatId = id_from_db(seat_id)?;
            let Some(info) = self.seat_info(&seat_id)? else {
                continue;
            };
            let bound = info.ended_at.unwrap_or(info.seat.last_seen_at);
            let events = self.seat_events(&seat_id)?;
            let mut intervals: Vec<(Timestamp, Timestamp)> = Vec::new();
            let mut conflicts = 0u64;
            for (index, conflict) in events.iter().enumerate() {
                if conflict.reason_code != Some(ReasonCode::ClaimConflict) || conflict.at < since {
                    continue;
                }
                conflicts += 1;
                let resolved = events[index + 1..]
                    .iter()
                    .find(|later| {
                        let shares = later.paths.iter().any(|p| conflict.paths.contains(p));
                        shares
                            && match later.phase {
                                Phase::PostTool => true,
                                Phase::PreTool => {
                                    later.outcome == Some(Outcome::Allow)
                                        && later.tool_kind.is_some_and(|k| k != ToolKind::Read)
                                }
                                _ => false,
                            }
                    })
                    .map_or(bound, |later| later.at.min(bound));
                intervals.push((conflict.at, resolved.max(conflict.at)));
            }
            intervals.sort();
            let mut waited = 0i64;
            let mut current: Option<(Timestamp, Timestamp)> = None;
            for (start, end) in intervals {
                current = match current {
                    Some((from, to)) if start <= to => Some((from, to.max(end))),
                    Some((from, to)) => {
                        waited += to.as_microsecond() - from.as_microsecond();
                        Some((start, end))
                    }
                    None => Some((start, end)),
                };
            }
            if let Some((from, to)) = current {
                waited += to.as_microsecond() - from.as_microsecond();
            }
            let entry = by_pilot
                .entry(info.seat.pilot.clone())
                .or_insert_with(|| ClaimWait {
                    pilot: info.seat.pilot.clone(),
                    conflicts: 0,
                    waited_secs: 0,
                });
            entry.conflicts += conflicts;
            entry.waited_secs += waited / 1_000_000;
        }
        Ok(by_pilot.into_values().collect())
    }

    /// Assembles the session record of `info`'s seat from the database.
    ///
    /// - `seat.worktree` holds the folder name only (absolute paths carry user names);
    /// - `intent` is redacted and cut to [`MAX_INTENT_CHARS`];
    /// - `files_changed` has, per path, the first `pre_hash` and the last `post_hash` of the
    ///   seat's attributions, in order of first change;
    /// - `guard_decisions.counts` counts every reason code; `notable` lists the first
    ///   [`MAX_NOTABLE`] deny and ask decisions, oldest first, with their text redacted and cut to
    ///   [`MAX_NOTABLE_CHARS`] (a claim conflict lists its paths: its stored message names the
    ///   holder).
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn assemble_record(
        &self,
        info: &SeatInfo,
        inputs: RecordInputs<'_>,
    ) -> Result<SessionRecord, StoreError> {
        let seat_id = &info.seat.seat_id;
        let mut seat = info.seat.clone();
        seat.worktree = worktree_name(&seat.worktree);

        let intent = info
            .intent
            .as_deref()
            .map(|text| cut((inputs.redact)(text).trim(), MAX_INTENT_CHARS))
            .filter(|text| !text.is_empty());

        let mut order: Vec<RepoPath> = Vec::new();
        let mut changes: HashMap<RepoPath, FileChange> = HashMap::new();
        for stored in self.seat_attributions(seat_id)? {
            match changes.get_mut(&stored.path) {
                Some(change) => change.post_hash = stored.post_hash,
                None => {
                    order.push(stored.path.clone());
                    changes.insert(
                        stored.path.clone(),
                        FileChange {
                            path: stored.path,
                            pre_hash: stored.pre_hash,
                            post_hash: stored.post_hash,
                        },
                    );
                }
            }
        }
        let files_changed: Vec<FileChange> = order
            .into_iter()
            .filter_map(|path| changes.remove(&path))
            .collect();

        let notable: Vec<NotableDecision> = self
            .seat_events(seat_id)?
            .into_iter()
            .filter_map(|event| {
                let outcome = event.outcome?;
                if !matches!(outcome, Outcome::Deny | Outcome::Ask) {
                    return None;
                }
                let reason_code = event.reason_code?;
                let text = match event.detail {
                    Some(detail) if reason_code != acs_core::ReasonCode::ClaimConflict => detail,
                    _ if !event.paths.is_empty() => event.paths.join(", "),
                    _ => event.tool_name.unwrap_or_default(),
                };
                Some(NotableDecision {
                    at: event.at,
                    reason_code,
                    outcome,
                    path_or_command: cut(&(inputs.redact)(&text), MAX_NOTABLE_CHARS),
                })
            })
            .take(MAX_NOTABLE)
            .collect();

        Ok(SessionRecord {
            spec: SpecVersion,
            session_id: info.session_id.clone(),
            seat,
            intent,
            claims: self.seat_claim_history(seat_id)?,
            files_changed,
            guard_decisions: GuardDecisions {
                counts: self.reason_counts(seat_id)?,
                notable,
            },
            commits: inputs.commits,
            lore_loaded: self
                .lore_loaded_for_seat(seat_id)?
                .into_iter()
                .map(|loaded| loaded.lore_id)
                .collect(),
            overrides: self.override_uses_for_seat(seat_id)?,
            started_at: info.seat.started_at,
            ended_at: inputs.ended_at,
            ended_reason: inputs.ended_reason,
            acs_version: inputs.acs_version.to_owned(),
        })
    }

    /// The seats whose record is due, oldest activity first: seats that ended, and seats of an
    /// agent harness whose `last_seen_at` is strictly older than `crashed_before` without an end
    /// (crashed), in both cases unless their record is up to date, that is, the seat's
    /// `last_seen_at` has not moved past the one recorded. Seats of the `human` pseudo-harness
    /// never have records.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn records_due(&self, crashed_before: Timestamp) -> Result<Vec<DueRecord>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.seat_id, s.ended_at IS NOT NULL FROM seats s \
             LEFT JOIN session_records r ON r.seat_id = s.seat_id \
             WHERE s.harness <> 'human' \
               AND (r.seat_id IS NULL OR s.last_seen_at > r.recorded_last_seen_at) \
               AND (s.ended_at IS NOT NULL OR s.last_seen_at < ?1) \
             ORDER BY s.last_seen_at, s.seat_id",
        )?;
        let rows: Vec<(String, bool)> = stmt
            .query_map(params![to_db(crashed_before)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut due = Vec::new();
        for (seat_id, ended) in rows {
            let seat_id: SeatId = id_from_db(seat_id)?;
            if let Some(info) = self.seat_info(&seat_id)? {
                due.push(DueRecord {
                    info,
                    reason: if ended {
                        EndedReason::Clean
                    } else {
                        EndedReason::Crashed
                    },
                });
            }
        }
        Ok(due)
    }

    /// What is remembered about the seat's record, if one was ever written or skipped.
    ///
    /// # Errors
    ///
    /// Any [`StoreError`].
    pub fn recorded_seat(&self, seat_id: &SeatId) -> Result<Option<RecordedSeat>, StoreError> {
        let raw: Option<(i64, String, Option<String>, bool)> = self
            .conn
            .prepare_cached(
                "SELECT recorded_last_seen_at, ended_reason, ledger_commit, signed \
                 FROM session_records WHERE seat_id = ?1",
            )?
            .query_row(params![seat_id.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        raw.map(|(seen, reason, commit, signed)| {
            Ok(RecordedSeat {
                recorded_last_seen_at: from_db(seen)?,
                ended_reason: enum_from_db(&reason)?,
                ledger_commit: commit,
                signed,
            })
        })
        .transpose()
    }

    /// Remembers that the seat's record was written (`ledger_commit` set) or skipped (`None`) as
    /// of `recorded_last_seen_at`, replacing what was remembered before.
    ///
    /// # Errors
    ///
    /// [`StoreError::Sqlite`] if the seat does not exist; any other [`StoreError`].
    pub fn mark_recorded(
        &self,
        info: &SeatInfo,
        recorded_last_seen_at: Timestamp,
        reason: EndedReason,
        ledger_commit: Option<&str>,
        signed: bool,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO session_records (seat_id, session_id, recorded_last_seen_at, \
                 ended_reason, ledger_commit, signed, written_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT (seat_id) DO UPDATE SET \
                 recorded_last_seen_at = excluded.recorded_last_seen_at, \
                 ended_reason = excluded.ended_reason, ledger_commit = excluded.ledger_commit, \
                 signed = excluded.signed, written_at = excluded.written_at",
            )?
            .execute(params![
                info.seat.seat_id.as_str(),
                info.session_id.as_str(),
                to_db(recorded_last_seen_at),
                enum_to_db(&reason)?,
                ledger_commit,
                signed,
                to_db(now),
            ])?;
        Ok(())
    }
}
